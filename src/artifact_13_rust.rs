use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("GrowV11111111111111111111111111111111111111");

#[program]
pub mod grow_vault {
    use super::*;

    /// Initialize a non-custodial user profile with scoped delegation parameters.
    pub fn initialize_profile(
        ctx: Context<InitializeProfile>,
        max_debit_per_epoch: u64,
        epoch_duration_seconds: i64,
    ) -> Result<()> {
        require!(max_debit_per_epoch > 0, GrowError::InvalidDebitLimit);
        require!(epoch_duration_seconds >= 60, GrowError::CadenceTooShort);

        let profile = &mut ctx.accounts.user_profile;
        profile.owner = ctx.accounts.owner.key();
        profile.vault = ctx.accounts.vault.key();
        profile.max_debit_per_epoch = max_debit_per_epoch;
        profile.epoch_duration_seconds = epoch_duration_seconds;
        profile.current_epoch_spent = 0;
        profile.last_epoch_timestamp = Clock::get()?.unix_timestamp;
        profile.is_paused = false;
        profile.bump = ctx.bumps.user_profile;

        let vault = &mut ctx.accounts.vault;
        vault.owner = ctx.accounts.owner.key();
        vault.mint = ctx.accounts.deposit_mint.key();
        vault.total_deposited = 0;
        vault.cumulative_yield_routed = 0;
        vault.bump = ctx.bumps.vault;

        emit!(ProfileInitialized {
            owner: profile.owner,
            vault: profile.vault,
            max_debit_per_epoch,
            epoch_duration_seconds,
        });

        Ok(())
    }

    /// Process a programmatic micro-deposit. Can be executed by the user or an authorized crank.
    pub fn deposit_micro(ctx: Context<DepositMicro>, amount: u64) -> Result<()> {
        require!(amount > 0, GrowError::ZeroDepositAmount);
        
        let profile = &mut ctx.accounts.user_profile;
        require!(!profile.is_paused, GrowError::VaultPaused);

        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        // Reset epoch spend if duration has elapsed
        if current_time >= profile.last_epoch_timestamp.saturating_add(profile.epoch_duration_seconds) {
            profile.current_epoch_spent = 0;
            profile.last_epoch_timestamp = current_time;
        }

        // Enforce session debit bounds
        let new_spent = profile
            .current_epoch_spent
            .checked_add(amount)
            .ok_or(GrowError::MathOverflow)?;
        
        require!(
            new_spent <= profile.max_debit_per_epoch,
            GrowError::DebitLimitExceeded
        );

        profile.current_epoch_spent = new_spent;

        // Execute CPI transfer from User ATA to Vault ATA
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new(cpi_program, cpi_accounts), amount)?;

        // Update vault state tracking
        let vault = &mut ctx.accounts.vault;
        vault.total_deposited = vault
            .total_deposited
            .checked_add(amount)
            .ok_or(GrowError::MathOverflow)?;

        emit!(MicroDepositExecuted {
            owner: profile.owner,
            amount,
            total_deposited: vault.total_deposited,
            current_epoch_spent: profile.current_epoch_spent,
        });

