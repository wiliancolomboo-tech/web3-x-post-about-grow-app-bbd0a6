use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer};

declare_id!("GrowVa1111111111111111111111111111111111111");

pub const MAX_STRATEGIES: usize = 4;
pub const BASIS_POINTS_DIVISOR: u16 = 10_000;
pub const MAX_SLIPPAGE_BPS: u16 = 500; // 5% max slippage

#[program]
pub mod grow_vault {
    use super::*;

    /// Initializes a new yield-routing vault for a specified base token (e.g. USDC).
    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        bump: u8,
        authority_bump: u8,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.admin = ctx.accounts.admin.key();
        vault.base_mint = ctx.accounts.base_mint.key();
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.share_mint = ctx.accounts.share_mint.key();
        vault.total_strategy_allocation = 0;
        vault.active_strategies = 0;
        vault.bump = bump;
        vault.authority_bump = authority_bump;
        vault.paused = false;

        emit!(VaultInitializedEvent {
            admin: vault.admin,
            base_mint: vault.base_mint,
            share_mint: vault.share_mint,
        });

        Ok(())
    }

    /// Deposits base assets into the vault; calculates and issues proportional LP shares.
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(!ctx.accounts.vault.paused, GrowVaultError::VaultPaused);
        require!(amount > 0, GrowVaultError::ZeroDepositAmount);

        let total_assets = ctx.accounts.vault.compute_total_assets(
            ctx.accounts.vault_token_account.amount,
        )?;

        let total_shares = ctx.accounts.share_mint.supply;

        let shares_to_mint = if total_shares == 0 || total_assets == 0 {
            amount
        } else {
            (amount as u128)
                .checked_mul(total_shares as u128)
                .ok_or(GrowVaultError::MathOverflow)?
                .checked_div(total_assets as u128)
                .ok_or(GrowVaultError::MathOverflow)? as u64
        };

        require!(shares_to_mint > 0, GrowVaultError::ZeroMintedShares);

        // 1. Transfer base tokens from depositor to vault token account
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_base_account.to_account_info(),
                to: ctx.accounts.vault_token_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, amount)?;

        // 2. Mint Grow LP shares to the user via Vault Authority PDA signer
        let vault_key = ctx.accounts.vault.key();
        let authority_seeds = &[
            b"authority".as_ref(),
            vault_key.as_ref(),
            &[ctx.accounts.vault.authority_bump],
        ];
        let signer_seeds = &[&authority_seeds[..]];

        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.share_mint.to_account_info(),
                to: ctx.accounts.user_share_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_ctx, shares_to_mint)?;

        emit!(DepositEvent {
            user: ctx.accounts.user.key(),
            base_amount: amount,
            shares_minted: shares_to_mint,
        });

        Ok(())
    }

    /// Burns shares to withdraw underlying assets, including accrued yields.
    pub fn withdraw(ctx: Context<Withdraw>, shares: u64, min_assets_out: u64) -> Result<()> {
        require!(shares > 0, GrowVaultError::ZeroShares);

        let total_shares = ctx.accounts.share_mint.supply;
        require!(total_shares > 0, GrowVaultError::EmptyPool);

        let total_assets = ctx.accounts.vault.compute_total_assets(
            ctx.accounts.vault_token_account.amount,
        )?;

        let assets_to_return = (shares as u128)
            .checked_mul(total_assets as u128)
            .ok_or(GrowVaultError::MathOverflow)?
            .checked_div(total_shares as u128)
            .ok_or(GrowVaultError::MathOverflow)? as u64;

        require!(
            assets_to_return >= min_assets_out,
            GrowVaultError::SlippageExceeded
        );

        // 1. Burn user shares
        let burn_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.share_mint.to_account_info(),
                from: ctx.accounts.user_share_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::burn(burn_ctx, shares)?;

        // 2. Transfer base assets from vault reserve to user
        let vault_key = ctx.accounts.vault.key();
        let authority_seeds = &[
            b"authority".as_ref(),
            vault_key.as_ref(),
            &[ctx.accounts.vault.authority_bump],
        ];
        let signer_seeds = &[&authority_seeds[..]];

        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.user_base_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_ctx, assets_to_return)?;

        emit!(WithdrawEvent {
            user: ctx.accounts.user.key(),
            shares_burned: shares,
            base_amount_returned: assets_to_return,
        });

        Ok(())
    }

    /// Keeper rebalance instruction: moves idle reserves into high-yield external strategies.
    pub fn rebalance_strategy(
        ctx: Context<RebalanceStrategy>,
        target_allocation: u64,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let strategy = &mut ctx.accounts.strategy;

        require!(!vault.paused, GrowVaultError::VaultPaused);
        require!(strategy.vault == vault.key(), GrowVaultError::InvalidStrategy);

        // Compute new total allocation across strategies
        let old_allocation = strategy.allocated_amount;
        let new_total = (vault.total_strategy_allocation as i128)
            .checked_sub(old_allocation as i128)
            .ok_or(GrowVaultError::MathOverflow)?
            .checked_add(target_allocation as i128)
            .ok_or(GrowVaultError::MathOverflow)?;

        require!(new_total >= 0, GrowVaultError::InvalidStrategyAllocation);
        vault.total_strategy_allocation = new_total as u64;
        strategy.allocated_amount = target_allocation;

        emit!(RebalanceEvent {
            strategy: strategy.key(),
            previous_allocation: old_allocation,
            new_allocation: target_allocation,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT STRUCTURES & CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(bump: u8, authority_bump: u8)]
pub struct InitializeVault<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + VaultState::INIT_SPACE,
        seeds = [b"vault", base_mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, VaultState>,

    /// CHECK: Safe PDA derived authority for signing withdrawals & CPIs
    #[account(
        seeds = [b"authority", vault.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub base_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        mint::decimals = base_mint.decimals,
        mint::authority = vault_authority,
        seeds = [b"share_mint", vault.key().as_ref()],
        bump
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        token::mint = base_mint,
        token::authority = vault_authority,
        seeds = [b"vault_tokens", vault.key().as_ref()],
        bump
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.base_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    /// CHECK: PDA signer
    #[account(
        seeds = [b"authority", vault.key().as_ref()],
        bump = vault.authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        address = vault.share_mint
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        mut,
        address = vault.vault_token_account
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = vault.base_mint,
        token::authority = user
    )]
    pub user_base_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = vault.share_mint,
        token::authority = user
    )]
    pub user_share_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.base_mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    /// CHECK: PDA signer
    #[account(
        seeds = [b"authority", vault.key().as_ref()],
        bump = vault.authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        address = vault.share_mint
    )]
    pub share_mint: Account<'info, Mint>,

    #[account(
        mut,
        address = vault.vault_token_account
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = vault.share_mint,
        token::authority = user
    )]
    pub user_share_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = vault.base_mint,
        token::authority = user
    )]
    pub user_base_account: Account<'info, TokenAccount>,

    pub user: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RebalanceStrategy<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.base_mint.as_ref()],
        bump = vault.bump,
        has_one = admin
    )]
    pub vault: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [b"strategy", vault.key().as_ref(), &[strategy.strategy_id]],
        bump = strategy.bump
    )]
    pub strategy: Account<'info, StrategyState>,

    pub admin: Signer<'info>,
}

