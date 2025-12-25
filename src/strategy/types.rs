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

#[derive(Debug, Clone, Default)]
pub struct NormalizedText {
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Entities {
    // people, orgs, countries, tickers, etc.
}

#[derive(Debug, Clone, Default)]
pub struct TimeWindow {
    // parsed time range, e.g., (start, end)
}

#[derive(Debug, Clone, Default)]
pub struct RawCandidate {
    pub market_id: String,
    pub bm25_score: f32,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub resolution_date: Option<i64>, // timestamp in seconds
}

#[derive(Debug, Clone, Default)]
pub struct ScoreComponents {
    pub bm25_norm: f64,
    pub entity_overlap: f64,
    pub number_overlap: f64,
    pub time_compat: f64,
    pub liquidity_score: f64,
    pub staleness_penalty: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ScoredCandidate {
    pub candidate: RawCandidate,
    pub score: f64,
    pub components: ScoreComponents,
}

#[derive(Debug, Clone, Default)]
pub struct ProbabilisticCandidate {
    pub candidate: RawCandidate,
    pub score: f64,
    pub probability: f64,
}

#[derive(Debug, Clone, Default)]
pub struct EdgedCandidate {
    pub candidate: RawCandidate,
    pub score: f64,
    pub probability: f64,
    pub market_price: f64,
    pub edge: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TradeSide {
    BuyYes,
    BuyNo,
}

impl Default for TradeSide {
    fn default() -> Self {
        Self::BuyYes
    }
}

#[derive(Debug, Clone, Default)]
pub struct SizedDecision {
    pub candidate: EdgedCandidate,
    pub side: TradeSide,
    pub kelly_fraction: f64,
    pub size_fraction: f64,
}
