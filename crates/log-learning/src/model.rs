use crate::SCHEMA;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_contexts: usize,
    pub max_templates: usize,
    pub max_transitions: usize,
    pub max_combinations: usize,
    pub minimum_support: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_contexts: 32,
            max_templates: 128,
            max_transitions: 512,
            max_combinations: 256,
            minimum_support: 30,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=128).contains(&self.max_contexts)
            || !(8..=256).contains(&self.max_templates)
            || !(16..=1024).contains(&self.max_transitions)
            || !(16..=512).contains(&self.max_combinations)
            || !(20..=10_000).contains(&self.minimum_support)
        {
            return Err("Limites do aprendizado fora do orçamento suportado".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Histogram {
    pub bins: [u64; 32],
}
impl Histogram {
    pub fn bin(value: f64) -> Option<usize> {
        (value.is_finite() && value >= 0.0).then(|| ((value + 1.0).log2().floor() as usize).min(31))
    }
    pub fn push(&mut self, value: Option<f64>) {
        if let Some(bin) = value.and_then(Self::bin) {
            self.bins[bin] = self.bins[bin].saturating_add(1);
        }
    }
    pub fn count(&self) -> u64 {
        self.bins.iter().fold(0u64, |n, v| n.saturating_add(*v))
    }
    pub fn tail(&self, value: f64) -> f64 {
        let Some(bin) = Self::bin(value) else {
            return 1.0;
        };
        (self.bins[bin..].iter().sum::<u64>() as f64 + 0.5) / (self.count() as f64 + 1.0)
    }
    pub fn upper_quantile(&self, quantile: f64) -> f64 {
        let target = (self.count() as f64 * quantile).ceil() as u64;
        let mut n = 0;
        for (bin, count) in self.bins.iter().enumerate() {
            n += count;
            if n >= target.max(1) {
                return 2f64.powi(bin as i32 + 1) - 1.0;
            }
        }
        0.0
    }
    fn halve(&mut self) {
        for n in &mut self.bins {
            *n /= 2;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cluster {
    pub tokens: Vec<String>,
    pub count: u64,
}

/// Incremental token clustering, not a claim of equivalence to Drain.
/// Fixed token and dictionary budgets make cost independent of log volume.
pub fn tokens(message: &str) -> Vec<String> {
    message
        .split_whitespace()
        .take(48)
        .map(|word| {
            let word = word.trim_matches(|c: char| {
                matches!(c, '"' | '\'' | ',' | ';' | '[' | ']' | '(' | ')')
            });
            if word.len() > 48
                || word.chars().any(|c| c.is_ascii_digit())
                || word.contains('=')
                || word.starts_with('/')
                || word.contains("://")
            {
                "*".into()
            } else {
                word.chars().take(48).collect::<String>().to_lowercase()
            }
        })
        .filter(|t| !t.is_empty())
        .collect()
}
fn matching(clusters: &[Cluster], words: &[String]) -> Option<usize> {
    clusters
        .iter()
        .enumerate()
        .filter_map(|(id, cluster)| {
            if cluster.tokens.len() != words.len() || words.is_empty() {
                return None;
            }
            let anchor = |s: &str| {
                matches!(
                    s,
                    "error"
                        | "erro"
                        | "failed"
                        | "failure"
                        | "success"
                        | "successfully"
                        | "sucesso"
                        | "timeout"
                        | "denied"
                        | "negado"
                        | "started"
                        | "completed"
                        | "cancelled"
                        | "fatal"
                )
            };
            if cluster
                .tokens
                .iter()
                .zip(words)
                .any(|(a, b)| a != b && (anchor(a) || anchor(b)))
            {
                return None;
            }
            let equal = cluster
                .tokens
                .iter()
                .zip(words)
                .filter(|(a, b)| a.as_str() != "*" && a == b)
                .count();
            let stable = cluster.tokens.iter().filter(|t| t.as_str() != "*").count();
            let similarity = equal as f64 / stable.max(1) as f64;
            (cluster.tokens == words
                || (equal >= 2 || words.len() == 1 && equal == 1) && similarity >= 0.7)
                .then_some((id, equal, stable))
        })
        .max_by_key(|(id, equal, stable)| (*equal, *stable, std::cmp::Reverse(*id)))
        .map(|v| v.0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub count: u64,
    pub failures: u64,
    pub clusters: Vec<Cluster>,
    pub latency: Histogram,
    pub bytes: Histogram,
    pub transitions: BTreeMap<String, u64>,
    pub outgoing: BTreeMap<u32, u64>,
    pub combinations: BTreeMap<String, u64>,
    pub overflow: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub schema_version: u32,
    pub limits: Limits,
    pub contexts: BTreeMap<String, Context>,
    pub generation: u64,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA,
            limits: Limits::default(),
            contexts: BTreeMap::new(),
            generation: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Observation<'a> {
    pub message: &'a str,
    pub failure: bool,
    pub latency_ms: Option<f64>,
    pub bytes: Option<f64>,
}
impl Observation<'_> {
    fn combination(&self, cluster: u32) -> String {
        format!(
            "{cluster}:{}:{}:{}",
            u8::from(self.failure),
            self.latency_ms
                .and_then(Histogram::bin)
                .map_or(32, |b| b / 2),
            self.bytes.and_then(Histogram::bin).map_or(32, |b| b / 2)
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Deviation {
    pub kind: String,
    pub surprise: f64,
    pub support: u64,
    pub detail: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Assessment {
    pub ready: bool,
    pub cluster: Option<u32>,
    pub deviations: Vec<Deviation>,
}
impl Assessment {
    pub fn unusual(&self) -> bool {
        !self.deviations.is_empty()
    }
}

impl Model {
    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()?;
        if self.schema_version != SCHEMA || self.contexts.len() > self.limits.max_contexts {
            return Err("Modelo local incompatível ou acima do orçamento".into());
        }
        for (key, c) in &self.contexts {
            if key.len() != 64
                || !key.bytes().all(|b| b.is_ascii_hexdigit())
                || c.clusters.len() > self.limits.max_templates
                || c.transitions.len() > self.limits.max_transitions
                || c.combinations.len() > self.limits.max_combinations
                || c.count > 1_000_000_000
                || c.failures > c.count
                || c.outgoing.len() > self.limits.max_templates
                || c.clusters.iter().any(|t| {
                    t.tokens.len() > 48
                        || t.tokens.iter().any(|s| s.len() > 192)
                        || t.count > c.count
                })
                || c.transitions
                    .iter()
                    .any(|(k, v)| k.len() > 24 || *v > c.count)
                || c.combinations
                    .iter()
                    .any(|(k, v)| k.len() > 48 || *v > c.count)
                || c.latency.count() > c.count
                || c.bytes.count() > c.count
            {
                return Err("Estrutura ou contagens do modelo local inválidas".into());
            }
        }
        Ok(())
    }
    pub fn train(
        &mut self,
        key: &str,
        event: &Observation<'_>,
        previous: Option<u32>,
    ) -> Option<u32> {
        if !self.contexts.contains_key(key) && self.contexts.len() >= self.limits.max_contexts {
            return None;
        }
        let words = tokens(event.message);
        let c = self.contexts.entry(key.into()).or_default();
        if c.count >= 65_536 {
            c.halve();
        }
        c.count += 1;
        c.failures += u64::from(event.failure);
        c.latency.push(event.latency_ms);
        c.bytes.push(event.bytes);
        let cluster = if let Some(id) = matching(&c.clusters, &words) {
            let pattern = &mut c.clusters[id];
            pattern.count += 1;
            for (a, b) in pattern.tokens.iter_mut().zip(&words) {
                if a != b {
                    *a = "*".into();
                }
            }
            Some(id)
        } else if !words.is_empty() && c.clusters.len() < self.limits.max_templates {
            c.clusters.push(Cluster {
                tokens: words,
                count: 1,
            });
            Some(c.clusters.len() - 1)
        } else {
            c.overflow += 1;
            None
        };
        if let Some(id) = cluster {
            let combination = event.combination(id as u32);
            if c.combinations.contains_key(&combination)
                || c.combinations.len() < self.limits.max_combinations
            {
                *c.combinations.entry(combination).or_default() += 1;
            } else {
                c.overflow += 1;
            }
            if let Some(before) = previous.filter(|v| (*v as usize) < c.clusters.len()) {
                *c.outgoing.entry(before).or_default() += 1;
                let transition = format!("{before}>{id}");
                if c.transitions.contains_key(&transition)
                    || c.transitions.len() < self.limits.max_transitions
                {
                    *c.transitions.entry(transition).or_default() += 1;
                } else {
                    c.overflow += 1;
                }
            }
        }
        cluster.map(|id| id as u32)
    }
    pub fn score(&self, key: &str, event: &Observation<'_>, previous: Option<u32>) -> Assessment {
        let Some(c) = self.contexts.get(key) else {
            return Assessment::default();
        };
        let cluster = matching(&c.clusters, &tokens(event.message)).map(|id| id as u32);
        let mut result = Assessment {
            ready: c.count >= self.limits.minimum_support,
            cluster,
            deviations: Vec::new(),
        };
        if !result.ready {
            return result;
        }
        let mut add = |kind: &str, probability: f64, support: u64, detail: String| {
            result.deviations.push(Deviation {
                kind: kind.into(),
                surprise: -probability.max(1e-12).log2(),
                support,
                detail,
            });
        };
        if cluster.is_none() && !event.message.trim().is_empty() && c.overflow == 0 {
            add("message_novelty",1.0/(c.count+1) as f64,c.count,"Família de mensagem ausente na referência local; conferir alteração legítima e conteúdo original.".into());
        }
        for (kind, value, histogram) in [
            ("latency_tail", event.latency_ms, &c.latency),
            ("bytes_tail", event.bytes, &c.bytes),
        ] {
            if let Some(value) = value.filter(|v| v.is_finite() && *v >= 0.0) {
                let tail = histogram.tail(value);
                if histogram.count() >= self.limits.minimum_support
                    && tail < 0.02
                    && value > histogram.upper_quantile(0.95).max(1.0) * 2.0
                {
                    add(kind,tail,histogram.count(),format!("Valor {value:.2} acima da referência p95 aproximada {:.2}; unidades preservadas.",histogram.upper_quantile(0.95)));
                }
            }
        }
        if event.failure && (c.failures as f64 / c.count as f64) < 0.02 {
            add(
                "unexpected_failure",
                (c.failures as f64 + 0.5) / (c.count as f64 + 1.0),
                c.count,
                format!(
                    "Falha observada; referência contém {} falhas em {} observações.",
                    c.failures, c.count
                ),
            );
        }
        if let Some(id) = cluster {
            if c.count >= self.limits.minimum_support * 3
                && c.combinations
                    .get(&event.combination(id))
                    .copied()
                    .unwrap_or(0)
                    == 0
                && c.overflow == 0
                && event.latency_ms.is_some()
                && event.bytes.is_some()
            {
                add("joint_novelty",1.0/(c.count+c.combinations.len() as u64+1) as f64,c.count,"Combinação de mensagem, resultado, duração e tamanho ausente na referência; não implica valores individuais extremos.".into());
            }
            if let Some(before) = previous {
                let outgoing = c.outgoing.get(&before).copied().unwrap_or(0);
                let count = c
                    .transitions
                    .get(&format!("{before}>{id}"))
                    .copied()
                    .unwrap_or(0);
                let p =
                    (count as f64 + 0.5) / (outgoing as f64 + 0.5 * (c.clusters.len() + 1) as f64);
                if outgoing >= self.limits.minimum_support && p < 0.02 && c.overflow == 0 {
                    let prefix = format!("{before}>");
                    let expected = c
                        .transitions
                        .iter()
                        .filter(|(k, _)| k.starts_with(&prefix))
                        .max_by_key(|(_, n)| **n)
                        .and_then(|(k, n)| {
                            k.split_once('>')
                                .and_then(|(_, s)| s.parse::<usize>().ok())
                                .map(|i| (i, *n))
                        });
                    let detail=expected.map(|(i,n)|format!("Após '{}', a referência segue para '{}' em {n}/{outgoing} transições; o caminho observado é incomum. Vínculo exige identificador explícito e horários distintos.",c.clusters[before as usize].tokens.join(" "),c.clusters[i].tokens.join(" ")))
                        .unwrap_or_else(||"Transição pouco observada na referência local.".into());
                    add("sequence_surprise", p, outgoing, detail);
                }
            }
        }
        result
    }
}
impl Context {
    fn halve(&mut self) {
        self.count /= 2;
        self.failures /= 2;
        self.latency.halve();
        self.bytes.halve();
        for t in &mut self.clusters {
            t.count /= 2;
        }
        for n in self
            .transitions
            .values_mut()
            .chain(self.outgoing.values_mut())
            .chain(self.combinations.values_mut())
        {
            *n /= 2;
        }
        self.overflow = self.overflow.div_ceil(2);
    }
}

/// Wilson bounds keep tiny cohorts from looking like convincing explanations.
pub fn rate_interval(hits: u64, total: u64) -> (f64, f64) {
    if total == 0 {
        return (0.0, 1.0);
    }
    let n = total as f64;
    let p = hits.min(total) as f64 / n;
    let z2 = 3.8416;
    let center = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
    let radius = 1.96 * ((p * (1.0 - p) + z2 / (4.0 * n)) / n).sqrt() / (1.0 + z2 / n);
    ((center - radius).max(0.0), (center + radius).min(1.0))
}
