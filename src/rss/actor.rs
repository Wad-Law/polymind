use crate::bus::types::Bus;
use crate::core::types::{Actor};
use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::{info};
use reqwest::Client;
use crate::config::config::RssCfg;

pub struct RssActor {
    pub bus: Bus,
    pub client: Client,
    pub rss_cfg: RssCfg,
    pub shutdown: CancellationToken
}

impl RssActor {
    pub fn new(bus: Bus, client: Client, rss_cfg: RssCfg, shutdown: CancellationToken) -> RssActor {
        Self { bus, client, rss_cfg, shutdown }
    }
}

#[async_trait::async_trait]
impl Actor for RssActor {
    async fn run(mut self) -> Result<()> {
        info!("RssActor started");
        
        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("RssActor: shutdown requested");
                    break;
                }
            }
        }
        info!("RssActor stopped cleanly");
        Ok(())
    }
}