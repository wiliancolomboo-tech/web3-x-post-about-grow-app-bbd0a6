# 1. Create a new directory and initialize Anchor workspace
mkdir -p grow-protocol && cd grow-protocol
anchor init grow_vault --template multiple-programs
cd grow_vault

# 2. Install dependencies
yarn add @solana/spl-token @coral-xyz/anchor chai mocha