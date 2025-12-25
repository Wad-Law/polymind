//! Provides:
//!   - TokenizationConfig: enables/disables features
//!   - TokenizedNews: wrapper with normalized text, tokens, stems, n-grams
//!   - normalize_for_matching
//!   - tokenize_basic (internal)

use crate::core::types::RawNews;
use crate::strategy::normalizers::normalize_for_matching;
use lazy_static::lazy_static;
use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::HashSet;

/// Configuration for the tokenization pipeline.
#[derive(Debug, Clone)]
pub struct TokenizationConfig {
    /// Whether to apply stemming (Porter) on tokens.
    pub use_stemming: bool,
    /// Whether to generate bigrams from tokens.
    pub generate_bigrams: bool,
    /// Whether to generate trigrams from tokens.
    pub generate_trigrams: bool,
    /// Whether to remove stopwords.
    pub remove_stopwords: bool,
    /// Extra stopwords (domain-specific) to be removed in addition to the built-ins.
    pub extra_stopwords: HashSet<String>,
}

impl Default for TokenizationConfig {
    fn default() -> Self {
        Self {
            use_stemming: true,
            generate_bigrams: true,
            generate_trigrams: true,
            remove_stopwords: true,
            extra_stopwords: HashSet::new(),
        }
    }
}

/// Result of tokenizing a single RawNews item.
#[derive(Debug, Clone)]
pub struct TokenizedNews {
    /// Original RawNews (owned).
    pub raw: RawNews,
    /// Normalized string (lowercase, URLs removed, deunicoded, whitespace collapsed).
    pub normalized: String,
    /// Base tokens extracted from normalized text.
    pub tokens: Vec<String>,
    /// Stems of tokens (if enabled; otherwise empty).
    pub stemmed_tokens: Vec<String>,
    /// Bigrams (space-joined, e.g. "ecb cuts").
    pub bigrams: Vec<String>,
    /// Trigrams (space-joined, e.g. "ecb cuts rates").
    pub trigrams: Vec<String>,
}

impl TokenizedNews {
    /// Build a TokenizedNews from a RawNews using the given configuration.
    pub fn from_raw(raw: RawNews, cfg: &TokenizationConfig) -> Self {
        // 1) Normalize
        let normalized = normalize_for_matching(&raw);

        // 2) Base tokens
        let mut tokens = tokenize_basic(&normalized, cfg);

        // 3) Stemming (optional)
        let mut stemmed_tokens = Vec::new();
        if cfg.use_stemming {
            stemmed_tokens = stem_tokens(&tokens);
        }

        // Decide which tokens to use for n-grams:
        let base_for_ngrams: &[String] = if cfg.use_stemming && !stemmed_tokens.is_empty() {
            &stemmed_tokens
        } else {
            &tokens
        };

        // 4) N-grams
        let bigrams = if cfg.generate_bigrams {
            build_ngrams(base_for_ngrams, 2)
        } else {
            Vec::new()
        };

        let trigrams = if cfg.generate_trigrams {
            build_ngrams(base_for_ngrams, 3)
        } else {
            Vec::new()
        };

        // If we want to guarantee `tokens` are not empty, we could fallback to stems.
        if tokens.is_empty() && !stemmed_tokens.is_empty() {
            tokens = stemmed_tokens.clone();
        }

        Self {
            raw,
            normalized,
            tokens,
            stemmed_tokens,
            bigrams,
            trigrams,
        }
    }
}

