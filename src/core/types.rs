use anyhow::Result;
use serde::Deserialize;

#[async_trait::async_trait]
pub trait Actor: Send + Sync + 'static {
    async fn run(self) -> Result<()>;
}

// ----------- Domain messages -----------------
#[derive(Clone, Debug)]
pub struct RawNews {
    pub content_hash: String,
    pub source: String,
    pub url: String,
    pub title: String,
    pub lede: String,
    pub ts_ms: i64,
    pub lang: String,
}

#[derive(Clone, Debug)]
pub struct MarketDataRequest{
    pub market_id: String
}

#[derive(Clone, Debug)]
pub struct MarketDataSnap {
    pub market_id: String,
    pub book_ts_ms: i64,
    pub best_bid: f32,
    pub best_ask: f32,
    pub bid_size: f32,
    pub ask_size: f32,
}

#[derive(Clone, Debug)]
pub struct Order {
    pub client_order_id: String,
    pub market_id: String,
    pub price: f32,
    pub size: f32,
}

#[derive(Clone, Debug)]
pub struct Execution {
    pub client_order_id: String,
    pub market_id: String,
    pub avg_px: f32,
    pub filled: f32,
    pub fee: f32,
    pub ts_ms: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PolyMarketEvent {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub markets: Option<Vec<PolyMarketMarket>>
}

#[derive(Clone, Debug, Deserialize)]
pub struct PolyMarketMarket {
    pub id: String,
}