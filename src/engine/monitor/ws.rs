use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::{
    RpcTransactionConfig, RpcTransactionLogsConfig, RpcTransactionLogsFilter,
};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature};
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiInstruction, UiMessage,
    UiTransactionEncoding,
};
use std::str::FromStr;

use crate::common::constants::{PUMP_PROGRAM, RAYDIUM_AMM_V4};
use crate::common::logger::Logger;
use crate::context::AppContext;
use crate::engine::snipe::{self, SnipeEvent};

pub async fn pumpfun_monitor(ctx: AppContext) -> Result<()> {
    run_monitor(ctx, PUMP_PROGRAM, "pump.fun").await
}

pub async fn raydium_monitor(ctx: AppContext) -> Result<()> {
    run_monitor(ctx, RAYDIUM_AMM_V4, "raydium").await
}

async fn run_monitor(ctx: AppContext, program: &str, label: &str) -> Result<()> {
    let logger = Logger::new(format!("[MONITOR {label}] => "));
    logger.log(format!("subscribing to logs for {program}"));
    let pubsub = PubsubClient::new(&ctx.config.rpc_wss)
        .await
        .map_err(|e| anyhow!("websocket connect failed: {e}"))?;
    let (mut stream, _unsub) = pubsub
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![program.to_string()]),
            RpcTransactionLogsConfig {
                commitment: Some(CommitmentConfig::processed()),
            },
        )
        .await
        .map_err(|e| anyhow!("logsSubscribe failed: {e}"))?;

    while let Some(response) = stream.next().await {
        let value = response.value;
        if value.err.is_some() {
            continue;
        }
        let logs = value.logs.join("\n");
        let kind = classify_logs(&logs, label);
        if kind == EventKind::Ignore {
            continue;
        }
        let sig = match Signature::from_str(&value.signature) {
            Ok(s) => s,
            Err(_) => continue,
        };
        match fetch_tx(&ctx, &sig).await {
            Ok(tx) => {
                if let Some(event) = parse_event(label, kind, &tx, &ctx, &value.signature) {
                    if let Err(err) = snipe::on_event(&ctx, event).await {
                        logger.error(format!("snipe handler: {err}"));
                    }
                }
            }
            Err(err) => logger.debug(format!("getTransaction {}: {err}", value.signature)),
        }
    }
    Err(anyhow!("{label} websocket stream ended"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventKind {
    Create,
    Buy,
    NewPool,
    Ignore,
}

fn classify_logs(logs: &str, label: &str) -> EventKind {
    let lower = logs.to_lowercase();
    if label == "pump.fun" {
        if lower.contains("instruction: create") {
            return EventKind::Create;
        }
        if lower.contains("instruction: buy") {
            return EventKind::Buy;
        }
    }
    if label == "raydium"
        && (lower.contains("initialize2") || lower.contains("instruction: initialize"))
    {
        return EventKind::NewPool;
    }
    EventKind::Ignore
}

async fn fetch_tx(
    ctx: &AppContext,
    sig: &Signature,
) -> Result<EncodedConfirmedTransactionWithStatusMeta> {
    ctx.state
        .rpc_nonblocking_client
        .get_transaction_with_config(
            sig,
            RpcTransactionConfig {
                encoding: Some(UiTransactionEncoding::Json),
                commitment: Some(CommitmentConfig::confirmed()),
                max_supported_transaction_version: Some(0),
            },
        )
        .await
        .map_err(|e| anyhow!("{e}"))
}

fn parse_event(
    label: &str,
    kind: EventKind,
    tx: &EncodedConfirmedTransactionWithStatusMeta,
    ctx: &AppContext,
    signature: &str,
) -> Option<SnipeEvent> {
    let keys = account_keys(tx)?;
    match (label, kind) {
        ("pump.fun", EventKind::Create) => {
            let mint = pump_ix_account(&keys, tx, 0)?;
            Some(SnipeEvent::NewPumpToken {
                mint,
                signature: signature.to_string(),
            })
        }
        ("pump.fun", EventKind::Buy) => {
            let mint = pump_ix_account(&keys, tx, 2)?;
            let buyer = pump_ix_account(&keys, tx, 6)?;
            let copy = ctx
                .config
                .copy_wallets
                .iter()
                .any(|w| w.to_string() == buyer);
            Some(SnipeEvent::PumpBuy {
                mint,
                buyer,
                copy_wallet: copy,
                signature: signature.to_string(),
            })
        }
        ("raydium", EventKind::NewPool) => {
            let mint = keys.get(1).cloned().unwrap_or_default();
            Some(SnipeEvent::NewRaydiumPool {
                mint,
                signature: signature.to_string(),
            })
        }
        _ => None,
    }
}

fn account_keys(tx: &EncodedConfirmedTransactionWithStatusMeta) -> Option<Vec<String>> {
    match &tx.transaction.transaction {
        EncodedTransaction::Json(ui) => match &ui.message {
            UiMessage::Raw(raw) => Some(raw.account_keys.clone()),
            UiMessage::Parsed(parsed) => Some(
                parsed
                    .account_keys
                    .iter()
                    .map(|k| k.pubkey.clone())
                    .collect(),
            ),
        },
        EncodedTransaction::LegacyBinary(_)
        | EncodedTransaction::Binary(_, _)
        | EncodedTransaction::Accounts(_) => None,
    }
}

fn pump_ix_account(
    keys: &[String],
    tx: &EncodedConfirmedTransactionWithStatusMeta,
    account_index: usize,
) -> Option<String> {
    let EncodedTransaction::Json(ui) = &tx.transaction.transaction else {
        return None;
    };
    let instructions = match &ui.message {
        UiMessage::Raw(raw) => raw
            .instructions
            .iter()
            .filter(|ix| {
                keys.get(ix.program_id_index as usize)
                    .map(|p| p == PUMP_PROGRAM)
                    .unwrap_or(false)
            })
            .filter_map(|ix| {
                ix.accounts
                    .get(account_index)
                    .and_then(|i| keys.get(*i as usize))
                    .cloned()
            })
            .next(),
        UiMessage::Parsed(parsed) => parsed.instructions.iter().find_map(|ix| match ix {
            UiInstruction::Compiled(c) => {
                if keys.get(c.program_id_index as usize).map(|p| p.as_str()) != Some(PUMP_PROGRAM) {
                    return None;
                }
                c.accounts
                    .get(account_index)
                    .and_then(|i| keys.get(*i as usize))
                    .cloned()
            }
            UiInstruction::Parsed(_) => None,
        }),
    };
    instructions
}

pub fn is_copy_wallet(ctx: &AppContext, wallet: &Pubkey) -> bool {
    ctx.config.copy_wallets.iter().any(|w| w == wallet)
}
