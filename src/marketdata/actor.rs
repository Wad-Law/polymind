use crate::bus::types::Bus;
use crate::core::types::Actor;
use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::marketdata::client::MarketDataClient;
use std::sync::Arc;

pub struct MarketPricingActor {
    pub bus: Bus,
    pub client: Arc<dyn MarketDataClient>,
    pub shutdown: CancellationToken,
}

impl MarketPricingActor {
    pub fn new(
        bus: Bus,
        client: Arc<dyn MarketDataClient>,
        shutdown: CancellationToken,
    ) -> MarketPricingActor {
        Self {
            bus,
            client,
            shutdown,
        }
    }
}

#[async_trait]
impl Actor for MarketPricingActor {
    async fn run(mut self) -> Result<()> {
        info!("MarketPricingActor started");
        let mut rx = self.bus.market_data_request.subscribe();
        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("MarketDataActor: shutdown requested");
                    break;
                }

                // market data requests
                res = rx.recv() => {
                    match res {
                        Ok(req) => {
                            let start = std::time::Instant::now();
                            match self.client.fetch_market_data(&req.market_id).await {
                                Ok(snap) => {
                                    if let Err(e) = self.bus.market_data.publish(snap).await {
                                        error!("Failed to publish market data: {:#}", e);
                                    }
                                    metrics::counter!("marketdata_requests_total", "status" => "success").increment(1);
                                    metrics::histogram!("marketdata_fetch_duration_seconds").record(start.elapsed().as_secs_f64());
                                }
                                Err(e) => {
                                    metrics::counter!("marketdata_requests_total", "status" => "error").increment(1);
                                    warn!("Failed to fetch market data for {}: {:#}", req.market_id, e);
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // a slow consumer skipped n messages
                            error!("MarketDataActor lagged by {n} MarketDataRequest messages");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // no more senders; decide whether to exit
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
    use crate::marketdata::polymarket::PolyMarketDataClient;
    use reqwest::Client;
    use std::time::Duration;

    fn mock_poly_cfg() -> PolyCfg {
        PolyCfg {
            base_url: "https://clob.polymarket.com".to_string(),
            gamma_events_url: "http://localhost/events".to_string(),
            gamma_markets_url: "http://localhost/markets".to_string(),
            market_list_refresh: Duration::from_secs(1),
            page_limit: 10,
            api_key: "".to_string(),
            api_secret: "".to_string(),
            passphrase: "".to_string(),
            token_decimals: 6,
            rpc_url: "http://localhost:8545".to_string(),
            data_api_url: "http://localhost/positions".to_string(),
        }
    }

    #[tokio::test]
    async fn test_market_data_actor_flow() {
        let bus = Bus::new();
        let http_client = Client::new();
        let cfg = mock_poly_cfg();
        let shutdown = CancellationToken::new();

        let client = Arc::new(PolyMarketDataClient::new(cfg, http_client));
        let _actor = MarketPricingActor::new(bus, client, shutdown);
        // assert_eq!(actor.poly_cfg...) is no longer valid as actor doesn't hold config
    }

    #[tokio::test]
    async fn test_url_construction() {
        let cfg = mock_poly_cfg();
        let market_id = "12345";
        let url = format!("{}/{}", cfg.gamma_markets_url, market_id);
        assert_eq!(url, "http://localhost/markets/12345");
    }
}
