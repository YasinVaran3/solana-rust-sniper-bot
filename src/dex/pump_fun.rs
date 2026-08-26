use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_program,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::instruction::close_account;
use std::{str::FromStr, sync::Arc};

use crate::common::constants::{
    pump_program, PUMP_BUY_DISCRIMINATOR, PUMP_SELL_DISCRIMINATOR, WSOL,
};
use crate::common::logger::Logger;
use crate::common::types::{Quote, SwapConfig, SwapDirection};
use crate::core::{
    math::{self, max_amount_with_slippage, min_amount_with_slippage},
    token, tx,
};
use crate::context::AppContext;

pub const PUMP_PROGRAM: &str = crate::common::constants::PUMP_PROGRAM;
pub const PUMP_GLOBAL: &str = crate::common::constants::PUMP_GLOBAL;
pub const PUMP_FEE_RECIPIENT: &str = crate::common::constants::PUMP_FEE_RECIPIENT;
pub const PUMP_ACCOUNT: &str = crate::common::constants::PUMP_EVENT_AUTHORITY;
pub const PUMP_BUY_METHOD: u64 = PUMP_BUY_DISCRIMINATOR;
pub const PUMP_SELL_METHOD: u64 = PUMP_SELL_DISCRIMINATOR;
pub const TEN_THOUSAND: u64 = crate::common::constants::TEN_THOUSAND;
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const RENT_PROGRAM: &str = "SysvarRent111111111111111111111111111111111";
pub const ASSOCIATED_TOKEN_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

pub struct Pump {
    pub rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    pub keypair: Arc<Keypair>,
    pub rpc_client: Option<Arc<solana_client::rpc_client::RpcClient>>,
}

impl Pump {
    pub fn new(
        rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
        rpc_client: Arc<solana_client::rpc_client::RpcClient>,
        keypair: Arc<Keypair>,
    ) -> Self {
        Self {
            rpc_nonblocking_client,
            keypair,
            rpc_client: Some(rpc_client),
        }
    }

    pub async fn swap(&self, mint: &str, swap_config: SwapConfig) -> Result<Vec<String>> {
        let logger = Logger::new("[SWAP IN PUMP.FUN] => ".to_string());
        let rpc = self
            .rpc_client
            .as_ref()
            .ok_or_else(|| anyhow!("rpc client missing"))?;
        let owner = self.keypair.pubkey();
        let mint_pk = Pubkey::from_str(mint)?;
        let token_ata = get_associated_token_address(&owner, &mint_pk);
        let pump_program_id = pump_program();
        let (bonding_curve, associated_bonding_curve, curve) =
            get_bonding_curve_account(rpc.clone(), &mint_pk, &pump_program_id).await?;

        if curve.complete {
            return Err(anyhow!(
                "bonding curve complete; route this mint through an AMM/Jupiter instead"
            ));
        }

        let mut instructions = Vec::new();
        match swap_config.swap_direction {
            SwapDirection::Buy => {
                if !token::account_exists(&self.rpc_nonblocking_client, &token_ata).await? {
                    instructions.push(create_associated_token_account(
                        &owner,
                        &owner,
                        &mint_pk,
                        &spl_token::ID,
                    ));
                }
                let sol_in = (swap_config.amount_in * 1_000_000_000.0).round() as u64;
                let expected = math::pump_buy_tokens_out(
                    sol_in,
                    curve.virtual_sol_reserves,
                    curve.virtual_token_reserves,
                    math::default_pump_fee_bps(),
                );
                let min_tokens = min_amount_with_slippage(expected, swap_config.slippage_bps());
                let max_sol = max_amount_with_slippage(sol_in, swap_config.slippage_bps());
                logger.log(format!(
                    "buy mint={mint} sol_in={sol_in} expected_tokens={expected} min_tokens={min_tokens}"
                ));
                instructions.push(pump_buy_instruction(
                    &owner,
                    &mint_pk,
                    &bonding_curve,
                    &associated_bonding_curve,
                    &token_ata,
                    curve.creator,
                    min_tokens.max(1),
                    max_sol.max(sol_in),
                )?);
            }
            SwapDirection::Sell => {
                let tokens_in = if swap_config.amount_in > 0.0 && swap_config.amount_in <= 1.0
                    && matches!(swap_config.in_type, crate::common::types::SwapInType::Pct)
                {
                    let bal = token::token_balance(&self.rpc_nonblocking_client, &token_ata).await?;
                    (bal as f64 * swap_config.amount_in) as u64
                } else if swap_config.amount_in >= 1.0 {
                    swap_config.amount_in as u64
                } else {
                    token::token_balance(&self.rpc_nonblocking_client, &token_ata).await?
                };
                if tokens_in == 0 {
                    return Err(anyhow!("no tokens to sell for {mint}"));
                }
                let expected_sol = math::pump_sell_sol_out(
                    tokens_in,
                    curve.virtual_sol_reserves,
                    curve.virtual_token_reserves,
                    math::default_pump_fee_bps(),
                );
                let min_sol = min_amount_with_slippage(expected_sol, swap_config.slippage_bps());
                logger.log(format!(
                    "sell mint={mint} tokens_in={tokens_in} expected_sol={expected_sol} min_sol={min_sol}"
                ));
                instructions.push(pump_sell_instruction(
                    &owner,
                    &mint_pk,
                    &bonding_curve,
                    &associated_bonding_curve,
                    &token_ata,
                    curve.creator,
                    tokens_in,
                    min_sol,
                )?);
                instructions.push(close_account(
                    &spl_token::ID,
                    &token_ata,
                    &owner,
                    &owner,
                    &[],
                )?);
            }
        }

        tx::new_signed_and_send(
            rpc,
            &self.keypair,
            instructions,
            swap_config.use_jito,
            1_000,
            400_000,
            0.0001,
            "https://ny.mainnet.block-engine.jito.wtf",
            false,
            &logger,
        )
        .await
    }

