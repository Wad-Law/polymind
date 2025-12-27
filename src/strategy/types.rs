// ============================
// Helper type outlines
// ============================

// Minimal placeholder types to express the pipeline stages.
// You can move these to a shared module later.

// ============================
// Helper type outlines
// ============================

// Minimal placeholder types to express the pipeline stages.
// You can move these to a shared module later.

use rust_decimal::Decimal;

#[derive(Debug, Clone, Default)]
pub struct RawCandidate {
    pub market_id: String,
    #[allow(dead_code)]
    pub bm25_score: f32,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub resolution_date: Option<i64>, // timestamp in seconds
}

#[derive(Debug, Clone, Default)]
pub struct EdgedCandidate {
    pub candidate: RawCandidate,
    #[allow(dead_code)]
    pub score: Decimal,
    pub probability: Decimal,
    pub market_price: Decimal,
    #[allow(dead_code)]
    pub edge: Decimal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TradeSide {
    Buy(String), // Specific Outcome to buy (e.g. "Yes", "No", "Trump")
}

impl Default for TradeSide {
    fn default() -> Self {
        Self::Buy("Yes".to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SizedDecision {
    pub candidate: EdgedCandidate,
    pub side: TradeSide,
    #[allow(dead_code)]
    pub kelly_fraction: Decimal,
    pub size_fraction: Decimal,
}
