use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("GrwV111111111111111111111111111111111111111");

#[program]
pub mod grow_vault {
    use super::*;

    /// Initialize the global vault configuration.
    pub fn initialize_global_config(
        ctx: Context<InitializeGlobalConfig>,
        harvest_fee_bps: u16,
    ) -> Result<()> {
        require!(harvest_fee_bps <= 1000, VaultError::ExcessiveFee); // Max 10%

        let config = &mut ctx.accounts.global_config;
        config.authority = ctx.accounts.authority.key();
        config.treasury = ctx.accounts.treasury.key();
        config.harvest_fee_bps = harvest_fee_bps;
        config.is_paused = false;
        config.bump = ctx.bumps.global_config;

        emit!(GlobalConfigInitialized {
            authority: config.authority,
            treasury: config.treasury,
            harvest_fee_bps,
        });

        Ok(())
    }

    /// Deposit underlying tokens into the isolated user vault.
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, VaultError::ZeroAmount);
        require!(!ctx.accounts.global_config.is_paused, VaultError::ProtocolPaused);

        let user_vault = &mut ctx.accounts.user_vault;
        let total_assets_before = ctx.accounts.reserve_token_account.amount;
        let total_shares = user_vault.total_shares;

        // Calculate share distribution (constant-product model)
        let shares_to_mint = if total_shares == 0 || total_assets_before == 0 {
            amount
        } else {
            (amount as u128)
                .checked_mul(total_shares as u128)
                .ok_or(VaultError::MathOverflow)?
                .checked_div(total_assets_before as u128)
                .ok_or(VaultError::MathOverflow)? as u64
        };

        require!(shares_to_mint > 0, VaultError::ZeroShares);

        // Execute CPI Token Transfer using TransferChecked for Token-2022 compatibility
        let transfer_cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            mint: ctx.accounts.underlying_mint.to_account_info(),
            to: ctx.accounts.reserve_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };

        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            transfer_cpi_accounts,
        );

        token_interface::transfer_checked(
            cpi_ctx,
            amount,
            ctx.accounts.underlying_mint.decimals,
        )?;

        // Update state
        user_vault.deposited_principal = user_vault
            .deposited_principal
            .checked_add(amount)
            .ok_or(VaultError::MathOverflow)?;
        user_vault.total_shares = user_vault
            .total_shares
            .checked_add(shares_to_mint)
            .ok_or(VaultError::MathOverflow)?;
        user_vault.last_interaction_slot = Clock::get()?.slot;

        emit!(DepositExecuted {
            user: ctx.accounts.user.key(),
            mint: ctx.accounts.underlying_mint.key(),
            amount,
            shares_minted: shares_to_mint,
            slot: user_vault.last_interaction_slot,
        });

        Ok(())
    }

    /// Autonomous keeper harvest and rebalance cycle.
    pub fn harvest_and_rebalance(
        ctx: Context<HarvestAndRebalance>,
        external_yield_realized: u64,
    ) -> Result<()> {
        require!(!ctx.accounts.global_config.is_paused, VaultError::ProtocolPaused);
        require!(external_yield_realized > 0, VaultError::ZeroAmount);

        let config = &ctx.accounts.global_config;
        let fee = (external_yield_realized as u128)
            .checked_mul(config.harvest_fee_bps as u128)
            .ok_or(VaultError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(VaultError::MathOverflow)? as u64;

        if fee > 0 {
            let underlying_mint_key = ctx.accounts.underlying_mint.key();
            let seeds = &[
                b"token_reserve".as_ref(),
                underlying_mint_key.as_ref(),
                &[ctx.bumps.reserve_token_account],
            ];
            let signer_seeds = &[&seeds[..]];

            let transfer_fee_accounts = TransferChecked {
                from: ctx.accounts.reserve_token_account.to_account_info(),
                mint: ctx.accounts.underlying_mint.to_account_info(),
                to: ctx.accounts.treasury_token_account.to_account_info(),
                authority: ctx.accounts.reserve_token_account.to_account_info(),
            };

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                transfer_fee_accounts,
                signer_seeds,
            );

            token_interface::transfer_checked(
                cpi_ctx,
                fee,
                ctx.accounts.underlying_mint.decimals,
            )?;
        }

        emit!(YieldHarvested {
            underlying_mint: ctx.accounts.underlying_mint.key(),
            gross_yield: external_yield_realized,
            protocol_fee: fee,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Account Validation Structs
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeGlobalConfig<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + GlobalConfig::INIT_SPACE,
        seeds = [b"global_config", authority.key().as_ref()],
        bump
    )]
    pub global_config: Account<'info, GlobalConfig>,

    /// CHECK: Protocol treasury safe address
    pub treasury: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"global_config", global_config.authority.as_ref()],
        bump = global_config.bump,
    )]
    pub global_config: Account<'info, GlobalConfig>,

    pub underlying_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserVault::INIT_SPACE,
        seeds = [b"user_vault", user.key().as_ref(), underlying_mint.key().as_ref()],
        bump
    )]
    pub user_vault: Account<'info, UserVault>,

    #[account(
        mut,
        token::mint = underlying_mint,
        token::authority = user,
        token::token_program = token_program
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"token_reserve", underlying_mint.key().as_ref()],
        bump,
        token::mint = underlying_mint,
        token::authority = reserve_token_account,
        token::token_program = token_program
    )]
    pub reserve_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct HarvestAndRebalance<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(
        seeds = [b"global_config", global_config.authority.as_ref()],
        bump = global_config.bump,
    )]
    pub global_config: Account<'info, GlobalConfig>,

    pub underlying_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        seeds = [b"token_reserve", underlying_mint.key().as_ref()],
        bump,
        token::mint = underlying_mint,
        token::authority = reserve_token_account,
        token::token_program = token_program
    )]
    pub reserve_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        constraint = treasury_token_account.owner == global_config.treasury @ VaultError::InvalidTreasury,
        token::mint = underlying_mint,
        token::token_program = token_program
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

// -----------------------------------------------------------------------------
// State Models
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct GlobalConfig {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub harvest_fee_bps: u16,
    pub is_paused: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct UserVault {
    pub deposited_principal: u64,
    pub total_shares: u64,
    pub last_interaction_slot: u64,
}

// -----------------------------------------------------------------------------
// Events & Errors
// -----------------------------------------------------------------------------

#[event]
pub struct GlobalConfigInitialized {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub harvest_fee_bps: u16,
}

#[event]
pub struct DepositExecuted {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
    pub shares_minted: u64,
    pub slot: u64,
}

#[event]
pub struct YieldHarvested {
    pub underlying_mint: Pubkey,
    pub gross_yield: u64,
    pub protocol_fee: u64,
    pub timestamp: i64,
}

#[error_code]
pub enum VaultError {
    #[msg("Calculation overflow in math operations")]
    MathOverflow,
    #[msg("Zero deposit or harvest amount specified")]
    ZeroAmount,
    #[msg("Zero shares minted from contribution")]
    ZeroShares,
    #[msg("Configured harvest fee exceeds safety maximum (1000 bps)")]
    ExcessiveFee,
    #[msg("Protocol operations are currently paused by governance")]
    ProtocolPaused,
    #[msg("Destination treasury token account does not match governance configuration")]
    InvalidTreasury,
}