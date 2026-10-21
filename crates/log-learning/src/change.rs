use crate::Histogram;

/// One-sided CUSUM over log durations; one constant-sized state per context.
/// A persistent moderate shift can accumulate even below the extreme-tail gate.
pub struct DurationChange {
    location: f64,
    positive: f64,
}
impl DurationChange {
    pub fn new(reference: &Histogram) -> Option<Self> {
        let count = reference.count();
        if count < 30 {
            return None;
        }
        let location = reference
            .bins
            .iter()
            .enumerate()
            .map(|(bin, n)| (bin as f64 + 0.5) * (*n as f64))
            .sum::<f64>()
            / count as f64;
        Some(Self {
            location,
            positive: 0.0,
        })
    }
    pub fn observe(&mut self, value: f64, observations: u64) -> bool {
        if !value.is_finite() || value < 0.0 || observations < 5 {
            return false;
        }
        self.observe_log((value + 1.0).log2(), observations)
    }
    /// Compare like with like: the reference and current window use the same
    /// logarithmic histogram centers. log(mean(duration)) is upward biased by
    /// isolated extremes and is not comparable to mean(log(duration)).
    pub fn observe_histogram(&mut self, window: &Histogram) -> bool {
        let count = window.count();
        if count < 5 {
            return false;
        }
        let location = window
            .bins
            .iter()
            .enumerate()
            .map(|(bin, n)| (bin as f64 + 0.5) * (*n as f64))
            .sum::<f64>()
            / count as f64;
        self.observe_log(location, count)
    }
    fn observe_log(&mut self, value: f64, observations: u64) -> bool {
        if value <= self.location {
            self.positive = 0.0;
            return false;
        }
        self.positive = (self.positive
            + (value - self.location - 0.5) * (observations.min(32) as f64).sqrt())
        .max(0.0);
        self.positive >= 8.0 && value > self.location + 0.5
    }
    pub fn reset(&mut self) {
        self.positive = 0.0;
    }
}
