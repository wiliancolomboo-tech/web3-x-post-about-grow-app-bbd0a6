use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("GrowVAuLT1111111111111111111111111111111111");

pub const VAULT_SEED: &[u8] = b"grow_vault";
pub const TOKEN_VAULT_SEED: &[u8] = b"vault_token";
pub const USER_POSITION_SEED: &[u8] = b"user_position";
pub const MINIMUM_INITIAL_DEPOSIT: u64 = 1_000; // Virtual offset burn to avoid donation attacks

#[program]
pub mod grow_vault {
    use super::*;

    /// Initializes a new high-efficiency micro-savings vault for a target SPL asset
    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        fee_basis_points: u16,
        keeper_bounty_bps: u16,
    ) -> Result<()> {
        require!(fee_basis_points <= 1000, GrowError::FeeTooHigh); // Max 10%
        require!(keeper_bounty_bps <= 500, GrowError::BountyTooHigh); // Max 5%

        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.asset_mint = ctx.accounts.asset_mint.key();
        vault.token_vault = ctx.accounts.token_vault.key();
        vault.fee_basis_points = fee_basis_points;
        vault.keeper_bounty_bps = keeper_bounty_bps;
        vault.total_shares = 0;
        vault.total_underlying = 0;
        vault.paused = false;
        vault.bump = ctx.bumps.vault;
        vault.token_vault_bump = ctx.bumps.token_vault;

        emit!(VaultInitializedEvent {
            vault: vault.key(),
            asset_mint: vault.asset_mint,
            authority: vault.authority,
        });

        Ok(())
    }

    /// Deposits tokens into the vault and issues proportional constant-product shares
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let user_position = &mut ctx.accounts.user_position;

        require!(!vault.paused, GrowError::VaultPaused);
        require!(amount > 0, GrowError::ZeroAmount);

        let shares_to_mint = if vault.total_shares == 0 {
            // Mitigate First-Depositor ERC4626 Inflation Attack
            require!(amount > MINIMUM_INITIAL_DEPOSIT, GrowError::DepositTooSmall);
            vault.total_shares = MINIMUM_INITIAL_DEPOSIT; // Dead shares virtual seed
            amount
                .checked_sub(MINIMUM_INITIAL_DEPOSIT)
                .ok_or(GrowError::MathOverflow)?
        } else {
            // shares = (amount * total_shares) / total_underlying
            (amount as u128)
                .checked_mul(vault.total_shares as u128)
                .ok_or(GrowError::MathOverflow)?
                .checked_div(vault.total_underlying as u128)
                .ok_or(GrowError::MathOverflow)? as u64
        };

        require!(shares_to_mint > 0, GrowError::ZeroSharesMinted);

        // Transfer funds from User to Vault PDA
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.user_token_account.to_account_info(),
                    to: ctx.accounts.token_vault.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            amount,
        )?;

        // Update internal accounting state
        vault.total_underlying = vault
            .total_underlying
            .checked_add(amount)
            .ok_or(GrowError::MathOverflow)?;
        vault.total_shares = vault
            .total_shares
            .checked_add(shares_to_mint)
            .ok_or(GrowError::MathOverflow)?;

        if user_position.owner == Pubkey::default() {
            user_position.owner = ctx.accounts.user.key();
            user_position.vault = vault.key();
            user_position.bump = ctx.bumps.user_position;
        }

        user_position.shares = user_position
            .shares
            .checked_add(shares_to_mint)
            .ok_or(GrowError::MathOverflow)?;
        user_position.last_deposit_slot = Clock::get()?.slot;

        emit!(DepositEvent {
            user: ctx.accounts.user.key(),
            amount,
            shares_minted: shares_to_mint,
        });

        Ok(())
    }

    /// Burns shares to redeem underlying assets with slippage guard
    pub fn withdraw(ctx: Context<Withdraw>, shares_to_burn: u64, min_assets_out: u64) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let user_position = &mut ctx.accounts.user_position;

        require!(!vault.paused, GrowError::VaultPaused);
        require!(shares_to_burn > 0, GrowError::ZeroAmount);
        require!(user_position.shares >= shares_to_burn, GrowError::InsufficientShares);

        // assets = (shares_to_burn * total_underlying) / total_shares
        let assets_to_return = (shares_to_burn as u128)
            .checked_mul(vault.total_underlying as u128)
            .ok_or(GrowError::MathOverflow)?
            .checked_div(vault.total_shares as u128)
            .ok_or(GrowError::MathOverflow)? as u64;

        require!(assets_to_return >= min_assets_out, GrowError::SlippageExceeded);

        // Update balances
        user_position.shares = user_position
            .shares
            .checked_sub(shares_to_burn)
            .ok_or(GrowError::MathOverflow)?;
        vault.total_shares = vault
            .total_shares
            .checked_sub(shares_to_burn)
            .ok_or(GrowError::MathOverflow)?;
        vault.total_underlying = vault
            .total_underlying
            .checked_sub(assets_to_return)
            .ok_or(GrowError::MathOverflow)?;

        // Transfer funds from Vault Token PDA to User
        let seeds = &[
            VAULT_SEED,
            vault.asset_mint.as_ref(),
            &[vault.bump],
        ];
        let signer = &[&seeds[..]];

        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token_vault.to_account_info(),
                    to: ctx.accounts.user_token_account.to_account_info(),
                    authority: vault.to_account_info(),
                },
                signer,
            ),
            assets_to_return,
        )?;

        emit!(WithdrawEvent {
            user: ctx.accounts.user.key(),
            shares_burned: shares_to_burn,
            assets_returned: assets_to_return,
        });

        Ok(())
    }

    /// Crank function: Harvests external yield, updates valuation, pays keeper bounty
    pub fn harvest_and_compound(ctx: Context<HarvestAndCompound>, accrued_yield: u64) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(!vault.paused, GrowError::VaultPaused);

        if accrued_yield > 0 {
            let keeper_bounty = (accrued_yield as u128)
                .checked_mul(vault.keeper_bounty_bps as u128)
                .ok_or(GrowError::MathOverflow)?
                .checked_div(10000)
                .ok_or(GrowError::MathOverflow)? as u64;

            let net_compounded = accrued_yield
                .checked_sub(keeper_bounty)
                .ok_or(GrowError::MathOverflow)?;

            // Underlying balance expands without minting new shares -> share price increases
            vault.total_underlying = vault
                .total_underlying
                .checked_add(net_compounded)
                .ok_or(GrowError::MathOverflow)?;

            if keeper_bounty > 0 {
                let seeds = &[
                    VAULT_SEED,
                    vault.asset_mint.as_ref(),
                    &[vault.bump],
                ];
                let signer = &[&seeds[..]];

                token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        Transfer {
                            from: ctx.accounts.token_vault.to_account_info(),
                            to: ctx.accounts.keeper_token_account.to_account_info(),
                            authority: vault.to_account_info(),
                        },
                        signer,
                    ),
                    keeper_bounty,
                )?;
            }

            emit!(CompoundEvent {
                gross_yield: accrued_yield,
                net_compounded,
                keeper_bounty,
            });
        }

        Ok(())
    }

    /// Admin Circuit Breaker: Freeze / Unfreeze vault operations
    pub fn set_paused(ctx: Context<AdminAuth>, paused: bool) -> Result<()> {
        ctx.accounts.vault.paused = paused;
        emit!(PauseToggledEvent { paused });
        Ok(())
    }
}