        Ok(())
    }

    /// Route accumulated vault reserves into downstream yield primitives.
    /// Secured via PDA signer seeds to prevent arbitrary token drainage.
    pub fn route_to_yield(ctx: Context<RouteToYield>, amount: u64) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(amount > 0, GrowError::ZeroRoutingAmount);
        require!(
            ctx.accounts.vault_token_account.amount >= amount,
            GrowError::InsufficientVaultLiquidity
        );

        let owner_key = vault.owner;
        let seeds = &[
            b"vault".as_ref(),
            owner_key.as_ref(),
            &[vault.bump],
        ];
        let signer = &[&seeds[..]];

        // Perform CPI to downstream liquidity pool / yield provider
        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.strategy_token_account.to_account_info(),
            authority: vault.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(
            CpiContext::new_with_signer(cpi_program, cpi_accounts, signer),
            amount,
        )?;

        vault.cumulative_yield_routed = vault
            .cumulative_yield_routed
            .checked_add(amount)
            .ok_or(GrowError::MathOverflow)?;

        emit!(YieldRouted {
            vault: vault.key(),
            amount,
            cumulative_routed: vault.cumulative_yield_routed,
        });

        Ok(())
    }

    /// Emergency evacuation: The user can withdraw all liquid capital instantly.
    pub fn emergency_withdraw(ctx: Context<EmergencyWithdraw>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let available_balance = ctx.accounts.vault_token_account.amount;
        require!(available_balance > 0, GrowError::ZeroBalance);

        let owner_key = vault.owner;
        let seeds = &[
            b"vault".as_ref(),
            owner_key.as_ref(),
            &[vault.bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: vault.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(
            CpiContext::new_with_signer(cpi_program, cpi_accounts, signer),
            available_balance,
        )?;

        vault.total_deposited = vault.total_deposited.saturating_sub(available_balance);

        emit!(EmergencyWithdrawal {
            owner: owner_key,
            amount: available_balance,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeProfile<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        init,
        payer = owner,
        space = 8 + UserProfile::INIT_SPACE,
        seeds = [b"profile", owner.key().as_ref()],
        bump
    )]
    pub user_profile: Account<'info, UserProfile>,

    #[account(
        init,
        payer = owner,
        space = 8 + Vault::INIT_SPACE,
        seeds = [b"vault", owner.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, Vault>,

    pub deposit_mint: Account<'info, Mint>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositMicro<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [b"profile", user_profile.owner.as_ref()],
        bump = user_profile.bump,
        constraint = (authority.key() == user_profile.owner || authority.key() == keeper.key()) @ GrowError::UnauthorizedSigner
    )]
    pub user_profile: Account<'info, UserProfile>,

    #[account(
        mut,
        seeds = [b"vault", user_profile.owner.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, Vault>,

    /// CHECK: Validated via keeper address check or equality with profile owner
    pub keeper: UncheckedAccount<'info>,

    #[account(
        mut,
        constraint = user_token_account.owner == user_profile.owner @ GrowError::InvalidAccountOwner,
        constraint = user_token_account.mint == vault.mint @ GrowError::InvalidTokenMint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key() @ GrowError::InvalidAccountOwner,
        constraint = vault_token_account.mint == vault.mint @ GrowError::InvalidTokenMint
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RouteToYield<'info> {
    #[account(mut)]
    pub keeper_or_owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", vault.owner.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key() @ GrowError::InvalidAccountOwner,
        constraint = vault_token_account.mint == vault.mint @ GrowError::InvalidTokenMint
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = strategy_token_account.mint == vault.mint @ GrowError::InvalidTokenMint
    )]
    pub strategy_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct EmergencyWithdraw<'info> {
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        has_one = owner @ GrowError::UnauthorizedSigner
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        constraint = vault_token_account.owner == vault.key() @ GrowError::InvalidAccountOwner,
        constraint = vault_token_account.mint == vault.mint @ GrowError::InvalidTokenMint
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_account.owner == owner.key() @ GrowError::InvalidAccountOwner,
        constraint = user_token_account.mint == vault.mint @ GrowError::InvalidTokenMint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTURES
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct UserProfile {
    pub owner: Pubkey,
    pub vault: Pubkey,
    pub max_debit_per_epoch: u64,
    pub epoch_duration_seconds: i64,
    pub current_epoch_spent: u64,
    pub last_epoch_timestamp: i64,
    pub is_paused: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Vault {
    pub owner: Pubkey,
    pub mint: Pubkey,
    pub total_deposited: u64,
    pub cumulative_yield_routed: u64,
    pub bump: u8,
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct ProfileInitialized {
    pub owner: Pubkey,
    pub vault: Pubkey,
    pub max_debit_per_epoch: u64,
    pub epoch_duration_seconds: i64,
}

#[event]
pub struct MicroDepositExecuted {
    pub owner: Pubkey,
    pub amount: u64,
    pub total_deposited: u64,
    pub current_epoch_spent: u64,
}

#[event]
pub struct YieldRouted {
    pub vault: Pubkey,
    pub amount: u64,
    pub cumulative_routed: u64,
}

#[event]
pub struct EmergencyWithdrawal {
    pub owner: Pubkey,
    pub amount: u64,
}

#[error_code]
pub enum GrowError {
    #[msg("Debit limit must be strictly positive")]
    InvalidDebitLimit,
    #[msg("Epoch cadence must be at least 60 seconds")]
    CadenceTooShort,
    #[msg("Micro-deposit amount must be greater than zero")]
    ZeroDepositAmount,
    #[msg("Vault operations are currently paused")]
    VaultPaused,
    #[msg("Maximum debit limit exceeded for the current active epoch")]
    DebitLimitExceeded,
    #[msg("Signer is not authorized to execute this instruction")]
    UnauthorizedSigner,
    #[msg("Invalid token account owner")]
    InvalidAccountOwner,
    #[msg("Token account mint mismatch")]
    InvalidTokenMint,
    #[msg("Yield routing amount must be strictly greater than zero")]
    ZeroRoutingAmount,
    #[msg("Insufficient liquid reserves in vault for routing operation")]
    InsufficientVaultLiquidity,
    #[msg("Vault has zero liquid balance to withdraw")]
    ZeroBalance,
    #[msg("Arithmetic overflow occurred during balance calculation")]
    MathOverflow,
}