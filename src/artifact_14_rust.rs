use anchor_lang::prelude::*;

#[account]
pub struct GlobalConfig {
    /// Public key of the protocol administrative authority
    pub admin: Pubkey,
    /// Public key of the off-chain oracle that verifies social (X) engagement
    pub attestation_oracle: Pubkey,
    /// Mint of the asset deposited into vaults (e.g., USDC or Wrapped SOL)
    pub vault_mint: Pubkey,
    /// Vault reward token account holding protocol rewards
    pub reward_reserve: Pubkey,
    /// Amount credited per verified X campaign post (e.g., 5_000_000 = 5 USDC)
    pub reward_per_social_action: u64,
    /// Total cumulative deposits in the protocol
    pub total_staked: u64,
    /// Canonical bump for config PDA
    pub bump: u8,
}

impl GlobalConfig {
    pub const LEN: usize = 8 + 32 + 32 + 32 + 32 + 8 + 8 + 1;
}

#[account]
pub struct UserVault {
    /// Owner of this isolated vault
    pub owner: Pubkey,
    /// Mint of the deposited collateral
    pub mint: Pubkey,
    /// Total balance held in the vault
    pub balance: u64,
    /// Yield shares allocated to user
    pub shares: u64,
    /// Slot number of the last compound/harvest operation
    pub last_interaction_slot: u64,
    /// Flag indicating whether the user has claimed their X post bonus
    pub has_claimed_social_bonus: bool,
    /// Canonical bump seed for the vault PDA
    pub bump: u8,
}

impl UserVault {
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 8 + 1 + 1;
}