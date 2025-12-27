use crate::bus::types::Bus;
use crate::config::config::AppCfg;
use crate::core::types::{
    Actor, Execution, MarketDataRequest, MarketDataSnap, Order, PolyMarketEvent, Portfolio, RawNews,
};
use crate::strategy::analyst::MarketAnalyst;
use crate::strategy::event_features::{EventFeatureExtractor, FeatureDictionaries};
use crate::strategy::exact_duplicate_detector::{
    ExactDuplicateDetector, ExactDuplicateDetectorConfig,
};
use crate::strategy::hard_filters::HardFilterer;
use crate::strategy::kelly::KellySizer;
use crate::strategy::market_index::MarketIndex;
use crate::strategy::sim_hash_cache::{SimHashCache, SimHashCacheConfig};
use crate::strategy::tokenization::{TokenizationConfig, TokenizedNews};
use crate::strategy::types::*;
use anyhow::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use crate::llm::LlmClient;
use crate::persistence::database::Database;

pub struct StrategyActor {
    pub bus: Bus,
    pub shutdown: CancellationToken,
    pub detector: ExactDuplicateDetector,
    pub sim_hash_cache: SimHashCache,
    pub event_feature_extractor: EventFeatureExtractor,
    pub market_index: MarketIndex,
    pub hard_filterer: HardFilterer,
    pub kelly_sizer: KellySizer,
    pub market_data_cache: HashMap<String, MarketDataSnap>,
    pub bankroll: Decimal,
    pub analyst: MarketAnalyst,
    pub db: Database,
    pub portfolio: Portfolio,
    pub status: crate::core::types::SystemStatus,
    pub top_candidates: usize,
    pub tokenization_config: TokenizationConfig,
}

impl StrategyActor {
    pub fn new(bus: Bus, shutdown: CancellationToken, cfg: &AppCfg, db: Database) -> StrategyActor {
        Self {
            bus,
            shutdown,
            detector: ExactDuplicateDetector::new(ExactDuplicateDetectorConfig::default()),
            sim_hash_cache: SimHashCache::new(SimHashCacheConfig::default()),
            event_feature_extractor: EventFeatureExtractor::new(
                FeatureDictionaries::default_minimal(),
            ),
            market_index: MarketIndex::new().expect("Failed to initialize MarketIndex"),
            hard_filterer: HardFilterer::new(),
            kelly_sizer: KellySizer::default(),
            market_data_cache: HashMap::new(),
            bankroll: Decimal::from_f64(cfg.strategy.sim_bankroll).unwrap_or(Decimal::ZERO),
            analyst: MarketAnalyst::new(
                LlmClient::new(cfg.llm.clone()),
                db.clone(),
                cfg.strategy.top_candidates,
            ),
            db,
            portfolio: Portfolio {
                positions: HashMap::new(),
                cash: Decimal::from_f64(cfg.strategy.sim_bankroll).unwrap_or(Decimal::ZERO), // Init cash from config
                total_equity: Decimal::ZERO,
            },
            status: crate::core::types::SystemStatus::Active,
            top_candidates: cfg.strategy.top_candidates,
            tokenization_config: TokenizationConfig::default(),
        }
    }

    fn decide_from_tick(&mut self, snap: &MarketDataSnap) -> Option<Order> {
        self.market_data_cache
            .insert(snap.market_id.clone(), snap.clone());
        // High-level responsibilities:
        // - Update internal view of prices/PNL (later via Monitoring/Risk).
        // - Possibly adjust / rebalance positions:
        //   * Take profit if price converged to belief.
        //   * Cut risk if edge has flipped.
        //   * Manage time-based exits as resolution approaches.
        //
        // For now: delegate to a placeholder:
        None
    }

    // ============================
    // Pipeline: News / Events
    // ============================

