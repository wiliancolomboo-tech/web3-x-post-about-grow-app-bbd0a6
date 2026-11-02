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

describe("grow-vault comprehensive test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;
  const connection = provider.connection;

  const owner = anchor.web3.Keypair.generate();
  const keeper = anchor.web3.Keypair.generate();
  const strategy = anchor.web3.Keypair.generate();

  let depositMint: anchor.web3.Pubkey;
  let userTokenAccount: anchor.web3.Pubkey;
  let vaultTokenAccount: anchor.web3.Pubkey;
  let strategyTokenAccount: anchor.web3.Pubkey;

  let profilePda: anchor.web3.Pubkey;
  let vaultPda: anchor.web3.Pubkey;

  const MAX_DEBIT_EPOCH = new anchor.BN(10_000_000); // 10 USDC (6 decimals)
  const EPOCH_CADENCE = new anchor.BN(3600); // 1 hour

  before(async () => {
    // Fund test wallets
    const airdropSig1 = await connection.requestAirdrop(owner.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    const airdropSig2 = await connection.requestAirdrop(keeper.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    await connection.confirmTransaction(airdropSig1);
    await connection.confirmTransaction(airdropSig2);

    // Create deposit mint (e.g., mock USDC)
    depositMint = await createMint(
      connection,
      owner,
      owner.publicKey,
      null,
      6
    );

    // Derive PDAs
    [profilePda] = anchor.web3.Pubkey.findProgramAddressSync(
      [Buffer.from("profile"), owner.publicKey.toBuffer()],
      program.programId
    );

    [vaultPda] = anchor.web3.Pubkey.findProgramAddressSync(
      [Buffer.from("vault"), owner.publicKey.toBuffer()],
      program.programId
    );

    // Create user token account and mint balance
    userTokenAccount = await createAccount(
      connection,
      owner,
      depositMint,
      owner.publicKey
    );

    await mintTo(
      connection,
      owner,
      depositMint,
      userTokenAccount,
      owner,
      100_000_000 // 100 USDC
    );

    // Create Vault token account owned by Vault PDA
    vaultTokenAccount = await createAccount(
      connection,
      owner,
      depositMint,
      vaultPda
    );

    // Create Strategy token account
    strategyTokenAccount = await createAccount(
      connection,
      owner,
      depositMint,
      strategy.publicKey
    );
  });

  it("1. Initializes profile and vault accounts correctly", async () => {
    await program.methods
      .initializeProfile(MAX_DEBIT_EPOCH, EPOCH_CADENCE)
      .accounts({
        owner: owner.publicKey,
        userProfile: profilePda,
        vault: vaultPda,
        depositMint: depositMint,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([owner])
      .rpc();

    const profileState = await program.account.userProfile.fetch(profilePda);
    expect(profileState.owner.toBase58()).to.equal(owner.publicKey.toBase58());
    expect(profileState.maxDebitPerEpoch.toString()).to.equal(MAX_DEBIT_EPOCH.toString());
    expect(profileState.currentEpochSpent.toNumber()).to.equal(0);

    const vaultState = await program.account.vault.fetch(vaultPda);
    expect(vaultState.owner.toBase58()).to.equal(owner.publicKey.toBase58());
    expect(vaultState.totalDeposited.toNumber()).to.equal(0);
  });

  it("2. Executes a valid micro-deposit via authorized owner signer", async () => {
    const depositAmount = new anchor.BN(2_500_000); // 2.5 USDC

    await program.methods
      .depositMicro(depositAmount)
      .accounts({
        authority: owner.publicKey,
        userProfile: profilePda,
        vault: vaultPda,
        keeper: keeper.publicKey,
        userTokenAccount: userTokenAccount,
        vaultTokenAccount: vaultTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([owner])
      .rpc();

    const vaultBalance = await getAccount(connection, vaultTokenAccount);
    expect(Number(vaultBalance.amount)).to.equal(2_500_000);

    const profileState = await program.account.userProfile.fetch(profilePda);
    expect(profileState.currentEpochSpent.toString()).to.equal(depositAmount.toString());
  });

  it("3. Prevents micro-deposit exceeding epoch debit bounds", async () => {
    // Already spent 2.5 USDC, limit is 10 USDC. Attempting 8.0 USDC must fail.
    const excessDeposit = new anchor.BN(8_000_000);

    try {
      await program.methods
        .depositMicro(excessDeposit)
        .accounts({
          authority: owner.publicKey,
          userProfile: profilePda,
          vault: vaultPda,
          keeper: keeper.publicKey,
          userTokenAccount: userTokenAccount,
          vaultTokenAccount: vaultTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([owner])
        .rpc();
      expect.fail("Transaction should have failed with DebitLimitExceeded");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("DebitLimitExceeded");
    }
  });

  it("4. Routes accumulated reserves to strategy via signed PDA CPI", async () => {
    const routeAmount = new anchor.BN(2_000_000); // 2 USDC

    await program.methods
      .routeToYield(routeAmount)
      .accounts({
        keeperOrOwner: owner.publicKey,
        vault: vaultPda,
        vaultTokenAccount: vaultTokenAccount,
        strategyTokenAccount: strategyTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([owner])
      .rpc();

    const strategyBalance = await getAccount(connection, strategyTokenAccount);
    expect(Number(strategyBalance.amount)).to.equal(2_000_000);

    const vaultBalance = await getAccount(connection, vaultTokenAccount);
    expect(Number(vaultBalance.amount)).to.equal(500_000); // 2.5 - 2.0 = 0.5 USDC
  });

  it("5. Allows emergency withdrawal bypassing yield routing", async () => {
    await program.methods
      .emergencyWithdraw()
      .accounts({
        owner: owner.publicKey,
        vault: vaultPda,
        vaultTokenAccount: vaultTokenAccount,
        userTokenAccount: userTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([owner])
      .rpc();

    const vaultBalance = await getAccount(connection, vaultTokenAccount);
    expect(Number(vaultBalance.amount)).to.equal(0);
  });
});