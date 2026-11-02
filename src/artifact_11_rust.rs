use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};
use crate::errors::VaultError;
use crate::math;
use crate::state::VaultConfig;

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault_config", vault_config.asset_mint.as_ref()],
        bump = vault_config.bump,
        has_one = asset_reserve,
        has_one = share_mint,
    )]
    pub vault_config: Box<Account<'info, VaultConfig>>,

    #[account(mut)]
    pub asset_reserve: Account<'info, TokenAccount>,

    #[account(mut)]
    pub share_mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = user_asset_ata.mint == vault_config.asset_mint,
        constraint = user_asset_ata.owner == user.key()
    )]
    pub user_asset_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_share_ata.mint == share_mint.key(),
        constraint = user_share_ata.owner == user.key()
    )]
    pub user_share_ata: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<Withdraw>, shares_in: u64, min_assets_out: u64) -> Result<()> {
    require!(shares_in > 0, VaultError::ZeroAmount);

    let total_shares = ctx.accounts.share_mint.supply;
    let total_assets = ctx.accounts.vault_config.total_managed_assets;

    let assets_to_return = math::convert_to_assets(shares_in, total_assets, total_shares)?;
    require!(
        assets_to_return >= min_assets_out,
        VaultError::WithdrawSlippageExceeded
    );
    require!(
        ctx.accounts.asset_reserve.amount >= assets_to_return,
        VaultError::InsufficientLiquidity
    );

    // 1. Burn user shares
    let burn_accounts = Burn {
        mint: ctx.accounts.share_mint.to_account_info(),
        from: ctx.accounts.user_share_ata.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    let burn_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), burn_accounts);
    token::burn(burn_ctx, shares_in)?;

    // 2. Transfer assets from reserve back to user
    let asset_mint_key = ctx.accounts.vault_config.asset_mint;
    let seeds = &[
        b"vault_config".as_ref(),
        asset_mint_key.as_ref(),
        &[ctx.accounts.vault_config.bump],
    ];
    let signer_seeds = &[&seeds[..]];

    let transfer_accounts = Transfer {
        from: ctx.accounts.asset_reserve.to_account_info(),
        to: ctx.accounts.user_asset_ata.to_account_info(),
        authority: ctx.accounts.vault_config.to_account_info(),
    };
    let transfer_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        transfer_accounts,
        signer_seeds,
    );
    token::transfer(transfer_ctx, assets_to_return)?;

    // 3. Update vault state
    let vault_config = &mut ctx.accounts.vault_config;
    vault_config.total_managed_assets = vault_config
        .total_managed_assets
        .checked_sub(assets_to_return)
        .ok_or(VaultError::MathOverflow)?;

    msg!(
        "Withdrawal complete: SharesBurned={}, AssetsReturned={}",
        shares_in,
        assets_to_return
    );
    Ok(())
}