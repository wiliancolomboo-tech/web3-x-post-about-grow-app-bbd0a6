# 1. Clone repository and install dependencies
git clone https://github.com/grow-app/grow-vault-core.git
cd grow-vault-core
yarn install

# 2. Build the Anchor program eBPF binary
anchor build

# 3. Synchronize program keys (if regenerating IDs)
anchor keys sync

# 4. Spin up local validator and execute complete automated test suite
anchor test