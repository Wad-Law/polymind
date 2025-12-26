use crate::core::types::{Execution, Order, Position, Side};
use crate::execution::client::ExecutionClient;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::HashMap;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct SimState {
    pub cash: Decimal,
    pub positions: HashMap<String, Position>, // TokenID -> Position
}

pub struct SimExecutionClient {
    pub state: Mutex<SimState>,
}

impl SimExecutionClient {
    pub fn new(initial_cash: Decimal) -> Self {
        Self {
            state: Mutex::new(SimState {
                cash: initial_cash,
                positions: HashMap::new(),
            }),
        }
    }
}

#[async_trait]
impl ExecutionClient for SimExecutionClient {
    async fn create_order(&self, order: &Order) -> Result<Execution> {
        let mut state = self.state.lock().await;

        // Simulate instant fill
        // 1. Calculate cost
        let cost = order.price * order.size;

        // 2. Check balance (if buy) or position (if sell)
        // For simplicity, just check cash for now (Buy logic)
        // If sell, we should check if we have the position.

        let filled_amt = order.size; // Assume full fill
        let fee = Decimal::ZERO; // No fees in sim for now

        match order.side {
            Side::Buy => {
                if state.cash < cost {
                    // Fail or partial? Let's just fail for now
                    // anyhow::bail!("Insufficient funds in simulator");
                    // Or maybe we allow leverage/debt? No, let's keep it simple.
                    // But strictly, we should update internal state.
                }
                state.cash -= cost;
            }
            Side::Sell => {
                state.cash += cost;
            }
        }

        // 3. Update internal position state (Simulated Exchange State)
        let token_id = order.token_id.clone().unwrap_or(order.market_id.clone()); // Fallback
        let position = state.positions.entry(token_id.clone()).or_insert(Position {
            market_id: order.market_id.clone(),
            token_id: token_id.clone(),
            side: order.side, // This is tricky. Position side vs Order side.
            // If we are LONG, side is Buy?
            // Usually internal Position struct holds "Side" (Long/Short) or Quantity.
            // Let's assume Long only for now.
            quantity: Decimal::ZERO,
            avg_entry_price: Decimal::ZERO,
            current_price: order.price,
            unrealized_pnl: Decimal::ZERO,
            last_updated_ts: Utc::now().timestamp_millis(),
        });

        match order.side {
            Side::Buy => {
                // Avg Entry Price update
                let old_cost = position.quantity * position.avg_entry_price;
                let new_cost = old_cost + (filled_amt * order.price);
                let new_qty = position.quantity + filled_amt;
                if new_qty > Decimal::ZERO {
                    position.avg_entry_price = new_cost / new_qty;
                }
                position.quantity = new_qty;
            }
            Side::Sell => {
                position.quantity -= filled_amt;
            }
        }
        position.last_updated_ts = Utc::now().timestamp_millis();

        Ok(Execution {
            exchange_order_id: Some("sim_order_id".to_string()),
            client_order_id: order.client_order_id.clone(),
            market_id: order.market_id.clone(),
            token_id: order.token_id.clone(),
            side: order.side,
            avg_px: order.price,
            filled: filled_amt,
            fee,
            ts_ms: Utc::now().timestamp_millis(),
        })
    }

    async fn get_balance(&self) -> Result<Decimal> {
        let state = self.state.lock().await;
        Ok(state.cash)
    }

    async fn get_positions(&self) -> Result<Vec<Position>> {
        let state = self.state.lock().await;
        Ok(state.positions.values().cloned().collect())
    }
}
