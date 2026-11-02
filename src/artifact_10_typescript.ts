import * as anchor from "@coral-xyz/anchor";
import {
  Keypair,
  PublicKey,
  Transaction,
  Ed25519Program,
  sendAndConfirmTransaction,
  SYSVAR_INSTRUCTIONS_PUBKEY,
} from "@solana/web3.js";
import { TOKEN_PROGRAM_ID } from "@solana/spl-token";
import * as nacl from "tweetnacl";

export async function submitTelemetryAndFormatPost(
  program: anchor.Program<any>,
  hardwareKeypair: Keypair,
  reporterKeypair: Keypair,
  deltaWh: number,
  ratedCapacityWatts: number,
  tokenMint: PublicKey,
  ownerTokenAccount: PublicKey
) {
  const timestamp = Math.floor(Date.now() / 1000);

  // 1. Build Payload
  const messageBuffer = Buffer.alloc(16);
  messageBuffer.writeBigUInt64LE(BigInt(deltaWh), 0);
  messageBuffer.writeBigInt64LE(BigInt(timestamp), 8);

  const signature = nacl.sign.detached(messageBuffer, hardwareKeypair.secretKey);

  // 2. Precompile instruction
  const ed25519Instruction = Ed25519Program.createInstructionWithPublicKey({
    publicKey: hardwareKeypair.publicKey.toBytes(),
    message: messageBuffer,
    signature,
  });

  const [protocolStatePda] = PublicKey.findProgramAddressSync(
    [Buffer.from("protocol_state")],
    program.programId
  );
  const [deviceAccountPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("device"), hardwareKeypair.publicKey.toBuffer()],
    program.programId
  );
  const [vaultAuthorityPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault_authority")],
    program.programId
  );

  // 3. Telemetry Instruction
  const telemetryIx = await program.methods
    .recordTelemetryAndMint(new anchor.BN(deltaWh), new anchor.BN(timestamp))
    .accounts({
      reporter: reporterKeypair.publicKey,
      protocolState: protocolStatePda,
      deviceAccount: deviceAccountPda,
      tokenMint,
      vaultAuthority: vaultAuthorityPda,
      deviceOwnerTokenAccount: ownerTokenAccount,
      ixSysvar: SYSVAR_INSTRUCTIONS_PUBKEY,
      tokenProgram: TOKEN_PROGRAM_ID,
    })
    .instruction();

  const tx = new Transaction().add(ed25519Instruction).add(telemetryIx);
  const txSignature = await sendAndConfirmTransaction(
    program.provider.connection,
    tx,
    [reporterKeypair]
  );

  // 4. Construct Social Proof String for X Integration
  const postContent = 
`⚡ [Grow App Telemetry Verified] ☀️
Device: ${hardwareKeypair.publicKey.toBase58().slice(0, 8)}...
Output: +${deltaWh} Wh clean energy generated
Proof: https://solscan.io/tx/${txSignature}
Settled on @solana via @GrowProtocol 🌿 #DePIN #SolanaReFi`;

  console.log("X Post Generated:\n", postContent);
  return { txSignature, postContent };
}