# 1. Set Solana configuration to Localnet
solana config set --url localhost

# 2. Generate deployment keypair if not already present
solana-keygen new --no-bip39-passphrase -s -o ~/.config/solana/id.json

# 3. Clone and navigate to project root
mkdir -p grow-app-deliverable && cd grow-app-deliverable
anchor init grow_engine --template multiple