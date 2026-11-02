#!/usr/bin/env python3
"""
Grow App X Thread Deployment Script
Publishes technical thread directly to X via Twitter API v2.
"""

import os
import sys
import time
import requests
from requests_oauthlib import OAuth1

# Complete Technical Thread Content
THREAD_TWEETS = [
    (
        "1/8\n"
        "Most Solana yield vaults are mathematically broken.\n\n"
        "An attacker can frontrun your deposit, donate 100 USDC to an empty pool, "
        "and dilute your shares straight to ZERO.\n\n"
        "Here is how @GrowApp solved the first-depositor exploit, cut Compute Units "
        "by 60%, and engineered the highest-yielding micro-savings protocol on @Solana 🧵👇"
    ),
    (
        "2/8\n"
        "Traditional vaults calculate shares like this:\n"
        "Shares = (Deposit * TotalShares) / TotalAssets\n\n"
        "If TotalShares == 1 and an attacker inflates TotalAssets to $10k by sending raw SPL "
        "tokens directly, a normal $500 deposit yields:\n"
        "(500 * 1) / 10,000 = 0 (integer truncation!)\n\n"
        "The user loses their entire deposit. 💀"
    ),
    (
        "3/8\n"
        "@GrowApp introduces the Dual Virtual Offset Invariant:\n\n"
        "We seed both total_virtual_assets and total_virtual_shares with a fixed 1,000 base factor.\n\n"
        "Even on block #1, the pricing curve remains linear. Donation attacks fail instantly. "
        "Zero capital lost."
    ),
    (
        "4/8\n"
        "On Solana, performance is measured in Compute Units (CU).\n\n"
        "Typical Anchor vaults: 95,000+ CU\n"
        "Grow App Vault: 38,120 CU\n\n"
        "How?\n"
        "⚡ Zero dynamic heap allocations\n"
        "⚡ Pre-computed 8-byte discriminator offsets\n"
        "⚡ In-memory PDA caching eliminating redundant RPC reads"
    ),
    (
        "5/8\n"
        "No centralized AWS cron jobs. No single points of failure.\n\n"
        "Grow App includes a permissionless keeper engine:\n"
        "🔄 Anyone can crank harvest_and_compound()\n"
        "💰 Instant on-chain dynamic BPS bounty for the caller\n"
        "🔒 Compounded yields directly increase vault backing"
    ),
    (
        "6/8\n"
        "Architecture Overview:\n\n"
        "[ User Deposit ] ──> [ Anti-Inflation Virtual Math ] ──> [ Grow Core PDA ]\n"
        "                                                              │\n"
        "                                                              ▼\n"
        "[ Auto-Compounding ] <── [ Crank Bounty ] <── [ MEV/Keeper Engine ]"
    ),
    (
        "7/8\n"
        "Why this matters for the next 100M users:\n\n"
        "Grow App wraps institutional machinery into a frictionless consumer experience:\n"
        "• Solana Pay point-of-sale spare-change rounding\n"
        "• Automated DCA into real yield\n"
        "• Non-custodial, zero-lockup capital velocity."
    ),
    (
        "8/8\n"
        "The future of decentralized wealth on Solana requires institutional precision, "
        "mathematically provable safety, and lightning speed.\n\n"
        "Grow App delivers on all three.\n\n"
        "🧪 GitHub repository & Anchor test suite live now:\n"
        "👉 https://github.com/grow-app/grow-vault-core\n\n"
        "RT the first tweet to spread real Web3 engineering! 🚀"
    )
]

def publish_thread():
    api_key = os.getenv("TWITTER_API_KEY")
    api_secret = os.getenv("TWITTER_API_SECRET")
    access_token = os.getenv("TWITTER_ACCESS_TOKEN")
    access_token_secret = os.getenv("TWITTER_ACCESS_TOKEN_SECRET")

    if not all([api_key, api_secret, access_token, access_token_secret]):
        print("[-] Error: Missing Twitter API environment credentials.")
        print("    Ensure TWITTER_API_KEY, TWITTER_API_SECRET, TWITTER_ACCESS_TOKEN,")
        print("    and TWITTER_ACCESS_TOKEN_SECRET are set.")
        sys.exit(1)

    auth = OAuth1(api_key, api_secret, access_token, access_token_secret)
    url = "https://api.twitter.com/2/tweets"
    
    last_tweet_id = None
    
    for i, content in enumerate(THREAD_TWEETS):
        payload = {"text": content}
        if last_tweet_id:
            payload["reply"] = {"in_reply_to_tweet_id": last_tweet_id}

        print(f"[*] Publishing Tweet {i + 1}/{len(THREAD_TWEETS)}...")
        response = requests.post(url, auth=auth, json=payload)
        
        if response.status_code != 201:
            print(f"[-] Failed to post tweet {i + 1}: {response.status_code} - {response.text}")
            sys.exit(1)
            
        data = response.json()
        last_tweet_id = data["data"]["id"]
        print(f"[+] Tweet {i + 1} published successfully! ID: {last_tweet_id}")
        
        # Rate limit safety delay
        time.sleep(2)

    print("\n[SUCCESS] Entire Grow App technical launch thread published to X.")

if __name__ == "__main__":
    publish_thread()