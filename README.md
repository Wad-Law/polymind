# Polymind
## System Overview
The system transforms live news into automated trades using the following steps:

1 - Ingest real-time news + market updates
2 - Normalize + clean text
3 - Extract entities & dates
4 - Retrieve candidate markets using BM25
5 - Filter + score markets with a transparent heuristic
6 - Select Top-K markets with diversity constraints
7 - Convert scores → probabilities
8 - Generate trade decisions based on edge vs market price
9 - Kelly-size positions with risk caps
10 - Execute orders with microstructure-aware rules
11 - Persist all data to PostgreSQL (relational) & QuestDB (time-series)

The system is fully event-driven and optimized for real-time reaction to macroeconomic headlines.

## Architecture
The system uses an actor model: each actor has isolated state, async message loops, and communicates via a shared pub/sub Bus.
Only one actor is allowed to mutate the databases: Monitoring/PersistenceActor

All others are pure transformers.

Actors:
 - RSSActor – RSS news ingestion 
 - FJActor – Fast macro alert ingestion 
 - PolymarketActor – Market metadata ingestion 
 - MarketDataActor – Provides real-time quotes and orderbook snapshots 
 - StrategyActor – Merged Matcher + Strategy (core logic)
 - ExecutionActor – Executes orders, returns fills 
 - RiskManagerActor – Exposure & risk limits
 - MonitoringPersistenceActor – Logging, metrics, and persistence (PostgreSQL + QuestDB)

Key Properties
 - Real-time and Event-driven
 - No shared mutable state 
 - Storage writes isolated to a single actor

## Actors
### Ingest Layer (News + Markets)
1- RSSActor
  - Pulls general RSS feeds. 
  - Normalizes articles into a unified RawNews format. 
  - Publishes to raw-news.

2- FJActor (FinancialJuice)
 - Fetches high-frequency macro-breaking alerts. 
 - Standardizes message format to RawNews. 
 - Publishes to raw-news.

3- PolymarketActor 
 - Periodically fetches all Polymarket markets. 
 - Normalizes fields: market_id, title, description, tags, outcomes, category, resolve_date, liquidity. 
 - Publishes to indexed-markets (for StrategyActor)

### market data actor
- The StrategyActor requests real-time prices/quotes for markets in its Top-K list. 
- MarketDataActor returns:
  - bid/ask/mid prices 
  - depth (bid/ask size)
  - 24h volume & open interest 
  - spreads, liquidity metrics

- Sends market data to StrategyActor and RiskManagerActor.
- Publishes:market-data

### Strategy Layer
StrategyActor

This is the core brain of the system.
It performs matching, ranking, scoring, probability estimation, edge analysis, risk-aware sizing, and trade decision generation.

Consumes:
  - raw-news 
  - indexed-markets 
  - market-data 
  - fills/execution reports

Communicates:
 - ⇆ MarketDataActor (quote requests/responses)
 - ⇆ ExecutionActor (orders/fills)
 - ⇆ RiskManagerActor (risk veto/limits)

Produces:
 - trade-decisions (orders)
 - Internal logs/metrics → Monitoring/PersistenceActor

### Execution layer

Receives trade decisions from StrategyActor. 
  - Performs real/mocked execution:
  - limit orders 
  - crossing when edge sufficient 
  - slicing + re-quoting 
  - TTL & cancellation rules

Sends:
  - fills/execution reports → StrategyActor + RiskManagerActor

Publishes:
  - orders 
  - fills

### Risk Layer

RiskManagerActor

Receives:
  - Market data from MarketDataActor 
  - Fills & order data from ExecutionActor

Maintains risk caps:
  - per-market exposure 
  - per-event exposure 
  - gross portfolio limits 
  - liquidity consumption limits

Can send risk veto / override signals to StrategyActor.


### Monitoring & Persistence Layer (Only actor allowed to touch DB)

MonitoringPersistenceActor

- Subscribes to all major pipelines (sampled or full). 
- Responsible for all mutations to storage.

Writes:
- PostgreSQL → relational/core data
- QuestDB → time-series data (ticks, PnL, metrics)

Produces:
- metrics 
- logs 
- latency measurements 
- PnL curves

###  Storage Architecture

PostgreSQL (relational layer)

Stores:
- markets 
- news 
- matches 
- signals 
- decisions 
- orders 
- fills 
- positions 
- risk limits 
- configuration

QuestDB (time-series layer)

Stores:
- ticks & quotes 
- orderbook snapshots 
- PnL timeseries 
- latency metrics 
- actor performance metrics

Only MonitoringPersistenceActor writes to these databases.


## Full Matching + Decision Pipeline (Deep Explanation)
Goal: convert unpredictable news text into clean, comparable tokens.

### 1 - Normalize + Tokenize
Goal: convert unpredictable news text into clean, comparable tokens.

Steps:

1 - Lowercase everything : Avoid differences between “FED” and “Fed”.
2 - Strip URLs & punctuation : Remove hyperlinks, emojis, special symbols.
3 - Collapse whitespace: Multiple spaces → one.
4 - Keep ALL-CAPS tokens: Important for economic entities: ECB, FOMC, GDP, CPI.
5 - Keep numbers Critical: rates, percentages, dates, years.
6 - Stopword removal: Minimal stopword list so meaning remains:“the”, “and”, “a”, “to”, etc.

