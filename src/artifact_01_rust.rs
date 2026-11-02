> #[account]
> pub struct GrowVault {
>     pub authority: Pubkey,
>     pub asset_mint: Pubkey,
>     pub share_mint: Pubkey,
>     pub total_deposited: u64,
>     pub total_shares: u64,
>     pub last_compound_slot: u64,
>     pub performance_fee_bps: u16,
>     pub bump: u8,
> }
>