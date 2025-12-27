use crate::strategy::types::{EdgedCandidate, SizedDecision, TradeSide};
use rust_decimal::Decimal;

pub struct KellySizer {
    kelly_multiplier: Decimal,
    max_position_fraction: Decimal,
}

impl Default for KellySizer {
    fn default() -> Self {
        Self {
            kelly_multiplier: Decimal::new(5, 1),      // 0.5
            max_position_fraction: Decimal::new(5, 2), // 0.05
        }
    }
}

impl KellySizer {
    #[allow(dead_code)]
    pub fn new(kelly_multiplier: Decimal, max_position_fraction: Decimal) -> Self {
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
                let k = (c.probability - c.market_price) / (Decimal::ONE - c.market_price);
                (TradeSide::Buy("Yes".to_string()), k)
            } else {
                // Buy No
                let k = (c.market_price - c.probability) / c.market_price;
                (TradeSide::Buy("No".to_string()), k)
            };

            // If edge is negative or zero (shouldn't happen with above logic unless equal), fraction is 0
            if raw_kelly <= Decimal::ZERO {
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
            score: Decimal::ZERO,
            probability: Decimal::new(6, 1),  // 0.6
            market_price: Decimal::new(5, 1), // 0.5
            edge: Decimal::new(1, 1),         // 0.1
        }];

        let decisions = sizer.size_positions(candidates);
        assert_eq!(decisions.len(), 1);
        let d = &decisions[0];
        assert_eq!(d.side, TradeSide::Buy("Yes".to_string()));
        // Kelly = (0.6 - 0.5) / 0.5 = 0.2
        assert!((d.kelly_fraction - Decimal::new(2, 1)).abs() < Decimal::new(1, 6));
        // Half Kelly = 0.1, Cap = 0.05 -> 0.05
        assert!((d.size_fraction - Decimal::new(5, 2)).abs() < Decimal::new(1, 6));
    }

    #[test]
    fn test_kelly_criterion_buy_no() {
        let sizer = KellySizer::default();

        let candidates = vec![EdgedCandidate {
            candidate: RawCandidate::default(),
            score: Decimal::ZERO,
            probability: Decimal::new(2, 1),  // 0.2
            market_price: Decimal::new(5, 1), // 0.5
            edge: Decimal::new(-3, 1),        // -0.3
        }];

        let decisions = sizer.size_positions(candidates);
        assert_eq!(decisions.len(), 1);
        let d = &decisions[0];
        assert_eq!(d.side, TradeSide::Buy("No".to_string()));
        // Kelly = (0.5 - 0.2) / 0.5 = 0.6
        assert!((d.kelly_fraction - Decimal::new(6, 1)).abs() < Decimal::new(1, 6));
        // Half Kelly = 0.3, Cap = 0.05 -> 0.05
        assert!((d.size_fraction - Decimal::new(5, 2)).abs() < Decimal::new(1, 6));
    }
}
