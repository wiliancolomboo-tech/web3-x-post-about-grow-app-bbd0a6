use anchor_lang::prelude::*;

#[error_code]
pub enum VaultError {
    #[msg("Arithmetic overflow or underflow detected.")]
    MathOverflow,
    #[msg("Deposit would yield fewer shares than minimum required slippage limit.")]
    SlippageExceeded,
    #[msg("Withdrawal would yield fewer assets than minimum required slippage limit.")]
    WithdrawSlippageExceeded,
    #[msg("The vault is currently paused by emergency authority.")]
    VaultPaused,
    #[msg("Performance fee cannot exceed 2000 basis points (20%).")]
    ExcessiveFee,
    #[msg("Zero deposit or withdrawal amount is invalid.")]
    ZeroAmount,
    #[msg("Insufficient liquidity available in the asset reserve.")]
    InsufficientLiquidity,
}