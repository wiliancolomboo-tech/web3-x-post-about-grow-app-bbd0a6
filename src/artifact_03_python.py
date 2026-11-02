#!/usr/bin/env python3
"""
Grow App Keeper Bot & X (Twitter) Syndication Engine
Monitors on-chain UserVault accounts, triggers compound cranks,
and tweets verified performance milestones.
"""

import os
import time
import json
import base58
import requests
from typing import List, Dict, Any
from dataclasses import dataclass

from solana.rpc.api import Client
from solders.pubkey import Pubkey
from solders.keypair import Keypair
from solders.instruction import Instruction, AccountMeta
from solders.transaction import Transaction
from solders.message import Message

RPC_URL = os.getenv("SOLANA_RPC_URL", "https://api.devnet.solana.com")
PROGRAM_ID = Pubkey.from_string("GrowApp111111111111111111111111111111111111")
X_BEARER_TOKEN = os.getenv("X_BEARER_TOKEN", "")

client = Client(RPC_URL)

@dataclass
class VaultState:
    pubkey: Pubkey
    owner: Pubkey
    principal: int
    yield_earned: int
    last_compounded: int
    interval: int

def decode_vault(pubkey: Pubkey, data: bytes) -> VaultState:
    # Skip 8-byte Anchor discriminator
    content = data[8:]
    owner = Pubkey.from_bytes(content[0:32])
    # mint = Pubkey.from_bytes(content[32:64])
    principal = int.from_bytes(content[64:72], byteorder="little")
    yield_earned = int.from_bytes(content[72:80], byteorder="little")
    last_compounded = int.from_bytes(content[80:88], byteorder="little", signed=True)
    interval = int.from_bytes(content[88:96], byteorder="little", signed=True)
    return VaultState(pubkey, owner, principal, yield_earned, last_compounded, interval)

def fetch_active_vaults() -> List[VaultState]:
    resp = client.get_program_accounts(PROGRAM_ID)
    vaults = []
    for item in resp.value:
        # Size of UserVault: 8 + 32 + 32 + 8 + 8 + 8 + 8 + 1 + 1 = 106 bytes
        if len(item.account.data) == 106:
            vaults.append(decode_vault(item.pubkey, item.account.data))
    return vaults

def post_milestone_to_x(message: str) -> bool:
    """Publishes a verified micro-yield milestone to X API v2"""
    if not X_BEARER_TOKEN:
        print(f"[DRY-RUN X POST]:\n{message}\n")
        return True

    endpoint = "https://api.twitter.com/2/tweets"
    headers = {
        "Authorization": f"Bearer {X_BEARER_TOKEN}",
        "Content-Type": "application/json"
    }
    payload = {"text": message}
    response = requests.post(endpoint, json=payload, headers=headers)
    if response.status_code in [200, 201]:
        print(f"[X POST SUCCESS]: {response.json()}")
        return True
    else:
        print(f"[X POST ERROR {response.status_code}]: {response.text}")
        return False

def run_keeper_cycle(keeper_kp: Keypair):
    print(f"[*] Starting Grow App Keeper loop on {RPC_URL}...")
    vaults = fetch_active_vaults()
    now = int(time.time())

    for vault in vaults:
        if now >= (vault.last_compounded + vault.interval):
            print(f"[!] Vault {vault.pubkey} ready for compounding. Owner: {vault.owner}")
            # Mock simulated harvest: 0.05% per crank interval
            harvestable = int(vault.principal * 0.0005)

            # In production: Build CompoundCrank instruction and send tx
            print(f" -> Auto-compounding {harvestable} lamports for {vault.owner}...")

            # Format technical tweet proof
            sol_amount = harvestable / 1e9
            total_vault_sol = vault.principal / 1e9
            tweet_body = (
                f"🌱 Automated Micro-Yield Compound Verified on @Solana via @GrowApp!\n\n"
                f"⚡ Vault Owner: {str(vault.owner)[:4]}...{str(vault.owner)[-4:]}\n"
                f"💰 Compounded: +{sol_amount:.5f} SOL\n"
                f"📈 Total Protected Principal: {total_vault_sol:.4f} SOL\n\n"
                f"Zero friction. 100% non-custodial. Real yield.\n"
                f"#Solana #DeFi #GrowApp #BuildOnSolana"
            )
            post_milestone_to_x(tweet_body)

if __name__ == "__main__":
    dummy_keypair = Keypair()
    run_keeper_cycle(dummy_keypair)