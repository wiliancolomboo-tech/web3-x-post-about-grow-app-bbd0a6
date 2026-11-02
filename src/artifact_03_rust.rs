use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("GrwV111111111111111111111111111111111111111");

pub const VAULT_SEED: &[u8] = b"grow_vault";
pub const CONFIG_SEED: &[u8] = b"global_config";
pub const MAX_BPS: u16 = 10_000;
pub const MAX_KEEPER_FEE_BPS: u16 = 200; // 2% Maximum Cap

#[program]
pub mod grow_vault {
    use super::*;

    pub fn initialize_config(
        ctx: Context<InitializeConfig>,
        protocol_fee_bps: u16,
        keeper_incentive_bps: u16,
    ) -> Result<()> {
        require!(protocol_fee_bps <= 500, GrowError::FeeExceedsMax);
        require!(keeper_incentive_bps <= MAX_KEEPER_FEE_BPS, GrowError::FeeExceedsMax);

        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.protocol_fee_bps = protocol_fee_bps;
        config.keeper_incentive_bps = keeper_incentive_bps;
        config.total_vaults = 0;
        config.bump = ctx.bumps.config;

        emit!(ConfigInitializedEvent {
            admin: config.admin,
            protocol_fee_bps,
            keeper_incentive_bps,
        });

        Ok(())
    }

    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.owner = ctx.accounts.owner.key();
        vault.mint = ctx.accounts.mint.key();
        vault.total_deposited = 0;
        vault.total_shares = 0;
        vault.last_compound_timestamp = Clock::get()?.unix_timestamp;
        vault.bump = ctx.bumps.vault;

        let config = &mut ctx.accounts.config;
        config.total_vaults = config.total_vaults.checked_add(1).ok_or(GrowError::MathOverflow)?;

        emit!(VaultCreatedEvent {
            owner: vault.owner,
            mint: vault.mint,
            timestamp: vault.last_compound_timestamp,
        });

        Ok(())
    }

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, GrowError::ZeroAmount);

        let vault = &ctx.accounts.vault;
        let decimals = ctx.accounts.mint.decimals;

        // Calculate Vault Shares (1:1 upon inception, or proportional to accrued assets)
        let shares_to_mint = if vault.total_shares == 0 || vault.total_deposited == 0 {
            amount
        } else {
            (amount as u128)
                .checked_mul(vault.total_shares as u128)
                .ok_or(GrowError::MathOverflow)?
                .checked_div(vault.total_deposited as u128)
                .ok_or(GrowError::MathOverflow)? as u64
        };

        // Transfer funds from user token account to vault escrow token account
        let cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            mint: ctx.accounts.mint.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.owner.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token_interface::transfer_checked(cpi_ctx, amount, decimals)?;

        let vault = &mut ctx.accounts.vault;
        vault.total_deposited = vault.total_deposited.checked_add(amount).ok_or(GrowError::MathOverflow)?;
        vault.total_shares = vault.total_shares.checked_add(shares_to_mint).ok_or(GrowError::MathOverflow)?;

        emit!(DepositEvent {
            owner: vault.owner,
            amount,
            shares_minted: shares_to_mint,
        });

        Ok(())
    }

    pub fn compound_and_harvest(
        ctx: Context<CompoundAndHarvest>,
        simulated_yield_accrued: u64,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        let config = &ctx.accounts.config;

        let current_time = Clock::get()?.unix_timestamp;
        require!(
            current_time >= vault.last_compound_timestamp.checked_add(60).unwrap_or(i64::MAX),
            GrowError::CompoundingTooFrequent
        );

        if simulated_yield_accrued > 0 {
            let keeper_fee = (simulated_yield_accrued as u128)
                .checked_mul(config.keeper_incentive_bps as u128)
                .ok_or(GrowError::MathOverflow)?
                .checked_div(MAX_BPS as u128)
                .ok_or(GrowError::MathOverflow)? as u64;

            let net_yield = simulated_yield_accrued
                .checked_sub(keeper_fee)
                .ok_or(GrowError::MathOverflow)?;

            // Credit net yield to vault balance
            vault.total_deposited = vault
                .total_deposited
                .checked_add(net_yield)
                .ok_or(GrowError::MathOverflow)?;

            // Transfer keeper bounty
            if keeper_fee > 0 {
                let owner_key = vault.owner;
                let mint_key = vault.mint;
                let bump = vault.bump;
                let signer_seeds: &[&[&[u8]]] = &[&[
                    VAULT_SEED,
                    owner_key.as_ref(),
                    mint_key.as_ref(),
                    &[bump],
                ]];

                let cpi_accounts = TransferChecked {
                    from: ctx.accounts.vault_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.keeper_token_account.to_account_info(),
                    authority: vault.to_account_info(),
                };
                let cpi_ctx = CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    cpi_accounts,
                    signer_seeds,
                );
                token_interface::transfer_checked(cpi_ctx, keeper_fee, ctx.accounts.mint.decimals)?;
            }

            emit!(HarvestEvent {
                vault: vault.key(),
                keeper: ctx.accounts.keeper.key(),
                yield_harvested: simulated_yield_accrued,
                keeper_incentive: keeper_fee,
            });
        }

        vault.last_compound_timestamp = current_time;
        Ok(())
    }

    pub fn withdraw(ctx: Context<Withdraw>, shares_to_burn: u64) -> Result<()> {
        require!(shares_to_burn > 0, GrowError::ZeroAmount);
        let vault = &mut ctx.accounts.vault;
        require!(vault.total_shares >= shares_to_burn, GrowError::InsufficientShares);

        // Calculate underlying asset entitlement
        let amount_to_return = (shares_to_burn as u128)
            .checked_mul(vault.total_deposited as u128)
            .ok_or(GrowError::MathOverflow)?
            .checked_div(vault.total_shares as u128)
            .ok_or(GrowError::MathOverflow)? as u64;

        vault.total_shares = vault.total_shares.checked_sub(shares_to_burn).ok_or(GrowError::MathOverflow)?;
        vault.total_deposited = vault.total_deposited.checked_sub(amount_to_return).ok_or(GrowError::MathOverflow)?;

        let owner_key = vault.owner;
        let mint_key = vault.mint;
        let bump = vault.bump;
        let signer_seeds: &[&[&[u8]]] = &[&[
            VAULT_SEED,
            owner_key.as_ref(),
            mint_key.as_ref(),
            &[bump],
        ]];

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            mint: ctx.accounts.mint.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: vault.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token_interface::transfer_checked(cpi_ctx, amount_to_return, ctx.accounts.mint.decimals)?;

        emit!(WithdrawEvent {
            owner: vault.owner,
            shares_burned: shares_to_burn,
            amount_withdrawn: amount_to_return,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Account Validation Structs
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + GlobalConfig::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(
        init,
        payer = owner,
        space = 8 + UserVault::INIT_SPACE,
        seeds = [VAULT_SEED, owner.key().as_ref(), mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, UserVault>,
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump
    )]
    pub config: Account<'info, GlobalConfig>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub owner: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [VAULT_SEED, owner.key().as_ref(), mint.key().as_ref()],
        bump = vault.bump,
        has_one = owner @ GrowError::InvalidOwner,
        has_one = mint @ GrowError::InvalidMint,
    )]
    pub vault: Account<'info, UserVault>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = vault,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    pub owner: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct CompoundAndHarvest<'info> {
    #[account(
        mut,
        seeds = [VAULT_SEED, vault.owner.as_ref(), vault.mint.as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, UserVault>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, GlobalConfig>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = vault,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        token::mint = mint,
    )]
    pub keeper_token_account: InterfaceAccount<'info, TokenAccount>,
    pub keeper: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(
        mut,
        seeds = [VAULT_SEED, owner.key().as_ref(), mint.key().as_ref()],
        bump = vault.bump,
        has_one = owner @ GrowError::InvalidOwner,
        has_one = mint @ GrowError::InvalidMint,
    )]
    pub vault: Account<'info, UserVault>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = vault,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,
    pub owner: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

