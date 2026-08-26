use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use solana_sdk::{
    instruction::Instruction,
    message::AddressLookupTableAccount,
    pubkey::Pubkey,
};

use crate::common::types::Quote;
use crate::core::tx;

const QUOTE_URL: &str = "https://lite-api.jup.ag/swap/v1/quote";
const SWAP_URL: &str = "https://lite-api.jup.ag/swap/v1/swap";
const SWAP_IX_URL: &str = "https://lite-api.jup.ag/swap/v1/swap-instructions";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JupiterQuote {
    pub input_mint: String,
    pub output_mint: String,
    pub in_amount: String,
    pub out_amount: String,
    #[serde(default)]
    pub price_impact_pct: String,
}

impl JupiterQuote {
    pub fn to_quote(&self, venue: &str) -> Quote {
        Quote {
            venue: venue.to_string(),
            input_mint: self.input_mint.clone(),
            output_mint: self.output_mint.clone(),
            amount_in: self.in_amount.parse().unwrap_or(0),
            amount_out: self.out_amount.parse().unwrap_or(0),
            price_impact_pct: self.price_impact_pct.parse().unwrap_or(0.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwapInstructions {
    pub instructions: Vec<Instruction>,
    pub lookup_table_addresses: Vec<Pubkey>,
}

pub async fn quote(
    http: &reqwest::Client,
    input_mint: &str,
    output_mint: &str,
    amount: u64,
    slippage_bps: u64,
    dexes: Option<&str>,
) -> Result<(JupiterQuote, Value)> {
    let mut req = http.get(QUOTE_URL).query(&[
        ("inputMint", input_mint),
        ("outputMint", output_mint),
        ("amount", &amount.to_string()),
        ("slippageBps", &slippage_bps.to_string()),
        ("restrictIntermediateTokens", "true"),
    ]);
    if let Some(dexes) = dexes {
        req = req.query(&[("dexes", dexes)]);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "jupiter quote failed: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }
    let raw: Value = resp.json().await?;
    let parsed: JupiterQuote = serde_json::from_value(raw.clone())?;
    Ok((parsed, raw))
}

pub async fn swap_transaction(
    http: &reqwest::Client,
    quote_response: &Value,
    user_public_key: &str,
    wrap_and_unwrap_sol: bool,
    tip_lamports: Option<u64>,
) -> Result<String> {
    let mut body = json!({
        "quoteResponse": quote_response,
        "userPublicKey": user_public_key,
        "wrapAndUnwrapSol": wrap_and_unwrap_sol,
        "dynamicComputeUnitLimit": true,
    });
    if let Some(tip) = tip_lamports {
        body["prioritizationFeeLamports"] = json!({ "jitoTipLamports": tip });
    }
    let resp = http.post(SWAP_URL).json(&body).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "jupiter swap failed: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }
    let v: Value = resp.json().await?;
    v.get("swapTransaction")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("jupiter swap missing swapTransaction"))
}

pub async fn swap_instructions(
    http: &reqwest::Client,
    quote_response: &Value,
    user_public_key: &str,
) -> Result<SwapInstructions> {
    let body = json!({
        "quoteResponse": quote_response,
        "userPublicKey": user_public_key,
        "wrapAndUnwrapSol": true,
        "dynamicComputeUnitLimit": true,
    });
    let resp = http.post(SWAP_IX_URL).json(&body).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "jupiter swap-instructions failed: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }
    let v: Value = resp.json().await?;
    let mut instructions = Vec::new();
    for key in [
        "computeBudgetInstructions",
        "setupInstructions",
    ] {
        if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
            for ix in arr {
                instructions.push(parse_ix(ix)?);
            }
        }
    }
    if let Some(ix) = v.get("swapInstruction") {
        instructions.push(parse_ix(ix)?);
    }
    if let Some(ix) = v.get("cleanupInstruction") {
        instructions.push(parse_ix(ix)?);
    }
    let lookup_table_addresses = v
        .get("addressLookupTableAddresses")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    Ok(SwapInstructions {
        instructions,
        lookup_table_addresses,
    })
}

fn parse_ix(ix: &Value) -> Result<Instruction> {
    let program_id = ix
        .get("programId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ix missing programId"))?;
    let accounts = ix
        .get("accounts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let data = ix
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ix missing data"))?;
    tx::ix_from_json(program_id, &accounts, data)
}

pub async fn load_address_lookup_tables(
    rpc: &solana_client::nonblocking::rpc_client::RpcClient,
    addresses: &[Pubkey],
) -> Result<Vec<AddressLookupTableAccount>> {
    let mut out = Vec::new();
    for addr in addresses {
        let account = rpc.get_account(addr).await?;
        match solana_sdk::address_lookup_table::state::AddressLookupTable::deserialize(&account.data)
        {
            Ok(table) => out.push(AddressLookupTableAccount {
                key: *addr,
                addresses: table.addresses.to_vec(),
            }),
            Err(_) => continue,
        }
    }
    Ok(out)
}

pub fn decode_swap_tx(b64: &str) -> Result<solana_sdk::transaction::VersionedTransaction> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
        .map_err(|e| anyhow!("invalid jupiter tx encoding: {e}"))?;
    bincode::deserialize(&bytes).map_err(|e| anyhow!("invalid jupiter tx: {e}"))
}
