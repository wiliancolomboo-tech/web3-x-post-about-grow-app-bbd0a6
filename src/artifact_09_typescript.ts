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

describe("Grow App Automated QA Test Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.GrowVault as Program<GrowVault>;

  const admin = anchor.web3.Keypair.generate();
  const feeCollector = anchor.web3.Keypair.generate();
  const user = anchor.web3.Keypair.generate();
  const keeper = anchor.web3.Keypair.generate();

  let mint: anchor.web3.PublicKey;
  let userTokenAccount: anchor.web3.PublicKey;
  let keeperTokenAccount: anchor.web3.PublicKey;
  let protocolFeeTokenAccount: anchor.web3.PublicKey;

  let configPda: anchor.web3.PublicKey;
  let vaultPda: anchor.web3.PublicKey;
  let vaultTokenAccountPda: anchor.web3.PublicKey;

  const DECIMALS = 6;
  const INITIAL_MINT_AMOUNT = 1_000_000_000; // 1,000 USDC
  const DEPOSIT_AMOUNT = new anchor.BN(500_000_000); // 500 USDC

  before(async () => {
    // Fund test wallets with native SOL
    const airdropSigs = await Promise.all([
      provider.connection.requestAirdrop(admin.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL),
      provider.connection.requestAirdrop(user.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL),
      provider.connection.requestAirdrop(keeper.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL),
      provider.connection.requestAirdrop(feeCollector.publicKey, 1 * anchor.web3.LAMPORTS_PER_SOL),
    ]);
    for (const sig of airdropSigs) {
      const latest = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({ signature: sig, ...latest });
    }

    // Deploy mock SPL Mint
    mint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      DECIMALS,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    // Create ATAs
    userTokenAccount = await createAccount(
      provider.connection,
      user,
      mint,
      user.publicKey,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    keeperTokenAccount = await createAccount(
      provider.connection,
      keeper,
      mint,
      keeper.publicKey,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    protocolFeeTokenAccount = await createAccount(
      provider.connection,
      admin,
      mint,
      feeCollector.publicKey,
      undefined,
      undefined,
      TOKEN_PROGRAM_ID
    );

    // Mint tokens to user
    await mintTo(
      provider.connection,
      admin,
      mint,
      userTokenAccount,
      admin,
      INITIAL_MINT_AMOUNT
    );

    // Derive PDAs
    [configPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("config")],
      program.programId
    );

    [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grow_vault"), user.publicKey.toBuffer(), mint.toBuffer()],
      program.programId
    );

    [vaultTokenAccountPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault_token_account"), vaultPda.toBuffer()],
      program.programId
    );
  });

  it("1. Initializes protocol global config with safe fee thresholds", async () => {
    const protocolFeeBps = new anchor.BN(100); // 1%
    const keeperFeeBps = new anchor.BN(50);   // 0.5%

    await program.methods
      .initializeProtocol(protocolFeeBps, keeperFeeBps)
      .accounts({
        config: configPda,
        admin: admin.publicKey,
        feeCollector: feeCollector.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([admin])
      .rpc();

    const configAccount = await program.account.protocolConfig.fetch(configPda);
    expect(configAccount.admin.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(configAccount.feeCollector.toBase58()).to.equal(feeCollector.publicKey.toBase58());
    expect(configAccount.protocolFeeBps.toNumber()).to.equal(100);
    expect(configAccount.keeperIncentiveBps.toNumber()).to.equal(50);
    expect(configAccount.isPaused).to.be.false;
  });

  it("2. Initializes an isolated Grow Vault for user", async () => {
    await program.methods
      .initializeVault()
      .accounts({
        vault: vaultPda,
        owner: user.publicKey,
        assetMint: mint,
        vaultTokenAccount: vaultTokenAccountPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const vaultState = await program.account.growVault.fetch(vaultPda);
    expect(vaultState.owner.toBase58()).to.equal(user.publicKey.toBase58());
    expect(vaultState.totalShares.toNumber()).to.equal(0);
    expect(vaultState.totalDeposited.toNumber()).to.equal(0);
  });

  it("3. Executes non-custodial user deposit with share issuance", async () => {
    await program.methods
      .deposit(DEPOSIT_AMOUNT)
      .accounts({
        config: configPda,
        vault: vaultPda,
        owner: user.publicKey,
        assetMint: mint,
        userTokenAccount: userTokenAccount,
        vaultTokenAccount: vaultTokenAccountPda,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const vaultState = await program.account.growVault.fetch(vaultPda);
    const vaultBal = await getAccount(provider.connection, vaultTokenAccountPda);

    expect(vaultState.totalDeposited.toString()).to.equal(DEPOSIT_AMOUNT.toString());
    expect(vaultState.totalShares.toString()).to.equal(DEPOSIT_AMOUNT.toString());
    expect(Number(vaultBal.amount)).to.equal(DEPOSIT_AMOUNT.toNumber());
  });

  it("4. Keeper crank compounds yield and distributes performance bounties", async () => {
    const mockYield = new anchor.BN(50_000_000); // 50 USDC yield

    // Simulate yield arrival by minting tokens directly into the vault ATA
    await mintTo(
      provider.connection,
      admin,
      mint,
      vaultTokenAccountPda,
      admin,
      mockYield.toNumber()
    );

    await program.methods
      .compoundYield(mockYield)
      .accounts({
        config: configPda,
        vault: vaultPda,
        assetMint: mint,
        vaultTokenAccount: vaultTokenAccountPda,
        keeper: keeper.publicKey,
        keeperTokenAccount: keeperTokenAccount,
        protocolFeeTokenAccount: protocolFeeTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([keeper])
      .rpc();

    const keeperBal = await getAccount(provider.connection, keeperTokenAccount);
    const protocolBal = await getAccount(provider.connection, protocolFeeTokenAccount);

    // Keeper fee: 50 * 50 / 10000 = 0.25 USDC (250,000 units)
    expect(Number(keeperBal.amount)).to.equal(250_000);
    // Protocol fee: 50 * 100 / 10000 = 0.50 USDC (500,000 units)
    expect(Number(protocolBal.amount)).to.equal(500_000);
  });

  it("5. Allows partial/full withdraw with accrued yield appreciation", async () => {
    const vaultStateBefore = await program.account.growVault.fetch(vaultPda);
    const userBalanceBefore = await getAccount(provider.connection, userTokenAccount);

    // Withdraw 100% of user shares
    await program.methods
      .withdraw(vaultStateBefore.totalShares)
      .accounts({
        vault: vaultPda,
        owner: user.publicKey,
        assetMint: mint,
        vaultTokenAccount: vaultTokenAccountPda,
        userTokenAccount: userTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([user])
      .rpc();

    const vaultStateAfter = await program.account.growVault.fetch(vaultPda);
    const userBalanceAfter = await getAccount(provider.connection, userTokenAccount);

    expect(vaultStateAfter.totalShares.toNumber()).to.equal(0);
    expect(vaultStateAfter.totalDeposited.toNumber()).to.equal(0);
    expect(Number(userBalanceAfter.amount)).to.be.greaterThan(
      Number(userBalanceBefore.amount) + DEPOSIT_AMOUNT.toNumber()
    );
  });

  it("6. Rejects unauthorized pause attempt", async () => {
    const maliciousActor = anchor.web3.Keypair.generate();
    try {
      await program.methods
        .togglePause(true)
        .accounts({
          config: configPda,
          admin: maliciousActor.publicKey,
        })
        .signers([maliciousActor])
        .rpc();
      expect.fail("Pause should fail with unauthorized admin");
    } catch (err: any) {
      expect(err.message).to.include("UnauthorizedAdmin");
    }
  });
});