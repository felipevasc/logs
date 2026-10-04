//! Immutable offline indicator packs. Bloom is only a negative prefilter;
//! every returned hit is confirmed against the exact canonical value.
use crate::{
    entities::{self, Role},
    model::Event,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub id: String,
    pub revision: String,
    pub source: String,
    pub indicators: Vec<Indicator>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Indicator {
    pub id: String,
    pub kind: String,
    pub value: String,
    pub valid_from: Option<i64>,
    pub expires: Option<i64>,
    pub context: String,
}
pub struct Index {
    exact: BTreeMap<(String, String), Vec<Indicator>>,
    bloom: Bloom,
}
pub struct Bloom {
    bits: Vec<u64>,
    hashes: usize,
    pub inserted: usize,
}
impl Bloom {
    pub fn new(capacity: usize) -> Self {
        Self {
            bits: vec![0; capacity.max(1).saturating_mul(12).div_ceil(64)],
            hashes: 8,
            inserted: 0,
        }
    }
    fn hashes(&self, key: &str) -> impl Iterator<Item = usize> + use<> {
        let digest = Sha256::digest(key.as_bytes());
        let a = u64::from_le_bytes(digest[..8].try_into().unwrap());
        let b = u64::from_le_bytes(digest[8..16].try_into().unwrap()) | 1;
        let size = self.bits.len() * 64;
        let hashes = self.hashes;
        (0..hashes).map(move |i| a.wrapping_add((i as u64).wrapping_mul(b)) as usize % size)
    }
    pub fn insert(&mut self, key: &str) {
        for bit in self.hashes(key) {
            self.bits[bit / 64] |= 1u64 << (bit % 64);
        }
        self.inserted += 1;
    }
    pub fn may_contain(&self, key: &str) -> bool {
        self.hashes(key)
            .all(|bit| self.bits[bit / 64] & (1u64 << (bit % 64)) != 0)
    }
    /// Estimate under uniform independent hashing, never a hard bound.
    pub fn false_positive_estimate(&self) -> f64 {
        (1.0 - (-((self.hashes * self.inserted) as f64) / (self.bits.len() * 64) as f64).exp())
            .powi(self.hashes as i32)
    }
}
pub fn canonical(kind: &str, value: &str) -> Option<String> {
    match kind {
        "ip" => value.parse::<std::net::IpAddr>().ok().map(|ip| match ip {
            std::net::IpAddr::V6(ip) => ip
                .to_ipv4_mapped()
                .map_or_else(|| ip.to_string(), |ip| ip.to_string()),
            ip => ip.to_string(),
        }),
        "domain" => {
            let value = value.trim_end_matches('.').to_ascii_lowercase();
            (!value.is_empty()
                && value.len() <= 253
                && value.split('.').all(|s| {
                    !s.is_empty()
                        && s.len() <= 63
                        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                }))
            .then_some(value)
        }
        "sha256" | "sha1" | "md5" => {
            let size = match kind {
                "sha256" => 64,
                "sha1" => 40,
                _ => 32,
            };
            (value.len() == size && value.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| value.to_ascii_lowercase())
        }
        "url" => (value.len() <= 8192
            && (value.starts_with("https://") || value.starts_with("http://")))
        .then(|| value.to_string()),
        _ => None,
    }
}
impl Catalog {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self.id.is_empty()
            || self.id.len() > 128
            || self.revision.is_empty()
            || self.source.is_empty()
            || self.indicators.len() > 20_000
        {
            return Err("Catálogo IOC exige versão 1, identidade, revisão, procedência e até 20.000 entradas".into());
        }
        let mut ids = std::collections::HashSet::new();
        for i in &self.indicators {
            if !ids.insert(&i.id)
                || i.id.is_empty()
                || i.id.len() > 160
                || canonical(&i.kind, &i.value).is_none()
                || i.context.len() > 2048
                || i.valid_from.zip(i.expires).is_some_and(|(a, b)| a >= b)
            {
                return Err(format!(
                    "IOC inválido, duplicado ou com intervalo incompatível: {}",
                    i.id
                ));
            }
        }
        Ok(())
    }
}
impl Index {
    pub fn new(catalog: &Catalog) -> Result<Self, String> {
        catalog.validate()?;
        let mut exact = BTreeMap::new();
        let mut bloom = Bloom::new(catalog.indicators.len());
        for indicator in &catalog.indicators {
            let value = canonical(&indicator.kind, &indicator.value).unwrap();
            bloom.insert(&format!("{}:{value}", indicator.kind));
            exact
                .entry((indicator.kind.clone(), value))
                .or_insert_with(Vec::new)
                .push(indicator.clone());
        }
        Ok(Self { exact, bloom })
    }
    pub fn matches(&self, event: &Event) -> Vec<(Indicator, String, String, bool)> {
        let mut values = Vec::new();
        for (kind, role) in [
            ("ip", Role::SrcIp),
            ("ip", Role::DstIp),
            ("domain", Role::Domain),
            ("url", Role::Url),
        ] {
            if let Some(value) = entities::explicit_value(event, role) {
                values.push((
                    kind.to_string(),
                    value.into_owned(),
                    entities::info(role).column.to_string(),
                ));
            }
        }
        for (kind, path) in [
            ("ip", "_sec.source_address"),
            ("ip", "_sec.destination"),
            ("url", "_sec.url"),
        ] {
            if let Some(value) = crate::security_normalize::field_text(event, path) {
                values.push((kind.into(), value, path.into()));
            }
        }
        for (kind, paths) in [
            (
                "sha256",
                vec!["file.hash.sha256", "process.hash.sha256", "sha256"],
            ),
            ("sha1", vec!["file.hash.sha1", "process.hash.sha1", "sha1"]),
            ("md5", vec!["file.hash.md5", "process.hash.md5", "md5"]),
        ] {
            for path in paths {
                if let Some(value) = crate::security_normalize::field_text(event, path) {
                    values.push((kind.to_string(), value, path.into()));
                }
            }
        }
        if let Some(hashes) = entities::explicit_value(event, Role::Hash) {
            for part in hashes.split(',') {
                if let Some((kind, value)) = part.trim().split_once('=') {
                    values.push((kind.to_ascii_lowercase(), value.to_string(), "@hash".into()));
                }
            }
        }
        let mut hits = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (kind, value, field) in values {
            let Some(value) = canonical(&kind, &value) else {
                continue;
            };
            if !self.bloom.may_contain(&format!("{kind}:{value}")) {
                continue;
            }
            if let Some(indicators) = self.exact.get(&(kind, value.clone())) {
                for indicator in indicators {
                    let known_time = event.timestamp.is_some();
                    if event.timestamp.is_some_and(|ts| {
                        indicator.valid_from.is_some_and(|start| ts < start)
                            || indicator.expires.is_some_and(|end| ts >= end)
                    }) {
                        continue;
                    }
                    if seen.insert((indicator.id.clone(), field.clone())) {
                        hits.push((indicator.clone(), value.clone(), field.clone(), known_time));
                    }
                }
            }
        }
        hits
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_bloom_never_discards_inserted_indicators_and_exact_confirmation_rejects_near_values() {
        let mut b = Bloom::new(5000);
        for n in 0..5000 {
            b.insert(&n.to_string());
        }
        for n in 0..5000 {
            assert!(b.may_contain(&n.to_string()));
        }
        assert!(b.false_positive_estimate() < 0.01);
        let c = Catalog {
            schema_version: 1,
            id: "c".into(),
            revision: "1".into(),
            source: "fixture".into(),
            indicators: vec![Indicator {
                id: "ip".into(),
                kind: "ip".into(),
                value: "192.0.2.1".into(),
                valid_from: Some(100),
                expires: Some(200),
                context: "test".into(),
            }],
        };
        let i = Index::new(&c).unwrap();
        let mut e = Event::empty();
        e.timestamp = Some(150);
        e.fields = json!({"destination.ip":"192.0.2.10"})
            .as_object()
            .unwrap()
            .clone();
        assert!(i.matches(&e).is_empty());
        e.fields.insert("destination.ip".into(), json!("192.0.2.1"));
        assert_eq!(i.matches(&e).len(), 1);
        e.timestamp = Some(200);
        assert!(i.matches(&e).is_empty());
        e.timestamp = None;
        assert!(!i.matches(&e)[0].3);
        e.fields.clear();
        e.message = "destination ip=192.0.2.1".into();
        assert!(i.matches(&e).is_empty());
    }
}
