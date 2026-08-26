use anyhow::Result;
use serde_json::Value;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signer::Signer,
};

use crate::common::constants::{pair_mints, WSOL};
use crate::common::logger::Logger;
use crate::common::types::Quote;
use crate::context::AppContext;
use crate::core::math;
use crate::core::tx;
use crate::dex::meteora::METEORA_DEXES;
use crate::dex::orca::ORCA_DEXES;
use crate::dex::pump_fun::{self, get_pump_info};
use crate::services::jupiter::{self, JupiterQuote};

const RAYDIUM_DEXES: &str = "Raydium,Raydium CLMM,Raydium CP";

#[derive(Debug, Clone)]
pub struct ArbOpportunity {
    pub pair: String,
    pub buy_venue: String,
    pub sell_venue: String,
    pub amount_in: u64,
    pub expected_out: u64,
    pub profit_lamports: i64,
    pub buy_quote: Quote,
    pub sell_out: u64,
}

pub async fn run(ctx: AppContext) -> Result<()> {
    let logger = Logger::new("[ARB] => ".to_string());
    if !ctx.config.arb_enabled {
        logger.log("ARB_ENABLED=false, exiting".into());
        return Ok(());
    }
    logger.log(format!(
        "scanning pairs={:?} size_sol={:.4} min_profit_sol={:.6} interval={:?}",
        ctx.config.arb_pairs,
        ctx.config.arb_amount_lamports as f64 / 1_000_000_000.0,
        ctx.config.arb_min_profit_lamports as f64 / 1_000_000_000.0,
        ctx.config.arb_interval
    ));
    loop {
        for pair in &ctx.config.arb_pairs {
            if let Err(err) = scan_pair(&ctx, pair, &logger).await {
                logger.debug(format!("{pair}: {err}"));
            }
        }
        tokio::time::sleep(ctx.config.arb_interval).await;
    }
}

async fn scan_pair(ctx: &AppContext, pair: &str, logger: &Logger) -> Result<()> {
    if let Some((a, b)) = pair_mints(pair) {
        return scan_stable_pair(ctx, pair, a, b, logger).await;
    }
    scan_mint_vs_pump(ctx, pair, logger).await
}

async fn scan_stable_pair(
    ctx: &AppContext,
    pair: &str,
    mint_a: &str,
    mint_b: &str,
    logger: &Logger,
) -> Result<()> {
    let amount = ctx.config.arb_amount_lamports;
    let venues = [
        ("raydium", RAYDIUM_DEXES),
        ("orca", ORCA_DEXES),
        ("meteora", METEORA_DEXES),
        ("jupiter", ""),
    ];

    let mut buys: Vec<(String, JupiterQuote, Value)> = Vec::new();
    for (name, dexes) in venues {
        let dex = if dexes.is_empty() { None } else { Some(dexes) };
        match jupiter::quote(
            &ctx.http,
            mint_a,
            mint_b,
            amount,
            ctx.config.slippage_bps,
            dex,
        )
        .await
        {
            Ok((q, raw)) => buys.push((name.to_string(), q, raw)),
            Err(_) => continue,
        }
    }
    if buys.len() < 2 {
        return Ok(());
    }

    let mut best: Option<ArbOpportunity> = None;
    for (buy_name, buy_q, _) in &buys {
        let tokens_out: u64 = buy_q.out_amount.parse().unwrap_or(0);
        if tokens_out == 0 {
            continue;
        }
        for (sell_name, dexes) in venues {
            if sell_name == buy_name {
                continue;
            }
            let dex = if dexes.is_empty() { None } else { Some(dexes) };
            let Ok((sell_q, _)) = jupiter::quote(
                &ctx.http,
                mint_b,
                mint_a,
                tokens_out,
                ctx.config.slippage_bps,
                dex,
            )
            .await
            else {
                continue;
            };
            let sol_back: u64 = sell_q.out_amount.parse().unwrap_or(0);
            let tip = crate::services::jito::tip_lamports(ctx.config.jito_tip_sol);
            let profit = math::net_profit(amount, sol_back, tip);
            if profit > ctx.config.arb_min_profit_lamports as i64 {
                let opp = ArbOpportunity {
                    pair: pair.to_string(),
                    buy_venue: buy_name.clone(),
                    sell_venue: sell_name.to_string(),
                    amount_in: amount,
                    expected_out: sol_back,
                    profit_lamports: profit,
                    buy_quote: buy_q.to_quote(buy_name),
                    sell_out: sol_back,
                };
                if best
                    .as_ref()
                    .map(|b| opp.profit_lamports > b.profit_lamports)
                    .unwrap_or(true)
                {
                    best = Some(opp);
                }
            }
        }
    }

    if let Some(opp) = best {
        logger.log(format!(
            "ARB {pair}: buy {} sell {} in={} back={} profit_lamports={}",
            opp.buy_venue, opp.sell_venue, opp.amount_in, opp.expected_out, opp.profit_lamports
        ));
        execute_two_leg(ctx, mint_a, mint_b, &opp, logger).await?;
    }
    Ok(())
}

