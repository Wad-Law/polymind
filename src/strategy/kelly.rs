use crate::strategy::types::{EdgedCandidate, SizedDecision, TradeSide};

pub struct KellySizer {
    kelly_multiplier: f64,
    max_position_fraction: f64,
}

impl Default for KellySizer {
    fn default() -> Self {
        Self {
            kelly_multiplier: 0.5,
            max_position_fraction: 0.05,
        }
    }
}

impl KellySizer {
    pub fn new(kelly_multiplier: f64, max_position_fraction: f64) -> Self {
        Self {
            kelly_multiplier,
            max_position_fraction,
        }
    }

    pub fn size_positions(&self, candidates: Vec<EdgedCandidate>) -> Vec<SizedDecision> {
        let mut decisions = Vec::new();

        for c in candidates {
            // Determine side and raw Kelly fraction
            // Formula: f = (p - price) / (1 - price) for Buy Yes
            //          f = (price - p) / price       for Buy No

            let (side, raw_kelly) = if c.probability > c.market_price {
                // Buy Yes
                let k = (c.probability - c.market_price) / (1.0 - c.market_price);
                (TradeSide::BuyYes, k)
            } else {
                // Buy No
                let k = (c.market_price - c.probability) / c.market_price;
                (TradeSide::BuyNo, k)
            };

            // If edge is negative or zero (shouldn't happen with above logic unless equal), fraction is 0
            if raw_kelly <= 0.0 {
                continue;
            }

            // Apply risk controls
            let mut size = raw_kelly * self.kelly_multiplier;
            if size > self.max_position_fraction {
                size = self.max_position_fraction;
            }

            decisions.push(SizedDecision {
                candidate: c,
                side,
                kelly_fraction: raw_kelly,
                size_fraction: size,
            });
        }
        decisions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::types::RawCandidate;

    #[test]
    fn test_kelly_criterion_buy_yes() {
        let sizer = KellySizer::default();

        let candidates = vec![EdgedCandidate {
            candidate: RawCandidate::default(),
            score: 0.0,
            probability: 0.6,
            market_price: 0.5,
            edge: 0.1,
        }];

        let decisions = sizer.size_positions(candidates);
        assert_eq!(decisions.len(), 1);
        let d = &decisions[0];
        assert_eq!(d.side, TradeSide::BuyYes);
        // Kelly = (0.6 - 0.5) / 0.5 = 0.2
        assert!((d.kelly_fraction - 0.2).abs() < 1e-6);
        // Half Kelly = 0.1, Cap = 0.05 -> 0.05
        assert!((d.size_fraction - 0.05).abs() < 1e-6);
    }

    #[test]
    fn test_kelly_criterion_buy_no() {
        let sizer = KellySizer::default();

        let candidates = vec![EdgedCandidate {
            candidate: RawCandidate::default(),
            score: 0.0,
            probability: 0.2,
            market_price: 0.5,
            edge: -0.3,
        }];

        let decisions = sizer.size_positions(candidates);
        assert_eq!(decisions.len(), 1);
        let d = &decisions[0];
        assert_eq!(d.side, TradeSide::BuyNo);
        // Kelly = (0.5 - 0.2) / 0.5 = 0.6
        assert!((d.kelly_fraction - 0.6).abs() < 1e-6);
        // Half Kelly = 0.3, Cap = 0.05 -> 0.05
        assert!((d.size_fraction - 0.05).abs() < 1e-6);
    }
}
