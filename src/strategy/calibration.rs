use crate::config::config::CalibrationCfg;
use crate::strategy::types::{ProbabilisticCandidate, ScoredCandidate};

pub struct ProbabilityCalibrator {
    cfg: CalibrationCfg,
}

impl ProbabilityCalibrator {
    pub fn new(cfg: CalibrationCfg) -> Self {
        Self { cfg }
    }

    pub fn map_scores_to_probabilities(
        &self,
        candidates: Vec<ScoredCandidate>,
    ) -> Vec<ProbabilisticCandidate> {
        let a = self.cfg.a;
        let b = self.cfg.b;
        let lambda = self.cfg.lambda;

        candidates
            .into_iter()
            .map(|c| {
                let score = c.score;
                // 1. Logistic transform
                let logit = a + b * score;
                let p_raw = 1.0 / (1.0 + (-logit).exp());

                // 2. Shrinkage
                let p_calibrated = 0.5 + lambda * (p_raw - 0.5);

                // 3. Clip (just in case)
                let probability = p_calibrated.max(0.0).min(1.0);

                ProbabilisticCandidate {
                    candidate: c.candidate,
                    score,
                    probability,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::types::RawCandidate;

    #[test]
    fn test_probability_calibration() {
        let cfg = CalibrationCfg {
            a: -3.0,
            b: 6.0,
            lambda: 0.5,
        };
        let calibrator = ProbabilityCalibrator::new(cfg);

        let calibrate = |score: f64| -> f64 {
            let candidates = vec![ScoredCandidate {
                candidate: RawCandidate::default(),
                score,
                components: Default::default(),
            }];
            let probs = calibrator.map_scores_to_probabilities(candidates);
            probs[0].probability
        };

        // Score 0.0 -> logit -3.0 -> p_raw ~0.047 -> p_cal ~0.27
        let p0 = calibrate(0.0);
        assert!(p0 > 0.25 && p0 < 0.30);

        // Score 0.5 -> logit 0.0 -> p_raw 0.5 -> p_cal 0.5
        let p50 = calibrate(0.5);
        assert!((p50 - 0.5).abs() < 1e-6);

        // Score 1.0 -> logit 3.0 -> p_raw ~0.95 -> p_cal ~0.72
        let p100 = calibrate(1.0);
        assert!(p100 > 0.70 && p100 < 0.75);
    }
}
