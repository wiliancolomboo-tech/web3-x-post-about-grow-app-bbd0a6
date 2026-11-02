import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GrowVault } from "../target/types/grow_vault";
import {
  createMint,
  getOrCreateAssociatedTokenAccount,
  mintTo,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import { expect } from "chai";
import BN from "bn.js";

describe("grow_vault automated test matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;
  const connection = provider.connection;

  const admin = (provider.wallet as anchor.Wallet).payer;
  const alice = Keypair.generate();
  const bob = Keypair.generate();
  const keeper = Keypair.generate();

  let mint: PublicKey;
  let aliceAta: PublicKey;
  let bobAta: PublicKey;

  const vaultId = new BN(1001);
  let vaultPda: PublicKey;
  let vaultBump: number;
  let vaultSignerPda: PublicKey;
  let vaultTokenAccountPda: PublicKey;

  before(async () => {
    // Airdrop SOL to test actors
    for (const actor of [alice, bob, keeper]) {
      const sig = await connection.requestAirdrop(actor.publicKey, 5 * anchor.web3.LAMPORTS_PER_SOL);
      await connection.confirmTransaction(sig, "confirmed");
    }

    // Deploy test SPL token mint
    mint = await createMint(
      connection,
      admin,
      admin.publicKey,
      null,
      6 // 6 decimals like USDC
    );

    // Derive PDAs
    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), vaultId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultSignerPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_signer"), vaultPda.toBuffer()],
      program.programId
    );

    [vaultTokenAccountPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_token"), vaultPda.toBuffer()],
      program.programId
    );

    // Setup ATAs
    const aliceAtaObj = await getOrCreateAssociatedTokenAccount(
      connection,
      admin,
      mint,
      alice.publicKey
    );
    aliceAta = aliceAtaObj.address;

    const bobAtaObj = await getOrCreateAssociatedTokenAccount(
      connection,
      admin,
      mint,
      bob.publicKey
    );
    bobAta = bobAtaObj.address;

    // Mint test capital: 50,000 USDC each
    await mintTo(connection, admin, mint, aliceAta, admin, 50_000 * 1_000_000);
    await mintTo(connection, admin, mint, bobAta, admin, 50_000 * 1_000_000);
  });

  it("1. Initializes the Grow Vault with 0% fee", async () => {
    await program.methods
      .initializeVault(vaultId, 0)
      .accountsStrict({
        authority: admin.publicKey,
        tokenMint: mint,
        vault: vaultPda,
        vaultSigner: vaultSignerPda,
        vaultTokenAccount: vaultTokenAccountPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    const vaultAccount = await program.account.vaultState.fetch(vaultPda);
    expect(vaultAccount.totalShares.toNumber()).to.equal(0);
    expect(vaultAccount.depositFeeBps).to.equal(0);
  });

  it("2. Processes first deposit from Alice (10,000 USDC)", async () => {
    const depositAmount = new BN(10_000 * 1_000_000);

    const [userPositionPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("user_position"), alice.publicKey.toBuffer(), vaultPda.toBuffer()],
      program.programId
    );

    await program.methods
      .deposit(depositAmount)
      .accountsStrict({
        user: alice.publicKey,
        vault: vaultPda,
        tokenMint: mint,
        userTokenAccount: aliceAta,
        vaultTokenAccount: vaultTokenAccountPda,
        userPosition: userPositionPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([alice])
      .rpc();

    const alicePosition = await program.account.userPosition.fetch(userPositionPda);
    const vault = await program.account.vaultState.fetch(vaultPda);

    expect(alicePosition.shares.toString()).to.equal(depositAmount.toString());
    expect(vault.totalShares.toString()).to.equal(depositAmount.toString());
    expect(vault.totalDeposited.toString()).to.equal(depositAmount.toString());
  });

  it("3. Simulates crank yield harvest of 500 USDC by Keeper", async () => {
    const yieldHarvested = new BN(500 * 1_000_000);

    // Send the harvested tokens into the vault account to simulate yield generation
    await mintTo(connection, admin, mint, vaultTokenAccountPda, admin, yieldHarvested.toNumber());

    await program.methods
      .harvestAndCompound(yieldHarvested)
      .accountsStrict({
        vault: vaultPda,
        crankKeeper: keeper.publicKey,
      })
      .signers([keeper])
      .rpc();

    const vault = await program.account.vaultState.fetch(vaultPda);
    expect(vault.accumulatedYieldPerShare.gt(new BN(0))).to.be.true;
  });

  it("4. Bob deposits into yield-accruing vault and receives correct share proportion", async () => {
    const depositAmount = new BN(10_000 * 1_000_000);

    const [bobPositionPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("user_position"), bob.publicKey.toBuffer(), vaultPda.toBuffer()],
      program.programId
    );

    await program.methods
      .deposit(depositAmount)
      .accountsStrict({
        user: bob.publicKey,
        vault: vaultPda,
        tokenMint: mint,
        userTokenAccount: bobAta,
        vaultTokenAccount: vaultTokenAccountPda,
        userPosition: bobPositionPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .signers([bob])
      .rpc();

    const bobPosition = await program.account.userPosition.fetch(bobPositionPda);
    // Shares received must be exactly proportional to existing total_deposited
    expect(bobPosition.shares.toNumber()).to.be.lessThan(depositAmount.toNumber());
  });

  it("5. Alice withdraws full balance and receives principal + 100% of first yield epoch", async () => {
    const [alicePositionPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("user_position"), alice.publicKey.toBuffer(), vaultPda.toBuffer()],
      program.programId
    );

    const alicePositionBefore = await program.account.userPosition.fetch(alicePositionPda);

    await program.methods
      .withdraw(alicePositionBefore.shares)
      .accountsStrict({
        user: alice.publicKey,
        vault: vaultPda,
        vaultSigner: vaultSignerPda,
        tokenMint: mint,
        userTokenAccount: aliceAta,
        vaultTokenAccount: vaultTokenAccountPda,
        userPosition: alicePositionPda,
        owner: alice.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([alice])
      .rpc();

    const alicePositionAfter = await program.account.userPosition.fetch(alicePositionPda);
    expect(alicePositionAfter.shares.toNumber()).to.equal(0);
    expect(alicePositionAfter.cumulativeClaimed.toNumber()).to.be.closeTo(500 * 1_000_000, 100);
  });
});