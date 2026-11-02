import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GrowTelemetryVerifier } from "../target/types/grow_telemetry_verifier";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_INSTRUCTIONS_PUBKEY,
  Ed25519Program,
  Transaction,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";
import * as nacl from "tweetnacl";

describe("grow_telemetry_verifier test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowTelemetryVerifier as Program<GrowTelemetryVerifier>;
  
  const owner = Keypair.generate();
  const deviceKeypair = Keypair.generate();
  let growTokenMint: PublicKey;
  let userTokenAccount: PublicKey;
  let deviceRegistryPda: PublicKey;
  let epochAccumulatorPda: PublicKey;
  let rewardAuthPda: PublicKey;
  let rewardAuthBump: number;

  before(async () => {
    // Fund owner
    const sig = await provider.connection.requestAirdrop(
      owner.publicKey,
      10 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

    // Derive PDAs
    [deviceRegistryPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("device"), deviceKeypair.publicKey.toBuffer()],
      program.programId
    );

    [rewardAuthPda, rewardAuthBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("reward_auth")],
      program.programId
    );

    const epochBuffer = Buffer.alloc(8);
    epochBuffer.writeBigUInt64LE(BigInt(0));
    [epochAccumulatorPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("epoch_stats"), epochBuffer],
      program.programId
    );

    // Setup Token Mint with rewardAuthPda as mint authority
    growTokenMint = await createMint(
      provider.connection,
      owner,
      rewardAuthPda,
      null,
      6
    );

    userTokenAccount = await createAccount(
      provider.connection,
      owner,
      growTokenMint,
      owner.publicKey
    );
  });

  it("registers an IoT smart meter successfully", async () => {
    await program.methods
      .registerDevice(deviceKeypair.publicKey, 377749, -1224194, 5000) // 5 kW capacity
      .accounts({
        owner: owner.publicKey,
        deviceRegistry: deviceRegistryPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([owner])
      .rpc();

    const reg = await program.account.deviceRegistry.fetch(deviceRegistryPda);
    expect(reg.owner.toBase58()).to.equal(owner.publicKey.toBase58());
    expect(reg.maxCapacityWatts).to.equal(5000);
    expect(reg.totalKwhLifetime.toNumber()).to.equal(0);
  });

  it("records valid energy with Ed25519 instruction sysvar verification", async () => {
    const timestamp = Math.floor(Date.now() / 1000);
    const kwhMilli = 50; // 50 milli-kWh

    // Prepare Ed25519 instruction
    const message = Buffer.from(`GrowEnergyTelemetry:${timestamp}:${kwhMilli}`);
    const signature = nacl.sign.detached(message, deviceKeypair.secretKey);

    const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
      publicKey: deviceKeypair.publicKey.toBytes(),
      message: message,
      signature: signature,
    });

    const recordIx = await program.methods
      .recordEnergy(
        new anchor.BN(kwhMilli),
        new anchor.BN(timestamp),
        Array.from(Buffer.alloc(32))
      )
      .accounts({
        submitter: owner.publicKey,
        deviceRegistry: deviceRegistryPda,
        owner: owner.publicKey,
        epochAccumulator: epochAccumulatorPda,
        rewardAuthority: rewardAuthPda,
        growTokenMint: growTokenMint,
        userTokenAccount: userTokenAccount,
        instructionSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    const tx = new Transaction().add(ed25519Ix).add(recordIx);
    await provider.sendAndConfirm(tx, [owner]);

    // Validate Account State
    const reg = await program.account.deviceRegistry.fetch(deviceRegistryPda);
    expect(reg.totalKwhLifetime.toNumber()).to.equal(50);

    // Validate Minted Rewards
    const userAta = await getAccount(provider.connection, userTokenAccount);
    expect(Number(userAta.amount)).to.equal(50);
  });

  it("fails when energy output violates physical inverter limits", async () => {
    const timestamp = Math.floor(Date.now() / 1000) + 10;
    const impossibleKwh = 50_000_000; // Impossible output in 10 seconds

    const message = Buffer.from(`GrowEnergyTelemetry:${timestamp}:${impossibleKwh}`);
    const signature = nacl.sign.detached(message, deviceKeypair.secretKey);

    const ed25519Ix = Ed25519Program.createInstructionWithPublicKey({
      publicKey: deviceKeypair.publicKey.toBytes(),
      message: message,
      signature: signature,
    });

    const recordIx = await program.methods
      .recordEnergy(
        new anchor.BN(impossibleKwh),
        new anchor.BN(timestamp),
        Array.from(Buffer.alloc(32))
      )
      .accounts({
        submitter: owner.publicKey,
        deviceRegistry: deviceRegistryPda,
        owner: owner.publicKey,
        epochAccumulator: epochAccumulatorPda,
        rewardAuthority: rewardAuthPda,
        growTokenMint: growTokenMint,
        userTokenAccount: userTokenAccount,
        instructionSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .instruction();

    const tx = new Transaction().add(ed25519Ix).add(recordIx);

    try {
      await provider.sendAndConfirm(tx, [owner]);
      expect.fail("Should have failed physical plausibility checks");
    } catch (err: any) {
      expect(err.toString()).to.include("ExcessiveGenerationReported");
    }
  });
});