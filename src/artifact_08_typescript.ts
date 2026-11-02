import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { expect } from "chai";
import { createMint, createAccount, mintTo } from "@solana/spl-token";
import { Keypair, PublicKey } from "@solana/web3.js";
import { GrowVault } from "../target/types/grow_vault";

describe("Grow Vault Unit & Integration Tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;
  let mint: PublicKey;
  let admin = Keypair.generate();
  let user = Keypair.generate();
  let userTokenAccount: PublicKey;
  let vaultTokenAccount: PublicKey;
  let vaultPda: PublicKey;
  let vaultAuthPda: PublicKey;
  let userSharesPda: PublicKey;

  before(async () => {
    // Airdrop SOL to test actors
    const sig1 = await provider.connection.requestAirdrop(admin.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    const sig2 = await provider.connection.requestAirdrop(user.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(sig1);
    await provider.connection.confirmTransaction(sig2);

    // Create mock USDC mint
    mint = await createMint(provider.connection, admin, admin.publicKey, null, 6);

    // Derive PDAs
    [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_state"), mint.toBuffer()],
      program.programId
    );
    [vaultAuthPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_auth"), vaultPda.toBuffer()],
      program.programId
    );
    [userSharesPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("user_shares"), vaultPda.toBuffer(), user.publicKey.toBuffer()],
      program.programId
    );

    // Setup Token Accounts
    userTokenAccount = await createAccount(provider.connection, user, mint, user.publicKey);
    vaultTokenAccount = await createAccount(provider.connection, admin, mint, vaultAuthPda);

    // Mint 1,000 tokens to user
    await mintTo(provider.connection, admin, mint, userTokenAccount, admin, 1_000_000_000);
  });

  it("Initializes the Grow Vault with 10% performance fee", async () => {
    await program.methods
      .initializeVault(100, 1000) // 1% harvest, 10% performance
      .accounts({
        vault: vaultPda,
        vaultAuthority: vaultAuthPda,
        underlyingMint: mint,
        admin: admin.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([admin])
      .rpc();

    const vaultData = await program.account.vaultState.fetch(vaultPda);
    expect(vaultData.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(vaultData.totalAssets.toNumber()).to.equal(0);
    expect(vaultData.totalShares.toNumber()).to.equal(0);
  });

  it("Executes deposit and mints exact 1:1 shares on initial entry", async () => {
    const depositAmount = new anchor.BN(500_000_000); // 500 units

    await program.methods
      .deposit(depositAmount)
      .accounts({
        vault: vaultPda,
        userShares: userSharesPda,
        userTokenAccount,
        vaultTokenAccount,
        vaultAuthority: vaultAuthPda,
        user: user.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const vaultData = await program.account.vaultState.fetch(vaultPda);
    const userSharesData = await program.account.userShares.fetch(userSharesPda);

    expect(vaultData.totalAssets.toNumber()).to.equal(500_000_000);
    expect(vaultData.totalShares.toNumber()).to.equal(500_000_000);
    expect(userSharesData.shares.toNumber()).to.equal(500_000_000);
  });

  it("Compounds external yield and increases share value", async () => {
    const yieldHarvested = new anchor.BN(100_000_000); // 100 units gross yield

    await program.methods
      .harvestAndCompound(yieldHarvested, 50)
      .accounts({
        vault: vaultPda,
        keeper: admin.publicKey,
      })
      .signers([admin])
      .rpc();

    const vaultData = await program.account.vaultState.fetch(vaultPda);
    // 100 gross yield - 10% fee = 90 compounded units
    expect(vaultData.totalAssets.toNumber()).to.equal(590_000_000);
    expect(vaultData.totalShares.toNumber()).to.equal(500_000_000);
  });

  it("Withdraws shares and returns amplified underlying principal", async () => {
    const sharesToBurn = new anchor.BN(500_000_000);

    await program.methods
      .withdraw(sharesToBurn)
      .accounts({
        vault: vaultPda,
        userShares: userSharesPda,
        userTokenAccount,
        vaultTokenAccount,
        vaultAuthority: vaultAuthPda,
        user: user.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const vaultData = await program.account.vaultState.fetch(vaultPda);
    const userSharesData = await program.account.userShares.fetch(userSharesPda);

    expect(vaultData.totalAssets.toNumber()).to.equal(0);
    expect(vaultData.totalShares.toNumber()).to.equal(0);
    expect(userSharesData.shares.toNumber()).to.equal(0);
  });
});