use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer};

declare_id!("Grow1111111111111111111111111111111111111111");

pub const VAULT_SEED: &[u8] = b"vault";
pub const VAULT_TOKEN_SEED: &[u8] = b"vault_tokens";
pub const SHARE_MINT_SEED: &[u8] = b"share_mint";

#[program]
pub mod grow_vault {
    use super::*;

    /// Initializes a new Grow dynamic yield vault for a given underlying asset.
    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.admin = ctx.accounts.admin.key();
        vault.keeper = ctx.accounts.keeper.key();
        vault.underlying_mint = ctx.accounts.underlying_mint.key();
        vault.share_mint = ctx.accounts.share_mint.key();
        vault.token_vault = ctx.accounts.token_vault.key();
        vault.total_vault_assets = 0;
        vault.last_harvest_slot = Clock::get()?.slot;
        vault.is_paused = false;
        vault.bump = ctx.bumps.vault;
        vault.vault_token_bump = ctx.bumps.token_vault;
        vault.share_mint_bump = ctx.bumps.share_mint;

        emit!(VaultInitializedEvent {
            admin: vault.admin,
            underlying_mint: vault.underlying_mint,
            share_mint: vault.share_mint,
        });

        Ok(())
    }

    /// Deposits underlying tokens into the vault and mints corresponding LP shares.
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(!ctx.accounts.vault.is_paused, GrowError::VaultPaused);
        require!(amount > 0, GrowError::ZeroAmount);

        let total_assets = ctx.accounts.vault.total_vault_assets;
        let total_shares = ctx.accounts.share_mint.supply;

        // Mathematical invariant: If total_shares == 0, shares = amount (1:1 initialization)
        // Otherwise: shares = (amount * total_shares) / total_assets
        let shares_to_mint = if total_shares == 0 || total_assets == 0 {
            amount
        } else {
            (amount as u128)
                .checked_mul(total_shares as u128)
                .ok_or(GrowError::MathOverflow)?
                .checked_div(total_assets as u128)
                .ok_or(GrowError::MathOverflow)? as u64
        };

        require!(shares_to_mint > 0, GrowError::SlippageExceeded);

        // CPI Transfer: User -> Vault Token Account
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token.to_account_info(),
            to: ctx.accounts.token_vault.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // PDA Signer seeds for minting shares
        let seeds = &[
            VAULT_SEED,
            ctx.accounts.vault.underlying_mint.as_ref(),
            &[ctx.accounts.vault.bump],
        ];
        let signer = &[&seeds[..]];

        // CPI MintTo: Mint shares to user
        let mint_accounts = MintTo {
            mint: ctx.accounts.share_mint.to_account_info(),
            to: ctx.accounts.user_share.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            mint_accounts,
            signer,
        );
        token::mint_to(mint_ctx, shares_to_mint)?;

        // Update state
        let vault = &mut ctx.accounts.vault;
        vault.total_vault_assets = vault
            .total_vault_assets
            .checked_add(amount)
            .ok_or(GrowError::MathOverflow)?;

        emit!(DepositEvent {
            user: ctx.accounts.user.key(),
            amount,
            shares_minted: shares_to_mint,
        });

        Ok(())
    }

    /// Redeems LP shares for underlying assets based on current exchange rate.
    pub fn withdraw(ctx: Context<Withdraw>, shares: u64) -> Result<()> {
        require!(shares > 0, GrowError::ZeroAmount);

        let total_shares = ctx.accounts.share_mint.supply;
        let total_assets = ctx.accounts.vault.total_vault_assets;

        require!(total_shares > 0, GrowError::EmptyVault);
        require!(ctx.accounts.user_share.amount >= shares, GrowError::InsufficientBalance);

        // Mathematical invariant: amount = (shares * total_assets) / total_shares
        let amount_to_return = (shares as u128)
            .checked_mul(total_assets as u128)
            .ok_or(GrowError::MathOverflow)?
            .checked_div(total_shares as u128)
            .ok_or(GrowError::MathOverflow)? as u64;

        require!(amount_to_return > 0, GrowError::ZeroAmount);
        require!(
            ctx.accounts.token_vault.amount >= amount_to_return,
            GrowError::InsufficientVaultLiquidity
        );

        // 1. Burn User LP shares
        let burn_accounts = Burn {
            mint: ctx.accounts.share_mint.to_account_info(),
            to: ctx.accounts.user_share.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let burn_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), burn_accounts);
        token::burn(burn_ctx, shares)?;

        // 2. PDA Signer seeds for transferring underlying tokens to user
        let seeds = &[
            VAULT_SEED,
            ctx.accounts.vault.underlying_mint.as_ref(),
            &[ctx.accounts.vault.bump],
        ];
        let signer = &[&seeds[..]];

        let transfer_accounts = Transfer {
            from: ctx.accounts.token_vault.to_account_info(),
            to: ctx.accounts.user_token.to_account_info(),
            authority: ctx.accounts.vault.to_account_info(),
        };
        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            transfer_accounts,
            signer,
        );
        token::transfer(transfer_ctx, amount_to_return)?;

        // Update state
        let vault = &mut ctx.accounts.vault;
        vault.total_vault_assets = vault
            .total_vault_assets
            .checked_sub(amount_to_return)
            .ok_or(GrowError::MathOverflow)?;

        emit!(WithdrawEvent {
            user: ctx.accounts.user.key(),
            shares_burned: shares,
            amount_returned: amount_to_return,
        });

        Ok(())
    }

    /// Autocompounds yield and updates the total_vault_assets invariant.
    pub fn harvest_and_compound(ctx: Context<HarvestAndCompound>, new_yield_accrued: u64) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(
            ctx.accounts.keeper.key() == vault.keeper || ctx.accounts.keeper.key() == vault.admin,
            GrowError::UnauthorizedKeeper
        );

        // In a live integration, accrued yield is swapped via Jupiter/Raydium CPI into underlying asset.
        // Here we record the accreted underlying assets verified through token vault reconciliation.
        let actual_balance = ctx.accounts.token_vault.amount;
        require!(
            actual_balance >= vault.total_vault_assets.checked_add(new_yield_accrued).unwrap_or(0),
            GrowError::AssetBalanceMismatch
        );

        vault.total_vault_assets = vault
            .total_vault_assets
            .checked_add(new_yield_accrued)
            .ok_or(GrowError::MathOverflow)?;
        vault.last_harvest_slot = Clock::get()?.slot;

        emit!(HarvestEvent {
            yield_accrued: new_yield_accrued,
            total_assets: vault.total_vault_assets,
            slot: vault.last_harvest_slot,
        });

        Ok(())
    }

    /// Administrative emergency pause toggle.
    pub fn set_paused(ctx: Context<AdminControl>, paused: bool) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(ctx.accounts.admin.key() == vault.admin, GrowError::Unauthorized);
        vault.is_paused = paused;
        emit!(PauseToggledEvent { is_paused: paused });
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: Safe keeper bot authority passed during initialization
    pub keeper: AccountInfo<'info>,

    pub underlying_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        seeds = [SHARE_MINT_SEED, vault.key().as_ref()],
        bump,
        mint::decimals = underlying_mint.decimals,
        mint::authority = vault,
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        seeds = [VAULT_TOKEN_SEED, vault.key().as_ref()],
        bump,
        token::mint = underlying_mint,
        token::authority = vault,
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        space = 8 + VaultState::LEN,
        seeds = [VAULT_SEED, underlying_mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, VaultState>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.underlying_mint.as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [VAULT_TOKEN_SEED, vault.key().as_ref()],
        bump = vault.vault_token_bump,
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [SHARE_MINT_SEED, vault.key().as_ref()],
        bump = vault.share_mint_bump,
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = user_token.owner == user.key(),
        constraint = user_token.mint == vault.underlying_mint
    )]
    pub user_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_share.owner == user.key(),
        constraint = user_share.mint == vault.share_mint
    )]
    pub user_share: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.underlying_mint.as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [VAULT_TOKEN_SEED, vault.key().as_ref()],
        bump = vault.vault_token_bump,
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [SHARE_MINT_SEED, vault.key().as_ref()],
        bump = vault.share_mint_bump,
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = user_token.owner == user.key(),
        constraint = user_token.mint == vault.underlying_mint
    )]
    pub user_token: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_share.owner == user.key(),
        constraint = user_share.mint == vault.share_mint
    )]
    pub user_share: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct HarvestAndCompound<'info> {
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.underlying_mint.as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [VAULT_TOKEN_SEED, vault.key().as_ref()],
        bump = vault.vault_token_bump,
    )]
    pub token_vault: Account<'info, TokenAccount>,
}

