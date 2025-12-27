use crate::core::types::RawNews;
use crate::strategy::normalizers::normalize_news_item_dedup_stage;
use ahash::AHasher;
use chrono::Utc;
use lru::LruCache;

use std::hash::{Hash, Hasher};
use std::num::{NonZero, NonZeroUsize};

/// Configuration for the exact duplication pipeline
#[derive(Debug, Clone)]
pub struct ExactDuplicateDetectorConfig {
    pub capacity: NonZeroUsize,
    pub ttl_hours: i64,
}

impl Default for ExactDuplicateDetectorConfig {
    fn default() -> Self {
        Self {
            capacity: NonZero::new(10_000)
                .expect("DuplicateDetector: lru cache can't be of size 0"),
            ttl_hours: 48,
        }
    }
}

pub struct ExactDuplicateDetector {
    cache: LruCache<u64, i64>,
    ttl_hours: i64,
}

impl ExactDuplicateDetector {
    pub fn new(config: ExactDuplicateDetectorConfig) -> Self {
        Self {
            cache: LruCache::new(config.capacity),
            ttl_hours: config.ttl_hours,
        }
    }

    fn hash_str(s: &str) -> u64 {
        let mut hasher = AHasher::default();
        s.hash(&mut hasher);
        hasher.finish()
    }

    // we don't have to clean expired keys, what matters is if it is still within ttl and we have
    //  a collision it is a duplicate
    pub fn is_duplicate(&mut self, news: &RawNews) -> bool {
        let normalized = normalize_news_item_dedup_stage(news);
        let hash = Self::hash_str(&normalized);
        let now = Utc::now().timestamp(); // seconds

        if let Some(&ts) = self.cache.get(&hash) {
            // still within TTL -> duplicate
            if now - ts <= self.ttl_hours * 3600 {
                return true;
            }
        }

        // not seen recently -> store & treat as new
        self.cache.put(hash, now);
        false
    }
    pub fn hydrate(&mut self, items: Vec<(String, String)>) {
        let now = Utc::now().timestamp();
        for (title, description) in items {
            // Reconstruct minimal RawNews for normalization
            let news = RawNews {
                title,
                description,
                url: "".to_string(),
                feed: "".to_string(),
                published: None,
                labels: vec![],
            };
            let normalized = normalize_news_item_dedup_stage(&news);
            let hash = Self::hash_str(&normalized);
            self.cache.put(hash, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::RawNews;

    fn make_news(title: &str) -> RawNews {
        RawNews {
            title: title.to_string(),
            url: "http://example.com".to_string(),
            description: "".to_string(),
            feed: "test".to_string(),
            published: None,
            labels: vec![],
        }
    }

    #[test]
    fn test_basic_duplication() {
        let config = ExactDuplicateDetectorConfig::default();
        let mut detector = ExactDuplicateDetector::new(config);

        let news1 = make_news("Breaking: Bitcoin hits 100k");

        // First time -> Not duplicate
        assert!(!detector.is_duplicate(&news1));

        // Second time -> Duplicate
        assert!(detector.is_duplicate(&news1));
    }

    #[test]
    fn test_normalization_duplication() {
        let config = ExactDuplicateDetectorConfig::default();
        let mut detector = ExactDuplicateDetector::new(config);

        let news1 = make_news("Breaking: Bitcoin hits 100k");
        let news2 = make_news("breaking: bitcoin hits 100k"); // Lowercase

        // First time -> Not duplicate
        assert!(!detector.is_duplicate(&news1));

        // Second time (different case) -> Duplicate
        assert!(detector.is_duplicate(&news2));
    }
}
