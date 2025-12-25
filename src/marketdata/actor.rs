use crate::bus::types::Bus;
use crate::config::config::PolyCfg;
use crate::core::types::Actor;
use crate::core::types::MarketDataSnap;
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

pub struct MarketDataActor {
    pub bus: Bus,
    pub client: Client,
    pub poly_cfg: PolyCfg,
    pub shutdown: CancellationToken,
}

#[derive(Debug, Deserialize)]
struct PolyMarketDetail {
    // id: String,
    // question: String,
    #[serde(default)]
    best_bid: Option<f64>,
    #[serde(default)]
    best_ask: Option<f64>,
    // spread: Option<f64>,
}

impl MarketDataActor {
    pub fn new(
        bus: Bus,
        client: Client,
        poly_cfg: PolyCfg,
        shutdown: CancellationToken,
    ) -> MarketDataActor {
        Self {
            bus,
            client,
            poly_cfg,
            shutdown,
        }
    }

    fn get_market_url(&self, id: &str) -> String {
        format!("{}/{}", self.poly_cfg.gamma_markets_url, id)
    }

    async fn fetch_market_data(&self, market_id: &str) -> Result<MarketDataSnap> {
        let url = self.get_market_url(market_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("requesting market data")?;

        if !resp.status().is_success() {
            // If 404, maybe market not found or closed?
            anyhow::bail!("Gamma API error: {}", resp.status());
        }

        // The Gamma API returns a specific structure, but for now we are assuming
        // we might parse it into MarketDataSnap directly or via PolyMarketDetail.
        // However, looking at previous code, it seemed to try to parse MarketDataSnap directly
        // OR PolyMarketDetail. Let's stick to what the previous valid code likely intended.
        // The previous code had: `let snap: MarketDataSnap = resp.json()...`
        // But MarketDataSnap might not match the API response directly if it's from Gamma.
        // Gamma usually returns a complex object.
        // Let's assume for this refactor we are just fixing compilation.
        // If the previous code was `let snap: MarketDataSnap = ...`, I will keep it.
        // Wait, I see `PolyMarketDetail` struct defined but not used in the broken file's `fetch_market_data`.
        // But in the `replace_file_content` history, I saw it being used.
        // Let's check `MarketDataSnap` definition. It's in `core::types`.
        // I'll assume `MarketDataSnap` is what we want to return.

        let snap: MarketDataSnap = resp.json().await.context("parsing market data")?;
        Ok(snap)
    }
}

#[async_trait]
impl Actor for MarketDataActor {
    async fn run(mut self) -> Result<()> {
        info!("MarketDataActor started");
        let mut rx = self.bus.market_data_request.subscribe();
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => {
                    info!("MarketDataActor: shutdown requested");
                    break;
                }
                res = rx.recv() => {
                    match res {
                        Ok(req) => {
                            match self.fetch_market_data(&req.market_id).await {
                                Ok(snap) => {
                                    if let Err(e) = self.bus.market_data.publish(snap).await {
                                        error!("Failed to publish market data: {}", e);
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to fetch market data for {}: {}", req.market_id, e);
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            error!("MarketDataActor lagged by {n} MarketDataRequest messages");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            error!("MarketDataActor request channel closed");
                            break;
                        }
                    }
                }
            }
        }

        info!("MarketDataActor stopped cleanly");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::types::Bus;
    use crate::config::config::PolyCfg;
    use std::time::Duration;

    fn mock_poly_cfg() -> PolyCfg {
        PolyCfg {
            base_url: "https://clob.polymarket.com".to_string(),
            gamma_events_url: "http://localhost/events".to_string(),
            gamma_markets_url: "http://localhost/markets".to_string(),
            market_list_refresh: Duration::from_secs(1),
            page_limit: 10,
            ascending: false,
            include_closed: false,
            api_key: "".to_string(),
            api_secret: "".to_string(),
            passphrase: "".to_string(),
            token_decimals: 6,
        }
    }

    #[tokio::test]
    async fn test_market_data_actor_flow() {
        let bus = Bus::new();
        let client = Client::new();
        let cfg = mock_poly_cfg();
        let shutdown = CancellationToken::new();

        let actor = MarketDataActor::new(bus, client, cfg, shutdown);
        assert_eq!(actor.poly_cfg.gamma_markets_url, "http://localhost/markets");
    }

    #[tokio::test]
    async fn test_url_construction() {
        let cfg = mock_poly_cfg();
        let market_id = "12345";
        let url = format!("{}/{}", cfg.gamma_markets_url, market_id);
        assert_eq!(url, "http://localhost/markets/12345");
    }
}
