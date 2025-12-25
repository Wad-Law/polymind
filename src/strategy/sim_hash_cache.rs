use ahash::AHasher;
use chrono::Utc;
use lru::LruCache;
use std::hash::{Hash, Hasher};
use std::num::{NonZero, NonZeroUsize};

/// Configuration for the exact duplication pipeline
#[derive(Debug, Clone)]
pub struct SimHashCacheConfig {
    pub capacity: NonZeroUsize,
    pub ttl_hours: i64,
    pub hamming_threshold: u32,
}

impl Default for SimHashCacheConfig {
    fn default() -> Self {
        Self {
            capacity: NonZero::new(10_000).expect(" SimHashCache: lru cache can't be of size 0"),
            ttl_hours: 48,
            hamming_threshold: 3,
        }
    }
}

pub struct SimHashCache {
    cache: LruCache<u64, i64>, // simhash -> timestamp
    ttl_hours: i64,
    hamming_threshold: u32,
}

impl SimHashCache {
    pub fn new(config: SimHashCacheConfig) -> Self {
        Self {
            cache: LruCache::new(config.capacity),
            ttl_hours: config.ttl_hours,
            hamming_threshold: config.hamming_threshold,
        }
    }

    /// Returns true if `hash` is considered a near-duplicate of any recent hash
    /// currently in the cache (within TTL and Hamming distance threshold).
    ///
    /// Each time this is called, we eject expired key, we shouldn't rely on LRU memory policy
    /// Expired entries must not participate in the similarity check.
    /// Note: This DOES NOT insert the hash. Call `insert` separately if you
    /// decide to treat this as a new event.
    pub fn is_near_duplicate(&mut self, hash: u64) -> bool {
        let now = Utc::now().timestamp();
        let ttl_secs = self.ttl_hours * 3600;

        // We'll collect keys to remove lazily to avoid borrowing issues.
        let mut expired_keys = Vec::new();

        for (existing_hash, &ts) in self.cache.iter() {
            // Mark expired entries for later removal.
            if now - ts > ttl_secs {
                expired_keys.push(*existing_hash);
                continue;
            }

            // Within TTL: check Hamming distance.
            let dist = hamming_distance(hash, *existing_hash);
            if dist <= self.hamming_threshold {
                return true;
            }
        }

        // Remove expired entries.
        for key in expired_keys {
            self.cache.pop(&key);
        }

        false
    }

    pub fn insert(&mut self, hash: u64) {
        let now = Utc::now().timestamp();
        self.cache.put(hash, now);
    }

    /// Compute a 64-bit SimHash from a list of tokens.
    /// An accumulator vector is used so that when two tokens are similar, their simhash
    /// output is similar in Hamming space
    ///
    /// it embeds similarity into bit-distance.
    ///
    /// This hashing solution ensures:
    ///  - Similar texts share many bit decisions -> low hamming distance
    ///  - Small token differences cause few bit flips -> stable output
    ///  - Big changes flip many bits -> Distance increases meaningfully
    ///
    /// Algorithm (standard SimHash):
    /// 1) For each token, hash it to 64 bits.
    /// 2) Maintain a 64-dim vector V[i].
    ///    - If bit i of token-hash is 1 -> V[i] += weight
    ///    - If bit i is 0             -> V[i] -= weight
    ///    (here weight = 1.0 for all tokens)
    /// 3) Final bit i of SimHash is 1 if V[i] > 0, else 0.
    ///
    /// TODO: upgrade to TF-IDF sim hashing
    pub fn sim_hash(&self, tokens: &[String]) -> u64 {
        if tokens.is_empty() {
            return 0;
        }

        // 64-dim accumulator
        let mut v = [0i32; 64];

        for token in tokens {
            let h = hash_token(token);

            // For each bit in the 64-bit hash
            for i in 0..64 {
                let bit_is_set = (h >> i) & 1 == 1;
                if bit_is_set {
                    v[i] += 1;
                } else {
                    v[i] -= 1;
                }
            }
        }

        // Build final hash: bit i = 1 if V[i] > 0 else 0
        let mut result: u64 = 0;
        for i in 0..64 {
            if v[i] > 0 {
                result |= 1u64 << i;
            }
        }

        result
    }
}

/// Hash a single token to a stable u64.
/// For SimHash, we just need a fast non-crypto hash.
fn hash_token(token: &str) -> u64 {
    let mut hasher = AHasher::default();
    token.hash(&mut hasher);
    hasher.finish()
}

fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hamming_distance() {
        assert_eq!(hamming_distance(0, 0), 0);
        assert_eq!(hamming_distance(0, 1), 1); // 00...00 vs 00...01
        assert_eq!(hamming_distance(0, 3), 2); // 00...00 vs 00...11
        assert_eq!(hamming_distance(0, u64::MAX), 64);
    }

    #[test]
    fn test_sim_hash_calculation() {
        let config = SimHashCacheConfig::default();
        let cache = SimHashCache::new(config);

        let tokens1 = vec!["hello".to_string(), "world".to_string()];
        let tokens2 = vec!["hello".to_string(), "world".to_string()];
        let tokens3 = vec!["hello".to_string(), "mars".to_string()];

        let h1 = cache.sim_hash(&tokens1);
        let h2 = cache.sim_hash(&tokens2);
        let h3 = cache.sim_hash(&tokens3);

        assert_eq!(h1, h2, "Identical tokens should produce identical hash");
        assert_ne!(h1, h3, "Different tokens should produce different hash");
        assert_ne!(h1, 0, "Hash should not be zero for non-empty input");
    }

    #[test]
    fn test_near_duplicate_detection() {
        let config = SimHashCacheConfig::default();
        let mut cache = SimHashCache::new(config);

        let hash = 0b0000_1111; // simple pattern
        cache.insert(hash);

        // Exact match
        assert!(cache.is_near_duplicate(hash));

        // 1 bit difference (dist=1, threshold=3) -> True
        assert!(cache.is_near_duplicate(0b0000_1110));

        // 3 bits difference (dist=3, threshold=3) -> True
        assert!(cache.is_near_duplicate(0b0000_1000));

        // 4 bits difference (dist=4, threshold=3) -> False
        assert!(!cache.is_near_duplicate(0b1111_0000));
    }
}
