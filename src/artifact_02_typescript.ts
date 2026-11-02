> const tx = await growClient.vaults.deposit({
>   vault: vaultPda,
>   amount: new BN(100_000_000),
>   user: wallet.publicKey,
>   minSharesExpected: minShares
> });
>