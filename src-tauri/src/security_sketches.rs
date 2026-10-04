//! Exploratory summaries only. Exact SQL decides hypotheses and memberships.
//! Hashing is domain separated SHA-256; probabilistic estimates state assumptions.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
pub const VERSION: &str = "sketches-1";
fn hash(domain: &str, value: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update([0]);
    h.update(value.as_bytes());
    u64::from_be_bytes(h.finalize()[..8].try_into().unwrap())
}
pub struct Hll {
    registers: Vec<u8>,
    p: u32,
}
impl Hll {
    pub fn new() -> Self {
        Self {
            registers: vec![0; 16384],
            p: 14,
        }
    }
    pub fn insert(&mut self, value: &str) {
        let h = hash("hll-1", value);
        let index = (h >> (64 - self.p)) as usize;
        let rank = ((h << self.p).leading_zeros() + 1).min(65 - self.p) as u8;
        self.registers[index] = self.registers[index].max(rank);
    }
    pub fn merge(&mut self, other: &Self) {
        assert_eq!(self.p, other.p);
        for (a, b) in self.registers.iter_mut().zip(&other.registers) {
            *a = (*a).max(*b);
        }
    }
    pub fn estimate(&self) -> f64 {
        let m = self.registers.len() as f64;
        let harmonic = self
            .registers
            .iter()
            .map(|&r| 2f64.powi(-i32::from(r)))
            .sum::<f64>();
        let raw = 0.7213 / (1.0 + 1.079 / m) * m * m / harmonic;
        let zero = self.registers.iter().filter(|&&r| r == 0).count();
        if raw <= 2.5 * m && zero > 0 {
            m * (m / zero as f64).ln()
        } else {
            raw
        }
    }
    pub fn describe(&self) -> Value {
        json!({"algorithm":"HyperLogLog","estimate":self.estimate(),"registers":self.registers.len(),"relative_standard_error":1.04/(self.registers.len() as f64).sqrt(),"hash_bits":64,"error_meaning":"Typical standard error under uniform hashing; not a guaranteed interval","approximate":true})
    }
}
pub struct CountMin {
    counters: Vec<u64>,
    width: usize,
    depth: usize,
    pub n: u64,
}
impl CountMin {
    pub fn new() -> Self {
        Self {
            counters: vec![0; 2048 * 7],
            width: 2048,
            depth: 7,
            n: 0,
        }
    }
    fn index(&self, depth: usize, value: &str) -> usize {
        depth * self.width + (hash(&format!("cms-1-{depth}"), value) % self.width as u64) as usize
    }
    pub fn insert(&mut self, value: &str) {
        self.n += 1;
        for row in 0..self.depth {
            let index = self.index(row, value);
            self.counters[index] += 1;
        }
    }
    pub fn estimate(&self, value: &str) -> u64 {
        (0..self.depth)
            .map(|row| self.counters[self.index(row, value)])
            .min()
            .unwrap_or(0)
    }
    pub fn describe(&self) -> Value {
        json!({"algorithm":"Count-Min","updates":self.n,"width":self.width,"depth":self.depth,"epsilon":std::f64::consts::E/self.width as f64,"delta":(-(self.depth as f64)).exp(),"error_meaning":"No undercount for insert-only streams; additive epsilon*N bound with probability at least 1-delta for one fixed query under independent uniform hash rows","approximate":true})
    }
}
/// Misra-Gries with k counters. Every frequency above N/(k+1) is retained;
/// retained counts are lower bounds, not estimates for rare or absent values.
pub struct Frequent {
    pub counters: BTreeMap<String, u64>,
    pub n: u64,
    pub decrements: u64,
    k: usize,
}
impl Frequent {
    pub fn new() -> Self {
        Self {
            counters: BTreeMap::new(),
            n: 0,
            decrements: 0,
            k: 64,
        }
    }
    pub fn insert(&mut self, value: &str) {
        self.n += 1;
        if let Some(n) = self.counters.get_mut(value) {
            *n += 1;
        } else if self.counters.len() < self.k {
            self.counters.insert(value.into(), 1);
        } else {
            self.decrements += 1;
            self.counters.retain(|_, n| {
                *n -= 1;
                *n > 0
            });
        }
    }
    pub fn describe(&self) -> Value {
        json!({"algorithm":"Misra-Gries","updates":self.n,"counters":self.k,"guaranteed_retention_above":self.n/(self.k as u64+1),"maximum_undercount":self.decrements,"approximate":true,"candidates":self.counters.iter().map(|(value,n)|json!({"value":value,"lower_bound":n,"upper_bound":n+self.decrements})).collect::<Vec<_>>()})
    }
}
#[derive(Clone)]
struct Tuple {
    value: f64,
    g: u64,
    delta: u64,
}
/// Deterministic Greenwald-Khanna summary. Insert and compress preserve rank
/// uncertainty; no value error or relative percentile error is claimed.
pub struct Quantiles {
    tuples: Vec<Tuple>,
    pub n: u64,
    epsilon: f64,
}
impl Quantiles {
    pub fn new() -> Self {
        Self {
            tuples: vec![],
            n: 0,
            epsilon: 0.005,
        }
    }
    pub fn insert(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.n += 1;
        let position = self.tuples.partition_point(|t| t.value <= value);
        let cap = (2.0 * self.epsilon * self.n as f64).floor() as u64;
        let delta = if position == 0 || position == self.tuples.len() {
            0
        } else {
            cap.saturating_sub(1)
        };
        self.tuples.insert(position, Tuple { value, g: 1, delta });
        // Compress backwards and preserve both endpoint values.
        for i in (1..self.tuples.len().saturating_sub(1)).rev() {
            if self.tuples[i].g + self.tuples[i + 1].g + self.tuples[i + 1].delta <= cap {
                let g = self.tuples.remove(i).g;
                self.tuples[i].g += g;
            }
        }
    }
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.tuples.is_empty() {
            return None;
        }
        if q <= 0.0 {
            return Some(self.tuples[0].value);
        }
        if q >= 1.0 {
            return Some(self.tuples.last().unwrap().value);
        }
        let rank = (q * (self.n - 1) as f64 + 1.0).ceil();
        let allowance = self.epsilon * self.n as f64;
        let mut minimum = 0u64;
        let mut previous = self.tuples[0].value;
        for t in &self.tuples {
            minimum += t.g;
            if (minimum + t.delta) as f64 > rank + allowance {
                return Some(previous);
            }
            previous = t.value;
        }
        Some(previous)
    }
    pub fn describe(&self, unit: &str) -> Value {
        json!({"algorithm":"Greenwald-Khanna","samples":self.n,"tuples":self.tuples.len(),"epsilon":self.epsilon,"rank_error":(self.epsilon*self.n as f64).ceil(),"unit":unit,"p50":self.quantile(0.5),"p95":self.quantile(0.95),"p99":self.quantile(0.99),"error_meaning":"Absolute rank uncertainty epsilon*N (rounded up); not a bound on the numeric value","approximate":true})
    }
}
pub struct Summaries {
    pub actors: Hll,
    pub destinations: Hll,
    pub latency: Quantiles,
    pub bytes: Quantiles,
    pub codes: CountMin,
    pub frequent_codes: Frequent,
}
impl Summaries {
    pub fn new() -> Self {
        Self {
            actors: Hll::new(),
            destinations: Hll::new(),
            latency: Quantiles::new(),
            bytes: Quantiles::new(),
            codes: CountMin::new(),
            frequent_codes: Frequent::new(),
        }
    }
    pub fn describe(&self) -> Value {
        json!({"version":VERSION,"scope":"entire admitted population; identities include namespace","purpose":"exploration only; every hypothesis and evidence membership is confirmed exactly","actors":self.actors.describe(),"destinations":self.destinations.describe(),"latency":self.latency.describe("ms"),"bytes":self.bytes.describe("B"),"event_code_frequency":self.codes.describe(),"frequent_event_codes":self.frequent_codes.describe()})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_sketches_preserve_probabilistic_and_deterministic_contracts() {
        let mut h = Hll::new();
        let mut left = Hll::new();
        let mut right = Hll::new();
        let mut cm = CountMin::new();
        let mut frequent = Frequent::new();
        let mut exact = BTreeMap::new();
        for n in 0..50000 {
            let item = n.to_string();
            h.insert(&item);
            if n % 2 == 0 {
                left.insert(&item)
            } else {
                right.insert(&item)
            };
            let code = if n % 4 == 0 {
                "hot".into()
            } else {
                format!("code-{}", n % 1000)
            };
            cm.insert(&code);
            frequent.insert(&code);
            *exact.entry(code).or_insert(0u64) += 1;
        }
        assert!((h.estimate() - 50000.0).abs() / 50000.0 < 0.035);
        let original = h.registers.clone();
        for n in 0..50000 {
            h.insert(&n.to_string());
        }
        assert_eq!(original, h.registers);
        left.merge(&right);
        assert_eq!(left.registers, h.registers);
        for (code, &n) in &exact {
            assert!(cm.estimate(code) >= n);
            if n > frequent.n / (frequent.k as u64 + 1) {
                assert!(frequent.counters.contains_key(code));
            }
            if let Some(lower) = frequent.counters.get(code) {
                assert!(*lower <= n && n <= lower + frequent.decrements);
            }
        }
        for reversed in [false, true] {
            let mut q = Quantiles::new();
            for n in 0..10000 {
                q.insert(if reversed { 9999 - n } else { n } as f64);
            }
            assert!(q.tuples.len() < 1000);
            for percent in 0..=100 {
                let rank = q.quantile(percent as f64 / 100.0).unwrap() as i64;
                let expected = (percent as f64 / 100.0 * 9999.0).ceil() as i64;
                assert!(
                    (rank - expected).abs() <= 51,
                    "{reversed} {percent}: {rank} {expected}"
                );
            }
        }
    }
}