async fn scan_mint_vs_pump(ctx: &AppContext, mint: &str, logger: &Logger) -> Result<()> {
    if mint.parse::<Pubkey>().is_err() {
        return Ok(());
    }
    let info = get_pump_info(ctx.state.rpc_client.clone(), mint).await?;
    let amount = ctx.config.arb_amount_lamports;
    let pump = pump_fun::Pump::from_ctx(ctx);

    if !info.complete {
        let pump_buy = pump.quote_buy(mint, amount).await?;
        if pump_buy.amount_out > 0 {
            if let Ok((jup, _)) = jupiter::quote(
                &ctx.http,
                mint,
                WSOL,
                pump_buy.amount_out,
                ctx.config.slippage_bps,
                None,
            )
            .await
            {
                let sol_back: u64 = jup.out_amount.parse().unwrap_or(0);
                let tip = crate::services::jito::tip_lamports(ctx.config.jito_tip_sol);
                let profit = math::net_profit(amount, sol_back, tip);
                if profit > ctx.config.arb_min_profit_lamports as i64 {
                    logger.log(format!(
                        "ARB pump->amm {mint} profit_lamports={profit} tokens={}",
                        pump_buy.amount_out
                    ));
                    execute_pump_then_amm(ctx, mint, amount, logger).await?;
                }
            }
        }
    }

    if let Ok((jup_buy, _)) = jupiter::quote(
        &ctx.http,
        WSOL,
        mint,
        amount,
        ctx.config.slippage_bps,
        None,
    )
    .await
    {
        let tokens: u64 = jup_buy.out_amount.parse().unwrap_or(0);
        if tokens > 0 && !info.complete {
            if let Ok(pump_sell) = pump.quote_sell(mint, tokens).await {
                let tip = crate::services::jito::tip_lamports(ctx.config.jito_tip_sol);
                let profit = math::net_profit(amount, pump_sell.amount_out, tip);
                if profit > ctx.config.arb_min_profit_lamports as i64 {
                    logger.log(format!(
                        "ARB amm->pump {mint} profit_lamports={profit}"
                    ));
                    execute_amm_then_pump(ctx, mint, amount, logger).await?;
                }
            }
        }
    }
    Ok(())
}

