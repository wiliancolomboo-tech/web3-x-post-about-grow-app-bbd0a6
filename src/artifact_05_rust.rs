use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("GrwV1tZ8Vn3r5VdGkP47Hj8cQfFmS2vBvR9kLmNwXyz1");

pub const PRECISION_SCALER: u128 = 1_000_000_000_000; // 1e12 fixed point factor

#[program]
pub mod grow_vault {
    use super::*;

    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        vault_id: u64,
        deposit_fee_bps: u16,
    ) -> Result<()> {
        require!(deposit_fee_bps <= 500, VaultError::ExcessiveFee); // Max 5%

        let vault = &mut ctx.accounts.vault;
        vault.authority = ctx.accounts.authority.key();
        vault.token_mint = ctx.accounts.token_mint.key();
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.vault_id = vault_id;
        vault.total_shares = 0;
        vault.total_deposited = 0;
        vault.accumulated_yield_per_share = 0;
        vault.last_update_ts = Clock::get()?.unix_timestamp;
        vault.deposit_fee_bps = deposit_fee_bps;
        vault.bump = ctx.bumps.vault;
        vault.signer_bump = ctx.bumps.vault_signer;

        emit!(VaultInitializedEvent {
            vault_id,
            authority: vault.authority,
            mint: vault.token_mint,
        });

        Ok(())
    }

    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, VaultError::ZeroAmount);

        let vault = &mut ctx.accounts.vault;
        let clock = Clock::get()?;

        // Calculate fee if applicable
        let fee = (amount as u128)
            .checked_mul(vault.deposit_fee_bps as u128)
            .ok_or(VaultError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(VaultError::MathOverflow)? as u64;

        let net_deposit = amount.checked_sub(fee).ok_or(VaultError::MathOverflow)?;

        // Share computation: If first deposit, shares = net_deposit. Else proportional.
        let shares_to_mint = if vault.total_shares == 0 || vault.total_deposited == 0 {
            net_deposit
        } else {
            ((net_deposit as u128)
                .checked_mul(vault.total_shares as u128)
                .ok_or(VaultError::MathOverflow)?)
            .checked_div(vault.total_deposited as u128)
            .ok_or(VaultError::MathOverflow)? as u64
        };

        require!(shares_to_mint > 0, VaultError::ZeroSharesMinted);

        // Update user state and accrued yield settlement
        let user_position = &mut ctx.accounts.user_position;
        if user_position.shares > 0 {
            let pending_yield = compute_pending_yield(
                user_position.shares,
                vault.accumulated_yield_per_share,
                user_position.last_acc_yield_index,
            )?;
            user_position.pending_rewards = user_position
                .pending_rewards
                .checked_add(pending_yield)
                .ok_or(VaultError::MathOverflow)?;
        }

        user_position.owner = ctx.accounts.user.key();
        user_position.vault = vault.key();
        user_position.shares = user_position
            .shares
            .checked_add(shares_to_mint)
            .ok_or(VaultError::MathOverflow)?;
        user_position.last_acc_yield_index = vault.accumulated_yield_per_share;
        user_position.last_interaction_ts = clock.unix_timestamp;

        vault.total_shares = vault
            .total_shares
            .checked_add(shares_to_mint)
            .ok_or(VaultError::MathOverflow)?;
        vault.total_deposited = vault
            .total_deposited
            .checked_add(net_deposit)
            .ok_or(VaultError::MathOverflow)?;

        // CPI: Transfer tokens from user to vault
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.user_token_account.to_account_info(),
                    mint: ctx.accounts.token_mint.to_account_info(),
                    to: ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            amount,
            ctx.accounts.token_mint.decimals,
        )?;

        emit!(DepositEvent {
            user: ctx.accounts.user.key(),
            deposit_amount: amount,
            shares_minted: shares_to_mint,
            net_amount: net_deposit,
        });

        Ok(())
    }

    pub fn withdraw(ctx: Context<Withdraw>, shares_to_burn: u64) -> Result<()> {
        let user_position = &mut ctx.accounts.user_position;
        let vault = &mut ctx.accounts.vault;

        require!(shares_to_burn > 0, VaultError::ZeroAmount);
        require!(user_position.shares >= shares_to_burn, VaultError::InsufficientShares);

        // Compute reward accrual prior to share balance modification
        let accrued_yield = compute_pending_yield(
            user_position.shares,
            vault.accumulated_yield_per_share,
            user_position.last_acc_yield_index,
        )?;
        let total_claimable = user_position
            .pending_rewards
            .checked_add(accrued_yield)
            .ok_or(VaultError::MathOverflow)?;

        // Capital payout = (shares_to_burn / total_shares) * total_deposited
        let capital_payout = ((shares_to_burn as u128)
            .checked_mul(vault.total_deposited as u128)
            .ok_or(VaultError::MathOverflow)?)
        .checked_div(vault.total_shares as u128)
        .ok_or(VaultError::MathOverflow)? as u64;

        let total_payout = capital_payout
            .checked_add(total_claimable)
            .ok_or(VaultError::MathOverflow)?;

        // State modifications
        user_position.shares = user_position
            .shares
            .checked_sub(shares_to_burn)
            .ok_or(VaultError::MathOverflow)?;
        user_position.last_acc_yield_index = vault.accumulated_yield_per_share;
        user_position.pending_rewards = 0;
        user_position.cumulative_claimed = user_position
            .cumulative_claimed
            .checked_add(total_claimable)
            .ok_or(VaultError::MathOverflow)?;

        vault.total_shares = vault
            .total_shares
            .checked_sub(shares_to_burn)
            .ok_or(VaultError::MathOverflow)?;
        vault.total_deposited = vault
            .total_deposited
            .checked_sub(capital_payout)
            .ok_or(VaultError::MathOverflow)?;

        // Transfer funds using vault authority signer seeds
        let vault_key = vault.key();
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"vault_signer",
            vault_key.as_ref(),
            &[vault.signer_bump],
        ]];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault_token_account.to_account_info(),
                    mint: ctx.accounts.token_mint.to_account_info(),
                    to: ctx.accounts.user_token_account.to_account_info(),
                    authority: ctx.accounts.vault_signer.to_account_info(),
                },
                signer_seeds,
            ),
            total_payout,
            ctx.accounts.token_mint.decimals,
        )?;

        emit!(WithdrawEvent {
            user: ctx.accounts.user.key(),
            shares_burned: shares_to_burn,
            capital_payout,
            yield_payout: total_claimable,
        });

        Ok(())
    }

    pub fn harvest_and_compound(
        ctx: Context<HarvestAndCompound>,
        gross_yield_generated: u64,
    ) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        require!(vault.total_shares > 0, VaultError::ZeroSharesVault);

        if gross_yield_generated > 0 {
            // Update the global accumulator index: index += (yield * 1e12) / total_shares
            let index_delta = (gross_yield_generated as u128)
                .checked_mul(PRECISION_SCALER)
                .ok_or(VaultError::MathOverflow)?
                .checked_div(vault.total_shares as u128)
                .ok_or(VaultError::MathOverflow)?;

            vault.accumulated_yield_per_share = vault
                .accumulated_yield_per_share
                .checked_add(index_delta)
                .ok_or(VaultError::MathOverflow)?;

            vault.last_update_ts = Clock::get()?.unix_timestamp;

            emit!(HarvestEvent {
                vault_id: vault.vault_id,
                yield_added: gross_yield_generated,
                new_acc_index: vault.accumulated_yield_per_share,
            });
        }

        Ok(())
    }
}

