use anyhow::{anyhow, Result};
use solana_sdk::{commitment_config::CommitmentConfig, signature::Keypair};
use std::sync::Arc;

use crate::common::config::require;
use crate::common::types::{SwapConfig, SwapDirection, SwapInType};

#[derive(Clone)]
pub struct AppState {
    pub rpc_client: Arc<solana_client::rpc_client::RpcClient>,
    pub rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    pub wallet: Arc<Keypair>,
}

pub fn import_env_var(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("Environment variable {key} is not set"))
}

pub fn create_rpc_client() -> Result<Arc<solana_client::rpc_client::RpcClient>> {
    let rpc_https = require("RPC_HTTPS")?;
    let rpc_client = solana_client::rpc_client::RpcClient::new_with_commitment(
        rpc_https,
        CommitmentConfig::processed(),
    );
    Ok(Arc::new(rpc_client))
}

pub async fn create_nonblocking_rpc_client(
) -> Result<Arc<solana_client::nonblocking::rpc_client::RpcClient>> {
    let rpc_https = require("RPC_HTTPS")?;
    let rpc_client = solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
        rpc_https,
        CommitmentConfig::processed(),
    );
    Ok(Arc::new(rpc_client))
}

pub fn import_wallet() -> Result<Arc<Keypair>> {
    let priv_key = require("PRIVATE_KEY")?;
    let bytes = bs58::decode(priv_key.trim())
        .into_vec()
        .map_err(|e| anyhow!("invalid PRIVATE_KEY base58: {e}"))?;
    let wallet = Keypair::try_from(bytes.as_slice())
        .map_err(|e| anyhow!("invalid PRIVATE_KEY bytes: {e}"))?;
    Ok(Arc::new(wallet))
}

pub fn build_http_client(proxy: Option<&str>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10));
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    Ok(builder.build()?)
}

pub fn default_swap_config(
    amount_in: f64,
    slippage_pct: u64,
    use_jito: bool,
    buy: bool,
) -> SwapConfig {
    SwapConfig {
        swap_direction: if buy {
            SwapDirection::Buy
        } else {
            SwapDirection::Sell
        },
        in_type: SwapInType::Qty,
        amount_in,
        slippage: slippage_pct,
        use_jito,
    }
}
