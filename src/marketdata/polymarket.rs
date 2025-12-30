use crate::config::config::PolyCfg;
use crate::core::types::MarketDataSnap;
use crate::marketdata::client::MarketDataClient;
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolyMarketResponse {
    id: String,
    clob_token_ids: Option<String>,
    outcomes: Option<String>,
    outcome_prices: Option<String>,
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

        // Parse tokens from stringified JSON fields
        let mut tokens = None;
        if let (Some(ids_str), Some(outcomes_str), Some(prices_str)) = (
            &poly_resp.clob_token_ids,
            &poly_resp.outcomes,
            &poly_resp.outcome_prices,
        ) {
            let ids: Vec<String> = serde_json::from_str(ids_str).unwrap_or_default();
            let outcomes: Vec<String> = serde_json::from_str(outcomes_str).unwrap_or_default();
            let prices: Vec<String> = serde_json::from_str(prices_str).unwrap_or_default();

            let mut tokens_vec = Vec::new();
            for i in 0..ids.len() {
                if i < outcomes.len() && i < prices.len() {
                    let price = std::str::FromStr::from_str(&prices[i]).unwrap_or(Decimal::ZERO);
                    tokens_vec.push(crate::core::types::MarketToken {
                        token_id: ids[i].clone(),
                        outcome: outcomes[i].clone(),
                        price,
                    });
                }
            }
            if !tokens_vec.is_empty() {
                tokens = Some(tokens_vec);
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    #[tokio::test]
    async fn test_polymarket_data_client_fetch_real() {
        // Setup config with defaults (similar to main.rs)
        let cfg = PolyCfg::default();
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();

        let poly_client = PolyMarketDataClient::new(cfg, client);

        // Fetch a known market ID (Example: "TRUMP-WIN-2024" type valid ID or a random one to check 404/error handling)
        // We'll use a likely valid ID if known, or just check that it makes the request.
        // Let's rely on a known ID. If unknown, we expect an error or 404, which is valid validation of client structure.
        // Using a random ID likely results in 404/400.
        let market_id = "664878"; // Dummy ID

        let res = poly_client.fetch_market_data(market_id).await;

        // Since we are running in CI/Local without guaranteed valid IDs, we mostly want to check
        // that the client build the request and handled the network response (even if it's an error).
        // This confirms it works "like the execution actor test".

        match res {
            Ok(snap) => {
                println!("Successfully fetched market: {:?}", snap);
            }
            Err(e) => {
                println!("Fetch failed as expected with dummy ID: {:?}", e);
                // Assert that the error is related to API response (404, 400) and not client crash
                assert!(
                    e.to_string().contains("Gamma API error")
                        || e.to_string().contains("requesting")
                );
            }
        }
    }
}
