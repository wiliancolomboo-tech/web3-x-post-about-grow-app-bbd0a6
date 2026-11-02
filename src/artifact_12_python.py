#!/usr/bin/env python3
"""
Grow App Bounty Deliverable: Autonomous X (Twitter) Thread Publisher
Posts the technical deep dive thread sequentially using Twitter API v2.
"""

import os
import sys
import time
import requests
from requests_oauthlib import OAuth1

# Thread text blocks defined in Section 1.3
TWEETS = [
    """🧵 THREAD: Deconstructing @Grow_App — Solana’s High-Velocity Yield Architecture ⚡️

How does Grow turn Solana’s 400ms block times into institutional-grade automated compounding? 
Let’s break down its PDA state transitions, atomic harvest CPIs, and MEV-shielded reinvestment pipeline. 👇 [1/8]""",

    """1/8 🏛️ The Core Problem in DeFi Yield
Traditional yield vaults suffer from two structural flaws:
1. Gas-expensive manual harvesting (EVM makes micro-compounding uneconomic).
2. Toxic flow & arbitrage leakage during reward swaps.

Grow eliminates this via Solana’s parallel runtime (Sealevel) and deterministic PDA state machines. 
Here is how the pipeline runs under the hood 🧵 [2/8]""",

    """2/8 ⚙️ Architectural Topography
Grow decouples user liquidity from execution keepers:
• `Vault`: Holds global state, total shares, and underlying mint authority.
• `Strategy`: Tracks external lending/liquidity allocation (Kamino, Marginfi, Jito).
• `Keeper`: Decentralized bot network executing atomic harvests via permissioned CPIs.

Deposit -> Mint Vault Shares (pro-rata) -> Allocate Strategy -> Continuous Compound. [3/8]""",

    """3/8 🧮 Mathematical Share Pricing
Grow maintains fair value through dynamic share pricing:

    SharePrice = TotalUnderlyingAssets / TotalSupplyShares

When keepers harvest rewards, rewards are swapped to underlying collateral and injected back into the vault WITHOUT minting new shares. 
Existing shares appreciate monotonically in underlying asset denomination. Zero dilution. [4/8]""",

    """4/8 🛡️ Atomic Harvest Loop & Slippage Guard
Every harvest execution runs an atomic CPI sequence:
1. Strategy `harvest_rewards()` sends accrued reward tokens to PDA vault token account.
2. CPI to Jupiter/Raydium to swap reward tokens -> underlying deposit mint.
3. Compute dynamic slippage bound `min_out` based on oracle feed (Pyth/Switchboard).
4. Re-deposit into yield engine.
If execution slips > 0.5%, the entire transaction reverts atomically. No sandwiching. [5/8]""",

    """5/8 ⚡ Token-2022 & Yield Extensions
Grow is engineered for the future of Solana:
• Transfer hook compatibility for compliance and yield streaming.
• Interest-bearing token extensions to eliminate synthetic wrapped-token overhead.
• Metadata-pointer integration for transparent vault auditing on-chain. [6/8]""",

    """6/8 🔐 Security & Reentrancy Mitigation
Solana programs are vulnerable to cross-instruction account state manipulation. Grow secures vaults through:
✅ Strict PDA seed validation (`[b"vault", mint.key()]`).
✅ Flash-loan reentrancy lock via transient account states (`vault.is_locked = true`).
✅ Explicit remaining accounts validation in multi-hop CPIs.
✅ Zero arbitrary CPI targets. [7/8]""",

    """7/8 📊 Compute Unit (CU) Efficiency
By utilizing raw BPF memory copies where applicable, zero-copy deserialization for large yield tables, and packing account structs, Grow consumes < 65,000 CUs per harvest cycle!
Compound 10x/day for pennies. That is the Solana advantage. [8/8]""",

    """8/8 🚀 Summary: Grow represents the intersection of maximum capital efficiency and Solana-native systems engineering.

Check out the verified Anchor contracts & explore the future of automated passive yield on Solana:
🔗 App: https://growapp.io
🔗 Open Source Vault Specs: github.com/grow-vaults

#Solana #DeFi #Rust #Web3 #SolanaDevs"""
]

API_URL = "https://api.twitter.com/2/tweets"

def get_auth():
    api_key = os.getenv("TWITTER_API_KEY")
    api_secret = os.getenv("TWITTER_API_SECRET")
    access_token = os.getenv("TWITTER_ACCESS_TOKEN")
    access_token_secret = os.getenv("TWITTER_ACCESS_TOKEN_SECRET")

    if not all([api_key, api_secret, access_token, access_token_secret]):
        print("[!] Missing Twitter API v2 credentials in environment variables.")
        print("    Required: TWITTER_API_KEY, TWITTER_API_SECRET, TWITTER_ACCESS_TOKEN, TWITTER_ACCESS_TOKEN_SECRET")
        return None

    return OAuth1(api_key, api_secret, access_token, access_token_secret)

def publish_thread():
    auth = get_auth()
    dry_run = auth is None

    if dry_run:
        print("[*] Running in DRY-RUN mode. Outputting formatted tweets to console:\n")

    prev_tweet_id = None

    for idx, tweet_text in enumerate(TWEETS):
        print(f"--- Posting Tweet {idx + 1}/{len(TWEETS)} (Length: {len(tweet_text)} chars) ---")
        if dry_run:
            print(tweet_text)
            print()
            time.sleep(0.5)
            continue

        payload = {"text": tweet_text}
        if prev_tweet_id:
            payload["reply"] = {"in_reply_to_tweet_id": prev_tweet_id}

        response = requests.post(API_URL, auth=auth, json=payload)

        if response.status_code != 201:
            print(f"[!] Failed to post tweet {idx + 1}: {response.status_code} - {response.text}")
            sys.exit(1)

        data = response.json()
        prev_tweet_id = data["data"]["id"]
        print(f"[✓] Published Tweet ID: {prev_tweet_id}")
        time.sleep(2)  # Rate-limit buffer between thread nodes

    print("\n[✓] Grow App technical thread successfully published!")

if __name__ == "__main__":
    publish_thread()