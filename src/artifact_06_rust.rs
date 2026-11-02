use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked, MintTo, Burn};

declare_id!("Grow111111111111111111111111111111111111111");

/// Scaling factor for virtual offsets preventing ERC-4626 / SPL vault inflation attacks.
pub const VIRTUAL_OFFSET_SHARES: u128 = 1_000_000_000; // 1e9 virtual shares
pub const VIRTUAL_OFFSET_ASSETS: u128 = 1;             // 1 virtual unit of base asset
pub const MAX_BPS: u64 = 10_000;
pub const MAX_KEEPER_FEE_BPS: u64 = 500;              // Max 5% reward to keeper crank

#[program]
pub mod grow_vault {
    use super::*;

    /// Initializes a new yield-bearing vault for a target asset.
    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        keeper_fee_bps: u64,
    ) -> Result<()> {
        require!(keeper_fee_bps <= MAX_KEEPER_FEE_BPS, GrowError::FeeExceedsLimit);

        let vault = &mut ctx.accounts.vault;
        vault.admin = ctx.accounts.admin.key();
        vault.base_mint = ctx.accounts.base_mint.key();
        vault.share_mint = ctx.accounts.share_mint.key();
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.bump = ctx.bumps.vault;
        vault.vault_authority_bump = ctx.bumps.vault_authority;
        vault.keeper_fee_bps = keeper_fee_bps;
        vault.is_paused = false;
        vault.total_deposited_assets = 0;

        emit!(VaultInitializedEvent {
            vault: vault.key(),
            base_mint: vault.base_mint,
            admin: vault.admin,
        });

        Ok(())
    }

    /// Deposits base assets in exchange for newly minted virtual-share scaled LP tokens.
    pub fn deposit(
        ctx: Context<Deposit>,
        assets_in: u64,
        min_shares_expected: u64,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(!vault.is_paused, GrowError::VaultPaused);
        require!(assets_in > 0, GrowError::ZeroAmount);

        // Pre-transfer balance verification (accommodates Token-2022 transfer fees)
        let balance_before = ctx.accounts.vault_token_account.amount;

        // Transfer base assets from user to vault reserve
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.user_token_account.to_account_info(),
                mint: ctx.accounts.base_mint.to_account_info(),
                to: ctx.accounts.vault_token_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token_interface::transfer_checked(
            transfer_ctx,
            assets_in,
            ctx.accounts.base_mint.decimals,
        )?;

        // Reload account to observe actual received delta
        ctx.accounts.vault_token_account.reload()?;
        let balance_after = ctx.accounts.vault_token_account.amount;
        let actual_assets_in = balance_after
            .checked_sub(balance_before)
            .ok_or(GrowError::MathUnderflow)?;

        // Current totals
        let total_shares = ctx.accounts.share_mint.supply;
        let total_assets = balance_before; // Virtual base relative to pre-deposit liquidity

        // Anti-inflation share calculation:
        // shares_to_mint = actual_assets_in * (total_shares + VIRTUAL_OFFSET_SHARES) / (total_assets + VIRTUAL_OFFSET_ASSETS)
        let numerator = (actual_assets_in as u128)
            .checked_mul((total_shares as u128).checked_add(VIRTUAL_OFFSET_SHARES).unwrap())
            .ok_or(GrowError::MathOverflow)?;
        let denominator = (total_assets as u128)
            .checked_add(VIRTUAL_OFFSET_ASSETS)
            .unwrap();

        let shares_to_mint = u64::try_from(numerator.checked_div(denominator).ok_or(GrowError::MathZeroDivision)?)
            .map_err(|_| GrowError::ConversionOverflow)?;

        require!(shares_to_mint >= min_shares_expected, GrowError::SlippageExceeded);

        // Mint calculated shares to user via Vault Authority PDA
        let vault_key = vault.key();
        let auth_seeds: &[&[&[u8]]] = &[&[
            b"vault_authority",
            vault_key.as_ref(),
            &[vault.vault_authority_bump],
        ]];

        let mint_to_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.share_mint.to_account_info(),
                to: ctx.accounts.user_share_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            auth_seeds,
        );
        token_interface::mint_to(mint_to_ctx, shares_to_mint)?;

        // Update state records
        vault.total_deposited_assets = vault
            .total_deposited_assets
            .checked_add(actual_assets_in)
            .ok_or(GrowError::MathOverflow)?;

        let user_ledger = &mut ctx.accounts.user_ledger;
        user_ledger.user = ctx.accounts.user.key();
        user_ledger.vault = vault.key();
        user_ledger.total_deposited = user_ledger
            .total_deposited
            .checked_add(actual_assets_in)
            .ok_or(GrowError::MathOverflow)?;
        user_ledger.shares_held = user_ledger
            .shares_held
            .checked_add(shares_to_mint)
            .ok_or(GrowError::MathOverflow)?;

        emit!(DepositEvent {
            user: ctx.accounts.user.key(),
            assets_deposited: actual_assets_in,
            shares_minted: shares_to_mint,
        });

        Ok(())
    }

    /// Burns LP shares to redeem underlying base assets plus compounded yield.
    pub fn withdraw(
        ctx: Context<Withdraw>,
        shares_to_burn: u64,
        min_assets_expected: u64,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(shares_to_burn > 0, GrowError::ZeroAmount);

        let total_shares = ctx.accounts.share_mint.supply;
        let total_assets = ctx.accounts.vault_token_account.amount;

        // Anti-inflation asset conversion:
        // assets_to_return = shares_to_burn * (total_assets + VIRTUAL_OFFSET_ASSETS) / (total_shares + VIRTUAL_OFFSET_SHARES)
        let numerator = (shares_to_burn as u128)
            .checked_mul((total_assets as u128).checked_add(VIRTUAL_OFFSET_ASSETS).unwrap())
            .ok_or(GrowError::MathOverflow)?;
        let denominator = (total_shares as u128)
            .checked_add(VIRTUAL_OFFSET_SHARES)
            .unwrap();

        let assets_to_return = u64::try_from(numerator.checked_div(denominator).ok_or(GrowError::MathZeroDivision)?)
            .map_err(|_| GrowError::ConversionOverflow)?;

        require!(assets_to_return >= min_assets_expected, GrowError::SlippageExceeded);
        require!(ctx.accounts.vault_token_account.amount >= assets_to_return, GrowError::InsufficientVaultLiquidity);

        // Burn user shares
        let burn_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.share_mint.to_account_info(),
                from: ctx.accounts.user_share_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token_interface::burn(burn_ctx, shares_to_burn)?;

        // Transfer underlying assets from vault authority PDA to user
        let vault_key = vault.key();
        let auth_seeds: &[&[&[u8]]] = &[&[
            b"vault_authority",
            vault_key.as_ref(),
            &[vault.vault_authority_bump],
        ]];

        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                mint: ctx.accounts.base_mint.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            auth_seeds,
        );
        token_interface::transfer_checked(
            transfer_ctx,
            assets_to_return,
            ctx.accounts.base_mint.decimals,
        )?;

        // Update tracking state
        vault.total_deposited_assets = vault.total_deposited_assets.saturating_sub(assets_to_return);

        let user_ledger = &mut ctx.accounts.user_ledger;
        user_ledger.shares_held = user_ledger
            .shares_held
            .checked_sub(shares_to_burn)
            .ok_or(GrowError::MathUnderflow)?;

        emit!(WithdrawEvent {
            user: ctx.accounts.user.key(),
            assets_returned: assets_to_return,
            shares_burned: shares_to_burn,
        });

        Ok(())
    }

    /// Autonomous keeper crank: claims rewards from underlying positions and incentives cranks.
    pub fn compound_rewards(
        ctx: Context<CompoundRewards>,
        harvested_reward_amount: u64,
    ) -> Result<()> {
        let vault = &ctx.accounts.vault;
        require!(!vault.is_paused, GrowError::VaultPaused);

        if harvested_reward_amount > 0 {
            // Calculate permissionless keeper incentive tip
            let keeper_fee = (harvested_reward_amount as u128)
                .checked_mul(vault.keeper_fee_bps as u128)
                .unwrap()
                .checked_div(MAX_BPS as u128)
                .unwrap() as u64;

            if keeper_fee > 0 {
                let vault_key = vault.key();
                let auth_seeds: &[&[&[u8]]] = &[&[
                    b"vault_authority",
                    vault_key.as_ref(),
                    &[vault.vault_authority_bump],
                ]];

                let keeper_transfer_ctx = CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.vault_token_account.to_account_info(),
                        mint: ctx.accounts.base_mint.to_account_info(),
                        to: ctx.accounts.keeper_token_account.to_account_info(),
                        authority: ctx.accounts.vault_authority.to_account_info(),
                    },
                    auth_seeds,
                );
                token_interface::transfer_checked(
                    keeper_transfer_ctx,
                    keeper_fee,
                    ctx.accounts.base_mint.decimals,
                )?;
            }

            emit!(CompoundEvent {
                vault: vault.key(),
                keeper: ctx.accounts.keeper.key(),
                harvested_amount: harvested_reward_amount,
                keeper_incentive: keeper_fee,
            });
        }

        Ok(())
    }

    /// Emergency administrative control to halt mutations.
    pub fn set_emergency_pause(ctx: Context<AdminControl>, is_paused: bool) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        vault.is_paused = is_paused;
        emit!(VaultPauseStatusEvent { vault: vault.key(), is_paused });
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT CONTEXT STRUCTURES
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        seeds = [b"share_mint", vault.key().as_ref()],
        bump,
        payer = admin,
        mint::decimals = base_mint.decimals,
        mint::authority = vault_authority,
    )]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        seeds = [b"vault", base_mint.key().as_ref()],
        bump,
        payer = admin,
        space = 8 + Vault::INIT_SPACE,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// CHECK: PDA acting as the programmatic authority for transfers and mints.
    #[account(
        seeds = [b"vault_authority", vault.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        init,
        seeds = [b"vault_reserve", vault.key().as_ref()],
        bump,
        payer = admin,
        token::mint = base_mint,
        token::authority = vault_authority,
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

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
        seeds = [b"vault", base_mint.key().as_ref()],
        bump = vault.bump,
        has_one = base_mint,
        has_one = share_mint,
        has_one = vault_token_account,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// CHECK: Vault signer PDA
    #[account(
        seeds = [b"vault_authority", vault.key().as_ref()],
        bump = vault.vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"share_mint", vault.key().as_ref()],
        bump
    )]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub user_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = user,
        seeds = [b"user_ledger", vault.key().as_ref(), user.key().as_ref()],
        bump,
        space = 8 + UserLedger::INIT_SPACE
    )]
    pub user_ledger: Box<Account<'info, UserLedger>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", base_mint.key().as_ref()],
        bump = vault.bump,
        has_one = base_mint,
        has_one = share_mint,
        has_one = vault_token_account,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault_authority", vault.key().as_ref()],
        bump = vault.vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        seeds = [b"share_mint", vault.key().as_ref()],
        bump
    )]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub user_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [b"user_ledger", vault.key().as_ref(), user.key().as_ref()],
        bump,
        has_one = user,
        has_one = vault,
    )]
    pub user_ledger: Box<Account<'info, UserLedger>>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct CompoundRewards<'info> {
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", base_mint.key().as_ref()],
        bump = vault.bump,
        has_one = base_mint,
        has_one = vault_token_account,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault_authority", vault.key().as_ref()],
        bump = vault.vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub keeper_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct AdminControl<'info> {
    #[account(
        mut,
        has_one = admin @ GrowError::UnauthorizedAdmin
    )]
    pub vault: Box<Account<'info, Vault>>,
    pub admin: Signer<'info>,
}

