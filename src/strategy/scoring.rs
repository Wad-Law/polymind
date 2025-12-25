use crate::strategy::event_features::{EventFeatures, TimeWindow};
use crate::strategy::types::{RawCandidate, ScoreComponents, ScoredCandidate};
use chrono::{DateTime, Utc};

use std::collections::HashSet;

#[derive(Default)]
pub struct Scorer;

impl Scorer {
    pub fn new() -> Self {
        Self
    }

    pub fn select_top_k(&self, candidates: Vec<ScoredCandidate>) -> Vec<ScoredCandidate> {
        // 1. Filter candidates
        let mut filtered: Vec<ScoredCandidate> = candidates
            .into_iter()
            .filter(|c| {
                // Minimum score thresholds
                // bm25_norm >= 0.3, entity_overlap >= 0.2
                // Note: if we have very few candidates, we might want to relax this.
                // For now, let's stick to the plan.
                c.components.bm25_norm >= 0.3 && c.components.entity_overlap >= 0.2
            })
            .collect();

        // If we have fewer than K, just return them sorted by score
        if filtered.len() <= 5 {
            filtered.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            return filtered;
        }

        // 2. MMR Selection
        // K = 5
        // lambda = 0.7
        let k = 5;
        let lambda = 0.7;
        let mut selected: Vec<ScoredCandidate> = Vec::with_capacity(k);
        let mut remaining = filtered;

        while selected.len() < k && !remaining.is_empty() {
            let mut best_idx = 0;
            let mut best_mmr = -1.0;

            for (i, candidate) in remaining.iter().enumerate() {
                // Calculate max similarity to already selected
                let max_sim = if selected.is_empty() {
                    0.0
                } else {
                    selected
                        .iter()
                        .map(|s| self.jaccard_similarity(&candidate.candidate, &s.candidate))
                        .fold(0.0_f64, |a, b| a.max(b))
                };

                let mmr = lambda * candidate.score - (1.0 - lambda) * max_sim;

                if mmr > best_mmr {
                    best_mmr = mmr;
                    best_idx = i;
                }
            }

            // Move best candidate from remaining to selected
            let best = remaining.remove(best_idx);
            selected.push(best);
        }

        selected
    }

    fn jaccard_similarity(&self, c1: &RawCandidate, c2: &RawCandidate) -> f64 {
        // Use tags for similarity
        let s1: HashSet<_> = c1.tags.iter().map(|s| s.to_lowercase()).collect();
        let s2: HashSet<_> = c2.tags.iter().map(|s| s.to_lowercase()).collect();

        if s1.is_empty() && s2.is_empty() {
            return 0.0;
        }

        let intersection = s1.intersection(&s2).count();
        let union = s1.union(&s2).count();

        intersection as f64 / union as f64
    }

    pub fn score(
        &self,
        candidates: Vec<RawCandidate>,
        features: &EventFeatures,
    ) -> Vec<ScoredCandidate> {
        // 1. Find max BM25 score for normalization
        let max_bm25 = candidates
            .iter()
            .map(|c| c.bm25_score)
            .fold(0.0_f32, |a, b| a.max(b));

        candidates
            .into_iter()
            .map(|c| self.score_single(c, features, max_bm25))
            .collect()
    }

