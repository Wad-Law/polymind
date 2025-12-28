use crate::bus::types::Bus;
use crate::config::config::PolyCfg;
use crate::core::types::{Actor, PolyMarketEvent};
use anyhow::{Context, Result};
use futures::{StreamExt, stream};
use reqwest::Client;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::log::warn;
use tracing::{error, info};

pub struct MarketDiscoveryActor {
    pub bus: Bus,
    pub client: Client,
    pub poly_cfg: PolyCfg,
    pub shutdown: CancellationToken,
}

impl MarketDiscoveryActor {
    pub fn new(
        bus: Bus,
        client: Client,
        poly_cfg: PolyCfg,
        shutdown: CancellationToken,
    ) -> MarketDiscoveryActor {
        Self {
            bus,
            client,
            poly_cfg,
            shutdown,
        }
    }

    async fn fetch_events_page(&self, offset: u32) -> Result<Vec<PolyMarketEvent>> {
        let url = self.poly_cfg.gamma_events_url.clone();
        let res = self
            .client
            .get(&url)
            .query(&[
                ("order", "startDate"),
                ("ascending", "false"),
                ("active", "true"),
                ("closed", "false"),
                ("limit", &self.poly_cfg.page_limit.to_string()),
                ("offset", &offset.to_string()),
            ])
            .send()
            .await
            .context("requesting polymarket events")?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!(
                "Polymarket API error: status={}, url={}, offset={}, body={}",
                status,
                url,
                offset,
                body
            );
        }

        let events = res
            .json::<Vec<PolyMarketEvent>>()
            .await
            .context("parsing polymarket events response")?;
        Ok(events)
    }

    async fn fetch_all_active_polymarket_events(&self) -> Result<Vec<PolyMarketEvent>> {
        let mut rows = Vec::new();
        let mut offset = 0;
        let mut consecutive_errors = 0;

        loop {
            // Be polite to the API
            if offset > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            match self.fetch_events_page(offset).await {
                Ok(page) => {
                    consecutive_errors = 0; // Reset error count on success
                    if page.is_empty() {
                        break;
                    }
                    let len = page.len();
                    rows.extend(page);

                    if len < self.poly_cfg.page_limit as usize {
                        break;
                    }
                    offset += self.poly_cfg.page_limit;
                }
                Err(e) => {
                    error!(
                        "MarketDiscoveryActor: Failed to fetch events page at offset {}: {:#}. Retrying...",
                        offset, e
                    );
                    consecutive_errors += 1;
                    if consecutive_errors >= 3 {
                        error!(
                            "MarketDiscoveryActor: Too many consecutive errors. Returning partial results."
                        );
                        break;
                    }
                    // Backoff before retry
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
        Ok(rows)
    }
}

#[async_trait::async_trait]
impl Actor for MarketDiscoveryActor {
    async fn run(mut self) -> Result<()> {
        info!("MarketDiscoveryActor started");

        // throttle the loop
        let mut tick = tokio::time::interval(self.poly_cfg.market_list_refresh); // refresh cadence

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("MarketDiscoveryActor: shutdown requested");
                    break;
                }

                //Fetch active polymarket events and markets
                _ = tick.tick() => {
                     match self.fetch_all_active_polymarket_events().await  {
                        Ok(poly_events) => {
                            if poly_events.is_empty() {
                                warn!("MarketDiscoveryActor: Fetched 0 events. MarketIndex might be empty.");
                            } else {
                                info!("MarketDiscoveryActor: Fetched {} events.", poly_events.len());
                            }

                            let bus = self.bus.clone();
                            let publish_futs = poly_events.into_iter().map(
                                move |ev| {
                                    let bus = bus.clone();
                                    async move { bus.polymarket_events.publish(ev).await }
                                });

                            // e.g. at most 32 concurrent publishes - BOUNDED concurrency to avoid blasting the bus
                            let results = stream::iter(publish_futs)
                                .buffer_unordered(32)
                                .collect::<Vec<_>>()
                                .await;

                            for res in results {
                                if let Err(e) = res {
                                    // Either `return Err(e)` or just log and continue
                                    error!(?e, "MarketDiscoveryActor: publish to polymarket_events failed");
                                }
                            }
                        }
                        Err(e) => {
                            error!("MarketDiscoveryActor: failed to fetch active poly market event: {:#}", e);
                            // backoff to avoid hot loop on repeated failures
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
        info!("MarketDiscoveryActor stopped cleanly");
        Ok(())
    }
}
