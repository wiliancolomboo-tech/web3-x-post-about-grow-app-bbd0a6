use crate::errors::VaultError;
use anchor_lang::prelude::*;

/// Virtual offset added to total shares and assets to prevent inflation attacks.
/// Even if total_shares = 0 and total_assets = 0, the denominator cannot be manipulated.
pub const VIRTUAL_SHARES_OFFSET: u128 = 1_000;
pub const VIRTUAL_ASSETS_OFFSET: u128 = 1;

/// Converts asset amount to vault shares:
/// shares = assets * (total_shares + 10^3) / (total_assets + 1)
pub fn convert_to_shares(
    assets: u64,
    total_assets: u64,
    total_shares: u64,
) -> Result<u64> {
    if assets == 0 {
        return Ok(0);
    }

    let numerator = (assets as u128)
        .checked_mul((total_shares as u128).checked_add(VIRTUAL_SHARES_OFFSET).unwrap())
        .ok_or(VaultError::MathOverflow)?;

    let denominator = (total_assets as u128)
        .checked_add(VIRTUAL_ASSETS_OFFSET)
        .unwrap();

    let shares = numerator
        .checked_div(denominator)
        .ok_or(VaultError::MathOverflow)?;

    u64::try_from(shares).map_err(|_| VaultError::MathOverflow.into())
}

/// Converts vault shares to underlying assets:
/// assets = shares * (total_assets + 1) / (total_shares + 10^3)
pub fn convert_to_assets(
    shares: u64,
    total_assets: u64,
    total_shares: u64,
) -> Result<u64> {
    if shares == 0 {
        return Ok(0);
    }

    let numerator = (shares as u128)
        .checked_mul((total_assets as u128).checked_add(VIRTUAL_ASSETS_OFFSET).unwrap())
        .ok_or(VaultError::MathOverflow)?;

    let denominator = (total_shares as u128)
        .checked_add(VIRTUAL_SHARES_OFFSET)
        .unwrap();

    let assets = numerator
        .checked_div(denominator)
        .ok_or(VaultError::MathOverflow)?;

    u64::try_from(assets).map_err(|_| VaultError::MathOverflow.into())
}