// ---------------- Helper Math ----------------
fn compute_pending_yield(shares: u64, global_idx: u128, user_idx: u128) -> Result<u64> {
    let delta = global_idx
        .checked_sub(user_idx)
        .ok_or(VaultError::MathOverflow)?;
    let pending = (shares as u128)
        .checked_mul(delta)
        .ok_or(VaultError::MathOverflow)?
        .checked_div(PRECISION_SCALER)
        .ok_or(VaultError::MathOverflow)?;
    Ok(pending as u64)
}

// ---------------- Contexts ----------------

#[derive(Accounts)]
#[instruction(vault_id: u64)]
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = 8 + VaultState::INIT_SPACE,
        seeds = [b"vault", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub vault: Account<'info, VaultState>,

    /// CHECK: Vault Signer PDA acting as token account authority
    #[account(
        seeds = [b"vault_signer", vault.key().as_ref()],
        bump
    )]
    pub vault_signer: UncheckedAccount<'info>,

    #[account(
        init,
        payer = authority,
        token::mint = token_mint,
        token::authority = vault_signer,
        seeds = [b"vault_token", vault.key().as_ref()],
        bump
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub mod deposit_ctx {
    use super::*;
    #[derive(Accounts)]
    pub struct Deposit<'info> {
        #[account(mut)]
        pub user: Signer<'info>,

        #[account(
            mut,
            seeds = [b"vault", vault.vault_id.to_le_bytes().as_ref()],
            bump = vault.bump,
            has_one = token_mint,
            has_one = vault_token_account
        )]
        pub vault: Account<'info, VaultState>,

        pub token_mint: InterfaceAccount<'info, Mint>,

        #[account(
            mut,
            token::mint = token_mint,
            token::authority = user
        )]
        pub user_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut)]
        pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(
            init_if_needed,
            payer = user,
            space = 8 + UserPosition::INIT_SPACE,
            seeds = [b"user_position", user.key().as_ref(), vault.key().as_ref()],
            bump
        )]
        pub user_position: Account<'info, UserPosition>,

        pub token_program: Interface<'info, TokenInterface>,
        pub system_program: Program<'info, System>,
    }
}
pub use deposit_ctx::*;

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", vault.vault_id.to_le_bytes().as_ref()],
        bump = vault.bump,
        has_one = token_mint,
        has_one = vault_token_account
    )]
    pub vault: Account<'info, VaultState>,

    /// CHECK: Validated via seeds
    #[account(
        seeds = [b"vault_signer", vault.key().as_ref()],
        bump = vault.signer_bump
    )]
    pub vault_signer: UncheckedAccount<'info>,

    pub token_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        token::mint = token_mint,
        token::authority = user
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"user_position", user.key().as_ref(), vault.key().as_ref()],
        bump,
        has_one = owner,
        has_one = vault
    )]
    pub user_position: Account<'info, UserPosition>,
    pub owner: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct HarvestAndCompound<'info> {
    #[account(
        mut,
        seeds = [b"vault", vault.vault_id.to_le_bytes().as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, VaultState>,

    pub crank_keeper: Signer<'info>,
}

