use crate::common::constants::{PUMP_FEE_BPS, TEN_THOUSAND};

pub fn apply_bps(amount: u64, bps: u64) -> u64 {
    amount.saturating_mul(bps) / TEN_THOUSAND
}

pub fn amount_after_fee(amount: u64, fee_bps: u64) -> u64 {
    amount.saturating_sub(apply_bps(amount, fee_bps))
}

pub fn min_amount_with_slippage(amount: u64, slippage_bps: u64) -> u64 {
    amount
        .saturating_mul(TEN_THOUSAND.saturating_sub(slippage_bps))
        / TEN_THOUSAND
}

pub fn max_amount_with_slippage(amount: u64, slippage_bps: u64) -> u64 {
    amount
        .saturating_mul(TEN_THOUSAND.saturating_add(slippage_bps))
        / TEN_THOUSAND
}

/// Constant-product buy on Pump.fun virtual reserves.
/// `sol_in` is lamports; returns expected token raw amount out.
pub fn pump_buy_tokens_out(
    sol_in: u64,
    virtual_sol: u64,
    virtual_token: u64,
    fee_bps: u64,
) -> u64 {
    if sol_in == 0 || virtual_sol == 0 || virtual_token == 0 {
        return 0;
    }
    let sol_after_fee = amount_after_fee(sol_in, fee_bps) as u128;
    let v_sol = virtual_sol as u128;
    let v_token = virtual_token as u128;
    let k = v_sol.saturating_mul(v_token);
    let new_sol = v_sol.saturating_add(sol_after_fee);
    if new_sol == 0 {
        return 0;
    }
    let new_token = k / new_sol;
    v_token.saturating_sub(new_token) as u64
}

/// Constant-product sell on Pump.fun virtual reserves.
/// Returns expected lamports out after the protocol fee.
pub fn pump_sell_sol_out(
    tokens_in: u64,
    virtual_sol: u64,
    virtual_token: u64,
    fee_bps: u64,
) -> u64 {
    if tokens_in == 0 || virtual_sol == 0 || virtual_token == 0 {
        return 0;
    }
    let t_in = tokens_in as u128;
    let v_sol = virtual_sol as u128;
    let v_token = virtual_token as u128;
    let k = v_sol.saturating_mul(v_token);
    let new_token = v_token.saturating_add(t_in);
    if new_token == 0 {
        return 0;
    }
    let new_sol = k / new_token;
    let sol_out = v_sol.saturating_sub(new_sol) as u64;
    amount_after_fee(sol_out, fee_bps)
}

pub fn pump_spot_price_sol_per_token(virtual_sol: u64, virtual_token: u64) -> f64 {
    if virtual_token == 0 {
        0.0
    } else {
        virtual_sol as f64 / virtual_token as f64
    }
}

pub fn net_profit(buy_cost: u64, sell_proceeds: u64, extra_fees: u64) -> i64 {
    sell_proceeds as i64 - buy_cost as i64 - extra_fees as i64
}

pub fn default_pump_fee_bps() -> u64 {
    PUMP_FEE_BPS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_then_sell_loses_the_fee() {
        let v_sol = 30 * 1_000_000_000;
        let v_token = 1_073_000_000_000_000;
        let sol_in = 100_000_000;
        let tokens = pump_buy_tokens_out(sol_in, v_sol, v_token, 100);
        assert!(tokens > 0);
        let sol_out = pump_sell_sol_out(tokens, v_sol + sol_in, v_token - tokens, 100);
        assert!(sol_out < sol_in);
    }

    #[test]
    fn slippage_bounds() {
        assert_eq!(min_amount_with_slippage(10_000, 100), 9_900);
        assert_eq!(max_amount_with_slippage(10_000, 100), 10_100);
    }
}
