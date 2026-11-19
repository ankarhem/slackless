//! Capped exponential reconnect backoff with jitter.

use std::time::Duration;

use rand::Rng;

/// `delay(n) = min(initial * 2^n, max)`, jittered into `[delay/2, delay]` ("equal jitter").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    /// 1 s doubling to a 60 s cap (slackdump's keeper uses the same bounds).
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// Un-jittered delay before reconnect attempt `attempt` (0-based). Non-decreasing, capped.
    pub fn base_delay(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.initial.saturating_mul(factor).min(self.max)
    }

    /// Jittered delay in `[base/2, base]` for `attempt`.
    pub fn delay(&self, attempt: u32, rng: &mut impl Rng) -> Duration {
        let base = self.base_delay(attempt);
        let half = base / 2;
        let spread = (base - half).as_millis() as u64;
        half + Duration::from_millis(rng.random_range(0..=spread))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn base_sequence_is_monotonic_and_capped() {
        let b = Backoff::default();
        let seq: Vec<_> = (0..200).map(|n| b.base_delay(n)).collect();
        assert_eq!(seq[0], Duration::from_secs(1));
        assert_eq!(seq[1], Duration::from_secs(2));
        assert_eq!(seq[5], Duration::from_secs(32));
        assert!(
            seq.windows(2).all(|w| w[0] <= w[1]),
            "not monotonic: {seq:?}"
        );
        assert!(seq.iter().all(|d| *d <= b.max));
        assert_eq!(*seq.last().unwrap(), b.max);
        assert_eq!(b.base_delay(u32::MAX), b.max);
    }

    #[test]
    fn jittered_delay_is_bounded() {
        let b = Backoff {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
        };
        let mut rng = StdRng::seed_from_u64(7);
        for attempt in 0..64 {
            let base = b.base_delay(attempt);
            for _ in 0..50 {
                let d = b.delay(attempt, &mut rng);
                assert!(
                    d >= base / 2 && d <= base,
                    "attempt {attempt}: {d:?} outside [{:?}, {base:?}]",
                    base / 2
                );
                assert!(d <= b.max);
            }
        }
    }
}
