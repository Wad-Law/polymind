use crate::bus::types::Bus;
use crate::config::config::PolyCfg;
use crate::core::types::{Actor, PolyMarketEvent};
use anyhow::{Context, Result};
use futures::{StreamExt, stream};
use reqwest::Client;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub struct PolyActor {
    pub bus: Bus,
    pub client: Client,
    pub poly_cfg: PolyCfg,
    pub shutdown: CancellationToken,
}

impl PolyActor {
    pub fn new(
        bus: Bus,
        client: Client,
        poly_cfg: PolyCfg,
        shutdown: CancellationToken,
    ) -> PolyActor {
        Self {
            bus,
            client,
            poly_cfg,
            shutdown,
        }
    }

    async fn fetch_events_page(&self, offset: u32) -> Result<Vec<PolyMarketEvent>> {
        let res = self
            .client
            .get(self.poly_cfg.gamma_events_url.clone())
            .query(&[
                ("order", "id"),
                ("ascending", "false"),
                ("active", "true"),
                ("limit", &self.poly_cfg.page_limit.to_string()),
                ("offset", &offset.to_string()),
            ])
            .send()
            .await
            .context("requesting events")?
            .error_for_status()
            .context("received non-success status for events request")?
            .json::<Vec<PolyMarketEvent>>()
            .await
            .context("parsing events response")?;
        Ok(res)
    }

    async fn fetch_all_active_polymarket_events(&self) -> Result<Vec<PolyMarketEvent>> {
        let mut rows = Vec::new();
        let mut offset = 0;

        loop {
            let page = self.fetch_events_page(offset).await?;

            if page.is_empty() {
                break;
            }
            let len = page.len();
            for ev in page {
                rows.push(ev);
            }
            if len < self.poly_cfg.page_limit as usize {
                break;
            }
            offset += self.poly_cfg.page_limit;
        }
        Ok(rows)
    }
}

#[async_trait::async_trait]
impl Actor for PolyActor {
    async fn run(mut self) -> Result<()> {
        info!("PolyActor started");

        // throttle the loop
        let mut tick = tokio::time::interval(self.poly_cfg.market_list_refresh); // refresh cadence

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("PolyActor: shutdown requested");
                    break;
                }

                //Fetch active polymarket events and markets
                _ = tick.tick() => {
                     match self.fetch_all_active_polymarket_events().await  {
                        Ok(poly_events) => {
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
                                    error!(?e, "publish to polymarket_events failed");
                                }
                            }
                        }
                        Err(e) => {
                            error!("PolyActor: failed to fetch active poly market event: {}", e);
                            // backoff to avoid hot loop on repeated failures
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
        info!("PolyActor stopped cleanly");
        Ok(())
    }
}
