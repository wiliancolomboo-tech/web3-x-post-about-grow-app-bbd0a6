use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("Grow111111111111111111111111111111111111111");

pub const MASTER_VAULT_SEED: &[u8] = b"grow_master_vault";
pub const USER_POSITION_SEED: &[u8] = b"grow_user_position";
pub const STRATEGY_SEED: &[u8] = b"grow_strategy";
pub const KEEPER_FEE_BPS: u64 = 50; // 0.50% keeper crank bounty
pub const BPS_DENOMINATOR: u64 = 10_000;

#[program]
pub mod grow_vault {
    use super::*;

    /// Initializes the Master Grow Vault for a designated SPL token mint.
    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.asset_mint = ctx.accounts.asset_mint.key();
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.total_shares = 0;
        vault.total_deposited_assets = 0;
        vault.strategy_count = 0;
        vault.paused = false;
        vault.bump = ctx.bumps.vault;

        emit!(VaultInitializedEvent {
            vault: vault.key(),
            asset_mint: vault.asset_mint,
            authority: vault.authority,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Deposits tokens into the Grow Vault and issues virtual compounding shares.
    pub fn deposit(ctx: Context<Deposit>, amount: u64, min_shares_out: u128) -> Result<()> {
        require!(!ctx.accounts.vault.paused, GrowError::ProtocolPaused);
        require!(amount > 0, GrowError::ZeroAmount);

        let vault = &ctx.accounts.vault;
        let decimals = ctx.accounts.asset_mint.decimals;

        // Virtual Share Pricing Math: prevent pool share inflation attack
        let shares_to_mint: u128 = if vault.total_shares == 0 || vault.total_deposited_assets == 0 {
            // 1:1 Initial conversion with 1e6 scaling
            (amount as u128)
                .checked_mul(1_000_000)
                .ok_or(GrowError::MathOverflow)?
        } else {
            // (deposit_amount * total_shares) / total_deposited_assets
            (amount as u128)
                .checked_mul(vault.total_shares)
                .ok_or(GrowError::MathOverflow)?
                .checked_div(vault.total_deposited_assets as u128)
                .ok_or(GrowError::MathOverflow)?
        };

        require!(
            shares_to_mint >= min_shares_out,
            GrowError::SlippageExceeded
        );

        // Execute CPI: Transfer user tokens to Vault Token Account
        let transfer_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            mint: ctx.accounts.asset_mint.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            transfer_accounts,
        );
        token_interface::transfer_checked(cpi_ctx, amount, decimals)?;

        // Mutate Vault State
        let vault_mut = &mut ctx.accounts.vault;
        vault_mut.total_shares = vault_mut
            .total_shares
            .checked_add(shares_to_mint)
            .ok_or(GrowError::MathOverflow)?;
        vault_mut.total_deposited_assets = vault_mut
            .total_deposited_assets
            .checked_add(amount)
            .ok_or(GrowError::MathOverflow)?;

        // Mutate User Position
        let position = &mut ctx.accounts.user_position;
        position.owner = ctx.accounts.user.key();
        position.vault = vault_mut.key();
        position.shares = position
            .shares
            .checked_add(shares_to_mint)
            .ok_or(GrowError::MathOverflow)?;
        position.last_deposit_timestamp = Clock::get()?.unix_timestamp;

        emit!(DepositEvent {
            vault: vault_mut.key(),
            user: ctx.accounts.user.key(),
            amount,
            shares_minted: shares_to_mint,
            timestamp: position.last_deposit_timestamp,
        });

        Ok(())
    }

    /// Withdraws shares back into underlying assets including accrued yield.
    pub fn withdraw(ctx: Context<Withdraw>, shares_to_burn: u128, min_assets_out: u64) -> Result<()> {
        require!(!ctx.accounts.vault.paused, GrowError::ProtocolPaused);
        require!(shares_to_burn > 0, GrowError::ZeroAmount);

        let position = &mut ctx.accounts.user_position;
        require!(position.shares >= shares_to_burn, GrowError::InsufficientShares);

        let vault = &ctx.accounts.vault;
        let decimals = ctx.accounts.asset_mint.decimals;

        // Calculate assets owed: (shares_to_burn * total_deposited_assets) / total_shares
        let assets_to_return: u64 = (shares_to_burn)
            .checked_mul(vault.total_deposited_assets as u128)
            .ok_or(GrowError::MathOverflow)?
            .checked_div(vault.total_shares)
            .ok_or(GrowError::MathOverflow)? as u64;

        require!(
            assets_to_return >= min_assets_out,
            GrowError::SlippageExceeded
        );

        // Update Position State
        position.shares = position
            .shares
            .checked_sub(shares_to_burn)
            .ok_or(GrowError::MathOverflow)?;

        // Update Vault State
        let vault_mut = &mut ctx.accounts.vault;
        vault_mut.total_shares = vault_mut
            .total_shares
            .checked_sub(shares_to_burn)
            .ok_or(GrowError::MathOverflow)?;
        vault_mut.total_deposited_assets = vault_mut
            .total_deposited_assets
            .checked_sub(assets_to_return)
            .ok_or(GrowError::MathOverflow)?;

        // Transfer funds using PDA signer seeds
        let mint_key = vault_mut.asset_mint;
        let bump_bytes = [vault_mut.bump];
        let signer_seeds: &[&[&[u8]]] = &[&[
            MASTER_VAULT_SEED,
            mint_key.as_ref(),
            &bump_bytes,
        ]];

        let transfer_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            mint: ctx.accounts.asset_mint.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: vault_mut.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            transfer_accounts,
            signer_seeds,
        );
        token_interface::transfer_checked(cpi_ctx, assets_to_return, decimals)?;

        emit!(WithdrawEvent {
            vault: vault_mut.key(),
            user: ctx.accounts.user.key(),
            shares_burned: shares_to_burn,
            assets_received: assets_to_return,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Permissionless keeper instruction to harvest external yield and compound capital.
    pub fn harvest_and_compound(ctx: Context<HarvestAndCompound>, harvested_amount: u64) -> Result<()> {
        require!(!ctx.accounts.vault.paused, GrowError::ProtocolPaused);
        require!(harvested_amount > 0, GrowError::ZeroAmount);

        let decimals = ctx.accounts.asset_mint.decimals;

        // Keeper Fee Calculation: (harvested_amount * 50) / 10,000 = 0.50%
        let keeper_incentive = harvested_amount
            .checked_mul(KEEPER_FEE_BPS)
            .ok_or(GrowError::MathOverflow)?
            .checked_div(BPS_DENOMINATOR)
            .ok_or(GrowError::MathOverflow)?;

        let net_compounded_yield = harvested_amount
            .checked_sub(keeper_incentive)
            .ok_or(GrowError::MathOverflow)?;

        let vault_mut = &mut ctx.accounts.vault;
        let mint_key = vault_mut.asset_mint;
        let bump_bytes = [vault_mut.bump];
        let signer_seeds: &[&[&[u8]]] = &[&[
            MASTER_VAULT_SEED,
            mint_key.as_ref(),
            &bump_bytes,
        ]];

        // Pay keeper bounty directly from yield
        if keeper_incentive > 0 {
            let fee_transfer_accounts = TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                mint: ctx.accounts.asset_mint.to_account_info(),
                to: ctx.accounts.keeper_token_account.to_account_info(),
                authority: vault_mut.to_account_info(),
            };
            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                fee_transfer_accounts,
                signer_seeds,
            );
            token_interface::transfer_checked(cpi_ctx, keeper_incentive, decimals)?;
        }

        // Accrue net yield to pool without minting shares -> increases share price!
        vault_mut.total_deposited_assets = vault_mut
            .total_deposited_assets
            .checked_add(net_compounded_yield)
            .ok_or(GrowError::MathOverflow)?;

        emit!(CompoundEvent {
            vault: vault_mut.key(),
            keeper: ctx.accounts.keeper.key(),
            gross_yield: harvested_amount,
            keeper_bounty: keeper_incentive,
            net_compounded: net_compounded_yield,
            new_share_price_ratio: (vault_mut.total_deposited_assets as u128)
                .checked_mul(1_000_000)
                .unwrap_or(0)
                .checked_div(vault_mut.total_shares.max(1))
                .unwrap_or(0),
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub fn initialize_vault_context() {}

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub asset_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = 8 + MasterVault::INIT_SPACE,
        seeds = [MASTER_VAULT_SEED, asset_mint.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, MasterVault>,

    #[account(
        init,
        payer = authority,
        token::mint = asset_mint,
        token::authority = vault,
        seeds = [b"vault_token_account", vault.key().as_ref()],
        bump
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [MASTER_VAULT_SEED, asset_mint.key().as_ref()],
        bump = vault.bump,
        has_one = asset_mint,
        has_one = vault_token_account
    )]
    pub vault: Account<'info, MasterVault>,

    pub asset_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserPosition::INIT_SPACE,
        seeds = [USER_POSITION_SEED, vault.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub user_position: Account<'info, UserPosition>,

    #[account(
        mut,
        token::mint = asset_mint,
        token::authority = user
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [MASTER_VAULT_SEED, asset_mint.key().as_ref()],
        bump = vault.bump,
        has_one = asset_mint,
        has_one = vault_token_account
    )]
    pub vault: Account<'info, MasterVault>,

    pub asset_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        seeds = [USER_POSITION_SEED, vault.key().as_ref(), user.key().as_ref()],
        bump,
        has_one = owner,
        has_one = vault
    )]
    pub user_position: Account<'info, UserPosition>,

