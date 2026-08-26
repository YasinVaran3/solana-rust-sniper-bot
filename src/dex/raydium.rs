use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};
use std::{str::FromStr, sync::Arc};

use crate::common::constants::WSOL;
use crate::common::logger::Logger;
use crate::common::types::SwapConfig;
use crate::context::AppContext;
use crate::core::tx;
use crate::services::jupiter;

pub const AMM_PROGRAM: &str = crate::common::constants::RAYDIUM_AMM_V4;

#[derive(Debug, Deserialize)]
pub struct PoolInfo {
    pub success: bool,
    pub data: PoolData,
}

#[derive(Debug, Deserialize)]
pub struct PoolData {
    pub data: Vec<Pool>,
}

impl PoolData {
    pub fn get_pool(&self) -> Option<Pool> {
        self.data.first().cloned()
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Pool {
    pub id: String,
    #[serde(rename = "programId")]
    pub program_id: String,
    #[serde(rename = "mintA")]
    pub mint_a: Mint,
    #[serde(rename = "mintB")]
    pub mint_b: Mint,
    #[serde(rename = "marketId")]
    pub market_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Mint {
    pub address: String,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
}

pub struct Raydium {
    pub rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    pub rpc_client: Option<Arc<solana_client::rpc_client::RpcClient>>,
    pub keypair: Arc<Keypair>,
    pub pool_id: Option<String>,
}

impl Raydium {
    pub fn new(
        rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
        rpc_client: Arc<solana_client::rpc_client::RpcClient>,
        keypair: Arc<Keypair>,
    ) -> Self {
        Self {
            rpc_nonblocking_client,
            keypair,
            rpc_client: Some(rpc_client),
            pool_id: None,
        }
    }

    pub async fn swap_via_jupiter(
        &self,
        ctx: &AppContext,
        mint: &str,
        swap_config: SwapConfig,
    ) -> Result<Vec<String>> {
        swap_mint_on_dex(ctx, mint, swap_config, Some("Raydium,Raydium CLMM,Raydium CP")).await
    }
}

pub async fn swap_mint_on_dex(
    ctx: &AppContext,
    mint: &str,
    swap_config: SwapConfig,
    dexes: Option<&str>,
) -> Result<Vec<String>> {
    let logger = Logger::new("[SWAP JUPITER] => ".to_string());
    let owner = ctx.state.wallet.pubkey();
    let (input_mint, output_mint, amount) = match swap_config.swap_direction {
        crate::common::types::SwapDirection::Buy => {
            (WSOL, mint, (swap_config.amount_in * 1_000_000_000.0).round() as u64)
        }
        crate::common::types::SwapDirection::Sell => {
            let ata = spl_associated_token_account::get_associated_token_address(
                &owner,
                &Pubkey::from_str(mint)?,
            );
            let bal = crate::core::token::token_balance(&ctx.state.rpc_nonblocking_client, &ata).await?;
            (mint, WSOL, bal)
        }
    };
    if amount == 0 {
        return Err(anyhow!("swap amount is 0"));
    }
    let (q, raw) = jupiter::quote(
        &ctx.http,
        input_mint,
        output_mint,
        amount,
        swap_config.slippage_bps(),
        dexes,
    )
    .await?;
    logger.log(format!(
        "quote {} -> {} in={} out={}",
        q.input_mint, q.output_mint, q.in_amount, q.out_amount
    ));
    let encoded = jupiter::swap_transaction(
        &ctx.http,
        &raw,
        &owner.to_string(),
        true,
        Some(crate::services::jito::tip_lamports(ctx.config.jito_tip_sol)),
    )
    .await?;
    let tx = jupiter::decode_swap_tx(&encoded)?;
    tx::send_versioned(
        &ctx.state.rpc_client,
        tx,
        swap_config.use_jito,
        &ctx.config.jito_block_engine_url,
        ctx.config.dry_run,
        &logger,
    )
    .await
}

pub async fn get_pool_info(http: &reqwest::Client, mint1: &str, mint2: &str) -> Result<PoolData> {
    let result = http
        .get("https://api-v3.raydium.io/pools/info/mint")
        .query(&[
            ("mint1", mint1),
            ("mint2", mint2),
            ("poolType", "standard"),
            ("poolSortField", "default"),
            ("sortType", "desc"),
            ("pageSize", "1"),
            ("page", "1"),
        ])
        .send()
        .await?
        .json::<PoolInfo>()
        .await
        .context("Failed to parse pool info JSON")?;
    if !result.success {
        return Err(anyhow!("raydium pool api returned success=false"));
    }
    Ok(result.data)
}

pub async fn pool_exists(http: &reqwest::Client, mint: &str) -> bool {
    get_pool_info(http, WSOL, mint)
        .await
        .ok()
        .and_then(|d| d.get_pool())
        .is_some()
}
