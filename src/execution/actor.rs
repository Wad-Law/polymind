use crate::bus::types::Bus;
use crate::core::types::Actor;
use crate::core::types::BalanceUpdate;
use anyhow::Result;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::execution::client::ExecutionClient;
use std::sync::Arc;

pub struct ExecutionActor {
    pub bus: Bus,
    pub shutdown: CancellationToken,
    pub client: Arc<dyn ExecutionClient>,
}

impl ExecutionActor {
    pub fn new(
        bus: Bus,
        shutdown: CancellationToken,
        client: Arc<dyn ExecutionClient>,
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

        // Initial proxy balance fetch
        match self.client.get_proxy_balance().await {
            Ok(bal) => {
                info!("Initial Bankroll: {} USDC", bal);
                let update = BalanceUpdate {
                    cash: bal,
                    ts: Utc::now().timestamp_millis(),
                };
                if let Err(e) = self.bus.balance.publish(update).await {
                    error!("Failed to publish initial balance: {:#}", e);
                }
            }
            Err(e) => {
                error!("Failed to fetch initial balance: {:#}", e);
            }
        }

        let mut rx = self.bus.orders.subscribe(); // broadcast::Receiver<Arc<MarketDataRequest>>
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                        info!("ExecutionActor: shutdown requested");
                        break;
                }

                // Periodic Reconciliation
                _ = interval.tick() => {
                    match self.client.get_positions().await {
                        Ok(positions) => {
                            metrics::counter!("execution_position_reconciliations_total").increment(1);
                            info!("Fetched {} positions for reconciliation", positions.len());
                            let snapshot = crate::core::types::PositionSnapshot {
                                positions,
                                timestamp: Utc::now().timestamp_millis(),
                            };
                            if let Err(e) = self.bus.positions_snapshot.publish(snapshot).await {
                                error!("Failed to publish position snapshot: {:#}", e);
                            }
                        }
                        Err(e) => error!("Failed to fetch positions: {:#}", e),
                    }
                }

                // Order requests
                // Order requests
                res = rx.recv() => {
                    match res {
                        Ok(req) => {
                            metrics::counter!("execution_orders_received_total").increment(1);
                            let order = req.as_ref();
                            info!("ExecutionActor received order: {:?}", order);

                            // Real execution via PolyExecutionClient
                            match self.client.create_order(order).await {
                                Ok(fill) => {
                                    metrics::counter!("execution_orders_filled_total").increment(1);
                                    info!("ExecutionActor executed fill: {:?}", fill);
                                    if let Err(_e) = self.bus.executions.publish(fill).await {
                                        error!("Failed to execute order: {:?}", _e);
                                    }
                                }
                                Err(_e) => {
                                    error!("Failed to execute order: {:?}", _e);
                                    // If execution fails, we still want to publish a "failed" fill (optional, or just log)
                                    // For now, let's just log as originally intended or use proper Execution struct if needed.
                                    // The type expected by bus.executions is Execution.
                                    /*
                                    let failed_execution = crate::core::types::Execution {
                                        client_order_id: order.client_order_id.clone(),
                                        market_id: order.market_id.clone(),
                                        avg_px: Decimal::ZERO,
                                        filled: Decimal::ZERO,
                                        fee: Decimal::ZERO,
                                        // Execution struct doesn't have 'side' field based on previous view?
                                        // Let's check core/types.rs content.
                                        // It has: client_order_id, market_id, avg_px, filled, fee, ts_ms.
                                        ts_ms: chrono::Utc::now().timestamp_millis(),
                                    };
                                    */
                                    // Actually, if it failed, maybe we shouldn't publish an execution?
                                    // The original code didn't. Let's revert the "failed fill" addition to avoid complicating types if not strictly required by the plan.
                                    // But if I want to keep it, I should use Execution.
                                    // Reverting the "failed fill" logic for now to keep it simple and fix the build.
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

    use rust_decimal::Decimal;
    use rust_decimal::prelude::FromPrimitive;
    use tokio::time::Duration; // For from_f64/from_str

    struct MockExecutionClient;
    #[async_trait::async_trait]
    impl ExecutionClient for MockExecutionClient {
        async fn create_order(&self, order: &Order) -> Result<crate::core::types::Execution> {
            Ok(crate::core::types::Execution {
                client_order_id: order.client_order_id.clone(),
                market_id: order.market_id.clone(),
                exchange_order_id: Some("mock-id".to_string()),
                token_id: Some("mock-token".to_string()),
                side: order.side,
                avg_px: order.price,
                filled: order.size,
                fee: Decimal::ZERO,
                ts_ms: chrono::Utc::now().timestamp_millis(),
            })
        }
        async fn get_proxy_balance(&self) -> Result<Decimal> {
            Ok(Decimal::from(1000))
        }
        async fn get_positions(&self) -> Result<Vec<crate::core::types::Position>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_execution_actor_flow() {
        let bus = Bus::new();
        let shutdown = CancellationToken::new();
        // Removed unused reqwest::Client

        let mock_client = Arc::new(MockExecutionClient);
        let actor = ExecutionActor::new(bus.clone(), shutdown.clone(), mock_client);

        // Spawn actor
        tokio::spawn(async move {
            actor.run().await.unwrap();
        });

        // Subscribe to executions
        let mut exec_rx = bus.executions.subscribe();
        let mut balance_rx = bus.balance.subscribe();

        // Give the actor a moment to start and publish initial balance
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify initial balance
        let bal_update = balance_rx.try_recv();
        assert!(bal_update.is_ok(), "Should receive initial balance update");

        // Publish Order
        let order = Order {
            client_order_id: "test-123".to_string(),
            market_id: "123456".to_string(),
            token_id: None,
            price: Decimal::from_f64(0.5).unwrap(),
            size: Decimal::from_f64(10.0).unwrap(),
            side: crate::core::types::Side::Buy,
        };
        bus.orders.publish(order.clone()).await.unwrap();

        // Wait for execution
        let res = tokio::time::timeout(Duration::from_secs(1), exec_rx.recv()).await;

        assert!(res.is_ok(), "Should receive execution from mock client");
        let execution = res.unwrap().unwrap();
        assert_eq!(execution.client_order_id, "test-123");

        // Shutdown
        shutdown.cancel();
    }
}