    /// CHECK: Safe via has_one check on user_position
    pub owner: AccountInfo<'info>,

    #[account(
        mut,
        token::mint = asset_mint,
        token::authority = user
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct HarvestAndCompound<'info> {
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [MASTER_VAULT_SEED, asset_mint.key().as_ref()],
        bump = vault.bump,
        has_one = asset_mint,
        has_one = vault_token_account
    )]
    pub vault: Account<'info, MasterVault>,

    pub asset_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        token::mint = asset_mint,
        token::authority = keeper
    )]
    pub keeper_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTS (Packed for minimum CU & Zero-Copy Efficiency)
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct MasterVault {
    pub authority: Pubkey,            // 32
    pub asset_mint: Pubkey,           // 32
    pub vault_token_account: Pubkey,  // 32
    pub total_shares: u128,           // 16
    pub total_deposited_assets: u64,  // 8
    pub strategy_count: u16,          // 2
    pub paused: bool,                 // 1
    pub bump: u8,                     // 1
}

#[account]
#[derive(InitSpace)]
pub struct UserPosition {
    pub owner: Pubkey,                // 32
    pub vault: Pubkey,                // 32
    pub shares: u128,                 // 16
    pub last_deposit_timestamp: i64,  // 8
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct VaultInitializedEvent {
    pub vault: Pubkey,
    pub asset_mint: Pubkey,
    pub authority: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct DepositEvent {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub amount: u64,
    pub shares_minted: u128,
    pub timestamp: i64,
}

#[event]
pub struct WithdrawEvent {
    pub vault: Pubkey,
    pub user: Pubkey,
    pub shares_burned: u128,
    pub assets_received: u64,
    pub timestamp: i64,
}

#[event]
pub struct CompoundEvent {
    pub vault: Pubkey,
    pub keeper: Pubkey,
    pub gross_yield: u64,
    pub keeper_bounty: u64,
    pub net_compounded: u64,
    pub new_share_price_ratio: u128,
    pub timestamp: i64,
}

#[error_code]
pub enum GrowError {
    #[msg("Operation rejected: Protocol vault is paused")]
    ProtocolPaused,
    #[msg("Input deposit or withdrawal amount must be greater than zero")]
    ZeroAmount,
    #[msg("Calculated shares or assets fall below minimum slippage threshold")]
    SlippageExceeded,
    #[msg("User position contains insufficient shares for withdrawal")]
    InsufficientShares,
    #[msg("Mathematical calculation triggered an overflow or underflow")]
    MathOverflow,
}