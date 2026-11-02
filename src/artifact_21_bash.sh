# Configure Solana CLI to Devnet
solana config set --url https://api.devnet.solana.com

# Airdrop SOL to deployment keypair
solana airdrop 2

# Sync program ID with the compiled binary keypair
anchor keys sync

# Deploy program to Devnet
anchor deploy --provider.cluster devnet

# Verify the deployed program
solana program show <PROGRAM_ID>