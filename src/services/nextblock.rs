use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use solana_sdk::transaction::VersionedTransaction;

/// Optional NextBlock / fast-send endpoint. Used when NEXTBLOCK_URL is set.
pub async fn send_tx(url: &str, tx: &VersionedTransaction) -> Result<String> {
    let bytes = bincode::serialize(tx)?;
    let encoded = bs58::encode(bytes).into_string();
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [encoded, { "encoding": "base58", "skipPreflight": true }]
    });
    let resp: Value = reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(anyhow!("nextblock error: {err}"));
    }
    resp.get("result")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|_| anyhow!("nextblock missing result"))
}
