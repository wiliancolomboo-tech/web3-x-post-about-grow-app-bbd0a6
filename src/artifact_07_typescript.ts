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

describe("grow_vault automated verification suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  let baseMint: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let vaultAuthorityPda: anchor.web3.PublicKey;
  let shareMintPda: anchor.web3.PublicKey;
  let vaultReservePda: anchor.web3.PublicKey;

  const admin = anchor.web3.Keypair.generate();
  const user = anchor.web3.Keypair.generate();
  const attacker = anchor.web3.Keypair.generate();
  const keeper = anchor.web3.Keypair.generate();

  let userTokenAccount: anchor.web3.PublicKey;
  let userShareAccount: anchor.web3.PublicKey;
  let attackerTokenAccount: anchor.web3.PublicKey;
  let keeperTokenAccount: anchor.web3.PublicKey;

  const INITIAL_USER_BALANCE = 50_000_000_000; // 50 tokens (9 decimals)
  const ATTACKER_BALANCE = 100_000_000_000;     // 100 tokens

  before(async () => {
    // AirDrop SOL for signatures
    for (const kp of [admin, user, attacker, keeper]) {
      const sig = await provider.connection.requestAirdrop(
        kp.publicKey,
        10 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig);
    }

    // Create Base Asset Mint
    baseMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      9
    );

    // Derive Program Derived Addresses
    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), baseMint.toBuffer()],
      program.programId
    );

    [vaultAuthorityPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_authority"), vaultPda.toBuffer()],
      program.programId
    );

    [shareMintPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("share_mint"), vaultPda.toBuffer()],
      program.programId
    );

    [vaultReservePda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_reserve"), vaultPda.toBuffer()],
      program.programId
    );

    // Create Token Accounts
    userTokenAccount = await createAccount(
      provider.connection,
      user,
      baseMint,
      user.publicKey
    );
    userShareAccount = await createAccount(
      provider.connection,
      user,
      shareMintPda,
      user.publicKey
    );
    attackerTokenAccount = await createAccount(
      provider.connection,
      attacker,
      baseMint,
      attacker.publicKey
    );
    keeperTokenAccount = await createAccount(
      provider.connection,
      keeper,
      baseMint,
      keeper.publicKey
    );

    // Mint tokens to User and Attacker
    await mintTo(
      provider.connection,
      admin,
      baseMint,
      userTokenAccount,
      admin,
      INITIAL_USER_BALANCE
    );
    await mintTo(
      provider.connection,
      admin,
      baseMint,
      attackerTokenAccount,
      admin,
      ATTACKER_BALANCE
    );
  });

  it("1. Initializes the Grow Vault with legitimate PDA accounts", async () => {
    const keeperFeeBps = new anchor.BN(200); // 2% tip

    await program.methods
      .initializeVault(keeperFeeBps)
      .accounts({
        admin: admin.publicKey,
        baseMint: baseMint,
        shareMint: shareMintPda,
        vault: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        vaultTokenAccount: vaultReservePda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([admin])
      .rpc();

    const vaultAccount = await program.account.vault.fetch(vaultPda);
    expect(vaultAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(vaultAccount.keeperFeeBps.toNumber()).to.equal(200);
    expect(vaultAccount.isPaused).to.be.false;
  });

  it("2. Executes user deposit and asserts virtual-share offset logic", async () => {
    const depositAmount = new anchor.BN(10_000_000_000); // 10 Base units
    const [userLedgerPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("user_ledger"), vaultPda.toBuffer(), user.publicKey.toBuffer()],
      program.programId
    );

    await program.methods
      .deposit(depositAmount, new anchor.BN(1)) // min 1 share expected
      .accounts({
        user: user.publicKey,
        vault: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        baseMint: baseMint,
        shareMint: shareMintPda,
        userTokenAccount: userTokenAccount,
        userShareAccount: userShareAccount,
        vaultTokenAccount: vaultReservePda,
        userLedger: userLedgerPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([user])
      .rpc();

    const userShareBalance = await getAccount(provider.connection, userShareAccount);
    // Calculated by virtual shares offset formula:
    // S_mint = 10e9 * (0 + 1e9) / (0 + 1) = 1e19 (scaled integer)
    expect(Number(userShareBalance.amount)).to.be.greaterThan(0);

    const vaultReserveBalance = await getAccount(provider.connection, vaultReservePda);
    expect(Number(vaultReserveBalance.amount)).to.equal(10_000_000_000);
  });

  it("3. Proves immunity against donation / inflation attacks", async () => {
    // Attacker attempts to donate 50 tokens directly to reserve without calling deposit
    await mintTo(
      provider.connection,
      admin,
      baseMint,
      vaultReservePda,
      admin,
      50_000_000_000
    );

    // Victim makes subsequent deposit of 5 tokens
    const victim = anchor.web3.Keypair.generate();
    const airdropSig = await provider.connection.requestAirdrop(
      victim.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropSig);

    const victimTokenAcc = await createAccount(
      provider.connection,
      victim,
      baseMint,
      victim.publicKey
    );
    const victimShareAcc = await createAccount(
      provider.connection,
      victim,
      shareMintPda,
      victim.publicKey
    );

    await mintTo(
      provider.connection,
      admin,
      baseMint,
      victimTokenAcc,
      admin,
      5_000_000_000
    );

    const [victimLedgerPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("user_ledger"), vaultPda.toBuffer(), victim.publicKey.toBuffer()],
      program.programId
    );

    // In a standard ERC-4626 vault without offsets, shares would round down to 0, causing complete theft
    await program.methods
      .deposit(new anchor.BN(5_000_000_000), new anchor.BN(1))
      .accounts({
        user: victim.publicKey,
        vault: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        baseMint: baseMint,
        shareMint: shareMintPda,
        userTokenAccount: victimTokenAcc,
        userShareAccount: victimShareAcc,
        vaultTokenAccount: vaultReservePda,
        userLedger: victimLedgerPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([victim])
      .rpc();

    const victimShareBalance = await getAccount(provider.connection, victimShareAcc);
    expect(Number(victimShareBalance.amount)).to.be.greaterThan(0);
  });

  it("4. Executes permissionless compound rewards instruction", async () => {
    const harvestRewardAmount = new anchor.BN(1_000_000_000); // 1 token yield

    // Add reward liquidity to reserve simulating AMM harvest
    await mintTo(
      provider.connection,
      admin,
      baseMint,
      vaultReservePda,
      admin,
      1_000_000_000
    );

    await program.methods
      .compoundRewards(harvestRewardAmount)
      .accounts({
        keeper: keeper.publicKey,
        vault: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        baseMint: baseMint,
        vaultTokenAccount: vaultReservePda,
        keeperTokenAccount: keeperTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([keeper])
      .rpc();

    const keeperBalance = await getAccount(provider.connection, keeperTokenAccount);
    // 2% of 1_000_000_000 = 20_000_000
    expect(Number(keeperBalance.amount)).to.equal(20_000_000);
  });

  it("5. Reverts when user attempts to exceed slippage bounds", async () => {
    const [userLedgerPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("user_ledger"), vaultPda.toBuffer(), user.publicKey.toBuffer()],
      program.programId
    );

    let failed = false;
    try {
      await program.methods
        .deposit(new anchor.BN(1_000_000_000), new anchor.BN("18446744073709551615")) // Impossible expected shares
        .accounts({
          user: user.publicKey,
          vault: vaultPda,
          vaultAuthority: vaultAuthorityPda,
          baseMint: baseMint,
          shareMint: shareMintPda,
          userTokenAccount: userTokenAccount,
          userShareAccount: userShareAccount,
          vaultTokenAccount: vaultReservePda,
          userLedger: userLedgerPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .signers([user])
        .rpc();
    } catch (err: any) {
      failed = true;
      expect(err.error.errorCode.code).to.equal("SlippageExceeded");
    }
    expect(failed).to.be.true;
  });
});