    pub async fn quote_buy(&self, mint: &str, lamports_in: u64) -> Result<Quote> {
        let rpc = self
            .rpc_client
            .as_ref()
            .ok_or_else(|| anyhow!("rpc client missing"))?;
        let info = get_pump_info(rpc.clone(), mint).await?;
        if info.complete {
            return Err(anyhow!("curve complete"));
        }
        let amount_out = math::pump_buy_tokens_out(
            lamports_in,
            info.virtual_sol_reserves,
            info.virtual_token_reserves,
            math::default_pump_fee_bps(),
        );
        Ok(Quote {
            venue: "pump.fun".into(),
            input_mint: WSOL.into(),
            output_mint: mint.to_string(),
            amount_in: lamports_in,
            amount_out,
            price_impact_pct: 0.0,
        })
    }

    pub async fn quote_sell(&self, mint: &str, tokens_in: u64) -> Result<Quote> {
        let rpc = self
            .rpc_client
            .as_ref()
            .ok_or_else(|| anyhow!("rpc client missing"))?;
        let info = get_pump_info(rpc.clone(), mint).await?;
        let amount_out = math::pump_sell_sol_out(
            tokens_in,
            info.virtual_sol_reserves,
            info.virtual_token_reserves,
            math::default_pump_fee_bps(),
        );
        Ok(Quote {
            venue: "pump.fun".into(),
            input_mint: mint.to_string(),
            output_mint: WSOL.into(),
            amount_in: tokens_in,
            amount_out,
            price_impact_pct: 0.0,
        })
    }
}

impl Pump {
    pub fn from_ctx(ctx: &AppContext) -> Self {
        Self::new(
            ctx.state.rpc_nonblocking_client.clone(),
            ctx.state.rpc_client.clone(),
            ctx.state.wallet.clone(),
        )
    }
}

