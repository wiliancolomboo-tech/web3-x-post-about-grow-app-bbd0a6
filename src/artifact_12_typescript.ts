import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GrowVault } from "../target/types/grow_vault";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { PublicKey, Keypair, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";

describe("grow_vault automated test matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  let underlyingMint: PublicKey;
  let userTokenAccount: PublicKey;
  let treasuryTokenAccount: PublicKey;
  let reserveTokenAccount: PublicKey;
  let globalConfigPda: PublicKey;
  let userVaultPda: PublicKey;

  const authority = provider.wallet as anchor.Wallet;
  const user = Keypair.generate();
  const treasury = Keypair.generate();
  const keeper = Keypair.generate();

  const INITIAL_MINT_AMOUNT = 1_000_000_000; // 1,000 USDC (6 decimals)
  const DEPOSIT_AMOUNT = 100_000_000; // 100 USDC

  before(async () => {
    // Fund test keypairs
    const airdropSigUser = await provider.connection.requestAirdrop(
      user.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropSigUser);

    const airdropSigKeeper = await provider.connection.requestAirdrop(
      keeper.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropSigKeeper);

    // Initialize Mint
    underlyingMint = await createMint(
      provider.connection,
      authority.payer,
      authority.publicKey,
      null,
      6
    );

    // Derive PDAs
    [globalConfigPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("global_config"), authority.publicKey.toBuffer()],
      program.programId
    );

    [reserveTokenAccount] = PublicKey.findProgramAddressSync(
      [Buffer.from("token_reserve"), underlyingMint.toBuffer()],
      program.programId
    );

    [userVaultPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("user_vault"),
        user.publicKey.toBuffer(),
        underlyingMint.toBuffer(),
      ],
      program.programId
    );

    // Create Token Accounts
    userTokenAccount = await createAccount(
      provider.connection,
      authority.payer,
      underlyingMint,
      user.publicKey
    );

    treasuryTokenAccount = await createAccount(
      provider.connection,
      authority.payer,
      underlyingMint,
      treasury.publicKey
    );

    // Mint underlying tokens to user
    await mintTo(
      provider.connection,
      authority.payer,
      underlyingMint,
      userTokenAccount,
      authority.payer,
      INITIAL_MINT_AMOUNT
    );
  });

  it("Initializes global protocol configuration", async () => {
    const harvestFeeBps = 500; // 5% fee

    await program.methods
      .initializeGlobalConfig(harvestFeeBps)
      .accounts({
        globalConfig: globalConfigPda,
        treasury: treasury.publicKey,
        authority: authority.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const configAccount = await program.account.globalConfig.fetch(globalConfigPda);
    expect(configAccount.authority.toBase58()).to.equal(authority.publicKey.toBase58());
    expect(configAccount.treasury.toBase58()).to.equal(treasury.publicKey.toBase58());
    expect(configAccount.harvestFeeBps).to.equal(harvestFeeBps);
    expect(configAccount.isPaused).to.be.false;
  });

  it("Executes user deposit into isolated vault PDA", async () => {
    await program.methods
      .deposit(new anchor.BN(DEPOSIT_AMOUNT))
      .accounts({
        user: user.publicKey,
        globalConfig: globalConfigPda,
        underlyingMint: underlyingMint,
        userVault: userVaultPda,
        userTokenAccount: userTokenAccount,
        reserveTokenAccount: reserveTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const userVaultState = await program.account.userVault.fetch(userVaultPda);
    expect(userVaultState.depositedPrincipal.toNumber()).to.equal(DEPOSIT_AMOUNT);
    expect(userVaultState.totalShares.toNumber()).to.equal(DEPOSIT_AMOUNT);

    const reserveAccount = await getAccount(provider.connection, reserveTokenAccount);
    expect(Number(reserveAccount.amount)).to.equal(DEPOSIT_AMOUNT);
  });

  it("Executes keeper harvest & enforces protocol fee split", async () => {
    const yieldHarvested = 10_000_000; // 10 USDC gross yield
    // Mint yield directly into reserve to simulate external CPI accrual
    await mintTo(
      provider.connection,
      authority.payer,
      underlyingMint,
      reserveTokenAccount,
      authority.payer,
      yieldHarvested
    );

    await program.methods
      .harvestAndRebalance(new anchor.BN(yieldHarvested))
      .accounts({
        keeper: keeper.publicKey,
        globalConfig: globalConfigPda,
        underlyingMint: underlyingMint,
        reserveTokenAccount: reserveTokenAccount,
        treasuryTokenAccount: treasuryTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([keeper])
      .rpc();

    const treasuryAccount = await getAccount(provider.connection, treasuryTokenAccount);
    // 5% of 10 USDC = 0.5 USDC = 500_000
    expect(Number(treasuryAccount.amount)).to.equal(500_000);
  });

  it("Rejects zero deposit amounts", async () => {
    try {
      await program.methods
        .deposit(new anchor.BN(0))
        .accounts({
          user: user.publicKey,
          globalConfig: globalConfigPda,
          underlyingMint: underlyingMint,
          userVault: userVaultPda,
          userTokenAccount: userTokenAccount,
          reserveTokenAccount: reserveTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();
      expect.fail("Deposit should have failed");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("ZeroAmount");
    }
  });
});