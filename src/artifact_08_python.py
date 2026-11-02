# scripts/publish_x_thread.py
import tweepy

API_KEY = "YOUR_TWITTER_API_KEY"
API_SECRET = "YOUR_TWITTER_API_SECRET"
ACCESS_TOKEN = "YOUR_ACCESS_TOKEN"
ACCESS_TOKEN_SECRET = "YOUR_ACCESS_TOKEN_SECRET"

client = tweepy.Client(
    consumer_key=API_KEY,
    consumer_secret=API_SECRET,
    access_token=ACCESS_TOKEN,
    access_token_secret=ACCESS_TOKEN_SECRET
)

tweets = [
    "Most DeFi yield aggregators fail on Solana because they treat it like an EVM fork...\n\nMeet @grow_sol: The autonomous liquidity router. 🧵👇",
    "Traditional yield farming requires active monitoring across @KaminoFinance, @DriftProtocol, and @marginfi...\n\nGrow solves this via a unified PDA-driven dynamic rebalancer running in < 65k CU.",
    "Security is non-negotiable:\n🛡️ Single-slot deposit lock\n🛡️ Epoch drawdown circuit breakers\n🛡️ Deterministic PDA derivation.\n\nRead the contracts: https://github.com/grow-fi/grow-vault"
]

prev_id = None
for tweet in tweets:
    response = client.create_tweet(text=tweet, in_reply_to_tweet_id=prev_id)
    prev_id = response.data["id"]
    print(f"Posted Tweet ID: {prev_id}")