// -----------------------------------------------------------------------------
// STATE DEFINITIONS
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct VaultState {
    pub admin: Pubkey,
    pub base_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub share_mint: Pubkey,
    pub total_strategy_allocation: u64,
    pub active_strategies: u8,
    pub bump: u8,
    pub authority_bump: u8,
    pub paused: bool,
}

impl VaultState {
    pub fn compute_total_assets(&self, idle_reserves: u64) -> Result<u64> {
        idle_reserves
            .checked_add(self.total_strategy_allocation)
            .ok_or(error!(GrowVaultError::MathOverflow))
    }
}

#[account]
#[derive(InitSpace)]
pub struct StrategyState {
    pub vault: Pubkey,
    pub strategy_id: u8,
    pub protocol_address: Pubkey,
    pub allocated_amount: u64,
    pub max_allocation_cap: u64,
    pub bump: u8,
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct VaultInitializedEvent {
    pub admin: Pubkey,
    pub base_mint: Pubkey,
    pub share_mint: Pubkey,
}

#[event]
pub struct DepositEvent {
    pub user: Pubkey,
    pub base_amount: u64,
    pub shares_minted: u64,
}

#[event]
pub struct WithdrawEvent {
    pub user: Pubkey,
    pub shares_burned: u64,
    pub base_amount_returned: u64,
}

#[event]
pub struct RebalanceEvent {
    pub strategy: Pubkey,
    pub previous_allocation: u64,
    pub new_allocation: u64,
}

#[error_code]
pub mod GrowVaultError {
    #[msg("Calculation overflowed standard bounds.")]
    MathOverflow,
    #[msg("Zero deposit amount is rejected.")]
    ZeroDepositAmount,
    #[msg("Zero shares minted; deposit too small.")]
    ZeroMintedShares,
    #[msg("Zero shares requested for redemption.")]
    ZeroShares,
    #[msg("No shares exist in the vault.")]
    EmptyPool,
    #[msg("Slippage limit violated during withdrawal.")]
    SlippageExceeded,
    #[msg("Vault is currently paused by admin.")]
    VaultPaused,
    #[msg("Invalid strategy target for this vault.")]
    InvalidStrategy,
    #[msg("Strategy allocation calculation resulted in negative balance.")]
    InvalidStrategyAllocation,
}