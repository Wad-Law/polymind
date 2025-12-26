use crate::core::types::MarketDataSnap;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait MarketDataClient: Send + Sync + 'static {
    async fn fetch_market_data(&self, market_id: &str) -> Result<MarketDataSnap>;
}
