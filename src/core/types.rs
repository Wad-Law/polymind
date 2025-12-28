use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[async_trait::async_trait]
pub trait Actor: Send + Sync + 'static {
    async fn run(self) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SystemStatus {
    Active,
    Halted(String), // Reason
}

// ----------- Domain messages -----------------
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawNews {
    #[allow(dead_code)]
    pub url: String,
    pub title: String,
    pub description: String,
    #[allow(dead_code)]
    pub feed: String,
    #[allow(dead_code)]
    pub published: Option<chrono::DateTime<chrono::Utc>>,
    #[allow(dead_code)]
    pub labels: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MarketDataRequest {
    pub market_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketToken {
    pub token_id: String,
    pub outcome: String, // "Yes", "No"
    pub price: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataSnap {
    pub market_id: String,
    pub book_ts_ms: i64,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    pub bid_size: Decimal,
    pub ask_size: Decimal,
    #[serde(default)]
    pub tokens: Option<Vec<MarketToken>>,
    #[serde(default)]
    pub question: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Copy, Serialize, Deserialize)]
pub enum Side {
    Buy,
    #[allow(dead_code)]
    Sell,
}

#[derive(Clone, Debug)]
pub struct Order {
    pub client_order_id: String,
    pub market_id: String,
    pub token_id: Option<String>, // Specific token to trade
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
}

#[derive(Clone, Debug)]
pub struct Execution {
    #[allow(dead_code)]
    pub exchange_order_id: Option<String>,
    #[allow(dead_code)]
    pub client_order_id: String,
    #[allow(dead_code)]
    pub market_id: String,
    #[allow(dead_code)]
    pub token_id: Option<String>,
    #[allow(dead_code)]
    pub side: Side,
    #[allow(dead_code)]
    pub avg_px: Decimal,
    #[allow(dead_code)]
    pub filled: Decimal,
    #[allow(dead_code)]
    pub fee: Decimal,
    #[allow(dead_code)]
    pub ts_ms: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PolyMarketEvent {
    #[allow(dead_code)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub markets: Option<Vec<PolyMarketMarket>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PolyMarketMarket {
    pub id: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(default, rename = "endDate")]
    pub end_date: Option<String>,
    #[serde(default, rename = "clobTokenIds")]
    pub clob_token_ids: Option<String>,
    #[serde(default)]
    pub outcomes: Option<String>,
    #[serde(default, rename = "outcomePrices")]
    pub outcome_prices: Option<String>,
}

impl PolyMarketMarket {
    pub fn get_tokens(&self) -> Vec<MarketToken> {
        let mut tokens_vec = Vec::new();

        if let (Some(ids_str), Some(outcomes_str), Some(prices_str)) =
            (&self.clob_token_ids, &self.outcomes, &self.outcome_prices)
        {
            // Try parsing and log errors if any
            let ids: Vec<String> = match serde_json::from_str(ids_str) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to parse clobTokenIds: '{}' - Error: {}", ids_str, e);
                    return Vec::new(); // Return empty if critical ID parsing fails
                }
            };

            let outcomes: Vec<String> = match serde_json::from_str(outcomes_str) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse outcomes: '{}' - Error: {}",
                        outcomes_str,
                        e
                    );
                    Vec::new()
                }
            };

            let prices: Vec<String> = match serde_json::from_str(prices_str) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse outcomePrices: '{}' - Error: {}",
                        prices_str,
                        e
                    );
                    Vec::new()
                }
            };

            for i in 0..ids.len() {
                if i < outcomes.len() && i < prices.len() {
                    let price = rust_decimal::Decimal::from_str_exact(&prices[i])
                        .unwrap_or(rust_decimal::Decimal::ZERO);

                    tokens_vec.push(MarketToken {
                        token_id: ids[i].clone(),
                        outcome: outcomes[i].clone(),
                        price,
                    });
                }
            }
        }
        tokens_vec
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Position {
    pub market_id: String,
    pub token_id: String,
    pub side: Side,
    pub quantity: Decimal,
    pub avg_entry_price: Decimal,
    pub current_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub last_updated_ts: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub cash: Decimal,
    pub ts: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub positions: Vec<Position>,
    pub timestamp: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Portfolio {
    pub positions: std::collections::HashMap<String, Position>, // key: token_id usually, or market_id+token_id
    pub cash: Decimal,
    pub total_equity: Decimal,
}

impl Portfolio {
    pub fn update_from_execution(
        &mut self,
        execution: &crate::core::types::Execution,
        token_id: &str,
    ) -> Option<Position> {
        // If we don't track cash here fully yet, we focus on Position update.
        // Assuming execution side matches position side for "opening".
        // If "closing", we reduce quantity.

        // Simple assumption:
        // Buy = Open/Increase Long
        // Sell = Close/Decrease Long (assuming we are long-only on these binary tokens for now)
        // Or if we support Shorting? Polymarket is usually Buy Yes / Buy No (which are both long variations).
        // Sending "Sell" usually means selling the token we own.

        let position = self
            .positions
            .entry(token_id.to_string())
            .or_insert(Position {
                market_id: execution.market_id.clone(),
                token_id: token_id.to_string(),
                side: execution.side,
                quantity: Decimal::ZERO,
                avg_entry_price: Decimal::ZERO,
                current_price: execution.avg_px, // Set current to execution price
                unrealized_pnl: Decimal::ZERO,
                last_updated_ts: execution.ts_ms,
            });

        match execution.side {
            Side::Buy => {
                // Weighted Average Entry Price
                // New Avg = ((OldQty * OldAvg) + (FillQty * FillPx)) / (OldQty + FillQty)
                let total_cost = (position.quantity * position.avg_entry_price)
                    + (execution.filled * execution.avg_px);
                let new_qty = position.quantity + execution.filled;

                if new_qty > Decimal::ZERO {
                    position.avg_entry_price = total_cost / new_qty;
                }
                position.quantity = new_qty;
            }
            Side::Sell => {
                // Selling reduces quantity but doesn't change Avg Entry Price (FIFO/Weighted assumption)
                // Realized PnL would be calculated here (Difference between Exit Price and Avg Entry).
                // position.quantity -= execution.filled;
                // If we go negative? Should be guarded.

                // PnL = (ExitPx - EntryPx) * Qty
                // We don't store RealizedPnL in Position struct (it's "Unrealized").
                // So just reduce Qty.
                position.quantity -= execution.filled;
                if position.quantity < Decimal::ZERO {
                    // We went short? Or error?
                    // For now allow negative to indicate short or error state
                }
            }
        }

        position.current_price = execution.avg_px; // Mark to market with latest fill?
        position.last_updated_ts = execution.ts_ms;

        Some(position.clone())
    }
}