pub async fn swap_with_ctx(ctx: &AppContext, mint: &str, swap_config: SwapConfig) -> Result<Vec<String>> {
    let logger = Logger::new("[SWAP IN PUMP.FUN] => ".to_string());
    let pump = Pump::from_ctx(ctx);
    let rpc = pump.rpc_client.as_ref().unwrap();
    let owner = pump.keypair.pubkey();
    let mint_pk = Pubkey::from_str(mint)?;
    let token_ata = get_associated_token_address(&owner, &mint_pk);
    let pump_program_id = pump_program();
    let (bonding_curve, associated_bonding_curve, curve) =
        get_bonding_curve_account(rpc.clone(), &mint_pk, &pump_program_id).await?;
    if curve.complete {
        return Err(anyhow!("bonding curve complete"));
    }

    let mut instructions = Vec::new();
    match swap_config.swap_direction {
        SwapDirection::Buy => {
            if !token::account_exists(&pump.rpc_nonblocking_client, &token_ata).await? {
                instructions.push(create_associated_token_account(
                    &owner,
                    &owner,
                    &mint_pk,
                    &spl_token::ID,
                ));
            }
            let sol_in = (swap_config.amount_in * 1_000_000_000.0).round() as u64;
            let expected = math::pump_buy_tokens_out(
                sol_in,
                curve.virtual_sol_reserves,
                curve.virtual_token_reserves,
                math::default_pump_fee_bps(),
            );
            let min_tokens = min_amount_with_slippage(expected, swap_config.slippage_bps()).max(1);
            let max_sol = max_amount_with_slippage(sol_in, swap_config.slippage_bps()).max(sol_in);
            instructions.push(pump_buy_instruction(
                &owner,
                &mint_pk,
                &bonding_curve,
                &associated_bonding_curve,
                &token_ata,
                curve.creator,
                min_tokens,
                max_sol,
            )?);
        }
        SwapDirection::Sell => {
            let tokens_in = token::token_balance(&pump.rpc_nonblocking_client, &token_ata).await?;
            if tokens_in == 0 {
                return Err(anyhow!("no tokens to sell"));
            }
            let expected_sol = math::pump_sell_sol_out(
                tokens_in,
                curve.virtual_sol_reserves,
                curve.virtual_token_reserves,
                math::default_pump_fee_bps(),
            );
            let min_sol = min_amount_with_slippage(expected_sol, swap_config.slippage_bps());
            instructions.push(pump_sell_instruction(
                &owner,
                &mint_pk,
                &bonding_curve,
                &associated_bonding_curve,
                &token_ata,
                curve.creator,
                tokens_in,
                min_sol,
            )?);
        }
    }

    tx::new_signed_and_send(
        rpc,
        &pump.keypair,
        instructions,
        swap_config.use_jito,
        ctx.config.unit_price,
        ctx.config.unit_limit,
        ctx.config.jito_tip_sol,
        &ctx.config.jito_block_engine_url,
        ctx.config.dry_run,
        &logger,
    )
    .await
}

fn pump_buy_instruction(
    user: &Pubkey,
    mint: &Pubkey,
    bonding_curve: &Pubkey,
    associated_bonding_curve: &Pubkey,
    user_ata: &Pubkey,
    creator: Option<Pubkey>,
    token_amount: u64,
    max_sol_cost: u64,
) -> Result<Instruction> {
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMP_BUY_DISCRIMINATOR.to_le_bytes());
    data.extend_from_slice(&token_amount.to_le_bytes());
    data.extend_from_slice(&max_sol_cost.to_le_bytes());
    Ok(Instruction {
        program_id: pump_program(),
        accounts: pump_trade_accounts(
            user,
            mint,
            bonding_curve,
            associated_bonding_curve,
            user_ata,
            creator,
        )?,
        data,
    })
}

fn pump_sell_instruction(
    user: &Pubkey,
    mint: &Pubkey,
    bonding_curve: &Pubkey,
    associated_bonding_curve: &Pubkey,
    user_ata: &Pubkey,
    creator: Option<Pubkey>,
    token_amount: u64,
    min_sol_output: u64,
) -> Result<Instruction> {
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMP_SELL_DISCRIMINATOR.to_le_bytes());
    data.extend_from_slice(&token_amount.to_le_bytes());
    data.extend_from_slice(&min_sol_output.to_le_bytes());
    Ok(Instruction {
        program_id: pump_program(),
        accounts: pump_trade_accounts(
            user,
            mint,
            bonding_curve,
            associated_bonding_curve,
            user_ata,
            creator,
        )?,
        data,
    })
}

