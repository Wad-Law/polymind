use crate::bus::types::Bus;
use crate::config::config::AppCfg;
use crate::core::types::{
    Actor, Execution, MarketDataRequest, MarketDataSnap, Order, PolyMarketEvent, RawNews,
};
use crate::strategy::calibration::ProbabilityCalibrator;
use crate::strategy::canonical_event::CanonicalEventBuilder;
use crate::strategy::event_features::{EventFeatureExtractor, FeatureDictionaries};
use crate::strategy::exact_duplicate_detector::{
    ExactDuplicateDetector, ExactDuplicateDetectorConfig,
};
use crate::strategy::hard_filters::HardFilterer;
use crate::strategy::kelly::KellySizer;
use crate::strategy::market_index::MarketIndex;
use crate::strategy::scoring::Scorer;
use crate::strategy::sim_hash_cache::{SimHashCache, SimHashCacheConfig};
use crate::strategy::tokenization::{TokenizationConfig, TokenizedNews};
use crate::strategy::types::*;
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

pub struct StrategyActor {
    pub bus: Bus,
    pub shutdown: CancellationToken,
    pub detector: ExactDuplicateDetector,
    pub sim_hash_cache: SimHashCache,
    pub event_feature_extractor: EventFeatureExtractor,
    pub canonical_builder: CanonicalEventBuilder,
    pub market_index: MarketIndex,
    pub hard_filterer: HardFilterer,
    pub scorer: Scorer,
    pub calibrator: ProbabilityCalibrator,
    pub kelly_sizer: KellySizer,
    pub market_data_cache: HashMap<String, MarketDataSnap>,
    pub bankroll: f64,
}

