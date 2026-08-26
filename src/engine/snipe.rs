use anyhow::{anyhow, Result};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::sleep;

use crate::common::logger::Logger;
use crate::common::types::{SwapConfig, SwapDirection, SwapInType};
use crate::common::utils::default_swap_config;
use crate::context::AppContext;
use crate::core::math;
use crate::core::risk::{ExitReason, Position};
use crate::core::token;
use crate::dex::pump_fun::{self, get_pump_info};
use crate::dex::raydium;

#[derive(Debug, Clone)]
pub enum SnipeEvent {
    NewPumpToken { mint: String, signature: String },
    PumpBuy {
        mint: String,
        buyer: String,
        copy_wallet: bool,
        signature: String,
    },
    NewRaydiumPool { mint: String, signature: String },
}

pub async fn run(ctx: AppContext) -> Result<()> {
    let logger = Logger::new("[SNIPE] => ".to_string());
    logger.log(format!(
        "starting sniper dry_run={} size_sol={} tp={} sl={} hold={:?}",
        ctx.config.dry_run,
        ctx.config.token_amount_sol,
        ctx.config.take_profit,
        ctx.config.stop_loss,
        ctx.config.max_hold
    ));
    manage_exits(ctx).await
}

pub async fn on_event(ctx: &AppContext, event: SnipeEvent) -> Result<()> {
    let logger = Logger::new("[SNIPE EVENT] => ".to_string());
    match event {
        SnipeEvent::NewPumpToken { mint, signature } => {
            logger.log(format!("new pump token {mint} sig={signature}"));
            try_buy(ctx, &mint, "create").await
        }
        SnipeEvent::PumpBuy {
            mint,
            buyer,
            copy_wallet,
            signature,
        } => {
            if copy_wallet {
                logger.log(format!("copy-wallet {buyer} bought {mint} sig={signature}"));
                try_buy(ctx, &mint, "copy").await
            } else {
                Ok(())
            }
        }
        SnipeEvent::NewRaydiumPool { mint, signature } => {
            logger.log(format!("new raydium pool around {mint} sig={signature}"));
            try_buy_amm(ctx, &mint).await
        }
    }
}

async fn try_buy(ctx: &AppContext, mint: &str, reason: &str) -> Result<()> {
    let logger = Logger::new("[SNIPE BUY] => ".to_string());
    let mint_pk = Pubkey::from_str(mint)?;
    if ctx.risk.positions.contains_key(&mint_pk) {
        return Ok(());
    }
    if let Err(reason) = ctx.risk.can_open(ctx.config.token_amount_lamports) {
        logger.log(format!("skip {mint}: {reason}"));
        return Ok(());
    }

    let info = get_pump_info(ctx.state.rpc_client.clone(), mint).await?;
    if info.complete {
        logger.log(format!("skip {mint}: already graduated"));
        return Ok(());
    }
    let liq_sol = info.virtual_sol_reserves as f64 / 1_000_000_000.0;
    if liq_sol < ctx.config.min_liquidity_sol {
        logger.log(format!(
            "skip {mint}: virtual sol {liq_sol:.4} < min {}",
            ctx.config.min_liquidity_sol
        ));
        return Ok(());
    }

    if ctx.config.skip_if_mint_authority {
        if let Ok(mint_acc) = token::mint_info(&ctx.state.rpc_nonblocking_client, &mint_pk).await {
            if mint_acc.mint_authority.is_some() {
                logger.log(format!("skip {mint}: mint authority still set"));
                return Ok(());
            }
        }
    }

    logger.log(format!(
        "buying {mint} reason={reason} sol={} dry_run={}",
        ctx.config.token_amount_sol, ctx.config.dry_run
    ));
    let cfg = SwapConfig {
        swap_direction: SwapDirection::Buy,
        in_type: SwapInType::Qty,
        amount_in: ctx.config.token_amount_sol,
        slippage: ctx.config.slippage_bps / 100,
        use_jito: ctx.config.use_jito,
    };
    match pump_fun::swap_with_ctx(ctx, mint, cfg).await {
        Ok(sigs) => {
            ctx.risk.record_success();
            let expected = math::pump_buy_tokens_out(
                ctx.config.token_amount_lamports,
                info.virtual_sol_reserves,
                info.virtual_token_reserves,
                math::default_pump_fee_bps(),
            );
            let price = math::pump_spot_price_sol_per_token(
                info.virtual_sol_reserves,
                info.virtual_token_reserves,
            );
            ctx.risk.open_position(Position::new(
                mint_pk,
                expected,
                ctx.config.token_amount_lamports,
                price,
            ));
            logger.log(format!("buy submitted {mint} sigs={sigs:?}"));
            Ok(())
        }
        Err(err) => {
            ctx.risk.record_failure();
            Err(anyhow!("buy {mint} failed: {err}"))
        }
    }
}

