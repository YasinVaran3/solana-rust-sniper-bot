use anyhow::{anyhow, Result};
use solana_program::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::{Account as TokenAccount, Mint};

pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(owner, mint)
}

pub async fn account_exists(
    client: &solana_client::nonblocking::rpc_client::RpcClient,
    pubkey: &Pubkey,
) -> Result<bool> {
    Ok(client.get_account(pubkey).await.is_ok())
}

pub async fn token_balance(
    client: &solana_client::nonblocking::rpc_client::RpcClient,
    ata: &Pubkey,
) -> Result<u64> {
    let account = client
        .get_account(ata)
        .await
        .map_err(|e| anyhow!("token account {ata} not found: {e}"))?;
    let parsed = TokenAccount::unpack(&account.data)
        .map_err(|e| anyhow!("failed to unpack token account: {e}"))?;
    Ok(parsed.amount)
}

pub async fn mint_info(
    client: &solana_client::nonblocking::rpc_client::RpcClient,
    mint: &Pubkey,
) -> Result<Mint> {
    let account = client
        .get_account(mint)
        .await
        .map_err(|e| anyhow!("mint {mint} not found: {e}"))?;
    Mint::unpack(&account.data).map_err(|e| anyhow!("failed to unpack mint: {e}"))
}

pub fn mint_authorities_revoked(mint: &Mint) -> bool {
    mint.mint_authority.is_none() && mint.freeze_authority.is_none()
}

pub async fn get_account_info(
    client: std::sync::Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    _keypair: std::sync::Arc<solana_sdk::signature::Keypair>,
    mint: &Pubkey,
    account: &Pubkey,
) -> Result<TokenAccount> {
    let acc = client
        .get_account(account)
        .await
        .map_err(|e| anyhow!("account {account} not found: {e}"))?;
    let parsed = TokenAccount::unpack(&acc.data)
        .map_err(|e| anyhow!("failed to unpack token account: {e}"))?;
    if parsed.mint != *mint {
        return Err(anyhow!("account mint mismatch"));
    }
    Ok(parsed)
}

pub async fn get_mint_info(
    client: std::sync::Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    _keypair: std::sync::Arc<solana_sdk::signature::Keypair>,
    address: &Pubkey,
) -> Result<Mint> {
    mint_info(client.as_ref(), address).await
}
