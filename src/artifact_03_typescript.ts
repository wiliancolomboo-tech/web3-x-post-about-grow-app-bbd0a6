import * as anchor from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import { getAccount } from "@solana/spl-token";
import * as fs from "fs";

// Configuration
const RPC_ENDPOINT = process.env.SOLANA_RPC || "http://127.0.0.1:8899";
const KEEPER_KEYPAIR_PATH = process.env.KEEPER_KEYPAIR || "./keeper-keypair.json";
const PROGRAM_ID = new PublicKey("Grow1111111111111111111111111111111111111111");

async function main() {
    const connection = new Connection(RPC_ENDPOINT, "confirmed");
    const rawSecret = JSON.parse(fs.readFileSync(KEEPER_KEYPAIR_PATH, "utf-8"));
    const keeper = Keypair.fromSecretKey(Uint8Array.from(rawSecret));
    const wallet = new anchor.Wallet(keeper);
    const provider = new anchor.AnchorProvider(connection, wallet, { commitment: "confirmed" });

    console.log(`[Grow Keeper] Initialized worker: ${keeper.publicKey.toBase58()}`);

    const idl = await anchor.Program.fetchIdl(PROGRAM_ID, provider);
    if (!idl) throw new Error("IDL not discovered on-chain");
    const program = new anchor.Program(idl, PROGRAM_ID, provider);

    // Monitoring Loop
    setInterval(async () => {
        try {
            console.log("[Grow Keeper] Scanning yield differentials across Solana...");
            
            // Query all Grow Vault instances
            const vaults = await program.account.vaultState.all();
            for (const vaultAccount of vaults) {
                const vaultData = vaultAccount.account as any;
                const underlyingMint = vaultData.underlyingMint as PublicKey;
                const tokenVault = vaultData.tokenVault as PublicKey;

                // Check actual token account balance on-chain
                const splAccountInfo = await getAccount(connection, tokenVault);
                const onChainBalance = BigInt(splAccountInfo.amount.toString());
                const recordedTotalAssets = BigInt(vaultData.totalVaultAssets.toString());

                // If balance exceeds recorded assets, yield was accrued and ready to compound
                if (onChainBalance > recordedTotalAssets) {
                    const yieldAccrued = onChainBalance - recordedTotalAssets;
                    console.log(`[Grow Keeper] Accrued yield found: ${yieldAccrued.toString()} base units. Compounding...`);

                    const tx = await program.methods
                        .harvestAndCompound(new anchor.BN(yieldAccrued.toString()))
                        .accounts({
                            keeper: keeper.publicKey,
                            vault: vaultAccount.publicKey,
                            tokenVault: tokenVault,
                        })
                        .rpc();

                    console.log(`[Grow Keeper] Successfully compounded! TX Hash: ${tx}`);
                }
            }
        } catch (err) {
            console.error("[Grow Keeper] Execution error in loop:", err);
        }
    }, 15000); // Polls every 15 seconds
}

main().catch(console.error);