Why this matters -> Tokens define the quality of all downstream stages: entity extraction, BM25 search, overlap scoring, etc.


### 2 - Entity & Date Extraction

Goal: convert unpredictable news text into clean, comparable tokens.

1 - Entities:

Use curated lists + Aho-Corasick for:

Central banks: FED, ECB, BOE, BOJ
Countries: US, China, Japan, etc.
Leaders: Biden, Putin, Lagarde
Economic terms: inflation, unemployment
Tickers/symbols: EURUSD, BTC, SPX

2 - Numbers:
integers (2024, 50)
percent (3%, 12.5%)
basis points (25bps, 50bp)
large numbers ($5B, 1.2T)

3 - Dates
Use regex + chrono rules:

Dec 15
next week
by year-end
Q4


Map natural language into ranges:
“year-end” → Dec 31 current year
“next week” → next Monday–Sunday window


Why this matters

Entities + dates allow:
  - massively improved filtering
  - relevance boosting
  - category alignment with markets
  - This is one of the strongest signals in the model.

### Candidate Generation with BM25 (Lexical Retrieval)
Goal: find 100 candidate markets quickly and cheaply.

Polymarket market titles are: short, literal, highly keyword-dependent
BM25 excels at ranking short, keyword-driven text.

BM25 Inputs:
 - title
 - description
 - tags
 - (sometimes) outcomes

BM25 Output: list of (market_id, score) sorted by lexical relevance

Why top-100: cheap, covers all plausible matches and provides baseline for later ranking

### Hard Filters (False Positive Reduction)
Date compatibility: Headline mentions “Dec 15” → market must resolve near that date.

Category alignment: If headline mentions “rates”, “Fed”, “inflation” → market must be in monetary/macro category.

Entity overlap requirement: If headline mentions “Biden”, market must reference Biden or related entities.

Liquidity guards -> Drop markets with:
 - insufficient open interest
 - low 24h volume
 - very wide spreads

Why - > Hard filters massively boost precision even before scoring.

### Heuristic Scoring (Simple & Transparent)

score =
0.50 * bm25_norm
+0.25 * entity_overlap
+0.10 * number_overlap
+0.10 * time_compat
+0.15 * liquidity_score
-0.05 * staleness_penalty

Parameter Explanation

- bm25_norm (50%) – lexical relevance
- entity_overlap (25%) – match on people/orgs/countries
- number_overlap (10%) – match on dates/numbers/rates
- time_compat (10%) – alignment of date ranges
- liquidity_score (15%) – spreads, OI, volume
- staleness_penalty (-5%) – penalize old markets

### Top-K with Diversity
After scoring:
1 -Filter:
- bm25_norm ≥ 0.3
- entity_overlap ≥ 0.2

2 -Apply MMR:
mmr = λ*score - (1-λ)*max_similarity
λ ≈ 0.7

3 - Select K = 5.

Why => Ensures coverage of different but relevant markets, avoiding duplicates.

### Dedup Headlines
To avoid reprocessing spam/duplicates:
- Normalize → AHash/xxHash 
- LRU TTL cache (24–72 hours)
- Optional SimHash for paraphrases

Why => Avoids redundant trades and reduces pipeline load.

### Score → Probability
Convert final score s into a belief p:

p_raw = sigmoid(a + b*s)
p = 0.5 + λ*(p_raw - 0.5)
clip p ∈ [0,1]

a, b tuned later
λ ∈ [0.2, 0.6] to shrink toward 0.5

Why => Models tend to be overconfident; shrinkage prevents extreme beliefs.

### Compare Belief vs Market Price
let:

y = YES midprice
p = belief

1 - Compute edge: edge = p - y

2 - Trading rule:
if edge > τ → Buy YES
else if edge < -τ → Buy NO
else → no trade

τ = 0.01–0.02 for fee/slippage buffer.

### Kelly-Style Sizing (with Risk Caps)
YES trade:
f* = (p - y) / (1 - y)

NO trade:
f* = (y - p) / y

Apply:
- fractional Kelly (κ = 0.1–0.33)
- per-market caps 
- per-event caps 
- portfolio gross caps 
- liquidity clipping (≤X% of book depth)

Contracts: contracts = floor( bankroll * f / price )

### Execution Rules (Practical Microstructure)
ExecutionActor enforces:

Limit-first
- Quote inside spread 
- Cross only if edge > threshold

Slicing
- Break large orders 
- Reprice on quote change

TTL / Cancellation 
- Order expires if not filled in N seconds

Slippage-aware: Incorporate spread + fee into edge check

Cooldown: Avoid flip-flopping on small price moves

## Storage

PostgreSQL (Relational)

Stores:
- markets
- news
- matches
- decisions
- orders
- fills
- signals
- positions
- config & risk limits

QuestDB (Time-series)

Stores:
- market ticks
- orderbook snapshots
- PnL curves
- latency metrics
- actor metrics