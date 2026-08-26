use anyhow::Result;

use crate::common::types::SwapConfig;
use crate::context::AppContext;
use crate::dex::raydium::swap_mint_on_dex;

pub const ORCA_DEXES: &str = "Whirlpool,Orca V1,Orca V2";

pub async fn swap(ctx: &AppContext, mint: &str, swap_config: SwapConfig) -> Result<Vec<String>> {
    swap_mint_on_dex(ctx, mint, swap_config, Some(ORCA_DEXES)).await
}
