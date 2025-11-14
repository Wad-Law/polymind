use crate::bus::types::Bus;
use crate::core::types::{Actor};
use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::{info};
use reqwest::Client;
use crate::config::config::{FinJuiceCfg};

pub struct FinJuiceActor {
    pub bus: Bus,
    pub client: Client,
    pub cfg: FinJuiceCfg,
    pub shutdown: CancellationToken
}

impl FinJuiceActor {
    pub fn new(bus: Bus, client: Client, cfg: FinJuiceCfg, shutdown: CancellationToken) -> FinJuiceActor {
        Self { bus, client, cfg, shutdown }
    }
}

#[async_trait::async_trait]
impl Actor for FinJuiceActor {
    async fn run(mut self) -> Result<()> {
        info!("FinJuiceActor started");

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("FinJuiceActor: shutdown requested");
                    break;
                }
            }
        }
        info!("FinJuiceActor stopped cleanly");
        Ok(())
    }
}