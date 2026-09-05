# Generate program address keypair
solana-keygen new -o target/deploy/grow_vault-keypair.json --no-bip39-passphrase --force

# Sync program ID into Rust source
anchor keys sync

# Compile the smart contract
anchor build