    /// Core pipeline for a fresh news item:
    /// 1) Check for halt
    /// 2) Exact Dedup based on raw news event
    /// 2) Normalize + tokenize
    /// 3) Semantic dedup
    /// 4) Entity & date extraction
    /// 5) Candidate generation using lexical and semantic lookup
    /// 6) Hard filters
    /// 7) llm scoring
    /// 8) Kelly sizing + risk caps
    /// 9) Build orders
    async fn handle_news_event(&mut self, raw_news: &RawNews) -> Vec<Order> {
        let order = Vec::new();

        // 1. Check for Halt
        if let crate::core::types::SystemStatus::Halted(reason) = &self.status {
            warn!("Skipping news processing due to HALT: {}", reason);
            return order;
        }

        // 2. Cheap exact dedup to eliminate trivial duplicates
        if self.detector.is_duplicate(raw_news) {
            return order; // Return empty if dup
        } else {
            info!("New event — continue pipeline.");
        }

        // Persist Event (fire and forget / log error)
        let event_db_id = match self.db.save_event(raw_news).await {
            Ok(id) => Some(id),
            Err(e) => {
                error!("Failed to save event to DB: {}", e);
                None
            }
        };

        // 2. Tokenize
        let tokenized_news = TokenizedNews::from_raw(raw_news.clone(), &self.tokenization_config);

        // 3. Semantic dedup (SimHash) to eliminate rewritten versions
        let h = self.sim_hash_cache.sim_hash(&tokenized_news.tokens);
        if self.sim_hash_cache.is_near_duplicate(h) {
            info!("Near-duplicate news skipped (SimHash).");
            return order;
        }
        self.sim_hash_cache.insert(h);

        // 4. lexical and semantic using hard coded rules => semantic is generated from lexical
        let now = Utc::now();
        let feat = self.event_feature_extractor.extract(&tokenized_news, now); // Lexical layer (layers, numbers, time window)

        // 5. Candidate generation with Hybrid Search (BM25 + Semantic)
        let raw_candidates =
            self.retrieve_candidates(tokenized_news.tokens.as_slice(), &raw_news.title);

        if raw_candidates.is_empty() {
            warn!("No candidates found for news: ({}). Skipping.", raw_news.title);
            return order;
        }

        // 6. Hard filters
        let entities = &feat.entities;
        let time_window = &feat.time_window;
        let filtered_candidates = self
            .hard_filterer
            .apply(raw_candidates, entities, time_window);

        // Take top N candidates for ensure_market_data optimization
        let top_candidates_for_data = filtered_candidates
            .iter()
            .take(self.top_candidates)
            .map(|c| c.clone())
            .collect::<Vec<_>>();

        info!(
            "Top {} candidates after filtering: {:?}",
            top_candidates_for_data.len(),
            top_candidates_for_data
                .iter()
                .map(|c| &c.market_id)
                .collect::<Vec<_>>()
        );

        // Ensure we have market data (Price + Question) for these candidates
        // Note: We need to convert back to RawCandidate for ensure_market_data or just pass cloned vec
        info!(
            "Ensuring market data for: {:?}",
            top_candidates_for_data
                .iter()
                .map(|c| &c.market_id)
                .collect::<Vec<_>>()
        );
        self.ensure_market_data(&top_candidates_for_data).await;

        // 7. Analyst (LLM Scoring)
        // Pass the candidates to analyst. Analyst will also limit to top_candidates internally,
        // but since we already filtered for data fetching efficiency, we pass what we have.
        // Analyst expects Vec<RawCandidate>.
        let edged_candidates = self
            .analyst
            .analyze_candidates(
                raw_news,
                filtered_candidates, // Pass all filtered, Analyst handles top_N logic
                &self.market_data_cache,
                event_db_id,
            )
            .await;

        // 8. Kelly Sizing
        let sized_decisions = self.kelly_sizer.size_positions(edged_candidates);

        // Persist Decisions
        for decision in &sized_decisions {
            if let Err(e) = self.db.save_decision(event_db_id, decision).await {
                error!("Failed to save decision: {}", e);
            }
        }

        // 9. Build Orders
        self.build_orders_from_sized_decisions(&sized_decisions)
            .await
    }

    fn retrieve_candidates(&mut self, tokens: &[String], raw_text: &str) -> Vec<RawCandidate> {
        let mut candidates = HashMap::new();

        // 1. BM25 Search
        if let Ok(results) = self.market_index.search(tokens, 50) {
            for c in results {
                candidates.insert(c.market_id.clone(), c);
            }
        } else {
            warn!("BM25 search failed");
        }

        // 2. Semantic Search
        if let Ok(results) = self.market_index.search_semantic(raw_text, 50) {
            for c in results {
                // If exists, we could merge scores or keep the one with higher score.
                // For now, just insert if not present (union).
                // Or maybe we want to boost score if found in both?
                // Let's just add unique ones.
                candidates.entry(c.market_id.clone()).or_insert(c);
            }
        } else {
            warn!("Semantic search failed");
        }

        candidates.into_values().collect()
    }

