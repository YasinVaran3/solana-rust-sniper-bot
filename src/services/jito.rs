use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use solana_sdk::{
    pubkey::Pubkey,
    transaction::{Transaction, VersionedTransaction},
};
use std::{str::FromStr, sync::Arc, time::Duration};
use tokio::time::sleep;

use crate::common::constants::LAMPORTS_PER_SOL;
use crate::common::logger::Logger;

const DEFAULT_TIPS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEj",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqk4Br97bvoYBn5V3yctKbCv",
];

static TIP_ACCOUNTS: Mutex<Vec<Pubkey>> = Mutex::new(Vec::new());
static HTTP: Mutex<Option<reqwest::Client>> = Mutex::new(None);

pub async fn init_tip_accounts() -> Result<()> {
    let mut tips = Vec::new();
    for t in DEFAULT_TIPS {
        if let Ok(pk) = Pubkey::from_str(t) {
            tips.push(pk);
        }
    }
    *TIP_ACCOUNTS.lock() = tips;
    *HTTP.lock() = Some(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?,
    );
    Ok(())
}

pub async fn get_tip_account() -> Result<Pubkey> {
    let guard = TIP_ACCOUNTS.lock();
    if guard.is_empty() {
        drop(guard);
        init_tip_accounts().await?;
        let guard = TIP_ACCOUNTS.lock();
        return guard
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("no jito tip accounts"));
    }
    let idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as usize)
        % guard.len();
    Ok(guard[idx])
}

pub fn tip_lamports(tip_sol: f64) -> u64 {
    let capped = tip_sol.min(0.1).max(0.0);
    (capped * LAMPORTS_PER_SOL as f64).round() as u64
}

pub async fn get_tip_value() -> Result<f64> {
    Ok(crate::common::config::env_f64("JITO_TIP_VALUE", 0.0001))
}

fn http() -> Result<reqwest::Client> {
    HTTP.lock()
        .clone()
        .ok_or_else(|| anyhow!("jito http client not initialized"))
}

fn encode_legacy(tx: &Transaction) -> Result<String> {
    let bytes = bincode::serialize(tx)?;
    Ok(bs58::encode(bytes).into_string())
}

fn encode_versioned(tx: &VersionedTransaction) -> Result<String> {
    let bytes = bincode::serialize(tx)?;
    Ok(bs58::encode(bytes).into_string())
}

pub async fn send_bundle(engine_url: &str, txs: &[Transaction]) -> Result<String> {
    let encoded = txs
        .iter()
        .map(encode_legacy)
        .collect::<Result<Vec<_>>>()?;
    send_encoded_bundle(engine_url, encoded).await
}

pub async fn send_versioned_bundle(engine_url: &str, txs: &[VersionedTransaction]) -> Result<String> {
    let encoded = txs
        .iter()
        .map(encode_versioned)
        .collect::<Result<Vec<_>>>()?;
    send_encoded_bundle(engine_url, encoded).await
}

async fn send_encoded_bundle(engine_url: &str, encoded: Vec<String>) -> Result<String> {
    let url = format!("{}/api/v1/bundles", engine_url.trim_end_matches('/'));
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendBundle",
        "params": [encoded]
    });
    let resp: Value = http()?
        .post(url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(anyhow!("jito sendBundle error: {err}"));
    }
    resp.get("result")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("jito sendBundle missing result: {resp}"))
}

pub async fn get_bundle_statuses(engine_url: &str, bundle_id: &str) -> Result<Value> {
    let url = format!("{}/api/v1/bundles", engine_url.trim_end_matches('/'));
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBundleStatuses",
        "params": [[bundle_id]]
    });
    let resp: Value = http()?
        .post(url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    Ok(resp)
}

pub async fn wait_for_bundle_confirmation(
    engine_url: &str,
    bundle_id: &str,
    interval: Duration,
    timeout: Duration,
    logger: &Logger,
) -> Result<Vec<String>> {
    let start = tokio::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(anyhow!("jito bundle {bundle_id} confirmation timed out"));
        }
        match get_bundle_statuses(engine_url, bundle_id).await {
            Ok(resp) => {
                if let Some(value) = resp
                    .pointer("/result/value")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                {
                    if let Some(err) = value.get("err") {
                        if !err.is_null() {
                            return Err(anyhow!("bundle landed with error: {err}"));
                        }
                    }
                    if let Some(txs) = value.get("transactions").and_then(|v| v.as_array()) {
                        let sigs: Vec<String> = txs
                            .iter()
                            .filter_map(|t| t.as_str().map(|s| s.to_string()))
                            .collect();
                        if !sigs.is_empty() {
                            return Ok(sigs);
                        }
                    }
                }
            }
            Err(err) => logger.log(format!("bundle status poll: {err}")),
        }
        sleep(interval).await;
    }
}

pub fn client_arc() -> Result<Arc<reqwest::Client>> {
    Ok(Arc::new(http()?))
}
