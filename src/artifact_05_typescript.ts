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

describe("Grow Protocol Vault E2E Verification Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  let assetMint: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let vaultTokenAccount: anchor.web3.PublicKey;
  let vaultBump: number;

  const authority = anchor.web3.Keypair.generate();
  const alice = anchor.web3.Keypair.generate();
  const keeper = anchor.web3.Keypair.generate();

  let aliceTokenAccount: anchor.web3.PublicKey;
  let keeperTokenAccount: anchor.web3.PublicKey;

  const INITIAL_ALICE_MINT = 10_000_000_000; // 10,000 tokens (6 decimals)
  const DEPOSIT_AMOUNT = new anchor.BN(1_000_000_000); // 1,000 tokens

  before(async () => {
    // Fund test keypairs
    for (const keypair of [authority, alice, keeper]) {
      const sig = await provider.connection.requestAirdrop(
        keypair.publicKey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    // Create Base Asset Mint (6 decimals, USDC simulation)
    assetMint = await createMint(
      provider.connection,
      authority,
      authority.publicKey,
      null,
      6
    );

    // Derive Master Vault PDA
    [vaultPda, vaultBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grow_master_vault"), assetMint.toBuffer()],
      program.programId
    );

    // Derive Vault Token Account
    [vaultTokenAccount] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_token_account"), vaultPda.toBuffer()],
      program.programId
    );

    // Create User ATAs
    aliceTokenAccount = await createAccount(
      provider.connection,
      alice,
      assetMint,
      alice.publicKey
    );
    keeperTokenAccount = await createAccount(
      provider.connection,
      keeper,
      assetMint,
      keeper.publicKey
    );

    await mintTo(
      provider.connection,
      authority,
      assetMint,
      aliceTokenAccount,
      authority,
      INITIAL_ALICE_MINT
    );
  });

  it("T01: Initializes Master Vault with valid PDA bindings", async () => {
    const tx = await program.methods
      .initializeVault()
      .accounts({
        authority: authority.publicKey,
        assetMint: assetMint,
        vault: vaultPda,
        vaultTokenAccount: vaultTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([authority])
      .rpc();

    const vaultState = await program.account.masterVault.fetch(vaultPda);
    expect(vaultState.authority.toBase58()).to.equal(authority.publicKey.toBase58());
    expect(vaultState.totalShares.toNumber()).to.equal(0);
    expect(vaultState.totalDepositedAssets.toNumber()).to.equal(0);
    expect(vaultState.bump).to.equal(vaultBump);
  });

  it("T02: Processes first micro-deposit and mints 1:1 scaled shares", async () => {
    const [userPositionPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grow_user_position"), vaultPda.toBuffer(), alice.publicKey.toBuffer()],
      program.programId
    );

    await program.methods
      .deposit(DEPOSIT_AMOUNT, new anchor.BN(0))
      .accounts({
        user: alice.publicKey,
        vault: vaultPda,
        assetMint: assetMint,
        userPosition: userPositionPda,
        userTokenAccount: aliceTokenAccount,
        vaultTokenAccount: vaultTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([alice])
      .rpc();

    const position = await program.account.userPosition.fetch(userPositionPda);
    const vaultState = await program.account.masterVault.fetch(vaultPda);

    // Initial deposit: 1,000 units * 1e6 scale
    const expectedShares = DEPOSIT_AMOUNT.mul(new anchor.BN(1_000_000));
    expect(position.shares.toString()).to.equal(expectedShares.toString());
    expect(vaultState.totalDepositedAssets.toString()).to.equal(DEPOSIT_AMOUNT.toString());
  });

  it("T03: Simulates yield accrual and triggers keeper harvest crank", async () => {
    // Simulate 100 tokens yield directly arriving in vault token account
    const yieldAmount = 100_000_000;
    await mintTo(
      provider.connection,
      authority,
      assetMint,
      vaultTokenAccount,
      authority,
      yieldAmount
    );

    const keeperBalBefore = (await getAccount(provider.connection, keeperTokenAccount)).amount;

    await program.methods
      .harvestAndCompound(new anchor.BN(yieldAmount))
      .accounts({
        keeper: keeper.publicKey,
        vault: vaultPda,
        assetMint: assetMint,
        keeperTokenAccount: keeperTokenAccount,
        vaultTokenAccount: vaultTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([keeper])
      .rpc();

    // 0.50% of 100_000_000 = 500_000 tokens
    const expectedBounty = 500_000;
    const keeperBalAfter = (await getAccount(provider.connection, keeperTokenAccount)).amount;
    expect(Number(keeperBalAfter - keeperBalBefore)).to.equal(expectedBounty);

    // Verify vault deposited assets increased by net yield (99.5 tokens)
    const vaultState = await program.account.masterVault.fetch(vaultPda);
    const expectedAssets = DEPOSIT_AMOUNT.toNumber() + (yieldAmount - expectedBounty);
    expect(vaultState.totalDepositedAssets.toNumber()).to.equal(expectedAssets);
  });

  it("T04: Withdraws user principal plus compounded yield", async () => {
    const [userPositionPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grow_user_position"), vaultPda.toBuffer(), alice.publicKey.toBuffer()],
      program.programId
    );

    const positionBefore = await program.account.userPosition.fetch(userPositionPda);
    const aliceBalBefore = (await getAccount(provider.connection, aliceTokenAccount)).amount;

    // Burn all shares
    await program.methods
      .withdraw(positionBefore.shares, new anchor.BN(0))
      .accounts({
        user: alice.publicKey,
        vault: vaultPda,
        assetMint: assetMint,
        userPosition: userPositionPda,
        owner: alice.publicKey,
        userTokenAccount: aliceTokenAccount,
        vaultTokenAccount: vaultTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([alice])
      .rpc();

    const aliceBalAfter = (await getAccount(provider.connection, aliceTokenAccount)).amount;
    const netReceived = Number(aliceBalAfter - aliceBalBefore);

    // Alice should receive initial 1,000 tokens + 99.5 tokens compounded yield = 1,099.5 tokens
    expect(netReceived).to.be.greaterThan(DEPOSIT_AMOUNT.toNumber());
    expect(netReceived).to.equal(1_099_500_000);
  });
});