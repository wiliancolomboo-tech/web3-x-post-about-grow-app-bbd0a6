import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GrowAttestor } from "../target/types/grow_attestor";
import {
  createMint,
  createAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";

describe("grow-attestor test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowAttestor as Program<GrowAttestor>;

  let vaultMint: anchor.web3.PublicKey;
  let vaultTokenAccount: anchor.web3.PublicKey;
  const admin = (provider.wallet as any).payer as anchor.web3.Keypair;
  const keeper = anchor.web3.Keypair.generate();

  let configPda: anchor.web3.PublicKey;
  let configBump: number;

  before(async () => {
    // 1. Fund the keeper
    const sig = await provider.connection.requestAirdrop(
      keeper.publicKey,
      5 * anchor.web3.LAMPORTS_PER_SOL
    );
    const latestBlockhash = await provider.connection.getLatestBlockhash();
    await provider.connection.confirmTransaction({
      signature: sig,
      ...latestBlockhash,
    });

    // 2. Create mock vault mint (e.g. USDC, 6 decimals)
    vaultMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // 3. Create mock Vault Token Account
    vaultTokenAccount = await createAccount(
      provider.connection,
      admin,
      vaultMint,
      admin.publicKey
    );

    // 4. Derive PDA
    [configPda, configBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("attestation_config"), vaultMint.toBuffer()],
      program.programId
    );
  });

  it("Initializes the Attestation Config successfully", async () => {
    const minSolvencyBps = 9800; // 98.00% minimum solvency required

    await program.methods
      .initializeAttestationConfig(configBump, minSolvencyBps)
      .accounts({
        config: configPda,
        vaultMint: vaultMint,
        vaultTokenAccount: vaultTokenAccount,
        keeperAuthority: keeper.publicKey,
        admin: admin.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const configAccount = await program.account.attestationConfig.fetch(configPda);
    expect(configAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(configAccount.keeperAuthority.toBase58()).to.equal(keeper.publicKey.toBase58());
    expect(configAccount.totalAttestations.toNumber()).to.equal(0);
    expect(configAccount.minSolvencyBps).to.equal(9800);
  });

  it("Records a valid vault attestation within acceptable solvency parameters", async () => {
    // Deposit 50,000 USDC into the liquid reserve
    await mintTo(
      provider.connection,
      admin,
      vaultMint,
      vaultTokenAccount,
      admin,
      50_000_000_000 // 50,000 units
    );

    const reportedTvl = new anchor.BN(100_000_000_000); // 100k TVL
    const strategyTvl = new anchor.BN(55_000_000_000); // 55k in strategies
    const reportedShares = new anchor.BN(90_000_000_000); // 90k shares
    const apyBps = 845; // 8.45% APY

    const epochId = new anchor.BN(0);
    const [recordPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("attestation_record"),
        configPda.toBuffer(),
        epochId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    await program.methods
      .recordAttestation(reportedTvl, reportedShares, apyBps, strategyTvl)
      .accounts({
        config: configPda,
        vaultTokenAccount: vaultTokenAccount,
        attestationRecord: recordPda,
        keeper: keeper.publicKey,
        keeperAuthority: keeper.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([keeper])
      .rpc();

    const record = await program.account.attestationRecord.fetch(recordPda);
    expect(record.epochId.toNumber()).to.equal(0);
    expect(record.reportedTvl.toString()).to.equal(reportedTvl.toString());
    expect(record.liquidReserve.toString()).to.equal("50000000000");
    expect(record.strategyTvl.toString()).to.equal(strategyTvl.toString());
    expect(record.calculatedApyBps).to.equal(845);
    // Solvency = (50k + 55k) / 100k = 105% = 10500 BPS
    expect(record.solvencyBps).to.equal(10500);

    const configAccount = await program.account.attestationConfig.fetch(configPda);
    expect(configAccount.totalAttestations.toNumber()).to.equal(1);
  });

  it("Rejects attestation if vault fails solvency check (undercollateralized)", async () => {
    const reportedTvl = new anchor.BN(200_000_000_000); // 200k TVL reported
    const strategyTvl = new anchor.BN(20_000_000_000); // 20k strategy TVL (50k liquid + 20k = 70k << 200k)
    const reportedShares = new anchor.BN(180_000_000_000);
    const apyBps = 950;

    const epochId = new anchor.BN(1);
    const [recordPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("attestation_record"),
        configPda.toBuffer(),
        epochId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    try {
      await program.methods
        .recordAttestation(reportedTvl, reportedShares, apyBps, strategyTvl)
        .accounts({
          config: configPda,
          vaultTokenAccount: vaultTokenAccount,
          attestationRecord: recordPda,
          keeper: keeper.publicKey,
          keeperAuthority: keeper.publicKey,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([keeper])
        .rpc();
      expect.fail("Transaction should have reverted due to InsolventVault");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("InsolventVault");
    }
  });

  it("Rejects attestation from unauthorized keeper signer", async () => {
    const unauthorizedKeeper = anchor.web3.Keypair.generate();
    const sig = await provider.connection.requestAirdrop(
      unauthorizedKeeper.publicKey,
      anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

    const epochId = new anchor.BN(1);
    const [recordPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("attestation_record"),
        configPda.toBuffer(),
        epochId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    try {
      await program.methods
        .recordAttestation(
          new anchor.BN(50_000_000_000),
          new anchor.BN(40_000_000_000),
          720,
          new anchor.BN(10_000_000_000)
        )
        .accounts({
          config: configPda,
          vaultTokenAccount: vaultTokenAccount,
          attestationRecord: recordPda,
          keeper: unauthorizedKeeper.publicKey,
          keeperAuthority: unauthorizedKeeper.publicKey,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([unauthorizedKeeper])
        .rpc();
      expect.fail("Transaction should have failed authorization check");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("UnauthorizedKeeper");
    }
  });
});