    async fn ensure_market_data(&mut self, candidates: &[RawCandidate]) {
        let missing_ids: Vec<String> = candidates
            .iter()
            .map(|c| c.market_id.clone())
            .filter(|id| !self.market_data_cache.contains_key(id))
            .collect();

        if missing_ids.is_empty() {
            return;
        }

        // Subscribe to market data updates *before* sending requests to avoid races
        let mut rx = self.bus.market_data.subscribe();

        for id in &missing_ids {
            let req = MarketDataRequest {
                market_id: id.clone(),
            };
            if let Err(e) = self.bus.market_data_request.publish(req).await {
                error!("Failed to publish market data request: {}", e);
            }
        }

        // Wait for data with a timeout
        let timeout = tokio::time::sleep(std::time::Duration::from_millis(2000));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    warn!("Timeout waiting for market data");
                    break;
                }
                res = rx.recv() => {
                    match res {
                        Ok(snap) => {
                            self.market_data_cache.insert(snap.market_id.clone(), (*snap).clone());
                            if missing_ids.iter().all(|id| self.market_data_cache.contains_key(id)) {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    }

    async fn build_orders_from_sized_decisions(&self, sized: &[SizedDecision]) -> Vec<Order> {
        let mut orders = Vec::new();
        // Use configured bankroll
        let bankroll = self.bankroll;

        for decision in sized {
            if decision.size_fraction <= Decimal::ZERO {
                continue;
            }

            // Calculate quantity based on bankroll and price
            // size_fraction is the fraction of bankroll to risk/invest
            // quantity = (bankroll * size_fraction) / price
            let price = decision.candidate.market_price;
            if price <= Decimal::ZERO {
                continue;
            }

            let quantity = (bankroll * decision.size_fraction) / price;

            // Generate a simple client order ID
            let client_order_id = format!(
                "{}-{}",
                decision.candidate.candidate.market_id,
                Utc::now().timestamp_micros()
            );

            // Resolve token_id
            let mut token_id = None;
            if let Some(snap) = self
                .market_data_cache
                .get(&decision.candidate.candidate.market_id)
            {
                if let Some(tokens) = &snap.tokens {
                    let target_outcome = match &decision.side {
                        TradeSide::Buy(outcome) => outcome,
                    };

                    // Case-insensitive match + Robust check (e.g. "Yes" vs "True")
                    // For now, strict case-insensitive match on the outcome label.
                    if let Some(t) = tokens
                        .iter()
                        .find(|t| t.outcome.eq_ignore_ascii_case(target_outcome))
                    {
                        token_id = Some(t.token_id.clone());
                    } else {
                        // Fallback: Log available outcomes to help debug multi-choice issues
                        let available: Vec<String> = tokens.iter().map(|t| t.outcome.clone()).collect();
                        warn!(
                            "Token ID not found for outcome '{}' in market {}. Available: {:?}",
                            target_outcome, decision.candidate.candidate.market_id, available
                        );
                    }
                }
            }

            let order = Order {
                client_order_id,
                market_id: decision.candidate.candidate.market_id.clone(),
                token_id,
                side: crate::core::types::Side::Buy, // Always Buy side for the specific token
                price,
                size: quantity,
            };

            // Persist Order
            if let Err(e) = self.db.save_order(&order).await {
                error!("Failed to save order: {}", e);
            }

            orders.push(order);
        }
        orders
    }

    #[allow(dead_code)]
    fn map_poly_event_to_news(&self, _evt: &PolyMarketEvent) -> RawNews {
        // TODO: convert PolyMarketEvent into a RawNews-like structure
        // Manually constructing RawNews since it doesn't implement Default
        RawNews {
            url: "".to_string(),
            title: "placeholder".to_string(),
            description: "".to_string(),
            feed: "polymarket".to_string(),
            published: Some(Utc::now()),
            labels: Vec::new(),
        }
    }

    async fn decide_from_news(&mut self, news: &RawNews) -> Option<Order> {
        // Full matching + decision pipeline for generic news.
        self.handle_news_event(news).await.into_iter().next()
    }

    async fn decide_from_poly_event(&mut self, event: &PolyMarketEvent) -> Option<Order> {
        if let Some(markets) = &event.markets {
            for market in markets {
                // Persist Market
                if let Err(e) = self.db.save_market(&market).await {
                    error!("Failed to save market {}: {}", market.id, e);
                }

                if market.closed {
                    // Remove from index if closed
                    if let Err(e) = self.market_index.delete_market(&market.id) {
                        error!("Failed to delete closed market {}: {}", market.id, e);
                    } else {
                        info!("MarketIndex: Removed closed market {}", market.id);
                    }
                } else {
                    let question = market
                        .question
                        .as_deref()
                        .or(event.title.as_deref())
                        .unwrap_or("");
                    let description = market
                        .description
                        .as_deref()
                        .or(event.description.as_deref())
                        .unwrap_or("");

                    if !question.is_empty() {
                        if let Err(e) =
                            self.market_index
                                .add_market(&market.id, question, description, "", None)
                        {
                            error!("Failed to index market {}: {}", market.id, e);
                        } else {
                            info!("MarketIndex: Added/Updated market {}", market.id);
                        }
                    }
                }
            }
        }
        None
    }

    async fn decide_from_executions(&mut self, execution: &Execution) -> Option<Order> {
        info!("StrategyActor received execution: {:?}", execution);

        // 1. Persist Execution
        if let Err(e) = self.db.save_execution(execution).await {
            error!("Failed to save execution: {}", e);
        }

        // 2. Update Portfolio
        // We need a token_id to uniquely identify the position.
        // The Execution may not have it if it comes from exchange drop copy generically,
        // but our Execution struct doesn't have it either (it has market_id).
        // However, standard Polymarket positions are on Token IDs (ERC1155).
        // Since we don't have token_id in Execution msg, we have to look it up or assume logic.
        // PROVISIONAL: We will assume we can derive or find it.
        // For now, let's try to match with an open order if we had one?
        // Or simpler: Use "MarketID-Side" as a composite key if we only hold one token per side per market?
        // Actually, we stored `token_id` in `Order` table. We could query DB for the order by `client_order_id` to get `token_id`.
        // This causes a DB read per execution, but is safe.

        // For this step, I will leave a TODO and use market_id as fallback valid token_id.
        let token_id = execution
            .token_id
            .clone()
            .unwrap_or(execution.market_id.clone());

        if let Some(updated_pos) = self.portfolio.update_from_execution(execution, &token_id) {
            info!("Updated position for {}: {:?}", token_id, updated_pos);

            // 3. Persist Position
            if let Err(e) = self.db.upsert_position(&updated_pos).await {
                error!("Failed to upsert position: {}", e);
            }
        }

        None
    }

    fn reconcile_positions(&mut self, snap: &crate::core::types::PositionSnapshot) {
        info!(
            "Reconciling portfolio with {} external positions",
            snap.positions.len()
        );

        // 1. Mark all current positions as potentially stale (optional, or just clear and rebuild?)
        // Clearing and rebuilding is safer to remove "zombie" positions (internal yes, external no).
        // However, we lose "avg_entry_price" if the external API doesn't provide it nicely.
        // Polymarket API provides average entry price usually. The struct Position has it.
        // Assuming the snapshot Position has correct data.

        // Let's iterate and overwrite.
        // Also need to identify positions present in internal but NOT in snapshot (closed manually?).

        let mut present_token_ids = std::collections::HashSet::new();

        for ext_pos in &snap.positions {
            present_token_ids.insert(ext_pos.token_id.clone());

            // Check drift
            if let Some(int_pos) = self.portfolio.positions.get(&ext_pos.token_id) {
                if int_pos.quantity != ext_pos.quantity {
                    warn!(
                        "Position DRIFT for {}: Internal {} vs External {}. Adjusting.",
                        ext_pos.token_id, int_pos.quantity, ext_pos.quantity
                    );
                }
            } else {
                info!(
                    "New external position found during reconciliation: {}",
                    ext_pos.token_id
                );
            }

            // Overwrite
            self.portfolio
                .positions
                .insert(ext_pos.token_id.clone(), ext_pos.clone());
        }

        // Remove zombies
        let internal_ids: Vec<String> = self.portfolio.positions.keys().cloned().collect();
        for id in internal_ids {
            if !present_token_ids.contains(&id) {
                warn!("Removing ZOMBIE position {} (not on exchange)", id);
                self.portfolio.positions.remove(&id);
            }
        }
    }
}

#[async_trait::async_trait]
impl Actor for StrategyActor {
    async fn run(mut self) -> Result<()> {
        info!("StrategyActor started");

        // Load positions from DB
        match self.db.load_positions().await {
            Ok(positions) => {
                info!("Loaded {} positions from database", positions.len());
                for pos in positions {
                    self.portfolio.positions.insert(pos.token_id.clone(), pos);
                }
            }
            Err(e) => {
                error!("Failed to load positions from database: {}", e);
            }
        }

        // Hydrate Duplicate Detector
        match self.db.load_recent_events(10000).await {
            Ok(events) => {
                let count = events.len();
                self.detector.hydrate(events.clone());
                self.sim_hash_cache.hydrate(events, &self.tokenization_config);
                info!("Hydrated Deduplication Cache & SimHash with {} recent events", count);
            }
            Err(e) => {
                error!("Failed to load recent events for deduplication: {}", e);
            }
        }

        // Subscribe to both broadcast streams
        let mut md_rx = self.bus.market_data.subscribe();
        let mut poly_rx = self.bus.polymarket_events.subscribe();
        let mut news_rx = self.bus.raw_news.subscribe();
        let mut executions_rx = self.bus.executions.subscribe();
        let mut balance_rx = self.bus.balance.subscribe();
        let mut status_rx = self.bus.system_status.subscribe();
        let mut snapshot_rx = self.bus.positions_snapshot.subscribe();

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("StrategyActor: shutdown requested");
                    break;
                }

                // System Status
                res = status_rx.recv() => {
                    match res {
                        Ok(status) => {
                            info!("StrategyActor received system status update: {:?}", status);
                            self.status = (*status).clone();
                            if let crate::core::types::SystemStatus::Halted(reason) = &self.status {
                                warn!("StrategyActor HALTED: {}", reason);
                            }
                        }
                        Err(e) => error!("System status stream error: {}", e),
                    }
                }

                // Position Reconciliation
                res = snapshot_rx.recv() => {
                    match res {
                        Ok(snap) => {
                            self.reconcile_positions(&snap);
                        }
                        Err(e) => error!("Position snapshot stream error: {}", e),
                    }
                }

                // Market data path
                res = md_rx.recv() => {
                    match res {
                        Ok(snap) => {
                            if let Some(order) = self.decide_from_tick(&snap) {
                                // Publish order to orders topic
                                self.bus.orders.publish(order).await?;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "StrategyActor lagged on market_data");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            error!("market_data stream closed; exiting StrategyActor");
                            break;
                        }
                    }
                }

                // News path
                res = news_rx.recv() => {
                    match res {
                        Ok(news) => {
                            if let Some(order) = self.decide_from_news(&news).await {
                                self.bus.orders.publish(order).await?;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "StrategyActor lagged on raw_news");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            error!("raw_news stream closed; exiting StrategyActor");
                            break;
                        }
                    }
                }

                // executions path
                res = executions_rx.recv() => {
                    match res {
                        Ok(executions) => {
                            if let Some(order) = self.decide_from_executions(&executions).await {
                                self.bus.orders.publish(order).await?;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "StrategyActor lagged on executions");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            error!("executions stream closed; exiting StrategyActor");
                            break;
                        }
                    }
                }

                // Polymarket events path
                res = poly_rx.recv() => {
                    match res {
                        Ok(event) => {
                            if let Some(order) = self.decide_from_poly_event(&event).await {
                                self.bus.orders.publish(order).await?;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "StrategyActor lagged on polymarket_events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            error!("polymarket_events stream closed; exiting StrategyActor");
                            break;
                        }
                    }
                }

                // Balance updates
                res = balance_rx.recv() => {
                    match res {
                        Ok(update) => {
                            info!("StrategyActor received balance update: {} USDC", update.cash);
                            self.portfolio.cash = update.cash;
                            self.bankroll = update.cash; // Update detailed bankroll too? Or keep separate?
                            // StrategyCfg has bankroll, but we want dynamic.
                            // Assuming self.bankroll is what we use for sizing.
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, "StrategyActor lagged on balance updates");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            error!("balance stream closed");
                            // Non-critical, continue
                        }
                    }
                }
            }
        }
        info!("StrategyActor stopped cleanly");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // use crate::config::config::CalibrationCfg;
    use crate::core::types::MarketToken;
    use crate::strategy::exact_duplicate_detector::ExactDuplicateDetectorConfig;
    use crate::strategy::sim_hash_cache::SimHashCacheConfig;
    use crate::strategy::types::{EdgedCandidate, RawCandidate, SizedDecision, TradeSide};

    #[tokio::test]
    #[ignore] // Skip test requiring Postgres connection
    async fn test_token_id_resolution() {
        // Setup minimal actor
        // Note: This test now requires a running Postgres instance.
        // Use `cargo test --ignored` if you have one running and DATABASE_URL set.
        let db_url = std::env::var("DATABASE_URL")
            .unwrap_or("postgres://user:pass@localhost:5432/db".to_string());
        let db = match crate::persistence::database::Database::new(&db_url).await {
            Ok(d) => d,
            Err(_) => return, // Skip if no DB
        };

        let mut actor = StrategyActor {
            bus: Bus::new(),
            shutdown: CancellationToken::new(),
            detector: ExactDuplicateDetector::new(ExactDuplicateDetectorConfig::default()),
            sim_hash_cache: SimHashCache::new(SimHashCacheConfig::default()),
            event_feature_extractor: EventFeatureExtractor::new(
                FeatureDictionaries::default_minimal(),
            ),
            market_index: MarketIndex::new().unwrap(),
            hard_filterer: HardFilterer::new(),
            // scorer: Scorer::new(),
            // calibrator: ProbabilityCalibrator::new(CalibrationCfg::default()),
            kelly_sizer: KellySizer::default(),
            market_data_cache: HashMap::new(),
            bankroll: Decimal::from_f64(1000.0).unwrap(),
            analyst: MarketAnalyst::new(
                LlmClient::new(crate::config::config::LlmCfg::default()),
                db.clone(),
                5,
            ),
            db,
            top_candidates: 5,
            portfolio: Portfolio::default(),
            status: crate::core::types::SystemStatus::Active,
            tokenization_config: TokenizationConfig::default(),
        };

        // Mock Market Data
        let market_id = "123456";
        let yes_token = "token_yes_123";
        let no_token = "token_no_123";

        let snap = MarketDataSnap {
            market_id: market_id.to_string(),
            book_ts_ms: 0,
            best_bid: Decimal::new(5, 1),   // 0.5
            best_ask: Decimal::new(6, 1),   // 0.6
            bid_size: Decimal::new(100, 0), // 100.0
            ask_size: Decimal::new(100, 0), // 100.0
            tokens: Some(vec![
                MarketToken {
                    token_id: yes_token.to_string(),
                    outcome: "Yes".to_string(),
                    price: Decimal::new(55, 2), // 0.55
                },
                MarketToken {
                    token_id: no_token.to_string(),
                    outcome: "No".to_string(),
                    price: Decimal::new(45, 2), // 0.45
                },
            ]),
            question: "Will the Fed hike rates?".to_string(),
        };
        actor.market_data_cache.insert(market_id.to_string(), snap);

        // Test BuyYes
        let decision_yes = SizedDecision {
            candidate: EdgedCandidate {
                candidate: RawCandidate {
                    market_id: market_id.to_string(),
                    ..Default::default()
                },
                score: Decimal::new(8, 1),         // 0.8
                probability: Decimal::new(7, 1),   // 0.7
                market_price: Decimal::new(55, 2), // 0.55
                edge: Decimal::new(15, 2),         // 0.15
            },
            kelly_fraction: Decimal::new(1, 1), // 0.1
            size_fraction: Decimal::new(1, 1),  // 0.1
            side: TradeSide::BuyYes,
        };

        let orders_yes = actor
            .build_orders_from_sized_decisions(&[decision_yes])
            .await;
        assert_eq!(orders_yes.len(), 1);
        assert_eq!(orders_yes[0].token_id, Some(yes_token.to_string()));

        // Test BuyNo
        let decision_no = SizedDecision {
            candidate: EdgedCandidate {
                candidate: RawCandidate {
                    market_id: market_id.to_string(),
                    ..Default::default()
                },
                score: Decimal::new(8, 1),         // 0.8
                probability: Decimal::new(3, 1),   // 0.3
                market_price: Decimal::new(45, 2), // 0.45
                edge: Decimal::new(15, 2),         // 0.15
            },
            kelly_fraction: Decimal::new(1, 1), // 0.1
            size_fraction: Decimal::new(1, 1),  // 0.1
            side: TradeSide::Buy("No".to_string()),
        };

        let orders_no = actor
            .build_orders_from_sized_decisions(&[decision_no])
            .await;
        assert_eq!(orders_no.len(), 1);
        assert_eq!(orders_no[0].token_id, Some(no_token.to_string()));
    }
}
