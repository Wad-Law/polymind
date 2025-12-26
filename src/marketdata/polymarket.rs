use crate::config::config::PolyCfg;
use crate::core::types::MarketDataSnap;
use crate::marketdata::client::MarketDataClient;
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PolyToken {
    token_id: String,
    outcome: String,
    price: Decimal,
}

#[derive(Debug, Deserialize)]
struct PolyMarketResponse {
    id: String,
    tokens: Option<Vec<PolyToken>>,
    best_bid: Option<Decimal>,
    best_ask: Option<Decimal>,
    question: String,
}

pub struct PolyMarketDataClient {
    client: Client,
    cfg: PolyCfg,
}

impl PolyMarketDataClient {
    pub fn new(cfg: PolyCfg, client: Client) -> Self {
        Self { client, cfg }
    }

    fn get_market_url(&self, id: &str) -> String {
        format!("{}/{}", self.cfg.gamma_markets_url, id)
    }
}

#[async_trait]
impl MarketDataClient for PolyMarketDataClient {
    async fn fetch_market_data(&self, market_id: &str) -> Result<MarketDataSnap> {
        let url = self.get_market_url(market_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("requesting market data")?;

        if !resp.status().is_success() {
            anyhow::bail!("Gamma API error: {}", resp.status());
        }

        let poly_resp: PolyMarketResponse = resp.json().await.context("parsing market data")?;

        let tokens = poly_resp.tokens.map(|ts| {
            ts.into_iter()
                .map(|t| crate::core::types::MarketToken {
                    token_id: t.token_id,
                    outcome: t.outcome,
                    price: t.price,
                })
                .collect()
        });

        Ok(MarketDataSnap {
            market_id: poly_resp.id,
            book_ts_ms: chrono::Utc::now().timestamp_millis(), // Approximate
            best_bid: poly_resp.best_bid.unwrap_or(Decimal::ZERO),
            best_ask: poly_resp.best_ask.unwrap_or(Decimal::ZERO),
            bid_size: Decimal::ZERO, // Not provided in simple endpoint
            ask_size: Decimal::ZERO,
            tokens,
            question: poly_resp.question,
        })
    }
}
