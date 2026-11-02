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
import { Keypair, PublicKey } from "@solana/web3.js";
import { expect } from "chai";

describe("grow-vault E2E QA Verification Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  const admin = Keypair.generate();
  const user = Keypair.generate();
  const keeper = Keypair.generate();

  let mint: PublicKey;
  let userAta: PublicKey;
  let keeperAta: PublicKey;
  let vaultAta: PublicKey;
  let configPda: PublicKey;
  let vaultPda: PublicKey;
  let vaultBump: number;

  const DECIMALS = 6;
  const INITIAL_MINT_AMOUNT = 1_000_000_000; // 1,000 Tokens (6 decimals)
  const DEPOSIT_AMOUNT = new anchor.BN(250_000_000); // 250 Tokens

  before(async () => {
    // Fund test keypairs via provider wallet
    for (const kp of [admin, user, keeper]) {
      const tx = await provider.connection.requestAirdrop(
        kp.publicKey,
        2 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(tx);
    }

    // Create Base Token Mint
    mint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      DECIMALS
    );

    // Derive Global Config PDA
    [configPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("global_config")],
      program.programId
    );

    // Derive User Vault PDA
    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("grow_vault"), user.publicKey.toBuffer(), mint.toBuffer()],
      program.programId
    );

    // Create User, Keeper, and Vault ATAs
    userAta = await createAccount(
      provider.connection,
      user,
      mint,
      user.publicKey
    );

    keeperAta = await createAccount(
      provider.connection,
      keeper,
      mint,
      keeper.publicKey
    );

    vaultAta = await createAccount(
      provider.connection,
      user,
      mint,
      vaultPda
    );

    // Mint tokens to user
    await mintTo(
      provider.connection,
      admin,
      mint,
      userAta,
      admin.publicKey,
      INITIAL_MINT_AMOUNT
    );
  });

  it("1. Initializes global protocol configuration", async () => {
    await program.methods
      .initializeConfig(100, 150) // 1% protocol fee, 1.5% keeper fee
      .accounts({
        config: configPda,
        admin: admin.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([admin])
      .rpc();

    const configAccount = await program.account.globalConfig.fetch(configPda);
    expect(configAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(configAccount.protocolFeeBps).to.equal(100);
    expect(configAccount.keeperIncentiveBps).to.equal(150);
  });

  it("2. Initializes non-custodial user vault", async () => {
    await program.methods
      .initializeVault()
      .accounts({
        vault: vaultPda,
        config: configPda,
        mint: mint,
        owner: user.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const vaultAccount = await program.account.userVault.fetch(vaultPda);
    expect(vaultAccount.owner.toBase58()).to.equal(user.publicKey.toBase58());
    expect(vaultAccount.totalDeposited.toNumber()).to.equal(0);
    expect(vaultAccount.totalShares.toNumber()).to.equal(0);
  });

  it("3. Executes verified deposit & share issuance", async () => {
    await program.methods
      .deposit(DEPOSIT_AMOUNT)
      .accounts({
        vault: vaultPda,
        mint: mint,
        userTokenAccount: userAta,
        vaultTokenAccount: vaultAta,
        owner: user.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const vaultAccount = await program.account.userVault.fetch(vaultPda);
    expect(vaultAccount.totalDeposited.toString()).to.equal(DEPOSIT_AMOUNT.toString());
    expect(vaultAccount.totalShares.toString()).to.equal(DEPOSIT_AMOUNT.toString());

    const vaultBalance = await getAccount(provider.connection, vaultAta);
    expect(vaultBalance.amount.toString()).to.equal(DEPOSIT_AMOUNT.toString());
  });

  it("4. Prevents compounding before cooldown passes", async () => {
    try {
      await program.methods
        .compoundAndHarvest(new anchor.BN(5_000_000))
        .accounts({
          vault: vaultPda,
          config: configPda,
          mint: mint,
          vaultTokenAccount: vaultAta,
          keeperTokenAccount: keeperAta,
          keeper: keeper.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([keeper])
        .rpc();
      expect.fail("Expected cooldown check to fail");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("CompoundingTooFrequent");
    }
  });

  it("5. Completes partial withdrawal with accurate share calculation", async () => {
    const sharesToWithdraw = new anchor.BN(100_000_000); // 100 Tokens

    await program.methods
      .withdraw(sharesToWithdraw)
      .accounts({
        vault: vaultPda,
        mint: mint,
        vaultTokenAccount: vaultAta,
        userTokenAccount: userAta,
        owner: user.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const vaultAccount = await program.account.userVault.fetch(vaultPda);
    expect(vaultAccount.totalShares.toNumber()).to.equal(150_000_000);
    expect(vaultAccount.totalDeposited.toNumber()).to.equal(150_000_000);

    const userTokenBalance = await getAccount(provider.connection, userAta);
    expect(userTokenBalance.amount.toString()).to.equal(
      (INITIAL_MINT_AMOUNT - 150_000_000).toString()
    );
  });
});