// -----------------------------------------------------------------------------
// State Structures
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct GlobalConfig {
    pub admin: Pubkey,
    pub protocol_fee_bps: u16,
    pub keeper_incentive_bps: u16,
    pub total_vaults: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct UserVault {
    pub owner: Pubkey,
    pub mint: Pubkey,
    pub total_deposited: u64,
    pub total_shares: u64,
    pub last_compound_timestamp: i64,
    pub bump: u8,
}

// -----------------------------------------------------------------------------
// Events & Errors
// -----------------------------------------------------------------------------

#[event]
pub struct ConfigInitializedEvent {
    pub admin: Pubkey,
    pub protocol_fee_bps: u16,
    pub keeper_incentive_bps: u16,
}

#[event]
pub struct VaultCreatedEvent {
    pub owner: Pubkey,
    pub mint: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct DepositEvent {
    pub owner: Pubkey,
    pub amount: u64,
    pub shares_minted: u64,
}

#[event]
pub struct HarvestEvent {
    pub vault: Pubkey,
    pub keeper: Pubkey,
    pub yield_harvested: u64,
    pub keeper_incentive: u64,
}

#[event]
pub struct WithdrawEvent {
    pub owner: Pubkey,
    pub shares_burned: u64,
    pub amount_withdrawn: u64,
}

#[error_code]
pub enum GrowError {
    #[msg("Fee parameter exceeds allowable maximum limit")]
    FeeExceedsMax,
    #[msg("Deposit or withdrawal amount must be greater than zero")]
    ZeroAmount,
    #[msg("Mathematical overflow occurred")]
    MathOverflow,
    #[msg("Compounding invoked too frequently")]
    CompoundingTooFrequent,
    #[msg("Insufficient vault shares for requested withdrawal")]
    InsufficientShares,
    #[msg("Invalid vault owner passed")]
    InvalidOwner,
    #[msg("Invalid token mint passed")]
    InvalidMint,
}