async fn execute_two_leg(
    ctx: &AppContext,
    mint_a: &str,
    mint_b: &str,
    opp: &ArbOpportunity,
    logger: &Logger,
) -> Result<()> {
    if let Err(reason) = ctx.risk.can_open(opp.amount_in) {
        logger.log(format!("skip arb: {reason}"));
        return Ok(());
    }
    let owner = ctx.state.wallet.pubkey().to_string();
    let buy_dex = dex_filter(&opp.buy_venue);
    let sell_dex = dex_filter(&opp.sell_venue);
    let (buy_q, buy_raw) = jupiter::quote(
        &ctx.http,
        mint_a,
        mint_b,
        opp.amount_in,
        ctx.config.slippage_bps,
        buy_dex,
    )
    .await?;
    let tokens: u64 = buy_q.out_amount.parse().unwrap_or(0);
    let (sell_q, sell_raw) = jupiter::quote(
        &ctx.http,
        mint_b,
        mint_a,
        tokens,
        ctx.config.slippage_bps,
        sell_dex,
    )
    .await?;
    let _ = sell_q;
    let buy_ixs = jupiter::swap_instructions(&ctx.http, &buy_raw, &owner).await?;
    let sell_ixs = jupiter::swap_instructions(&ctx.http, &sell_raw, &owner).await?;
    let mut instructions: Vec<Instruction> = Vec::new();
    instructions.extend(buy_ixs.instructions);
    instructions.extend(sell_ixs.instructions);
    let mut alt_addrs = buy_ixs.lookup_table_addresses;
    alt_addrs.extend(sell_ixs.lookup_table_addresses);
    let alts =
        jupiter::load_address_lookup_tables(&ctx.state.rpc_nonblocking_client, &alt_addrs).await?;
    let blockhash = ctx.state.rpc_client.get_latest_blockhash()?;
    let tx = tx::compile_v0(&ctx.state.wallet, &instructions, blockhash, &alts)?;
    let sigs = tx::send_versioned(
        &ctx.state.rpc_client,
        tx,
        ctx.config.use_jito,
        &ctx.config.jito_block_engine_url,
        ctx.config.dry_run,
        logger,
    )
    .await?;
    logger.log(format!("arb submitted sigs={sigs:?}"));
    ctx.risk.record_success();
    Ok(())
}

async fn execute_pump_then_amm(
    ctx: &AppContext,
    mint: &str,
    amount: u64,
    logger: &Logger,
) -> Result<()> {
    let buy = crate::common::types::SwapConfig {
        swap_direction: crate::common::types::SwapDirection::Buy,
        in_type: crate::common::types::SwapInType::Qty,
        amount_in: amount as f64 / 1_000_000_000.0,
        slippage: ctx.config.slippage_bps / 100,
        use_jito: ctx.config.use_jito,
    };
    let sigs = pump_fun::swap_with_ctx(ctx, mint, buy).await?;
    logger.log(format!("pump buy {sigs:?}"));
    let sell = crate::common::types::SwapConfig {
        swap_direction: crate::common::types::SwapDirection::Sell,
        in_type: crate::common::types::SwapInType::Pct,
        amount_in: 1.0,
        slippage: ctx.config.slippage_bps / 100,
        use_jito: ctx.config.use_jito,
    };
    let sigs = crate::dex::raydium::swap_mint_on_dex(ctx, mint, sell, None).await?;
    logger.log(format!("amm sell {sigs:?}"));
    Ok(())
}

async fn execute_amm_then_pump(
    ctx: &AppContext,
    mint: &str,
    amount: u64,
    logger: &Logger,
) -> Result<()> {
    let buy = crate::common::types::SwapConfig {
        swap_direction: crate::common::types::SwapDirection::Buy,
        in_type: crate::common::types::SwapInType::Qty,
        amount_in: amount as f64 / 1_000_000_000.0,
        slippage: ctx.config.slippage_bps / 100,
        use_jito: ctx.config.use_jito,
    };
    let sigs = crate::dex::raydium::swap_mint_on_dex(ctx, mint, buy, None).await?;
    logger.log(format!("amm buy {sigs:?}"));
    let sell = crate::common::types::SwapConfig {
        swap_direction: crate::common::types::SwapDirection::Sell,
        in_type: crate::common::types::SwapInType::Pct,
        amount_in: 1.0,
        slippage: ctx.config.slippage_bps / 100,
        use_jito: ctx.config.use_jito,
    };
    let sigs = pump_fun::swap_with_ctx(ctx, mint, sell).await?;
    logger.log(format!("pump sell {sigs:?}"));
    Ok(())
}

fn dex_filter(venue: &str) -> Option<&'static str> {
    match venue {
        "raydium" => Some(RAYDIUM_DEXES),
        "orca" => Some(ORCA_DEXES),
        "meteora" => Some(METEORA_DEXES),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math::net_profit;

    #[test]
    fn profitable_round_trip() {
        assert!(net_profit(1_000_000_000, 1_020_000_000, 100_000) > 0);
        assert!(net_profit(1_000_000_000, 1_000_050_000, 100_000) < 0);
    }
}