    fn score_single(
        &self,
        candidate: RawCandidate,
        features: &EventFeatures,
        max_bm25: f32,
    ) -> ScoredCandidate {
        // 1. BM25 Norm (50%)
        let bm25_norm = if max_bm25 > 0.0 {
            (candidate.bm25_score / max_bm25) as f64
        } else {
            0.0
        };

        // 2. Entity Overlap (25%)
        let entity_overlap = self.calc_entity_overlap(&candidate, &features.entities);

        // 3. Number Overlap (10%)
        let number_overlap = self.calc_number_overlap(&candidate, &features.numbers);

        // 4. Time Compatibility (10%)
        let time_compat = self.calc_time_compat(&candidate, &features.time_window);

        // 5. Liquidity Score (15%)
        // Placeholder: we don't have liquidity data in RawCandidate yet.
        let liquidity_score = 0.0;

        // 6. Staleness Penalty (-5%)
        let staleness_penalty = self.calc_staleness_penalty(&candidate);

        // Weighted Sum
        let score = 0.50 * bm25_norm
            + 0.25 * entity_overlap
            + 0.10 * number_overlap
            + 0.10 * time_compat
            + 0.15 * liquidity_score
            - 0.05 * staleness_penalty;

        // Clip to [0, 1] (though penalties might make it negative, usually we want a probability-like score)
        // Let's allow negative for now, or clip at 0.
        let final_score = score.max(0.0).min(1.0);

        ScoredCandidate {
            candidate,
            score: final_score,
            components: ScoreComponents {
                bm25_norm,
                entity_overlap,
                number_overlap,
                time_compat,
                liquidity_score,
                staleness_penalty,
            },
        }
    }

    fn calc_entity_overlap(
        &self,
        candidate: &RawCandidate,
        entities: &[crate::strategy::event_features::Entity],
    ) -> f64 {
        if entities.is_empty() {
            return 0.0;
        }

        let mut matches = 0;
        for entity in entities {
            let val = entity.value.to_lowercase();
            let hit = candidate
                .tags
                .iter()
                .any(|t| t.to_lowercase().contains(&val))
                || candidate.title.to_lowercase().contains(&val)
                || candidate.description.to_lowercase().contains(&val);
            if hit {
                matches += 1;
            }
        }

        matches as f64 / entities.len() as f64
    }

    fn calc_number_overlap(
        &self,
        candidate: &RawCandidate,
        numbers: &[crate::strategy::event_features::NumberToken],
    ) -> f64 {
        if numbers.is_empty() {
            return 0.0;
        }

        let mut matches = 0;
        for num in numbers {
            // Simple string check for raw value
            // In reality, you'd want numeric tolerance (e.g. 3.2 vs 3.20)
            let val = &num.raw;
            let hit = candidate.title.contains(val) || candidate.description.contains(val);
            if hit {
                matches += 1;
            }
        }

        matches as f64 / numbers.len() as f64
    }

    fn calc_time_compat(&self, candidate: &RawCandidate, time_window: &Option<TimeWindow>) -> f64 {
        let tw = match time_window {
            Some(t) => t,
            None => return 0.0,
        };

        let res_ts = match candidate.resolution_date {
            Some(ts) => ts,
            None => return 0.0,
        };

        let res_date = DateTime::from_timestamp(res_ts, 0).unwrap_or_default();

        // Bonus if resolution is WITHIN the window
        if res_date >= tw.start && res_date <= tw.end {
            1.0
        } else {
            0.0
        }
    }

