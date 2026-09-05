import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { GrowVault } from "../target/types/grow_vault";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

describe("grow_vault_integration_matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  let baseMint: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let vaultBump: number;
  let authorityPda: anchor.web3.PublicKey;
  let authorityBump: number;
  let shareMintPda: anchor.web3.PublicKey;
  let vaultTokenPda: anchor.web3.PublicKey;

  const admin = (provider.wallet as anchor.Wallet).payer;
  const testUser = anchor.web3.Keypair.generate();

  let userBaseAccount: anchor.web3.PublicKey;
  let userShareAccount: anchor.web3.PublicKey;

  const INITIAL_USER_BALANCE = 10_000_000_000; // 10,000 USDC (6 decimals)
  const DEPOSIT_AMOUNT = 1_000_000_000; // 1,000 USDC

  before(async () => {
    // 1. Airdrop SOL to test user
    const sig = await provider.connection.requestAirdrop(
      testUser.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);

    // 2. Create Base Token Mint (USDC simulation)
    baseMint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // 3. Derive PDAs
    [vaultPda, vaultBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), baseMint.toBuffer()],
      program.programId
    );

    [authorityPda, authorityBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("authority"), vaultPda.toBuffer()],
      program.programId
    );

    [shareMintPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("share_mint"), vaultPda.toBuffer()],
      program.programId
    );

    [vaultTokenPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_tokens"), vaultPda.toBuffer()],
      program.programId
    );

    // 4. Create user token accounts
    userBaseAccount = await createAccount(
      provider.connection,
      testUser,
      baseMint,
      testUser.publicKey
    );

    userShareAccount = await createAccount(
      provider.connection,
      testUser,
      shareMintPda,
      testUser.publicKey
    );

    // 5. Fund test user
    await mintTo(
      provider.connection,
      admin,
      baseMint,
      userBaseAccount,
      admin.publicKey,
      INITIAL_USER_BALANCE
    );
  });

  it("Initializes the Grow Vault with correct PDA authority and parameters", async () => {
    await program.methods
      .initializeVault(vaultBump, authorityBump)
      .accounts({
        vault: vaultPda,
        vaultAuthority: authorityPda,
        baseMint: baseMint,
        shareMint: shareMintPda,
        vaultTokenAccount: vaultTokenPda,
        admin: admin.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([admin])
      .rpc();

    const vaultAccount = await program.account.vaultState.fetch(vaultPda);
    expect(vaultAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(vaultAccount.baseMint.toBase58()).to.equal(baseMint.toBase58());
    expect(vaultAccount.totalStrategyAllocation.toNumber()).to.equal(0);
  });

  it("Executes user deposit: transfers base assets and issues proportional shares", async () => {
    await program.methods
      .deposit(new anchor.BN(DEPOSIT_AMOUNT))
      .accounts({
        vault: vaultPda,
        vaultAuthority: authorityPda,
        shareMint: shareMintPda,
        vaultTokenAccount: vaultTokenPda,
        userBaseAccount: userBaseAccount,
        userShareAccount: userShareAccount,
        user: testUser.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([testUser])
      .rpc();

    const userShares = await getAccount(provider.connection, userShareAccount);
    const vaultTokens = await getAccount(provider.connection, vaultTokenPda);

    expect(Number(userShares.amount)).to.equal(DEPOSIT_AMOUNT);
    expect(Number(vaultTokens.amount)).to.equal(DEPOSIT_AMOUNT);
  });

  it("Fails when withdrawal violates slippage tolerance", async () => {
    const excessiveMinOut = new anchor.BN(DEPOSIT_AMOUNT + 1_000_000); // Demanding more than available

    try {
      await program.methods
        .withdraw(new anchor.BN(DEPOSIT_AMOUNT), excessiveMinOut)
        .accounts({
          vault: vaultPda,
          vaultAuthority: authorityPda,
          shareMint: shareMintPda,
          vaultTokenAccount: vaultTokenPda,
          userShareAccount: userShareAccount,
          userBaseAccount: userBaseAccount,
          user: testUser.publicKey,
          tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        })
        .signers([testUser])
        .rpc();
      expect.fail("Expected transaction to fail with SlippageExceeded");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("SlippageExceeded");
    }
  });

  it("Executes valid full withdrawal and verifies round-trip token balance integrity", async () => {
    const userSharesBefore = await getAccount(provider.connection, userShareAccount);

    await program.methods
      .withdraw(new anchor.BN(userSharesBefore.amount.toString()), new anchor.BN(DEPOSIT_AMOUNT))
      .accounts({
        vault: vaultPda,
        vaultAuthority: authorityPda,
        shareMint: shareMintPda,
        vaultTokenAccount: vaultTokenPda,
        userShareAccount: userShareAccount,
        userBaseAccount: userBaseAccount,
        user: testUser.publicKey,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([testUser])
      .rpc();

    const userBaseAfter = await getAccount(provider.connection, userBaseAccount);
    const userSharesAfter = await getAccount(provider.connection, userShareAccount);

    expect(Number(userSharesAfter.amount)).to.equal(0);
    expect(Number(userBaseAfter.amount)).to.equal(INITIAL_USER_BALANCE);
  });
});