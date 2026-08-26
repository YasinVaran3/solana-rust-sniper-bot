use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapDirection {
    Buy,
    Sell,
}

impl AsRef<str> for SwapDirection {
    fn as_ref(&self) -> &str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapInType {
    Qty,
    Pct,
}

#[derive(Clone, Debug)]
pub struct SwapConfig {
    pub swap_direction: SwapDirection,
    pub in_type: SwapInType,
    pub amount_in: f64,
    pub slippage: u64,
    pub use_jito: bool,
}

impl SwapConfig {
    pub fn slippage_bps(&self) -> u64 {
        self.slippage.saturating_mul(100)
    }
}

#[derive(Clone, Debug)]
pub struct Quote {
    pub venue: String,
    pub input_mint: String,
    pub output_mint: String,
    pub amount_in: u64,
    pub amount_out: u64,
    pub price_impact_pct: f64,
}

impl Quote {
    pub fn out_per_in(&self) -> f64 {
        if self.amount_in == 0 {
            0.0
        } else {
            self.amount_out as f64 / self.amount_in as f64
        }
    }
}
