# Polymind: Autonomous News Trading Bot

Polymind is a high-performance, event-driven trading bot written in Rust. It is designed to autonomously ingest real-time news, analyze sentiment and relevance using NLP and LLMs, and execute trades on prediction markets (specifically Polymarket) based on probabilistic signals.

## 🚀 Key Features

### 1. **Event-Driven Architecture**
Built on a modular **Actor System** using `tokio` broadcast channels (The **Bus**), ensuring low latency and clean separation of concerns:
- **News Ingestion**: Dedicated actors (`finjuice`, `rss`) for retrieving real-time news from financial feeds and RSS sources.
- **Discovery**: Responsible for finding and indexing new prediction markets from providers.
- **Strategy**: Core logic engine (Filtering, Deduplication, Scoring, Sizing).
- **Market Data**: Retrieves live prices and order books from Gamma/Polymarket APIs.
- **Execution**: Management of orders via EIP-712 signing and PolyMarket CTF Exchange interaction.

### 2. **Advanced NLP Pipeline**
- **Tokenization**: Custom pipeline with stemming, stopword removal, and n-gram generation (bigrams/trigrams).
- **SimHash**: Fast locality-sensitive hashing for detecting near-duplicate news events.
- **Hybrid Search**: Combines **BM25** (keyword matching via `tantivy`) and **Semantic Search** (embeddings via `fastembed`) to instantly find relevant prediction markets for breaking news.
- **LLM Integration**: Interfaces with LLMs for high-level semantic analysis and probability estimation.

### 3. **Quantitative Strategy**
- **Financial Precision**: Uses `rust_decimal` for all financial calculations (prices, sizes, bankroll) to avoid floating-point errors.
- **Kelly Criterion**: Dynamic position sizing based on estimated edge and probability.
- **Risk Management**: "Kill Switch" functionality via `RiskActor` which halts trading upon breaching daily loss or drawdown limits.
- **Position Reconciliation**: Periodic synchronization with the Polymarket Data API to correct internal state drift and remove "zombie" positions.

## 🛠️ Setup & Configuration

### Prerequisites
- Rust (latest stable)
- Node.js (for some auxiliary scripts if needed)

### Configuration

**1. `config.yml`**
Base configuration for URLs, timeouts, and feed sources. Modify this file to change Polmarket API endpoints or RSS feeds.
```yaml
polymarket:
  baseUrl: "https://api.polymarket.com"
  gammaMarketsUrl: "https://gamma-api.polymarket.com/markets"
  rpcUrl: "https://polygon-rpc.com"
  dataApiUrl: "https://data-api.polymarket.com"
# ...
```

**2. Environment Variables (.env)**
Sensitive keys should be set in a `.env` file:

```bash
# Polymarket Credentials
POLY_API_KEY="your_api_key"
POLY_API_SECRET="your_api_secret"
POLY_PASSPHRASE="your_passphrase"

# LLM Config
LLM_PROVIDER="openai" # or "anthropic"
LLM_API_KEY="sk-..."
```

### Running
```bash
# Run the ingestor
cargo run --bin ingestor

# Run tests
cargo test
```

## 📂 Project Structure

- `src/core`: Shared types and domain definitions (`RawNews`, `Order`, `Signal`).
- `src/bus`: Event bus definitions and channel types.
- `src/finjuice`: Financial news feed integration.
- `src/rss`: RSS feed poller actors.
- `src/discovery`: Market discovery actors.
- `src/strategy`:
    - `actor.rs`: Main strategy coordination.
    - `tokenization.rs`: NLP processing.
    - `sim_hash_cache.rs`: Deduplication logic.
    - `kelly.rs`: Position sizing math.
    - `market_index.rs`: Hybrid search index (BM25 + Semantic).
- `src/risk`: Risk management actor and logic.
- `src/marketdata`: Market price fetching actors.
- `src/execution`: Order signing, submission, and state reconciliation actors.

## 🔮 Next Steps

1.  **Backtesting Engine**: Create a simulation mode to replay historical news and validate strategy performance.
2.  **Live Monitoring Dashboard**: A TUI or Web UI to monitor active positions, PnL, and system health in real-time.
3.  **Multi-LLM Consensus**: Query multiple models and aggregate scores for higher confidence signals.