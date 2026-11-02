const tx = await program.methods
  .depositAndDca(new BN(50_000_000), minOutShares)
  .accounts({ vault: userVaultPda, ... })
  .rpc();