fn pump_trade_accounts(
    user: &Pubkey,
    mint: &Pubkey,
    bonding_curve: &Pubkey,
    associated_bonding_curve: &Pubkey,
    user_ata: &Pubkey,
    creator: Option<Pubkey>,
) -> Result<Vec<AccountMeta>> {
    let mut accounts = vec![
        AccountMeta::new_readonly(Pubkey::from_str(PUMP_GLOBAL)?, false),
        AccountMeta::new(Pubkey::from_str(PUMP_FEE_RECIPIENT)?, false),
        AccountMeta::new_readonly(*mint, false),
        AccountMeta::new(*bonding_curve, false),
        AccountMeta::new(*associated_bonding_curve, false),
        AccountMeta::new(*user_ata, false),
        AccountMeta::new(*user, true),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(spl_token::ID, false),
    ];
    if let Some(creator) = creator {
        let (creator_vault, _) =
            Pubkey::find_program_address(&[b"creator-vault", creator.as_ref()], &pump_program());
        accounts.push(AccountMeta::new(creator_vault, false));
    }
    accounts.push(AccountMeta::new_readonly(
        Pubkey::from_str(PUMP_ACCOUNT)?,
        false,
    ));
    accounts.push(AccountMeta::new_readonly(pump_program(), false));
    Ok(accounts)
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RaydiumInfo {
    pub base: f64,
    pub quote: f64,
    pub price: f64,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PumpInfo {
    pub mint: String,
    pub bonding_curve: String,
    pub associated_bonding_curve: String,
    pub raydium_pool: Option<String>,
    pub raydium_info: Option<RaydiumInfo>,
    pub complete: bool,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub total_supply: u64,
}

#[derive(Debug, Clone)]
pub struct BondingCurveAccount {
    pub discriminator: u64,
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
    pub creator: Option<Pubkey>,
}

pub async fn get_bonding_curve_account(
    rpc_client: Arc<solana_client::rpc_client::RpcClient>,
    mint: &Pubkey,
    program_id: &Pubkey,
) -> Result<(Pubkey, Pubkey, BondingCurveAccount)> {
    let bonding_curve = get_pda(mint, program_id)?;
    let associated_bonding_curve = get_associated_token_address(&bonding_curve, mint);
    let bonding_curve_data = rpc_client.get_account_data(&bonding_curve).map_err(|err| {
        anyhow!("Failed to get bonding curve account data: {bonding_curve}, err: {err}")
    })?;
    let bonding_curve_account = parse_bonding_curve(&bonding_curve_data)?;
    Ok((
        bonding_curve,
        associated_bonding_curve,
        bonding_curve_account,
    ))
}

pub fn get_pda(mint: &Pubkey, program_id: &Pubkey) -> Result<Pubkey> {
    let seeds = [b"bonding-curve".as_ref(), mint.as_ref()];
    let (bonding_curve, _bump) = Pubkey::find_program_address(&seeds, program_id);
    Ok(bonding_curve)
}

fn parse_bonding_curve(data: &[u8]) -> Result<BondingCurveAccount> {
    if data.len() < 49 {
        return Err(anyhow!("bonding curve data too short: {} bytes", data.len()));
    }
    let u64_at = |offset: usize| -> Result<u64> {
        let bytes: [u8; 8] = data[offset..offset + 8]
            .try_into()
            .map_err(|_| anyhow!("bonding curve slice"))?;
        Ok(u64::from_le_bytes(bytes))
    };
    let creator = if data.len() >= 81 {
        Pubkey::try_from(&data[49..81]).ok()
    } else {
        None
    };
    Ok(BondingCurveAccount {
        discriminator: u64_at(0)?,
        virtual_token_reserves: u64_at(8)?,
        virtual_sol_reserves: u64_at(16)?,
        real_token_reserves: u64_at(24)?,
        real_sol_reserves: u64_at(32)?,
        token_total_supply: u64_at(40)?,
        complete: data[48] != 0,
        creator,
    })
}

pub async fn get_pump_info(
    rpc_client: Arc<solana_client::rpc_client::RpcClient>,
    mint: &str,
) -> Result<PumpInfo> {
    let mint = Pubkey::from_str(mint)?;
    let program_id = pump_program();
    let (bonding_curve, associated_bonding_curve, bonding_curve_account) =
        get_bonding_curve_account(rpc_client, &mint, &program_id).await?;
    Ok(PumpInfo {
        mint: mint.to_string(),
        bonding_curve: bonding_curve.to_string(),
        associated_bonding_curve: associated_bonding_curve.to_string(),
        raydium_pool: None,
        raydium_info: None,
        complete: bonding_curve_account.complete,
        virtual_sol_reserves: bonding_curve_account.virtual_sol_reserves,
        virtual_token_reserves: bonding_curve_account.virtual_token_reserves,
        total_supply: bonding_curve_account.token_total_supply,
    })
}

pub fn min_amount_with_slippage(input_amount: u64, slippage_bps: u64) -> u64 {
    math::min_amount_with_slippage(input_amount, slippage_bps)
}

pub fn max_amount_with_slippage(input_amount: u64, slippage_bps: u64) -> u64 {
    math::max_amount_with_slippage(input_amount, slippage_bps)
}
