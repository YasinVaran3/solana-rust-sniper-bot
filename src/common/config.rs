use anyhow::{anyhow, Result};
use solana_sdk::pubkey::Pubkey;
use std::{env, str::FromStr, time::Duration};

use crate::common::constants::LAMPORTS_PER_SOL;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub dry_run: bool,
    pub slippage_bps: u64,
    pub token_amount_sol: f64,
    pub token_amount_lamports: u64,
    pub max_positions: usize,
    pub max_sol_per_trade_lamports: u64,
    pub max_daily_loss_lamports: u64,
    pub take_profit: f64,
    pub stop_loss: f64,
    pub trailing_stop_pct: f64,
    pub max_hold: Duration,
    pub copy_wallets: Vec<Pubkey>,
    pub min_liquidity_sol: f64,
    pub skip_if_mint_authority: bool,
    pub arb_enabled: bool,
    pub arb_min_profit_lamports: u64,
    pub arb_amount_lamports: u64,
    pub arb_interval: Duration,
    pub arb_pairs: Vec<String>,
    pub use_jito: bool,
    pub jito_block_engine_url: String,
    pub jito_tip_sol: f64,
    pub unit_price: u64,
    pub unit_limit: u32,
    pub http_proxy: Option<String>,
    pub rpc_https: String,
    pub rpc_wss: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let token_amount_sol = env_f64("TOKEN_AMOUNT", 0.05);
        let slippage_pct = env_u64("SLIPPAGE", 10);
        Ok(Self {
            dry_run: env_bool("DRY_RUN", true),
            slippage_bps: slippage_pct.saturating_mul(100),
            token_amount_sol,
            token_amount_lamports: sol_to_lamports(token_amount_sol),
            max_positions: env_u64("MAX_POSITIONS", 3) as usize,
            max_sol_per_trade_lamports: sol_to_lamports(env_f64("MAX_SOL_PER_TRADE", 0.2)),
            max_daily_loss_lamports: sol_to_lamports(env_f64("MAX_DAILY_LOSS_SOL", 0.5)),
            take_profit: env_f64("TP", 2.0),
            stop_loss: env_f64("SL", 0.5),
            trailing_stop_pct: env_f64("TRAILING_STOP_PCT", 0.2),
            max_hold: Duration::from_secs(env_u64("TIME_EXCEED", 90)),
            copy_wallets: parse_pubkeys(&env_string("COPY_WALLETS", "")),
            min_liquidity_sol: env_f64("MIN_LIQUIDITY_SOL", 0.5),
            skip_if_mint_authority: env_bool("SKIP_IF_MINT_AUTHORITY", true),
            arb_enabled: env_bool("ARB_ENABLED", true),
            arb_min_profit_lamports: sol_to_lamports(env_f64("ARB_MIN_PROFIT_SOL", 0.005)),
            arb_amount_lamports: sol_to_lamports(env_f64("ARB_AMOUNT_SOL", 0.25)),
            arb_interval: Duration::from_millis(env_u64("ARB_INTERVAL_MS", 500)),
            arb_pairs: env_string("ARB_PAIRS", "SOL-USDC,SOL-USDT")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            use_jito: env_bool("USE_JITO", true),
            jito_block_engine_url: env_string(
                "JITO_BLOCK_ENGINE_URL",
                "https://ny.mainnet.block-engine.jito.wtf",
            ),
            jito_tip_sol: env_f64("JITO_TIP_VALUE", 0.0001),
            unit_price: env_u64("UNIT_PRICE", 1_000),
            unit_limit: env_u64("UNIT_LIMIT", 400_000) as u32,
            http_proxy: env::var("HTTP_PROXY").ok().filter(|s| !s.is_empty()),
            rpc_https: require("RPC_HTTPS")?,
            rpc_wss: require("RPC_WSS")?,
        })
    }
}

pub fn require(key: &str) -> Result<String> {
    env::var(key).map_err(|_| anyhow!("environment variable {key} is not set"))
}

pub fn env_string(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

pub fn env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
    }
}

pub fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn env_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol.max(0.0) * LAMPORTS_PER_SOL as f64).round() as u64
}

pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL as f64
}

fn parse_pubkeys(raw: &str) -> Vec<Pubkey> {
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| Pubkey::from_str(s).ok())
        .collect()
}
