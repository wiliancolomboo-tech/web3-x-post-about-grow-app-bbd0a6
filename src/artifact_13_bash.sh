# Set Solana CLI to Devnet
solana config set --url https://api.devnet.solana.com

# Airdrop SOL to test wallet
solana airdrop 2

# Deploy program
anchor deploy --provider.cluster devnet

# Verify program binary on-chain
solana program show <PROGRAM_ID>