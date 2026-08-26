use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
pub const TEN_THOUSAND: u64 = 10_000;

pub const WSOL: &str = "So11111111111111111111111111111111111111112";
pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";

pub const PUMP_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const PUMP_GLOBAL: &str = "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf";
pub const PUMP_FEE_RECIPIENT: &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
pub const PUMP_EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";

pub const RAYDIUM_AMM_V4: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
pub const COMPUTE_BUDGET: &str = "ComputeBudget111111111111111111111111111111";

pub const PUMP_BUY_DISCRIMINATOR: u64 = 16927863322537952870;
pub const PUMP_SELL_DISCRIMINATOR: u64 = 12502976635542562355;

pub const PUMP_FEE_BPS: u64 = 100; // 1%

pub fn pubkey(s: &str) -> Pubkey {
    Pubkey::from_str(s).expect("invalid hardcoded pubkey")
}

pub fn wsol() -> Pubkey {
    pubkey(WSOL)
}

pub fn pump_program() -> Pubkey {
    pubkey(PUMP_PROGRAM)
}

pub fn raydium_amm() -> Pubkey {
    pubkey(RAYDIUM_AMM_V4)
}

pub fn compute_budget() -> Pubkey {
    pubkey(COMPUTE_BUDGET)
}

pub fn pair_mints(pair: &str) -> Option<(&'static str, &'static str)> {
    match pair.to_uppercase().as_str() {
        "SOL-USDC" | "USDC-SOL" => Some((WSOL, USDC)),
        "SOL-USDT" | "USDT-SOL" => Some((WSOL, USDT)),
        _ => None,
    }
}
