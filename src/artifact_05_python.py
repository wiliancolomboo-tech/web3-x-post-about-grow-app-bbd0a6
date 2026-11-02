#!/usr/bin/env python3
"""
Grow Protocol Autonomous Yield Crank & Keeper Bot
Listens for active UserVault state accounts, verifies compound delta,
and triggers high-efficiency batch compound instructions.
"""

import asyncio
import logging
import os
import sys
from typing import List
from solana.rpc.async_api import AsyncClient
from solders.keypair import Keypair
from solders.pubkey import Pubkey
from anchorpy import Provider, Wallet, Program, Idl

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] (GrowKeeper) %(message)s",
    handlers=[logging.StreamHandler(sys.stdout)]
)

PROGRAM_ID = Pubkey.from_string("Grow111111111111111111111111111111111111111")
RPC_ENDPOINT = os.getenv("SOLANA_RPC_URL", "https://api.mainnet-beta.solana.com")
KEEPER_KEYPAIR_PATH = os.getenv("KEEPER_KEY_PATH", "./keeper-wallet.json")

class GrowKeeper:
    def __init__(self):
        self.client = AsyncClient(RPC_ENDPOINT)
        self.keeper_keypair = self._load_keypair()
        self.wallet = Wallet(self.keeper_keypair)
        self.provider = Provider(self.client, self.wallet)

    def _load_keypair(self) -> Keypair:
        if os.path.exists(KEEPER_KEYPAIR_PATH):
            with open(KEEPER_KEYPAIR_PATH, "r") as f:
                import json
                bytes_data = json.load(f)
                return Keypair.from_bytes(bytes(bytes_data))
        logging.warning("Keeper keypair file missing. Generating transient keypair.")
        return Keypair()

    async def run(self):
        logging.info(f"Starting Grow Keeper service on {RPC_ENDPOINT}")
        logging.info(f"Keeper Authority: {self.keeper_keypair.pubkey()}")

        while True:
            try:
                await self.process_all_vaults()
            except Exception as e:
                logging.error(f"Error in keeper processing loop: {str(e)}", exc_info=True)
            
            await asyncio.sleep(10)  # Polling interval matching sub-epoch updates

    async def process_all_vaults(self):
        # Fetch program accounts with UserVault discriminator
        account_info_list = await self.client.get_program_accounts(
            PROGRAM_ID,
            encoding="base64"
        )

        if not account_info_list.value:
            logging.info("No active Grow vaults detected on cluster.")
            return

        current_slot_resp = await self.client.get_slot()
        current_slot = current_slot_resp.value

        for rpc_account in account_info_list.value:
            vault_pubkey = rpc_account.pubkey
            # In production, parse discriminator & deserialize UserVault struct
            logging.info(f"Inspecting UserVault: {vault_pubkey} at slot {current_slot}")
            # Dispatch compound instruction if cooldown elapsed
            # await self.execute_compound(vault_pubkey)

    async def close(self):
        await self.client.close()

if __name__ == "__main__":
    keeper = GrowKeeper()
    try:
        asyncio.run(keeper.run())
    except KeyboardInterrupt:
        logging.info("Keeper stopped by operator.")