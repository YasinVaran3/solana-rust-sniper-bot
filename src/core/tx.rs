use anyhow::{anyhow, Result};
use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::{Transaction, VersionedTransaction},
};
use std::time::Duration;
use tokio::time::Instant;

use crate::common::constants::compute_budget;
use crate::common::logger::Logger;
use crate::services::jito;

pub fn set_compute_unit_limit(units: u32) -> Instruction {
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction {
        program_id: compute_budget(),
        accounts: vec![],
        data,
    }
}

pub fn set_compute_unit_price(micro_lamports: u64) -> Instruction {
    let mut data = vec![3u8];
    data.extend_from_slice(&micro_lamports.to_le_bytes());
    Instruction {
        program_id: compute_budget(),
        accounts: vec![],
        data,
    }
}

pub fn prepend_budget(mut instructions: Vec<Instruction>, unit_price: u64, unit_limit: u32) -> Vec<Instruction> {
    let mut prefix = vec![
        set_compute_unit_price(unit_price),
        set_compute_unit_limit(unit_limit),
    ];
    prefix.append(&mut instructions);
    prefix
}

pub fn compile_legacy(
    payer: &Keypair,
    instructions: &[Instruction],
    blockhash: Hash,
) -> Result<Transaction> {
    Ok(Transaction::new_signed_with_payer(
        instructions,
        Some(&payer.pubkey()),
        &[payer],
        blockhash,
    ))
}

pub fn compile_v0(
    payer: &Keypair,
    instructions: &[Instruction],
    blockhash: Hash,
    alts: &[solana_sdk::message::AddressLookupTableAccount],
) -> Result<VersionedTransaction> {
    let msg = v0::Message::try_compile(&payer.pubkey(), instructions, alts, blockhash)?;
    Ok(VersionedTransaction::try_new(
        VersionedMessage::V0(msg),
        &[payer],
    )?)
}

pub async fn new_signed_and_send(
    client: &solana_client::rpc_client::RpcClient,
    keypair: &Keypair,
    mut instructions: Vec<Instruction>,
    use_jito: bool,
    unit_price: u64,
    unit_limit: u32,
    tip_sol: f64,
    block_engine_url: &str,
    dry_run: bool,
    logger: &Logger,
) -> Result<Vec<String>> {
    if !use_jito {
        instructions = prepend_budget(instructions, unit_price, unit_limit);
    } else {
        instructions.insert(0, set_compute_unit_limit(unit_limit));
    }

    let recent_blockhash = client.get_latest_blockhash()?;
    let txn = compile_legacy(keypair, &instructions, recent_blockhash)?;

    if dry_run {
        logger.log(format!(
            "DRY_RUN: compiled {} instruction(s), skip broadcast",
            txn.message.instructions.len()
        ));
        return Ok(vec!["dry-run".to_string()]);
    }

    let start_time = Instant::now();
    let txs = if use_jito {
        let tip_account = jito::get_tip_account().await?;
        let tip_lamports = jito::tip_lamports(tip_sol);
        logger.log(format!(
            "jito tip account: {tip_account}, tip sol: {tip_sol}, lamports: {tip_lamports}"
        ));
        let tip_ix = solana_sdk::system_instruction::transfer(
            &keypair.pubkey(),
            &tip_account,
            tip_lamports,
        );
        let mut with_tip = txn.message.instructions.clone();
        let mut ixs = instructions;
        ixs.push(tip_ix);
        let bundle_txn = compile_legacy(keypair, &ixs, recent_blockhash)?;
        let _ = with_tip;
        let bundle_id = jito::send_bundle(block_engine_url, &[bundle_txn]).await?;
        logger.log(format!("bundle_id: {bundle_id}"));
        jito::wait_for_bundle_confirmation(
            block_engine_url,
            &bundle_id,
            Duration::from_millis(800),
            Duration::from_secs(12),
            logger,
        )
        .await?
    } else {
        let sig = client.send_and_confirm_transaction(&txn)?;
        logger.log(format!("signature: {sig}"));
        vec![sig.to_string()]
    };

    logger.log(format!("tx elapsed: {:?}", start_time.elapsed()));
    Ok(txs)
}

pub async fn send_versioned(
    client: &solana_client::rpc_client::RpcClient,
    tx: VersionedTransaction,
    use_jito: bool,
    block_engine_url: &str,
    dry_run: bool,
    logger: &Logger,
) -> Result<Vec<String>> {
    if dry_run {
        logger.log("DRY_RUN: skip versioned broadcast".into());
        return Ok(vec!["dry-run".to_string()]);
    }
    if use_jito {
        let bundle_id = jito::send_versioned_bundle(block_engine_url, &[tx]).await?;
        logger.log(format!("bundle_id: {bundle_id}"));
        jito::wait_for_bundle_confirmation(
            block_engine_url,
            &bundle_id,
            Duration::from_millis(800),
            Duration::from_secs(12),
            logger,
        )
        .await
    } else {
        let sig = client.send_transaction(&tx)?;
        logger.log(format!("signature: {sig}"));
        Ok(vec![sig.to_string()])
    }
}

pub fn ix_from_json(
    program_id: &str,
    accounts: &[serde_json::Value],
    data_b64: &str,
) -> Result<Instruction> {
    let program_id: Pubkey = program_id.parse()?;
    let mut metas = Vec::new();
    for acc in accounts {
        let pubkey: Pubkey = acc
            .get("pubkey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing pubkey"))?
            .parse()?;
        let is_signer = acc
            .get("isSigner")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let is_writable = acc
            .get("isWritable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        metas.push(if is_writable {
            if is_signer {
                AccountMeta::new(pubkey, true)
            } else {
                AccountMeta::new(pubkey, false)
            }
        } else {
            AccountMeta::new_readonly(pubkey, is_signer)
        });
    }
    let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_b64)
        .map_err(|e| anyhow!("invalid instruction data: {e}"))?;
    Ok(Instruction {
        program_id,
        accounts: metas,
        data,
    })
}
