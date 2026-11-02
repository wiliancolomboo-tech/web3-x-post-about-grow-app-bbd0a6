# Export environment variables for the keeper
export SOLANA_RPC_URL="https://api.devnet.solana.com"
export ORACLE_KEYPAIR_JSON="[...1,2,3...]" # Array format private key
export TWITTER_API_KEY="your_api_key"
export TWITTER_API_SECRET="your_api_secret"
export TWITTER_ACCESS_TOKEN="your_access_token"
export TWITTER_ACCESS_SECRET="your_access_secret"

# Start the keeper daemon
npx ts-node scripts/x_grow_keeper.ts