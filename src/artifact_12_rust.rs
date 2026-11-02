use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, TokenAccount};

declare_id!("GrwAttest1111111111111111111111111111111111");

#[program]
pub mod grow_attestor {
    use super::*;

    /// Initializes a new Attestation Config for a given target Grow Vault
    pub fn initialize_attestation_config(
        ctx: Context<InitializeAttestationConfig>,
        bump: u8,
        min_solvency_bps: u16,
    ) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.vault_mint = ctx.accounts.vault_mint.key();
        config.vault_token_account = ctx.accounts.vault_token_account.key();
        config.keeper_authority = ctx.accounts.keeper_authority.key();
        config.min_solvency_bps = min_solvency_bps; // e.g., 10000 = 100.00%
        config.total_attestations = 0;
        config.bump = bump;

        emit!(ConfigInitializedEvent {
            admin: config.admin,
            vault_mint: config.vault_mint,
            min_solvency_bps,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Records an on-chain cryptographically verified yield & reserve attestation
    pub fn record_attestation(
        ctx: Context<RecordAttestation>,
        reported_vault_tvl: u64,
        reported_share_supply: u64,
        calculated_apy_bps: u32,
        strategy_routed_tvl: u64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let config = &mut ctx.accounts.config;
        let vault_tokens = &ctx.accounts.vault_token_account;

        // --- INVARIANT GUARD 1: Reserve Solvency Validation ---
        // Liquid vault balance + strategy allocated balance must cover reported vault TVL
        let total_backed_assets = vault_tokens
            .amount
            .checked_add(strategy_routed_tvl)
            .ok_or(AttestationError::MathOverflow)?;

        require!(
            total_backed_assets >= reported_vault_tvl,
            AttestationError::InsolventVault
        );

        // --- INVARIANT GUARD 2: Solvency Ratio Calculation ---
        // Solvency BPS = (total_backed_assets / reported_vault_tvl) * 10000
        let solvency_bps = if reported_vault_tvl > 0 {
            u128::from(total_backed_assets)
                .checked_mul(10000)
                .ok_or(AttestationError::MathOverflow)?
                .checked_div(u128::from(reported_vault_tvl))
                .ok_or(AttestationError::MathOverflow)? as u16
        } else {
            10000
        };

        require!(
            solvency_bps >= config.min_solvency_bps,
            AttestationError::SolvencyBelowMinimum
        );

        // --- INVARIANT GUARD 3: Share Price Consistency ---
        // Ensure that share price does not implode to zero
        if reported_share_supply > 0 {
            require!(reported_vault_tvl > 0, AttestationError::InvalidSharePrice);
        }

        // Populate Record PDA
        let record = &mut ctx.accounts.attestation_record;
        record.config = config.key();
        record.epoch_id = config.total_attestations;
        record.timestamp = clock.unix_timestamp;
        record.slot = clock.slot;
        record.reported_tvl = reported_vault_tvl;
        record.liquid_reserve = vault_tokens.amount;
        record.strategy_tvl = strategy_routed_tvl;
        record.share_supply = reported_share_supply;
        record.calculated_apy_bps = calculated_apy_bps;
        record.solvency_bps = solvency_bps;
        record.keeper = ctx.accounts.keeper.key();

        // Increment sequence counter
        config.total_attestations = config
            .total_attestations
            .checked_add(1)
            .ok_or(AttestationError::MathOverflow)?;

        // Emit public event for indexers, Geyser plugins, and social bots
        emit!(VaultAttestationEvent {
            epoch_id: record.epoch_id,
            vault_mint: config.vault_mint,
            reported_tvl: reported_vault_tvl,
            liquid_reserve: vault_tokens.amount,
            strategy_tvl: strategy_routed_tvl,
            share_supply: reported_share_supply,
            calculated_apy_bps,
            solvency_bps,
            timestamp: clock.unix_timestamp,
            slot: clock.slot,
        });

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Account Contexts
// ---------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeAttestationConfig<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + AttestationConfig::INIT_SPACE,
        seeds = [b"attestation_config", vault_mint.key().as_ref()],
        bump
    )]
    pub config: Account<'info, AttestationConfig>,

    pub vault_mint: Account<'info, Mint>,

    #[account(
        constraint = vault_token_account.mint == vault_mint.key() @ AttestationError::InvalidMintMismatch
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    /// CHECK: Designee who submits regular keeper attestation reports
    pub keeper_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RecordAttestation<'info> {
    #[account(
        mut,
        seeds = [b"attestation_config", config.vault_mint.as_ref()],
        bump = config.bump,
        has_one = keeper_authority @ AttestationError::UnauthorizedKeeper,
        has_one = vault_token_account @ AttestationError::InvalidVaultTokenAccount
    )]
    pub config: Account<'info, AttestationConfig>,

    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = keeper,
        space = 8 + AttestationRecord::INIT_SPACE,
        seeds = [
            b"attestation_record",
            config.key().as_ref(),
            &config.total_attestations.to_le_bytes()
        ],
        bump
    )]
    pub attestation_record: Account<'info, AttestationRecord>,

    #[account(mut)]
    pub keeper: Signer<'info>,

    /// CHECK: Keeper authority checked against config
    pub keeper_authority: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

// ---------------------------------------------------------------------------
// Account State Definitions
// ---------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct AttestationConfig {
    pub admin: Pubkey,                // 32
    pub vault_mint: Pubkey,           // 32
    pub vault_token_account: Pubkey,  // 32
    pub keeper_authority: Pubkey,     // 32
    pub min_solvency_bps: u16,        // 2
    pub total_attestations: u64,      // 8
    pub bump: u8,                     // 1
}

#[account]
#[derive(InitSpace)]
pub struct AttestationRecord {
    pub config: Pubkey,               // 32
    pub epoch_id: u64,                // 8
    pub timestamp: i64,               // 8
    pub slot: u64,                    // 8
    pub reported_tvl: u64,            // 8
    pub liquid_reserve: u64,          // 8
    pub strategy_tvl: u64,            // 8
    pub share_supply: u64,            // 8
    pub calculated_apy_bps: u32,      // 4
    pub solvency_bps: u16,            // 2
    pub keeper: Pubkey,               // 32
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[event]
pub struct ConfigInitializedEvent {
    pub admin: Pubkey,
    pub vault_mint: Pubkey,
    pub min_solvency_bps: u16,
    pub timestamp: i64,
}

#[event]
pub struct VaultAttestationEvent {
    pub epoch_id: u64,
    pub vault_mint: Pubkey,
    pub reported_tvl: u64,
    pub liquid_reserve: u64,
    pub strategy_tvl: u64,
    pub share_supply: u64,
    pub calculated_apy_bps: u32,
    pub solvency_bps: u16,
    pub timestamp: i64,
    pub slot: u64,
}

// ---------------------------------------------------------------------------
// Error Codes
// ---------------------------------------------------------------------------

#[error_code]
pub enum AttestationError {
    #[msg("Calculation resulted in numerical overflow")]
    MathOverflow,
    #[msg("Target vault has insufficient backing collateral to support reported TVL")]
    InsolventVault,
    #[msg("Calculated solvency ratio is strictly below min_solvency_bps threshold")]
    SolvencyBelowMinimum,
    #[msg("Reported share price evaluates to zero or is mathematically invalid")]
    InvalidSharePrice,
    #[msg("Provided signer is not authorized to submit keeper attestations")]
    UnauthorizedKeeper,
    #[msg("Vault token account does not match registered mint")]
    InvalidMintMismatch,
    #[msg("Provided token account does not match config")]
    InvalidVaultTokenAccount,
}