async fn try_buy_amm(ctx: &AppContext, mint: &str) -> Result<()> {
    let logger = Logger::new("[SNIPE AMM] => ".to_string());
    let mint_pk = Pubkey::from_str(mint)?;
    if ctx.risk.positions.contains_key(&mint_pk) {
        return Ok(());
    }
    if let Err(reason) = ctx.risk.can_open(ctx.config.token_amount_lamports) {
        logger.log(format!("skip amm {mint}: {reason}"));
        return Ok(());
    }
    let cfg = default_swap_config(
        ctx.config.token_amount_sol,
        ctx.config.slippage_bps / 100,
        ctx.config.use_jito,
        true,
    );
    match raydium::swap_mint_on_dex(ctx, mint, cfg, Some("Raydium,Raydium CLMM,Raydium CP")).await {
        Ok(sigs) => {
            ctx.risk.record_success();
            ctx.risk.open_position(Position::new(
                mint_pk,
                0,
                ctx.config.token_amount_lamports,
                1.0,
            ));
            logger.log(format!("amm buy submitted {mint} sigs={sigs:?}"));
            Ok(())
        }
        Err(err) => {
            ctx.risk.record_failure();
            Err(err)
        }
    }
}

pub async fn manage_exits(ctx: AppContext) -> Result<()> {
    let logger = Logger::new("[SNIPE EXIT] => ".to_string());
    loop {
        sleep(Duration::from_millis(800)).await;
        let mints: Vec<Pubkey> = ctx.risk.positions.iter().map(|e| *e.key()).collect();
        for mint in mints {
            let Some(mut pos) = ctx.risk.positions.get_mut(&mint) else {
                continue;
            };
            let mint_s = mint.to_string();
            let price = match current_price(&ctx, &mint_s).await {
                Ok(p) => p,
                Err(err) => {
                    logger.debug(format!("mark {mint_s}: {err}"));
                    continue;
                }
            };
            pos.mark(price);
            let reason = ctx.risk.should_exit(&pos, price);
            drop(pos);
            if let Some(reason) = reason {
                logger.log(format!(
                    "exiting {mint_s} reason={reason:?} price={price:.12}"
                ));
                if let Err(err) = sell(&ctx, &mint_s).await {
                    logger.error(format!("sell {mint_s} failed: {err}"));
                    ctx.risk.record_failure();
                    continue;
                }
                let cost = ctx
                    .risk
                    .positions
                    .get(&mint)
                    .map(|p| p.cost_lamports)
                    .unwrap_or(0);
                let proceeds = (price * cost as f64) as i64;
                let realized = proceeds - cost as i64;
                ctx.risk.close_position(&mint, realized);
                logger.log(format!(
                    "closed {mint_s} realized_lamports={realized} session_pnl={}",
                    ctx.risk.realized_pnl()
                ));
            }
        }
    }
}

async fn current_price(ctx: &AppContext, mint: &str) -> Result<f64> {
    if let Ok(info) = get_pump_info(ctx.state.rpc_client.clone(), mint).await {
        if !info.complete {
            return Ok(math::pump_spot_price_sol_per_token(
                info.virtual_sol_reserves,
                info.virtual_token_reserves,
            ));
        }
    }
    let (q, _) = crate::services::jupiter::quote(
        &ctx.http,
        mint,
        crate::common::constants::WSOL,
        1_000_000,
        ctx.config.slippage_bps,
        None,
    )
    .await?;
    let out: u64 = q.out_amount.parse().unwrap_or(0);
    Ok(out as f64 / 1_000_000.0)
}

async fn sell(ctx: &AppContext, mint: &str) -> Result<Vec<String>> {
    let cfg = SwapConfig {
        swap_direction: SwapDirection::Sell,
        in_type: SwapInType::Pct,
        amount_in: 1.0,
        slippage: ctx.config.slippage_bps / 100,
        use_jito: ctx.config.use_jito,
    };
    match pump_fun::swap_with_ctx(ctx, mint, cfg.clone()).await {
        Ok(sigs) => Ok(sigs),
        Err(_) => raydium::swap_mint_on_dex(ctx, mint, cfg, None).await,
    }
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
