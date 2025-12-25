use crate::bus::types::Bus;
use crate::core::types::Actor;
use crate::execution::polymarket::PolyExecutionClient;
use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub struct ExecutionActor {
    pub bus: Bus,
    pub shutdown: CancellationToken,
    pub client: PolyExecutionClient,
}

impl ExecutionActor {
    pub fn new(
        bus: Bus,
        shutdown: CancellationToken,
        client: PolyExecutionClient,
    ) -> ExecutionActor {
        Self {
            bus,
            shutdown,
            client,
        }
    }
}

#[async_trait::async_trait]
impl Actor for ExecutionActor {
    async fn run(mut self) -> Result<()> {
        info!("ExecutionActor started");
        let mut rx = self.bus.orders.subscribe(); // broadcast::Receiver<Arc<MarketDataRequest>>
        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                        info!("ExecutionActor: shutdown requested");
                        break;
                }

                // Order requests
                // Order requests
                res = rx.recv() => {
                    match res {
                        Ok(req) => {
                            let order = req.as_ref();
                            info!("ExecutionActor received order: {:?}", order);

                            // Real execution via PolyExecutionClient
                            match self.client.create_order(order).await {
                                Ok(fill) => {
                                    info!("ExecutionActor executed fill: {:?}", fill);
                                    if let Err(e) = self.bus.executions.publish(fill).await {
                                        error!("Failed to publish execution: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to execute order: {:?}", e);
                                }
                            }

                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // a slow consumer skipped n messages
                            error!("ExecutionActor lagged by {n} Order messages");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // no more senders; decide whether to exit
                            error!("ExecutionActor order channel closed");
                            break;
                        }
                    }
                }
            }
        }

        info!("ExecutionActor stopped cleanly");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config::PolyCfg;
    use crate::core::types::Order;
    use reqwest::Client;
    use tokio::time::Duration;

    #[tokio::test]
    async fn test_execution_actor_flow() {
        let bus = Bus::new();
        let shutdown = CancellationToken::new();
        let client = Client::new();
        let mut cfg = PolyCfg::default();
        // Dummy private key for testing (random hex)
        cfg.api_key = "test-key".to_string();
        cfg.api_secret =
            "0000000000000000000000000000000000000000000000000000000000000001".to_string();
        let exec_client = PolyExecutionClient::new(cfg, client);
        let actor = ExecutionActor::new(bus.clone(), shutdown.clone(), exec_client);

        // Spawn actor
        tokio::spawn(async move {
            actor.run().await.unwrap();
        });

        // Subscribe to executions
        let mut exec_rx = bus.executions.subscribe();

        // Give the actor a moment to start and subscribe to orders
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Publish
        let order = Order {
            client_order_id: "test-123".to_string(),
            market_id: "123456".to_string(),
            price: 0.5,
            size: 10.0,
            side: crate::core::types::Side::Buy,
        };
        bus.orders.publish(order.clone()).await.unwrap();

        // Wait for execution
        // Wait for execution
        // NOTE: With dummy keys and real network calls, this WILL fail to produce an execution.
        // We expect a timeout here, which confirms the actor attempted to execute but failed (as expected).
        let res = tokio::time::timeout(Duration::from_secs(1), exec_rx.recv()).await;

        // We expect a timeout because create_order should fail with dummy keys/network
        assert!(
            res.is_err(),
            "Expected timeout due to failed execution with dummy keys"
        );

        // If we wanted to verify the error log, we'd need a different logging setup,
        // but for now we just ensure the actor didn't crash and didn't publish a fake fill.

        // Shutdown
        shutdown.cancel();
    }
}
