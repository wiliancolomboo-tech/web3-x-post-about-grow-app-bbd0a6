use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};
use crate::errors::VaultError;
use crate::state::VaultConfig;

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub asset_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = VaultConfig::LEN,
        seeds = [b"vault_config", asset_mint.key().as_ref()],
        bump
    )]
    pub vault_config: Box<Account<'info, VaultConfig>>,

    #[account(
        init,
        payer = authority,
        seeds = [b"share_mint", vault_config.key().as_ref()],
        bump,
        mint::decimals = asset_mint.decimals,
        mint::authority = vault_config,
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        token::mint = asset_mint,
        token::authority = vault_config,
        seeds = [b"asset_reserve", vault_config.key().as_ref()],
        bump
    )]
    pub asset_reserve: Account<'info, TokenAccount>,

    /// CHECK: Protocol treasury address safe
    pub treasury: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(ctx: Context<InitializeVault>, performance_fee_bps: u16) -> Result<()> {
    require!(performance_fee_bps <= 2000, VaultError::ExcessiveFee);

    let vault_config = &mut ctx.accounts.vault_config;
    vault_config.authority = ctx.accounts.authority.key();
    vault_config.asset_mint = ctx.accounts.asset_mint.key();
    vault_config.share_mint = ctx.accounts.share_mint.key();
    vault_config.asset_reserve = ctx.accounts.asset_reserve.key();
    vault_config.treasury = ctx.accounts.treasury.key();
    vault_config.performance_fee_bps = performance_fee_bps;
    vault_config.total_managed_assets = 0;
    vault_config.is_paused = false;
    vault_config.bump = ctx.bumps.vault_config;
    vault_config.share_mint_bump = ctx.bumps.share_mint;

    msg!("Grow Vault initialized: Authority={}", vault_config.authority);
    Ok(())
}