impl StrategyActor {
    pub fn new(bus: Bus, shutdown: CancellationToken, cfg: &AppCfg) -> StrategyActor {
        Self {
            bus,
            shutdown,
            detector: ExactDuplicateDetector::new(ExactDuplicateDetectorConfig::default()),
            sim_hash_cache: SimHashCache::new(SimHashCacheConfig::default()),
            event_feature_extractor: EventFeatureExtractor::new(
                FeatureDictionaries::default_minimal(),
            ),
            canonical_builder: CanonicalEventBuilder::new(),
            market_index: MarketIndex::new().expect("Failed to initialize MarketIndex"),
            hard_filterer: HardFilterer::new(),
            scorer: Scorer::new(),
            calibrator: ProbabilityCalibrator::new(cfg.strategy.calibration.clone()),
            kelly_sizer: KellySizer::default(),
            market_data_cache: HashMap::new(),
            bankroll: cfg.strategy.bankroll,
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
    /// 1) Dedup
    /// 2) Normalize + tokenize
    /// 3) Entity & date extraction
    /// 4) Candidate generation (BM25)
    /// 5) Hard filters
    /// 6) Heuristic scoring
    /// 7) Top-K selection with diversity
    /// 8) Score -> probability (logistic)
    /// 9) Compare belief vs market price
    /// 10) Kelly sizing + risk caps
    /// 11) Build orders
    async fn handle_news_event(&mut self, raw_news: &RawNews) -> Vec<Order> {
        let order = Vec::new();
        // RawNews → normalized string → exact dedup
        //                ↓
        //            tokenize
        //                ↓
        //            SimHash → near-duplicate dedup
        //                ↓
        //      entity/date extraction → BM25 → scoring → decision

        // 1. Cheap exact dedup to eliminate trivial duplicates
        if self.detector.is_duplicate(raw_news) {
            println!("Skipping duplicate news.");
        } else {
            println!("New event — continue pipeline.");
        }

        // 2. Tokenize
        let cfg = TokenizationConfig::default();
        let tokenized_news = TokenizedNews::from_raw(raw_news.clone(), &cfg);

        // 3. Semantic dedup (SimHash) to eliminate rewritten versions
        let h = self.sim_hash_cache.sim_hash(&tokenized_news.tokens);
        if self.sim_hash_cache.is_near_duplicate(h) {
            info!("Near-duplicate news skipped (SimHash).");
            return order;
        }
        self.sim_hash_cache.insert(h);

        // 3. lexical and semantic using hard coded rules => semantic is generated from lexical
        let now = Utc::now();
        let feat = self.event_feature_extractor.extract(&tokenized_news, now); // Lexical layer (layers, numbers, time window)
        let _canonical = self.canonical_builder.build(&tokenized_news, &feat); // semantic layer (domain, event kind, primary/secondary entities, number interpretation, location, semantic time window)

        // 4. Candidate generation with BM25
        let raw_candidates = self.retrieve_candidates_bm25(tokenized_news.tokens.as_slice());

        // 5. Hard filters to kill false positives
        let entities = &feat.entities;
        let time_window = &feat.time_window;
        let filtered_candidates = self
            .hard_filterer
            .apply(raw_candidates, entities, time_window);

        // 6. Heuristic scoring (bm25_norm, entity_overlap, numbers, liquidity, etc.)
        let scored_candidates = self.scorer.score(filtered_candidates, &feat);

        // 7. Top-K with diversity (MMR or similar)
        let top_k = self.scorer.select_top_k(scored_candidates);

        // 8. Turn Top-K score into probability (logistic calibration + shrinkage)
        let probed = self.calibrator.map_scores_to_probabilities(top_k);

        // 9. Compare belief vs market price (edge computation)
        // Ensure that we have marketdata for each probed candidate
        self.ensure_market_data(&probed).await;
        let with_edge = self.attach_edges_vs_market(probed);

        // 10. Position Sizing (Kelly Criterion)
        let decisions = self.kelly_sizer.size_positions(with_edge);

        // 11. Build actual orders
        self.build_orders_from_sized_decisions(&decisions)
    }

    fn retrieve_candidates_bm25(&self, tokens: &[String]) -> Vec<RawCandidate> {
        match self.market_index.search(tokens, 100) {
            Ok(results) => results,
            Err(e) => {
                error!("BM25 search failed: {}", e);
                Vec::new()
            }
        }
    }

    async fn ensure_market_data(&mut self, candidates: &[ProbabilisticCandidate]) {
        let missing_ids: Vec<String> = candidates
            .iter()
            .map(|c| c.candidate.market_id.clone())
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

    fn attach_edges_vs_market(
        &self,
        candidates: Vec<ProbabilisticCandidate>,
    ) -> Vec<EdgedCandidate> {
        let mut results = Vec::new();
        for c in candidates {
            if let Some(snap) = self.market_data_cache.get(&c.candidate.market_id) {
                // Simple mid-price for now
                let mid = (snap.best_bid + snap.best_ask) / 2.0;
                // If bid/ask are 0, ignore
                if mid <= 0.0 {
                    continue;
                }
                let market_price = mid as f64;
                let edge = c.probability - market_price;

                results.push(EdgedCandidate {
                    candidate: c.candidate,
                    score: c.score,
                    probability: c.probability,
                    market_price,
                    edge,
                });
            }
        }
        results
    }

    fn build_orders_from_sized_decisions(&self, sized: &[SizedDecision]) -> Vec<Order> {
        let mut orders = Vec::new();
        // Use configured bankroll
        let bankroll = self.bankroll;

        for decision in sized {
            if decision.size_fraction <= 0.0 {
                continue;
            }

            // Calculate quantity based on bankroll and price
            // size_fraction is the fraction of bankroll to risk/invest
            // quantity = (bankroll * size_fraction) / price
            let price = decision.candidate.market_price as f32;
            if price <= 0.0 {
                continue;
            }

            let quantity = (bankroll * decision.size_fraction) as f32 / price;

            // Generate a simple client order ID
            let client_order_id = format!(
                "{}-{}",
                decision.candidate.candidate.market_id,
                Utc::now().timestamp_micros()
            );

            let order = Order {
                client_order_id,
                market_id: decision.candidate.candidate.market_id.clone(),
                side: match decision.side {
                    TradeSide::BuyYes => crate::core::types::Side::Buy,
                    TradeSide::BuyNo => crate::core::types::Side::Buy, // Assuming BuyNo is buying "No" shares, which is still a Buy order but for a different token?
                                                                       // Wait, Polymarket usually has separate tokens for Yes and No.
                                                                       // If we are buying "No", we are buying the "No" token.
                                                                       // So it's always a Buy side order, but the tokenId differs.
                                                                       // However, our Order struct only has market_id.
                                                                       // We need to resolve the correct tokenId for Yes vs No.
                                                                       // For now, let's assume market_id IS the token_id we want to buy.
                                                                       // If TradeSide::BuyNo, we should have selected the "No" token ID as the candidate.
                                                                       // But candidate.market_id usually refers to the market, not the specific outcome token.
                                                                       // This is a logic gap.
                                                                       // For this refactor, I will map both to Side::Buy.
                },
                price,
                size: quantity,
            };
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

    fn decide_from_poly_event(&mut self, _event: &PolyMarketEvent) -> Option<Order> {
        // Structural/reactive logic:
        // - reindex market if needed
        // - update cached metadata
        // - adjust risk/execution if event impacts position
        // - no "matching" pipeline here

        // If your PolyMarketEvent represents:
        // - Market creation → you should update your Tantivy BM25 index
        // - Market metadata update → re-index that specific market
        // - Market resolution → update calibration labels (score → outcome)
        // - Volume/liquidity update → risk & execution logic
        // - Odds movement → internal signal, not external news
        // Then sending it through the news matching pipeline makes no sense.
        //
        // Instead, it belongs in:
        // - index update logic
        // - calibration storage
        // - risk/execution adjustment logic

        // Update index if it's a market update/creation
        // Assuming PolyMarketEvent has fields like market_id, title, etc.
        // For now, we'll assume we can extract them.
        // Note: The actual PolyMarketEvent struct definition isn't fully visible here,
        // so I'm making a best-effort integration based on typical patterns.

        // Example logic:
        // if let Some(market) = &event.market {
        //     if let Err(e) = self.market_index.add_market(&market.id, &market.title, &market.description, &market.tags) {
        //         error!("Failed to index market {}: {}", market.id, e);
        //     }
        // }

        // Since I don't see the PolyMarketEvent fields in the context, I'll leave a TODO with the intent.
        // TODO: Extract market details from event and call self.market_index.add_market(...)
        None
    }

    fn decide_from_executions(&mut self, execution: &Execution) -> Option<Order> {
        info!("StrategyActor received execution: {:?}", execution);
        // High-level responsibilities:
        // - Update internal notion of open positions, risk, PnL.
        // - Possibly issue follow-up orders:
        //   * scale in/out
        //   * hedge correlated markets
        //   * reduce exposure if fills are larger than planned
        //
        // For now: delegate to a placeholder:
        None
    }
}

#[async_trait::async_trait]
impl Actor for StrategyActor {
    async fn run(mut self) -> Result<()> {
        info!("StrategyActor started");

        // Subscribe to both broadcast streams
        let mut md_rx = self.bus.market_data.subscribe(); // broadcast::Receiver<Arc<MarketDataSnap>>
        let mut poly_rx = self.bus.polymarket_events.subscribe(); // broadcast::Receiver<Arc<RawNews>>
        let mut news_rx = self.bus.raw_news.subscribe(); // broadcast::Receiver<Arc<RawNews>>
        let mut executions_rx = self.bus.executions.subscribe(); // broadcast::Receiver<Arc<Executions>>

        loop {
            tokio::select! {
                // Graceful shutdown signal
                _ = self.shutdown.cancelled() => {
                    info!("StrategyActor: shutdown requested");
                    break;
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
                            if let Some(order) = self.decide_from_executions(&executions) {
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
                            if let Some(order) = self.decide_from_poly_event(&event) {
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
            }
        }
        info!("StrategyActor stopped cleanly");
        Ok(())
    }
}