// ---------------- Accounts Contexts ----------------

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub asset_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = 8 + VaultState::INIT_SPACE,
        seeds = [VAULT_SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        init,
        payer = authority,
        token::mint = asset_mint,
        token::authority = vault,
        seeds = [TOKEN_VAULT_SEED, vault.key().as_ref()],
        bump
    )]
    pub token_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.asset_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [TOKEN_VAULT_SEED, vault.key().as_ref()],
        bump = vault.token_vault_bump
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserPosition::INIT_SPACE,
        seeds = [USER_POSITION_SEED, user.key().as_ref(), vault.key().as_ref()],
        bump
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == vault.asset_mint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.asset_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [TOKEN_VAULT_SEED, vault.key().as_ref()],
        bump = vault.token_vault_bump
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [USER_POSITION_SEED, user.key().as_ref(), vault.key().as_ref()],
        bump = user_position.bump,
        constraint = user_position.owner == user.key()
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == vault.asset_mint
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct HarvestAndCompound<'info> {
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.asset_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [TOKEN_VAULT_SEED, vault.key().as_ref()],
        bump = vault.token_vault_bump
    )]
    pub token_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = keeper_token_account.mint == vault.asset_mint
    )]
    pub keeper_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AdminAuth<'info> {
    #[account(
        constraint = authority.key() == vault.authority @ GrowError::Unauthorized
    )]
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [VAULT_SEED, vault.asset_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,
}

// ---------------- State Definitions ----------------

#[account]
#[derive(InitSpace)]
pub struct VaultState {
    pub authority: Pubkey,          // 32
    pub asset_mint: Pubkey,         // 32
    pub token_vault: Pubkey,        // 32
    pub total_shares: u64,          // 8
    pub total_underlying: u64,      // 8
    pub fee_basis_points: u16,      // 2
    pub keeper_bounty_bps: u16,     // 2
    pub paused: bool,               // 1
    pub bump: u8,                   // 1
    pub token_vault_bump: u8,       // 1
}

#[account]
#[derive(InitSpace)]
pub struct UserPosition {
    pub owner: Pubkey,              // 32
    pub vault: Pubkey,              // 32
    pub shares: u64,                // 8
    pub last_deposit_slot: u64,     // 8
    pub bump: u8,                   // 1
}

// ---------------- Error Codes ----------------

#[error_code]
pub enum GrowError {
    #[msg("Administrative fee exceeds maximum safety threshold (10%).")]
    FeeTooHigh,
    #[msg("Keeper bounty exceeds maximum threshold (5%).")]
    BountyTooHigh,
    #[msg("Contract execution is paused by emergency circuit breaker.")]
    VaultPaused,
    #[msg("Transaction amount must be strictly greater than zero.")]
    ZeroAmount,
    #[msg("First deposit must exceed virtual offset threshold to prevent inflation exploit.")]
    DepositTooSmall,
    #[msg("Calculated share allocation resolved to zero.")]
    ZeroSharesMinted,
    #[msg("Insufficient user share balance for redemption.")]
    InsufficientShares,
    #[msg("Redemption returned assets lower than specified slippage minimum.")]
    SlippageExceeded,
    #[msg("Mathematical overflow occurred during fixed-point calculation.")]
    MathOverflow,
    #[msg("Caller lacks required administrative privilege.")]
    Unauthorized,
}

// ---------------- Events ----------------

#[event]
pub struct VaultInitializedEvent {
    pub vault: Pubkey,
    pub asset_mint: Pubkey,
    pub authority: Pubkey,
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
    pub assets_returned: u64,
}

#[event]
pub struct CompoundEvent {
    pub gross_yield: u64,
    pub net_compounded: u64,
    pub keeper_bounty: u64,
}

#[event]
pub struct PauseToggledEvent {
    pub paused: bool,
}