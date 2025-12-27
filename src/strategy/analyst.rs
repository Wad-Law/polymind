use crate::core::types::{MarketDataSnap, RawNews};
use crate::llm::LlmClient;
use crate::persistence::database::Database;
use crate::strategy::types::{EdgedCandidate, RawCandidate};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use std::collections::HashMap;
use tracing::{error, info};

pub struct MarketAnalyst {
    llm_client: LlmClient,
    db: Database,
    top_candidates: usize,
}

impl MarketAnalyst {
    pub fn new(llm_client: LlmClient, db: Database, top_candidates: usize) -> Self {
        Self {
            llm_client,
            db,
            top_candidates,
        }
    }

    pub async fn analyze_candidates(
        &self,
        raw_news: &RawNews,
        candidates: Vec<RawCandidate>,
        market_data_cache: &HashMap<String, MarketDataSnap>,
        event_db_id: Option<i64>,
    ) -> Vec<EdgedCandidate> {
        let mut edged_candidates = Vec::new();

        // Take top N candidates for LLM analysis to save costs
        let top_candidates = candidates
            .into_iter()
            .take(self.top_candidates)
            .collect::<Vec<_>>();

        // Note: ensure_market_data must be called by the caller (StrategyActor) before calling this,
        // because Analyst doesn't have access to the bus to request data.

        for candidate in top_candidates {
            // ... (market question retrieval)
            let market_question = if let Some(snap) = market_data_cache.get(&candidate.market_id) {
                snap.question.clone()
            } else {
                "Unknown Market Question".to_string()
            };

            // Extract available outcomes
            let outcomes = if let Some(snap) = market_data_cache.get(&candidate.market_id) {
                if let Some(tokens) = &snap.tokens {
                    tokens.iter().map(|t| t.outcome.clone()).collect()
                } else {
                    vec!["Yes".to_string(), "No".to_string()]
                }
            } else {
                vec!["Yes".to_string(), "No".to_string()]
            };

            // Call LLM
            match self
                .llm_client
                .analyze(&raw_news.title, &market_question, &outcomes)
                .await
            {
                Ok((signal, prompt)) => {
                    info!("LLM Signal for {}: {:?}", candidate.market_id, signal);

                    // Persist Signal
                    if let Some(eid) = event_db_id {
                        if let Err(e) = self
                            .db
                            .save_signal(eid, &candidate.market_id, &signal, &prompt, self.llm_client.model())
                            .await
                        {
                            error!("Failed to save signal: {}", e);
                        }
                    }

                    // Convert signal to TradeSide and Probability
                    if signal.sentiment != "None" && outcomes.iter().any(|o| o.eq_ignore_ascii_case(&signal.sentiment)) {
                        let prob = Decimal::from_f64(0.5 + (signal.confidence * 0.5))
                                    .unwrap_or(Decimal::new(5, 1));
                        
                        // Get market price for the specific outcome
                        let market_price = if let Some(snap) = market_data_cache.get(&candidate.market_id) {
                            if let Some(tokens) = &snap.tokens {
                                tokens.iter()
                                    .find(|t| t.outcome.eq_ignore_ascii_case(&signal.sentiment))
                                    .map(|t| t.price)
                                    .unwrap_or(Decimal::new(5, 1))
                            } else {
                                Decimal::new(5, 1)
                            }
                        } else {
                             Decimal::new(5, 1)
                        };

                         if prob > Decimal::new(6, 1) {
                            let edged = EdgedCandidate {
                                candidate: candidate.clone(),
                                score: prob,
                                probability: prob,
                                market_price,
                                edge: prob - market_price,
                            };
                            edged_candidates.push(edged);
                        }
                    }
                }
                Err(e) => {
                    error!("LLM analysis failed for {}: {:?}", candidate.market_id, e);
                }
            }
        }
        edged_candidates
    }
}