// ---------------- State Structures ----------------

#[account]
#[derive(InitSpace)]
pub struct VaultState {
    pub authority: Pubkey,
    pub token_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub vault_id: u64,
    pub total_shares: u64,
    pub total_deposited: u64,
    pub accumulated_yield_per_share: u128,
    pub last_update_ts: i64,
    pub deposit_fee_bps: u16,
    pub bump: u8,
    pub signer_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct UserPosition {
    pub owner: Pubkey,
    pub vault: Pubkey,
    pub shares: u64,
    pub last_acc_yield_index: u128,
    pub pending_rewards: u64,
    pub cumulative_claimed: u64,
    pub last_interaction_ts: i64,
}

// ---------------- Events ----------------

#[event]
pub struct VaultInitializedEvent {
    pub vault_id: u64,
    pub authority: Pubkey,
    pub mint: Pubkey,
}

#[event]
pub struct DepositEvent {
    pub user: Pubkey,
    pub deposit_amount: u64,
    pub shares_minted: u64,
    pub net_amount: u64,
}

#[event]
pub struct WithdrawEvent {
    pub user: Pubkey,
    pub shares_burned: u64,
    pub capital_payout: u64,
    pub yield_payout: u64,
}

#[event]
pub struct HarvestEvent {
    pub vault_id: u64,
    pub yield_added: u64,
    pub new_acc_index: u128,
}

// ---------------- Errors ----------------

#[error_code]
pub enum VaultError {
    #[msg("Calculation overflow occurred")]
    MathOverflow,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Zero shares calculated for deposit")]
    ZeroSharesMinted,
    #[msg("Insufficient shares in user position")]
    InsufficientShares,
    #[msg("Deposit fee exceeds maximum allowed limit (500 bps)")]
    ExcessiveFee,
    #[msg("Operation invalid on zero share vault")]
    ZeroSharesVault,
}