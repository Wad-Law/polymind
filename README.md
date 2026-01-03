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

### System Architecture

```mermaid
graph TD
    %% Sources
    RSS[RSS Actor] -->|RawNews| Bus
    FinJuice[FinJuice Actor] -->|RawNews| Bus
    Gamma[Gamma Poller] -->|PolyMarketEvent| Bus

    %% Bus
    subgraph Event Bus
        Bus((Bus Channels))
    end
    
    %% Strategy Loop
    Bus -->|RawNews / Events / MarketData| Strategy[Strategy Actor]
    Strategy -->|MarketDataRequest| Bus
    MarketData[MarketData Actor] -->|MarketDataSnap| Bus
    
    %% Execution Loop
    Strategy -->|Order| Bus
    Bus -->|Order| Execution[Execution Actor]
    Execution -->|Execution / Balance / PositionSnap| Bus

    %% Risk Loop
    Strategy -->|PortfolioUpdate| Bus
    Bus -->|PortfolioUpdate / Balance| Risk[Risk Actor]
    Risk -->|SystemStatus Halted| Bus
    Bus -->|SystemStatus| Strategy
```

### 2. **Advanced NLP Pipeline**
- **Tokenization**: Custom pipeline with stemming, stopword removal, and n-gram generation (bigrams/trigrams).
- **SimHash**: Fast locality-sensitive hashing for detecting near-duplicate news events.
- **Hybrid Search**: Combines **BM25** (keyword matching via `tantivy`) and **Semantic Search** (embeddings via `fastembed`) to instantly find relevant prediction markets for breaking news.
- **LLM Integration**: Interfaces with LLMs for high-level semantic analysis and probability estimation.

### 3. **Quantitative Strategy**
- **Financial Precision**: Uses `rust_decimal` for all financial calculations (prices, sizes, bankroll) to avoid floating-point errors.
- **Hybrid Search**: Combines **BM25** (keyword matching) and **Semantic Search** (embeddings) to instantly find relevant prediction markets for breaking news.
- **Kelly Criterion**: Dynamic position sizing based on estimated edge and probability.
    - **Minimum Order Enforcement**: Automatically bumps orders to meet Polymarket's $1.00 minimum value constraint, or skips them if the risk exceeds `max_position_fraction` (Safety Cap).
- **Risk Management**:
    - **Global Circuit Breaker**: Tracks **Net Liquidation Value (NLV)** and halts trading if global drawdown exceeds 10%.
    - **Liquidation Protocol**: Upon a global halt, the `StrategyActor` immediately liquidates **ALL** active positions.
    - **Per-Position Stop Loss**: Automatically liquidates individual positions if they breach a 20% drawdown limit.
- **Position Reconciliation**: Periodic synchronization with the Polymarket Data API to correct internal state drift and remove "zombie" positions.
- **Auditability**: Full database persistence of every event, market snapshot, order, and execution.

### 4. **Reliable Execution**
- **Gnosis Safe Support**: Automatically derives and verifies Proxy Wallet balance via JSON-RPC to ensure accurate buying power even with complex wallet setups.
- **Marketable Limit Orders**: Simulates "Market" orders using aggressively priced Limit Orders (Buy @ $0.99 / Sell @ $0.01) with `FAK` (Fill-And-Kill) time-in-force, ensuring immediate execution or cancellation without resting orders on the book.
- **EIP-712 Signing**: Secure, typed data signing for all CLOB interactions.

### 5. **Observability Stack**
Comprehensive monitoring via a deployed **Prometheus / Grafana / ELK** stack:
- **System Metrics**: CPU, Memory, Disk usage, and Process Heartbeats.
- **Business Metrics**: News Ingestion Rate, Candidate Finding Rate, Order Fill Rate.
- **Strategy Performance**: Latency histograms (Analyst, End-to-End), Kelly Sizing distribution, and Signal Confidence tracking.
- **Logs**: Centralized structured logging in Elasticsearch/Kibana for debugging distributed actor flows.

### 6. **Database Schema**
The system uses a strictly typed PostgreSQL schema with explicit foreign keys to ensure data integrity and auditability:
- **`events`**: Validated news items.
- **`candidate_markets`**: Markets retrieved as potential matches for an event.
- **`signals`**: LLM decisions/scores.
- **`market_data_snapshots`**: Exact state of the order book (prices/sizes) at the moment a decision was made.
- **`orders`**: Orders generated by the Strategy Engine, referencing the `snapshot_id` and using `client_order_id` as the primary key.
- **`executions`**: Filled trades linked to `orders` via `client_order_id`.

## 🛠️ Setup & Configuration

### Prerequisites
- Rust (latest stable)
- **PostgreSQL 16+** (Required for persistence)
- **Docker & Docker Compose** (For DB and Observability stack)

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

# Database Config
DATABASE_URL="postgres://user:password@localhost:5432/polymind"

# LLM Config
LLM_MODEL="gpt-5-nano" # Default
LLM_API_KEY="sk-..."
```

### Running
```bash
### Running

1. **Start Infrastructure** (Postgres + Observability):
    ```bash
    docker-compose up -d
    ```

2. **Run Polymind**:
    ```bash
    cargo run --bin polymind
    ```
    *Ensure `DATABASE_URL` is set.*

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
2.  **Low-Latency Upgrades**: Replace market data polling with WebSocket subscriptions.
3.  **Multi-LLM Consensus**: Query multiple models and aggregate scores for higher confidence signals.
4.  **PersistenceActor**: throttling, decoupling, latency reduction: subscribes to topics and writes them to the database.