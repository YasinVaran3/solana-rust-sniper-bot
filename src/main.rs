#![allow(deprecated)]

use anyhow::Result;
use clap::{Parser, ValueEnum};
use raydium_pump_snipe_bot::{
    common::{
        config::AppConfig,
        logger::Logger,
        utils::{
            build_http_client, create_nonblocking_rpc_client, create_rpc_client, import_wallet,
            AppState,
        },
    },
    context::AppContext,
    core::risk::RiskEngine,
    engine::{arbitrage, monitor, snipe},
    services::jito,
};
use solana_sdk::signer::Signer;
use std::{sync::Arc, time::Instant};

#[derive(Clone, Debug, ValueEnum)]
enum Mode {
    Snipe,
    Arb,
    All,
}

#[derive(Parser, Debug)]
#[command(name = "raydium-pump-snipe-bot", about = "Solana sniper + cross-DEX arbitrage")]
struct Cli {
    #[arg(long, value_enum, default_value_t = Mode::All)]
    mode: Mode,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let logger = Logger::new("[INIT] => ".to_string());

    let config = Arc::new(AppConfig::from_env()?);
    let rpc_client = create_rpc_client()?;
    let rpc_nonblocking_client = create_nonblocking_rpc_client().await?;
    let wallet = import_wallet()?;
    let http = build_http_client(config.http_proxy.as_deref())?;

    if config.use_jito {
        jito::init_tip_accounts().await?;
    }

    let state = AppState {
        rpc_client,
        rpc_nonblocking_client,
        wallet: wallet.clone(),
    };
    let ctx = AppContext {
        risk: Arc::new(RiskEngine::from_config(&config)),
        state,
        config: config.clone(),
        http,
        started: Instant::now(),
    };

    logger.log(format!(
        "ready\n\t\t\t\t [mode]: {:?}\n\t\t\t\t [rpc]: {}\n\t\t\t\t [wss]: {}\n\t\t\t\t [wallet]: {}\n\t\t\t\t [dry_run]: {}\n\t\t\t\t [slippage_bps]: {}\n\t\t\t\t [snipe_sol]: {}\n",
        cli.mode,
        config.rpc_https,
        config.rpc_wss,
        wallet.pubkey(),
        config.dry_run,
        config.slippage_bps,
        config.token_amount_sol
    ));

    match cli.mode {
        Mode::Snipe => {
            tokio::select! {
                res = monitor::pumpfun_monitor(ctx.clone()) => res,
                res = monitor::raydium_monitor(ctx.clone()) => res,
                res = snipe::run(ctx) => res,
            }
        }
        Mode::Arb => arbitrage::run(ctx).await,
        Mode::All => {
            tokio::select! {
                res = monitor::pumpfun_monitor(ctx.clone()) => res,
                res = monitor::raydium_monitor(ctx.clone()) => res,
                res = snipe::run(ctx.clone()) => res,
                res = arbitrage::run(ctx) => res,
            }
        }
    }
}