/// Base tokenizer for matching:
/// - extracts word tokens and financial/number tokens
/// - applies stopwords filtering if enabled
fn tokenize_basic(text: &str, cfg: &TokenizationConfig) -> Vec<String> {
    lazy_static! {
        // Matches:
        //   words:           ecb, inflation, lagarde
        //   numbers:         2026, 10
        //   decimals:        1.25, 0.5
        //   numbers+%:       3%, 2.5%
        //   numbers+bps/bp: 25bps, 50bp
        static ref TOKEN_RE: Regex = Regex::new(
            r"[a-zA-Z]+|\d+(?:\.\d+)?(?:%|bps|bp)?"
        ).unwrap();

        // Baseline stopwords (generic + newsy).
        static ref BASE_STOPWORDS: HashSet<&'static str> = {
            let words = [
                // generic
                "the","a","an","of","and","or","to","in","on","for","with","by",
                "at","from","is","are","was","were","be","this","that","it","as",
                "will","may","might","could","should",
                // Global / General News
                "breaking", "latest", "update", "exclusive", "report", "reports", "reporting",
                "live", "video", "watch", "sources", "source", "claim", "claims", "alleged",
                "official", "officials", "announcement", "media", "social", "trending",
                "viral", "confirms", "confirmed", "says", "said", "according", "statement",
                "developing", "coverage", "analysis", "expert", "experts", "interview",
                "opinion", "editorial", "commentary", "headline", "press", "briefing",
                //Crypto / Blockchain
                "crypto", "cryptocurrency", "blockchain", "token", "tokens", "altcoin",
                "altcoins", "memecoin", "memecoins", "defi", "nft", "nfts", "web3",
                "staking", "airdrop", "exchange", "wallet", "wallets", "mining", "miners",
                "chain", "bridge", "layer2", "l2", "dao", "smart", "contract", "protocol",
                "project", "ecosystem", "platform", "bull", "bear", "pump", "dump", "rally",
                "crash", "volume", "liquidity", "listing", "listed", "futures", "perpetuals",
                "binance", "coinbase", "kraken", "huobi", "okx",
                // Finance / Markets / Macro
                "market", "markets", "trading", "trader", "traders", "investors", "analyst",
                "analysts", "stock", "stocks", "shares", "equity", "equities", "bonds",
                "treasury", "yield", "yields", "index", "indices", "futures", "options",
                "etf", "fund", "hedge", "portfolio", "asset", "assets", "commodities",
                "currency", "forex", "ipo", "earnings", "quarterly", "outlook", "forecast",
                "guidance", "upgrade", "downgrade", "neutral", "overweight", "underweight",
                "bullish", "bearish", "volatility", "risk", "sentiment", "liquidity",
                "valuation", "price target", "rating", "momentum", "macro", "micro",
                // Elections & Politics
                "campaign", "rally", "speech", "debate", "poll", "polls", "polling", "vote",
                "votes", "voter", "voters", "turnout", "candidate", "senate", "house",
                "congress", "parliament", "minister", "administration", "government",
                "cabinet", "policy", "bipartisan", "democrat", "republican", "conservative",
                "liberal", "left", "right", "partisan", "spokesperson", "press secretary",
                // Sports
                "match", "game", "games", "season", "playoff", "playoffs", "league",
                "tournament", "championship", "victory", "defeat", "team", "teams", "coach",
                "coaching", "player", "players", "lineup", "score", "scoring", "injury",
                "injured", "referee", "transfer", "contract extension", "signing",
                "training", "friendly",
                // Technology & AI
                "startup", "tech", "platform", "app", "feature", "update", "version",
                "release", "beta", "launch", "developer", "developers", "user", "users",
                "software", "hardware", "device", "chip", "chipset", "silicon", "cloud",
                "compute", "datacenter", "cybersecurity", "encryption", "algorithm",
                // Geo-Conflict / Military
                "forces", "troops", "military", "army", "navy", "air force", "drone",
                "drones", "strike", "strikes", "attack", "attacks", "operation", "operations",
                "defense", "security", "border", "territory", "ceasefire", "escalation",
                "casualties", "wounded", "injured", "equipment", "base", "bases",
                // Health
                "health", "healthcare", "hospital", "hospitals", "clinic", "patient",
                "patients", "medical", "physician", "doctor", "doctors", "vaccine",
                "vaccines", "vaccination", "treatment", "research", "outbreak", "symptom",
                "symptoms", "variant", "variants", "spread", "cases", "deaths",
                // Legal
                "arrest", "arrested", "trial", "lawsuit", "sue", "sued", "court", "judge",
                "jury", "attorney", "charged", "charges", "allegations", "bail", "hearing",
                "ruling", "verdict", "appeal", "investigation", "police", "authorities",
                "custody", "interrogation", "suspect",
                // Entertainment
                "celebrity", "celebrities", "star", "stars", "actor", "actress", "singer",
                "influencer", "viral", "gossip", "drama", "interview", "reality", "premiere",
                "album", "tour", "fan", "fans", "paparazzi", "trending", "leak", "leaked",
                "rumor", "rumored", "scandal"
            ];
            words.iter().cloned().collect()
        };
    }

    let mut tokens = Vec::new();

    for m in TOKEN_RE.find_iter(text) {
        let token = m.as_str().to_lowercase();

        if token.len() <= 1 {
            continue;
        }

        if cfg.remove_stopwords {
            if BASE_STOPWORDS.contains(token.as_str()) {
                continue;
            }
            if cfg.extra_stopwords.contains(&token) {
                continue;
            }
        }

        tokens.push(token);
    }

    tokens
}