// -----------------------------------------------------------------------------
// STATE STORAGE
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct Vault {
    pub admin: Pubkey,
    pub base_mint: Pubkey,
    pub share_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub total_deposited_assets: u64,
    pub keeper_fee_bps: u64,
    pub is_paused: bool,
    pub bump: u8,
    pub vault_authority_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct UserLedger {
    pub user: Pubkey,
    pub vault: Pubkey,
    pub total_deposited: u64,
    pub shares_held: u64,
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct VaultInitializedEvent {
    pub vault: Pubkey,
    pub base_mint: Pubkey,
    pub admin: Pubkey,
}

#[event]
pub struct DepositEvent {
    pub user: Pubkey,
    pub assets_deposited: u64,
    pub shares_minted: u64,
}

#[event]
pub struct WithdrawEvent {
    pub user: Pubkey,
    pub assets_returned: u64,
    pub shares_burned: u64,
}

#[event]
pub struct CompoundEvent {
    pub vault: Pubkey,
    pub keeper: Pubkey,
    pub harvested_amount: u64,
    pub keeper_incentive: u64,
}

#[event]
pub struct VaultPauseStatusEvent {
    pub vault: Pubkey,
    pub is_paused: bool,
}

#[error_code]
pub enum GrowError {
    #[msg("Administrative authorization check failed.")]
    UnauthorizedAdmin,
    #[msg("Requested operation exceeds permissible BPS limit.")]
    FeeExceedsLimit,
    #[msg("Vault operations are currently paused.")]
    VaultPaused,
    #[msg("Transaction amount must be strictly positive.")]
    ZeroAmount,
    #[msg("Calculated shares or assets violates user slippage thresholds.")]
    SlippageExceeded,
    #[msg("Vault lacks sufficient liquidity to service redemption.")]
    InsufficientVaultLiquidity,
    #[msg("Integer multiplication overflow occurred.")]
    MathOverflow,
    #[msg("Integer arithmetic underflow occurred.")]
    MathUnderflow,
    #[msg("Division by zero in conversion calculation.")]
    MathZeroDivision,
    #[msg("Numerical value failed conversion downcast.")]
    ConversionOverflow,
}