    fn calc_staleness_penalty(&self, candidate: &RawCandidate) -> f64 {
        // If resolution date is very old, penalize.
        // But hard filters should have caught this.
        // Let's say if it resolved > 30 days ago, penalize.
        let res_ts = match candidate.resolution_date {
            Some(ts) => ts,
            None => return 0.0,
        };

        let res_date = DateTime::from_timestamp(res_ts, 0).unwrap_or_default();
        let now = Utc::now();

        if res_date < now - chrono::Duration::days(30) {
            1.0
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::event_features::{Entity, EntityKind};

    #[test]
    fn test_scoring_basic() {
        let scorer = Scorer::new();

        let c1 = RawCandidate {
            market_id: "1".to_string(),
            bm25_score: 10.0,
            title: "Apple earnings".to_string(),
            ..Default::default()
        };

        let c2 = RawCandidate {
            market_id: "2".to_string(),
            bm25_score: 5.0,
            title: "Microsoft earnings".to_string(),
            ..Default::default()
        };

        let candidates = vec![c1, c2];
        let features = EventFeatures {
            entities: vec![Entity {
                kind: EntityKind::Company,
                value: "Apple".to_string(),
            }],
            ..Default::default()
        };

        let scored = scorer.score(candidates, &features);

        assert_eq!(scored.len(), 2);
        // c1: bm25_norm=1.0, entity=1.0 -> score = 0.5 + 0.25 = 0.75
        // c2: bm25_norm=0.5, entity=0.0 -> score = 0.25

        assert!(scored[0].score > scored[1].score);
        assert!((scored[0].score - 0.75).abs() < 1e-6);
        assert!((scored[1].score - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_mmr_diversity() {
        let scorer = Scorer::new();

        // High score, "tech"
        let c1 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "1".to_string(),
                tags: vec!["tech".to_string(), "ai".to_string()],
                ..Default::default()
            },
            score: 0.9,
            components: ScoreComponents {
                bm25_norm: 1.0,
                entity_overlap: 1.0,
                ..Default::default()
            },
        };

        // High score, "tech" (duplicate-ish)
        let c2 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "2".to_string(),
                tags: vec!["tech".to_string(), "ai".to_string()],
                ..Default::default()
            },
            score: 0.88,
            components: ScoreComponents {
                bm25_norm: 1.0,
                entity_overlap: 1.0,
                ..Default::default()
            },
        };

        // Medium score, "politics" (diverse)
        let c3 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "3".to_string(),
                tags: vec!["politics".to_string(), "usa".to_string()],
                ..Default::default()
            },
            score: 0.7,
            components: ScoreComponents {
                bm25_norm: 0.8,
                entity_overlap: 0.5,
                ..Default::default()
            },
        };

        // Low score, filtered out
        let c4 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "4".to_string(),
                ..Default::default()
            },
            score: 0.1,
            components: ScoreComponents {
                bm25_norm: 0.1,
                entity_overlap: 0.0,
                ..Default::default()
            },
        };

        // Add more candidates to trigger MMR (need > 5 filtered)
        let c5 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "5".to_string(),
                ..Default::default()
            },
            score: 0.6,
            components: ScoreComponents {
                bm25_norm: 0.6,
                entity_overlap: 0.2,
                ..Default::default()
            },
        };
        let c6 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "6".to_string(),
                ..Default::default()
            },
            score: 0.5,
            components: ScoreComponents {
                bm25_norm: 0.5,
                entity_overlap: 0.2,
                ..Default::default()
            },
        };
        let c7 = ScoredCandidate {
            candidate: RawCandidate {
                market_id: "7".to_string(),
                ..Default::default()
            },
            score: 0.4,
            components: ScoreComponents {
                bm25_norm: 0.4,
                entity_overlap: 0.2,
                ..Default::default()
            },
        };

        let candidates = vec![c1, c2, c3, c4, c5, c6, c7];
        let selected = scorer.select_top_k(candidates);

        // Should select c1 (best score)
        // Then c3 (diverse), despite lower score than c2
        // c2 might be selected 3rd if K allows, but c3 should jump ahead of c2 in MMR ranking if lambda/sim weights align

        // With lambda=0.7:
        // 1. Pick c1 (score 0.9)
        // 2. Remaining: c2 (score 0.88, sim(c1)=1.0), c3 (score 0.7, sim(c1)=0.0)
        //    MMR(c2) = 0.7*0.88 - 0.3*1.0 = 0.616 - 0.3 = 0.316
        //    MMR(c3) = 0.7*0.7 - 0.3*0.0 = 0.49
        //    -> Pick c3!
        // 3. c5 (0.6) -> MMR 0.42 > c2 (0.316) -> Pick c5
        // 4. c6 (0.5) -> MMR 0.35 > c2 (0.316) -> Pick c6
        // 5. c2 is picked last among top 5

        assert_eq!(selected.len(), 5);
        assert_eq!(selected[0].candidate.market_id, "1");
        assert_eq!(selected[1].candidate.market_id, "3");
        // c2 is pushed down due to high similarity with c1
        assert_eq!(selected[4].candidate.market_id, "2");
    }
}