/// Stem tokens using the Porter stemmer (English).
fn stem_tokens(tokens: &[String]) -> Vec<String> {
    let stemmer = Stemmer::create(Algorithm::English);
    tokens.iter().map(|t| stemmer.stem(t).to_string()).collect()
}

/// Build n-grams (n >= 2) from a token list.
/// n=2 -> bigrams, n=3 -> trigrams.
fn build_ngrams(tokens: &[String], n: usize) -> Vec<String> {
    let len = tokens.len();
    if len < n || n < 2 {
        return Vec::new();
    }

    let mut ngrams = Vec::with_capacity(len.saturating_sub(n) + 1);

    for i in 0..=(len - n) {
        // Join tokens[i..i+n] with a space.
        let slice = &tokens[i..i + n];
        let joined = slice.join(" ");
        ngrams.push(joined);
    }

    ngrams
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_raw(text: &str) -> RawNews {
        RawNews {
            title: text.to_string(),
            url: "http://example.com".to_string(),
            description: "".to_string(),
            feed: "test".to_string(),
            published: None,
            labels: vec![],
        }
    }

    #[test]
    fn test_basic_tokenization() {
        let cfg = TokenizationConfig::default();
        let raw = make_raw("ECB raises rates by 25bps");
        let tokenized = TokenizedNews::from_raw(raw, &cfg);

        // "ecb" -> stem "ecb"
        // "raises" -> stem "rais"
        // "rates" -> stem "rate"
        // "by" -> stopword
        // "25bps" -> "25bps"

        let stems = &tokenized.stemmed_tokens;
        assert!(stems.contains(&"ecb".to_string()));
        assert!(stems.contains(&"rais".to_string()));
        assert!(stems.contains(&"rate".to_string()));
        assert!(stems.contains(&"25bps".to_string()));
        assert!(!stems.contains(&"by".to_string()));
    }

    #[test]
    fn test_stopword_removal() {
        let cfg = TokenizationConfig::default();
        let raw = make_raw("The quick brown fox jumps over the lazy dog");
        let tokenized = TokenizedNews::from_raw(raw, &cfg);

        // "the", "over" are stopwords
        let tokens = &tokenized.tokens;
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"over".to_string()));
        assert!(tokens.contains(&"quick".to_string()));
    }

    #[test]
    fn test_ngrams() {
        let mut cfg = TokenizationConfig::default();
        cfg.generate_bigrams = true;
        cfg.generate_trigrams = true;

        let raw = make_raw("central bank digital currency");
        let tokenized = TokenizedNews::from_raw(raw, &cfg);

        // Stems: central, bank, digit, currenc
        // Bigrams: "central bank", "bank digit", "digit currenc"

        let bigrams = &tokenized.bigrams;
        assert!(bigrams.contains(&"central bank".to_string()));
        assert!(bigrams.contains(&"bank digit".to_string()));
        assert!(bigrams.contains(&"digit currenc".to_string()));

        // Trigrams: "central bank digit", "bank digit currenc"
        let trigrams = &tokenized.trigrams;
        assert!(trigrams.contains(&"central bank digit".to_string()));
    }

    #[test]
    fn test_config_flags() {
        let mut cfg = TokenizationConfig::default();
        cfg.use_stemming = false;
        cfg.remove_stopwords = false;

        let raw = make_raw("The rates");
        let tokenized = TokenizedNews::from_raw(raw, &cfg);

        // No stemming -> "rates" stays "rates"
        // No stopwords -> "the" stays

        assert!(tokenized.tokens.contains(&"rates".to_string()));
        assert!(tokenized.tokens.contains(&"the".to_string()));
        assert!(tokenized.stemmed_tokens.is_empty());
    }
}
