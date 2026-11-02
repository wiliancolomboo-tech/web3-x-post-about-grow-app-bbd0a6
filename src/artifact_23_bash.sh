# Configure Solana CLI for Devnet
solana config set --url https://api.devnet.solana.com

# Airdrop devnet SOL to your deployer keypair
solana airdrop 2

# Deploy Anchor program to Devnet
anchor deploy --provider.cluster devnet