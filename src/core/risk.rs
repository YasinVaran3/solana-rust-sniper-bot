use dashmap::DashMap;
use solana_sdk::pubkey::Pubkey;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use crate::common::config::AppConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    TakeProfit,
    StopLoss,
    TrailingStop,
    TimeStop,
}

#[derive(Debug, Clone)]
pub struct Position {
    pub mint: Pubkey,
    pub token_amount: u64,
    pub cost_lamports: u64,
    pub entry_price: f64,
    pub peak_price: f64,
    pub opened_at: Instant,
}

impl Position {
    pub fn new(mint: Pubkey, token_amount: u64, cost_lamports: u64, entry_price: f64) -> Self {
        Self {
            mint,
            token_amount,
            cost_lamports,
            entry_price,
            peak_price: entry_price,
            opened_at: Instant::now(),
        }
    }

    pub fn mark(&mut self, price: f64) {
        if price > self.peak_price {
            self.peak_price = price;
        }
    }

    pub fn multiple(&self, price: f64) -> f64 {
        if self.entry_price <= 0.0 {
            0.0
        } else {
            price / self.entry_price
        }
    }
}

pub struct RiskEngine {
    pub max_positions: usize,
    pub max_sol_per_trade: u64,
    pub max_daily_loss: u64,
    pub take_profit: f64,
    pub stop_loss: f64,
    pub trailing_stop_pct: f64,
    pub max_hold: Duration,
    realized_pnl: AtomicI64,
    consecutive_fails: AtomicU8,
    pub positions: DashMap<Pubkey, Position>,
}

impl RiskEngine {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            max_positions: config.max_positions,
            max_sol_per_trade: config.max_sol_per_trade_lamports,
            max_daily_loss: config.max_daily_loss_lamports,
            take_profit: config.take_profit,
            stop_loss: config.stop_loss,
            trailing_stop_pct: config.trailing_stop_pct,
            max_hold: config.max_hold,
            realized_pnl: AtomicI64::new(0),
            consecutive_fails: AtomicU8::new(0),
            positions: DashMap::new(),
        }
    }

    pub fn can_open(&self, sol_amount: u64) -> Result<(), String> {
        if self.circuit_open() {
            return Err("circuit breaker open after consecutive failures".into());
        }
        if self.positions.len() >= self.max_positions {
            return Err("max concurrent positions reached".into());
        }
        if sol_amount > self.max_sol_per_trade {
            return Err("trade size exceeds MAX_SOL_PER_TRADE".into());
        }
        let pnl = self.realized_pnl.load(Ordering::Relaxed);
        if pnl < 0 && pnl.unsigned_abs() >= self.max_daily_loss {
            return Err("daily loss limit reached".into());
        }
        Ok(())
    }

    pub fn open_position(&self, position: Position) {
        self.positions.insert(position.mint, position);
        self.consecutive_fails.store(0, Ordering::Relaxed);
    }

    pub fn close_position(&self, mint: &Pubkey, realized: i64) -> Option<Position> {
        self.realized_pnl.fetch_add(realized, Ordering::Relaxed);
        self.positions.remove(mint).map(|(_, p)| p)
    }

    pub fn record_failure(&self) {
        self.consecutive_fails.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_success(&self) {
        self.consecutive_fails.store(0, Ordering::Relaxed);
    }

    pub fn circuit_open(&self) -> bool {
        self.consecutive_fails.load(Ordering::Relaxed) >= 3
    }

    pub fn should_exit(&self, position: &Position, current_price: f64) -> Option<ExitReason> {
        let multiple = position.multiple(current_price);
        if multiple >= self.take_profit {
            return Some(ExitReason::TakeProfit);
        }
        if multiple <= self.stop_loss {
            return Some(ExitReason::StopLoss);
        }
        if position.opened_at.elapsed() >= self.max_hold {
            return Some(ExitReason::TimeStop);
        }
        if self.trailing_stop_pct > 0.0
            && position.peak_price > position.entry_price
            && current_price <= position.peak_price * (1.0 - self.trailing_stop_pct)
        {
            return Some(ExitReason::TrailingStop);
        }
        None
    }

    pub fn realized_pnl(&self) -> i64 {
        self.realized_pnl.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RiskEngine {
        RiskEngine {
            max_positions: 1,
            max_sol_per_trade: 1_000_000_000,
            max_daily_loss: 1_000_000_000,
            take_profit: 2.0,
            stop_loss: 0.5,
            trailing_stop_pct: 0.2,
            max_hold: Duration::from_secs(60),
            realized_pnl: AtomicI64::new(0),
            consecutive_fails: AtomicU8::new(0),
            positions: DashMap::new(),
        }
    }

    #[test]
    fn take_profit_hits() {
        let risk = sample();
        let pos = Position::new(Pubkey::new_unique(), 1, 1, 1.0);
        assert_eq!(risk.should_exit(&pos, 2.1), Some(ExitReason::TakeProfit));
        assert_eq!(risk.should_exit(&pos, 0.4), Some(ExitReason::StopLoss));
    }
}
