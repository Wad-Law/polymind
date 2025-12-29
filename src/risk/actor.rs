use crate::bus::types::Bus;
use crate::core::types::{Actor, SystemStatus};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

pub struct RiskActor {
    bus: Bus,
    shutdown: CancellationToken,

    // Config
    max_daily_loss_pct: Decimal,
    max_drawdown_pct: Decimal,

    // State
    initial_balance: Option<Decimal>,
    peak_balance: Option<Decimal>,
    current_balance: Decimal,

    // Status
    status: SystemStatus,
}

impl RiskActor {
    pub fn new(bus: Bus, shutdown: CancellationToken) -> Self {
        Self {
            bus,
            shutdown,
            max_daily_loss_pct: Decimal::new(5, 2), // 0.05 = 5%
            max_drawdown_pct: Decimal::new(10, 2),  // 0.10 = 10%
            initial_balance: None,
            peak_balance: None,
            current_balance: Decimal::ZERO,
            status: SystemStatus::Active,
        }
    }

    #[tracing::instrument(skip(self))]
    async fn check_risk(&mut self) -> Result<()> {
        if let SystemStatus::Halted(_) = self.status {
            return Ok(()); // Already halted
        }

        // 1. Check Max Drawdown from Peak
        if let Some(peak) = self.peak_balance {
            if self.current_balance < peak {
                let drawdown = (peak - self.current_balance) / peak;
                if drawdown > self.max_drawdown_pct {
                    let reason = format!(
                        "Max Drawdown exceeded: {:.2}% > {:.2}%",
                        drawdown * Decimal::from(100),
                        self.max_drawdown_pct * Decimal::from(100)
                    );
                    metrics::gauge!("risk_drawdown_pct").set(drawdown.to_f64().unwrap_or(0.0));
                    self.trigger_halt(&reason).await?;
                    return Ok(());
                }
                metrics::gauge!("risk_drawdown_pct").set(drawdown.to_f64().unwrap_or(0.0));
            }
        }

        // 2. Check Daily Loss (vs Initial)
        if let Some(initial) = self.initial_balance {
            if self.current_balance < initial {
                let loss = (initial - self.current_balance) / initial;
                if loss > self.max_daily_loss_pct {
                    let reason = format!(
                        "Daily Loss limit exceeded: {:.2}% > {:.2}%",
                        loss * Decimal::from(100),
                        self.max_daily_loss_pct * Decimal::from(100)
                    );
                    metrics::gauge!("risk_daily_loss_pct").set(loss.to_f64().unwrap_or(0.0));
                    self.trigger_halt(&reason).await?;
                    return Ok(());
                }
                metrics::gauge!("risk_daily_loss_pct").set(loss.to_f64().unwrap_or(0.0));
            }
        }

        metrics::counter!("risk_checks_total", "status" => "ok").increment(1);

        Ok(())
    }

    async fn trigger_halt(&mut self, reason: &str) -> Result<()> {
        metrics::counter!("risk_checks_total", "status" => "halted").increment(1);
        warn!("RISK HALT TRIGGERED: {}", reason);
        self.status = SystemStatus::Halted(reason.to_string());
        self.bus.system_status.publish(self.status.clone()).await?;
        Ok(())
    }
}

#[async_trait]
impl Actor for RiskActor {
    async fn run(mut self) -> Result<()> {
        info!("RiskActor started");

        // Subscribe to relevant topics
        let mut balance_rx = self.bus.balance.subscribe();
        let mut executions_rx = self.bus.executions.subscribe();

        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => {
                    info!("RiskActor: shutdown requested");
                    break;
                }

                // Monitor Balance
                res = balance_rx.recv() => {
                    match res {
                        Ok(update) => {
                            self.current_balance = update.cash;

                            // Set initial balance if first time
                            if self.initial_balance.is_none() {
                                self.initial_balance = Some(update.cash);
                                info!("RiskActor: Initial Balance set to {}", update.cash);
                            }

                            // Update peak balance
                            if self.peak_balance.map_or(true, |peak| update.cash > peak) {
                                self.peak_balance = Some(update.cash);
                            }

                            if let Err(e) = self.check_risk().await {
                                error!("RiskActor: Risk check failed: {}", e);
                            }
                        }
                        Err(e) => error!("RiskActor: Balance stream error: {:#}", e),
                    }
                }

                // Monitor Executions (just for logging exposure for now, or updating PnL estimation if needed)
                res = executions_rx.recv() => {
                     match res {
                        Ok(exec) => {
                            // In future: Track open positions exposure vs Bankroll
                            info!("RiskActor observed execution: {:?}", exec.client_order_id);
                        }
                        Err(e) => error!("RiskActor: Execution stream error: {}", e),
                    }
                }
            }
        }

        Ok(())
    }
}