#[derive(Accounts)]
pub struct AdminControl<'info> {
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.underlying_mint.as_ref()],
        bump = vault.bump,
        has_one = admin @ GrowError::Unauthorized
    )]
    pub vault: Account<'info, VaultState>,
}

// -----------------------------------------------------------------------------
// STATE DEFINITIONS & EVENTS
// -----------------------------------------------------------------------------

#[account]
pub struct VaultState {
    pub admin: Pubkey,              // 32
    pub keeper: Pubkey,             // 32
    pub underlying_mint: Pubkey,     // 32
    pub share_mint: Pubkey,          // 32
    pub token_vault: Pubkey,         // 32
    pub total_vault_assets: u64,     // 8
    pub last_harvest_slot: u64,      // 8
    pub is_paused: bool,             // 1
    pub bump: u8,                    // 1
    pub vault_token_bump: u8,        // 1
    pub share_mint_bump: u8,         // 1
}

impl VaultState {
    pub const LEN: usize = 32 + 32 + 32 + 32 + 32 + 8 + 8 + 1 + 1 + 1 + 1 + 64; // + padding
}

#[event]
pub struct VaultInitializedEvent {
    pub admin: Pubkey,
    pub underlying_mint: Pubkey,
    pub share_mint: Pubkey,
}

#[event]
pub struct DepositEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub shares_minted: u64,
}

#[event]
pub struct WithdrawEvent {
    pub user: Pubkey,
    pub shares_burned: u64,
    pub amount_returned: u64,
}

#[event]
pub struct HarvestEvent {
    pub yield_accrued: u64,
    pub total_assets: u64,
    pub slot: u64,
}

#[event]
pub struct PauseToggledEvent {
    pub is_paused: bool,
}

#[error_code]
pub enum GrowError {
    #[msg("Vault is currently paused by admin.")]
    VaultPaused,
    #[msg("Amount must be greater than zero.")]
    ZeroAmount,
    #[msg("Arithmetic overflow occurred during conversion.")]
    MathOverflow,
    #[msg("Requested shares zero or below tolerance.")]
    SlippageExceeded,
    #[msg("Insufficient user token or share balance.")]
    InsufficientBalance,
    #[msg("Insufficient liquidity available in vault token balance.")]
    InsufficientVaultLiquidity,
    #[msg("Vault currently holds zero shares.")]
    EmptyVault,
    #[msg("Unauthorized signer.")]
    Unauthorized,
    #[msg("Signer is not an authorized keeper bot.")]
    UnauthorizedKeeper,
    #[msg("Actual token balance does not match reported accrued yield.")]
    AssetBalanceMismatch,
}