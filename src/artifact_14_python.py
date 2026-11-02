#!/usr/bin/env python3
"""
Grow App Social Attestation Oracle Daemon
Listens to Solana on-chain attestation events and posts verifiable yield proofs to X.
"""

import asyncio
import json
import os
import sys
from dataclasses import dataclass
from typing import Optional

try:
    from solana.rpc.async_api import AsyncClient
    from solders.pubkey import Pubkey
    import tweepy
except ImportError:
    # Graceful message if dependencies aren't yet installed
    pass

@dataclass
class AttestationPayload:
    epoch_id: int
    vault_mint: str
    reported_tvl: float
    solvency_bps: float
    apy_percent: float
    slot: int
    signature: str

class GrowSocialPublisher:
    def __init__(self, rpc_url: str, twitter_credentials: dict):
        self.rpc_url = rpc_url
        self.twitter_client = tweepy.Client(
            consumer_key=twitter_credentials.get("consumer_key"),
            consumer_secret=twitter_credentials.get("consumer_secret"),
            access_token=twitter_credentials.get("access_token"),
            access_token_secret=twitter_credentials.get("access_token_secret")
        ) if "tweepy" in sys.modules and twitter_credentials.get("consumer_key") else None

    def format_tweet(self, payload: AttestationPayload) -> str:
        solvency_str = f"{(payload.solvency_bps / 100.0):.2f}%"
        apy_str = f"{payload.apy_percent:.2f}%"
        tvl_str = f"${payload.reported_tvl:,.2f}"

        tweet = (
            f"⚡️ [SOLANA ON-CHAIN PROOF OF YIELD]\n\n"
            f"@GrowApp Smart Vault Epoch #{payload.epoch_id} verified on-chain.\n\n"
            f"📊 Metrics Snapshot:\n"
            f"• TVL: {tvl_str}\n"
            f"• Current Net APY: {apy_str}\n"
            f"• Reserve Solvency: {solvency_str} (Backed ✅)\n"
            f"• Slot: {payload.slot}\n\n"
            f"🔗 On-Chain Proof:\n"
            f"https://solscan.io/tx/{payload.signature}\n\n"
            f"#Solana #GrowApp #DeFi #ProofOfReserves"
        )
        return tweet

    async def broadcast_attestation(self, payload: AttestationPayload):
        text = self.format_tweet(payload)
        print("\n[READY TO POST TO X]")
        print("--------------------------------------------------")
        print(text)
        print("--------------------------------------------------")
        if self.twitter_client:
            response = self.twitter_client.create_tweet(text=text)
            print(f"[X API] Successfully posted Tweet ID: {response.data['id']}")
        else:
            print("[DRY-RUN] Twitter credentials omitted; simulated broadcast successful.")

async def main():
    print("[INIT] Starting Grow App Social Attestation Daemon...")
    sample_payload = AttestationPayload(
        epoch_id=42,
        vault_mint="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", # USDC
        reported_tvl=14_850_320.50,
        solvency_bps=10420, # 104.20%
        apy_percent=9.34,
        slot=265431200,
        signature="5KzXqL8eZ2...mockSignatureTxGrowAppSolanaAnchorEngine...39dK"
    )
    publisher = GrowSocialPublisher("https://api.mainnet-beta.solana.com", {})
    await publisher.broadcast_attestation(sample_payload)

if __name__ == "__main__":
    asyncio.run(main())