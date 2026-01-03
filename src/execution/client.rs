use crate::core::types::{Execution, Order, Position};
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

#[async_trait]
pub trait ExecutionClient: Send + Sync + 'static {
    async fn create_order(&self, order: &Order) -> Result<Execution>;
    async fn get_proxy_balance(&self) -> Result<Decimal>;
    async fn get_positions(&self) -> Result<Vec<Position>>;
}
