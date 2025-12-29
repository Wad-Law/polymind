use crate::bus::types::Bus;
use crate::config::config::RssCfg;
use crate::core::types::{Actor, RawNews};
use anyhow::{Context, Result};
use futures::{StreamExt, stream};
use reqwest::Client;
use rss::Channel;
use std::time::Duration;

use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub struct RssActor {
    pub bus: Bus,
    pub client: Client,
    pub rss_cfg: RssCfg,
    pub shutdown: CancellationToken,
}

impl RssActor {
    pub fn new(bus: Bus, client: Client, rss_cfg: RssCfg, shutdown: CancellationToken) -> RssActor {
        Self {
            bus,
            client,
            rss_cfg,
            shutdown,
        }
    }

    async fn fetch_rss_news(&mut self) -> Result<Vec<RawNews>> {
        let start = std::time::Instant::now();
        let client = self.client.clone();
        let feeds = self.rss_cfg.feeds.clone();

        // Create futures: one per feed
        let fetches = feeds.into_iter().map(move |feed| {
            let client = client.clone();
            async move {
                let resp = client
                    .get(&feed.url)
                    .send()
                    .await
                    .with_context(|| format!("RssActor: Failed to GET {}", feed.url))?;

                let body = resp.text().await?;
                let channel = Channel::read_from(body.as_bytes())
                    .with_context(|| format!("RssActor: Failed to parse RSS from {}", feed.url))?;

                let items = channel.items.into_iter().map(|item| RawNews {
                    feed: feed.id.clone(),
                    title: item.title().unwrap_or("").to_string(),
                    url: item.link().unwrap_or("").to_string(),
                    published: item
                        .pub_date()
                        .and_then(|d| chrono::DateTime::parse_from_rfc2822(d).ok())
                        .map(|dt| dt.with_timezone(&chrono::Utc)),
                    description: item.description().unwrap_or("").to_string(),
                    labels: Vec::new(),
                });

                Ok::<Vec<RawNews>, anyhow::Error>(items.collect())
            }
        });

        // Fetch concurrently with bounded parallelism
        let results = stream::iter(fetches)
            .buffer_unordered(8) // 8 feeds in parallel
            .collect::<Vec<_>>()
            .await;

        // Flatten and handle errors
        let mut all_news = Vec::new();
        for result in results {
            match result {
                Ok(mut items) => all_news.append(&mut items),
                Err(e) => {
                    error!("RssActor: fetch failed: {:?}", e);
                }
            }
        }

        metrics::histogram!("rss_fetch_duration_seconds").record(start.elapsed().as_secs_f64());
        Ok(all_news)
    }
}

#[async_trait::async_trait]
impl Actor for RssActor {
    async fn run(mut self) -> Result<()> {
        info!("RssActor started");

        // throttle the loop
        let mut tick = interval(Duration::from_secs(self.rss_cfg.refresh.as_secs())); // refresh cadence

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("RssActor: shutdown requested");
                    break;
                }

                //Fetch rss news
                _ = tick.tick() => {
                    match self.fetch_rss_news().await  {
                        Ok(rss_news) => {
                            metrics::counter!("rss_fetches_total", "status" => "success").increment(1);
                            metrics::counter!("rss_items_fetched_total").increment(rss_news.len() as u64);
                            let bus = self.bus.clone();
                            stream::iter(
                                rss_news
                                    .into_iter()
                                    .map(move |ev|
                                    {
                                        let bus = bus.clone();
                                        async move { bus.raw_news.publish(ev).await }
                                    })
                            )
                            .buffer_unordered(32)       // cap concurrency
                            .for_each(|res| async {
                                if let Err(e) = res {
                                    error!(?e, "RssActor: publish to raw_news failed");
                                }
                            })
                            .await;
                        }
                        Err(e) => {
                            metrics::counter!("rss_fetches_total", "status" => "error").increment(1);
                            error!("RssActor: failed to fetch active poly market event: {:#}", e);
                            // backoff to avoid hot loop on repeated failures
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
        info!("RssActor stopped cleanly");
        Ok(())
    }
}
