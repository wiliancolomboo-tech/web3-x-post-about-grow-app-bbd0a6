import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GrowVault } from "../target/types/grow_vault";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";

describe("grow_vault automated test matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  let mint: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let tokenVaultPda: anchor.web3.PublicKey;
  let userPositionPda: anchor.web3.PublicKey;

  const authority = anchor.web3.Keypair.generate();
  const user = anchor.web3.Keypair.generate();
  const keeper = anchor.web3.Keypair.generate();

  let userTokenAccount: anchor.web3.PublicKey;
  let keeperTokenAccount: anchor.web3.PublicKey;

  before(async () => {
    // Fund test wallets
    for (const kp of [authority, user, keeper]) {
      const sig = await provider.connection.requestAirdrop(
        kp.publicKey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    // Create Base Asset Mint (e.g. mock USDC with 6 decimals)
    mint = await createMint(
      provider.connection,
      authority,
      authority.publicKey,
      null,
      6
    );

    // Derive PDAs
    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grow_vault"), mint.toBuffer()],
      program.programId
    );

    [tokenVaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_token"), vaultPda.toBuffer()],
      program.programId
    );

    [userPositionPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("user_position"), user.publicKey.toBuffer(), vaultPda.toBuffer()],
      program.programId
    );

    // Create Associated Token Accounts
    userTokenAccount = await createAccount(
      provider.connection,
      user,
      mint,
      user.publicKey
    );

    keeperTokenAccount = await createAccount(
      provider.connection,
      keeper,
      mint,
      keeper.publicKey
    );

    // Mint 10,000 USDC to User
    await mintTo(
      provider.connection,
      authority,
      mint,
      userTokenAccount,
      authority,
      10_000 * 1_000_000
    );
  });

  it("Initializes the Grow Vault with verified PDA derivations", async () => {
    const feeBps = 100; // 1%
    const keeperBountyBps = 50; // 0.5%

    await program.methods
      .initializeVault(feeBps, keeperBountyBps)
      .accounts({
        authority: authority.publicKey,
        assetMint: mint,
        vault: vaultPda,
        tokenVault: tokenVaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([authority])
      .rpc();

    const vaultAccount = await program.account.vaultState.fetch(vaultPda);
    expect(vaultAccount.authority.toBase58()).to.equal(authority.publicKey.toBase58());
    expect(vaultAccount.totalShares.toNumber()).to.equal(0);
    expect(vaultAccount.totalUnderlying.toNumber()).to.equal(0);
    expect(vaultAccount.paused).to.be.false;
  });

  it("Executes first deposit and enforces anti-inflation dead shares", async () => {
    const depositAmount = new anchor.BN(50 * 1_000_000); // 50 USDC

    await program.methods
      .deposit(depositAmount)
      .accounts({
        user: user.publicKey,
        vault: vaultPda,
        tokenVault: tokenVaultPda,
        userPosition: userPositionPda,
        userTokenAccount: userTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const vaultAccount = await program.account.vaultState.fetch(vaultPda);
    const userPosition = await program.account.userPosition.fetch(userPositionPda);

    // Initial shares = amount - 1000 virtual offset
    expect(vaultAccount.totalUnderlying.toNumber()).to.equal(50_000_000);
    expect(userPosition.shares.toNumber()).to.equal(50_000_000 - 1_000);
    expect(vaultAccount.totalShares.toNumber()).to.equal(50_000_000);
  });

  it("Keeper crank harvests yield and updates share exchange rate", async () => {
    // Simulate 5 USDC external yield generated and injected into token vault
    const yieldAmount = 5 * 1_000_000;
    await mintTo(
      provider.connection,
      authority,
      mint,
      tokenVaultPda,
      authority,
      yieldAmount
    );

    const vaultPre = await program.account.vaultState.fetch(vaultPda);

    // Harvest crank trigger
    await program.methods
      .harvestAndCompound(new anchor.BN(yieldAmount))
      .accounts({
        keeper: keeper.publicKey,
        vault: vaultPda,
        tokenVault: tokenVaultPda,
        keeperTokenAccount: keeperTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([keeper])
      .rpc();

    const vaultPost = await program.account.vaultState.fetch(vaultPda);
    const keeperBalance = await getAccount(provider.connection, keeperTokenAccount);

    // 0.5% of 5,000,000 = 25,000 bounty to keeper
    expect(Number(keeperBalance.amount)).to.equal(25_000);
    expect(vaultPost.totalUnderlying.toNumber()).to.be.greaterThan(vaultPre.totalUnderlying.toNumber());
  });

  it("Withdraws assets with strict slippage protection", async () => {
    const userPositionPre = await program.account.userPosition.fetch(userPositionPda);
    const sharesToBurn = userPositionPre.shares;

    const userBalancePre = await getAccount(provider.connection, userTokenAccount);

    // Execution with acceptable slippage limit
    await program.methods
      .withdraw(sharesToBurn, new anchor.BN(50_000_000))
      .accounts({
        user: user.publicKey,
        vault: vaultPda,
        tokenVault: tokenVaultPda,
        userPosition: userPositionPda,
        userTokenAccount: userTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const userBalancePost = await getAccount(provider.connection, userTokenAccount);
    const delta = Number(userBalancePost.amount) - Number(userBalancePre.amount);

    // User gained yield proportionally
    expect(delta).to.be.greaterThan(50_000_000);
  });

  it("Enforces emergency circuit breaker toggles", async () => {
    // Pause vault
    await program.methods
      .setPaused(true)
      .accounts({
        authority: authority.publicKey,
        vault: vaultPda,
      })
      .signers([authority])
      .rpc();

    let threwError = false;
    try {
      await program.methods
        .deposit(new anchor.BN(10_000_000))
        .accounts({
          user: user.publicKey,
          vault: vaultPda,
          tokenVault: tokenVaultPda,
          userPosition: userPositionPda,
          userTokenAccount: userTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([user])
        .rpc();
    } catch (_err) {
      threwError = true;
    }

    expect(threwError).to.be.true;

    // Unpause vault
    await program.methods
      .setPaused(false)
      .accounts({
        authority: authority.publicKey,
        vault: vaultPda,
      })
      .signers([authority])
      .rpc();
  });
});