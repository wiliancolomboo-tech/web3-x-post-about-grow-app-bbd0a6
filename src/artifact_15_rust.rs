use anchor_lang::prelude::*;

#[error_code]
pub enum GrowVaultError {
    #[msg("The provided cryptographic signature for the X post attestation is invalid.")]
    InvalidSocialSignature,
    #[msg("The social campaign yield bonus has already been claimed for this vault.")]
    SocialBonusAlreadyClaimed,
    #[msg("Numerical overflow detected during vault mathematical execution.")]
    ArithmeticOverflow,
    #[msg("Insufficient liquidity available in the protocol reward reserve.")]
    InsufficientReserveBalance,
    #[msg("Slippage limit exceeded during automated yield rebalance.")]
    SlippageExceeded,
    #[msg("Unauthorized keeper execution attempt.")]
    UnauthorizedKeeper,
}