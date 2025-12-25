use crate::strategy::event_features::{Entity, TimeWindow};
use crate::strategy::types::RawCandidate;

#[derive(Default)]
pub struct HardFilterer;

impl HardFilterer {
    pub fn new() -> Self {
        Self
    }

    pub fn apply(
        &self,
        candidates: Vec<RawCandidate>,
        entities: &[Entity],
        time_window: &Option<TimeWindow>,
    ) -> Vec<RawCandidate> {
        candidates
            .into_iter()
            .filter(|c| {
                // 1. Time Window Filter
                if let Some(tw) = time_window {
                    if let Some(res_ts) = c.resolution_date {
                        let res_date =
                            chrono::DateTime::from_timestamp(res_ts, 0).unwrap_or_default();

                        if res_date < tw.start {
                            return false;
                        }
                    }
                }

                // 2. Entity Filter
                if !entities.is_empty() && !self.check_entity_overlap(c, entities) {
                    return false;
                }

                true
            })
            .collect()
    }

    fn check_entity_overlap(&self, candidate: &RawCandidate, entities: &[Entity]) -> bool {
        for entity in entities {
            let val = entity.value.to_lowercase();

            // Check tags
            if candidate
                .tags
                .iter()
                .any(|t| t.to_lowercase().contains(&val))
            {
                return true;
            }

            // Check title
            if candidate.title.to_lowercase().contains(&val) {
                return true;
            }

            // Check description
            if candidate.description.to_lowercase().contains(&val) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::event_features::{Entity, EntityKind, TimeWindow};
    use crate::strategy::types::RawCandidate;
    use chrono::{Duration, Utc};

    #[test]
    fn test_hard_filter_time_window() {
        let filterer = HardFilterer::new();

        let now = Utc::now();
        let next_week = now + Duration::days(7);

        let tw = TimeWindow {
            start: next_week,
            end: next_week + Duration::days(7),
        };

        let c1 = RawCandidate {
            market_id: "1".to_string(),
            resolution_date: Some(now.timestamp()), // Resolves NOW (too early)
            ..Default::default()
        };

        let c2 = RawCandidate {
            market_id: "2".to_string(),
            resolution_date: Some((next_week + Duration::days(1)).timestamp()), // Resolves in window
            ..Default::default()
        };

        let c3 = RawCandidate {
            market_id: "3".to_string(),
            resolution_date: None, // No date, should pass
            ..Default::default()
        };

        let candidates = vec![c1, c2, c3];
        let filtered = filterer.apply(candidates, &[], &Some(tw));

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].market_id, "2");
        assert_eq!(filtered[1].market_id, "3");
    }

    #[test]
    fn test_hard_filter_entities() {
        let filterer = HardFilterer::new();

        let entity_apple = Entity {
            kind: EntityKind::Company,
            value: "Apple".to_string(),
        };
        let entities = vec![entity_apple];

        // Match by tag
        let c1 = RawCandidate {
            market_id: "1".to_string(),
            tags: vec!["tech".to_string(), "apple".to_string()],
            ..Default::default()
        };

        // Match by title
        let c2 = RawCandidate {
            market_id: "2".to_string(),
            title: "Will Apple release a new iPhone?".to_string(),
            ..Default::default()
        };

        // Match by description
        let c3 = RawCandidate {
            market_id: "3".to_string(),
            description: "Prediction market for Apple stock price".to_string(),
            ..Default::default()
        };

        // No match
        let c4 = RawCandidate {
            market_id: "4".to_string(),
            title: "Microsoft earnings".to_string(),
            tags: vec!["tech".to_string(), "msft".to_string()],
            ..Default::default()
        };

        let candidates = vec![c1, c2, c3, c4];
        let filtered = filterer.apply(candidates, &entities, &None);

        assert_eq!(filtered.len(), 3);
        assert!(filtered.iter().any(|c| c.market_id == "1"));
        assert!(filtered.iter().any(|c| c.market_id == "2"));
        assert!(filtered.iter().any(|c| c.market_id == "3"));
        assert!(!filtered.iter().any(|c| c.market_id == "4"));

        // Test empty entities -> pass all
        let c5 = RawCandidate {
            market_id: "5".to_string(),
            ..Default::default()
        };
        let candidates_empty = vec![c5];
        let filtered_empty = filterer.apply(candidates_empty, &[], &None);
        assert_eq!(filtered_empty.len(), 1);
    }
}
