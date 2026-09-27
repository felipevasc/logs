//! Versioned evidence policy. Impact never determines confidence.
use serde::{Deserialize, Serialize};

pub const POLICY_VERSION: &str = "evidence-3";
pub const NORMALIZATION_VERSION: &str = "normalization-4";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub level: u8,
    pub rationale: String,
    pub missing: Vec<String>,
    pub benign: Vec<String>,
    pub required: Vec<String>,
    pub products: Vec<String>,
    pub maturity: String,
    pub version: String,
    /// A separately reviewed, held-out evaluation, not synthetic unit tests.
    pub validation: Option<Validation>,
    /// What the evidence establishes. Strength does not assert successful compromise.
    pub claim: String,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            level: 1,
            rationale: "Correspondência ainda sem avaliação contextual.".into(),
            missing: vec!["Corroboração e validação representativa da regra".into()],
            benign: vec![],
            required: vec![],
            products: vec![],
            maturity: "unassessed".into(),
            version: "1".into(),
            validation: None,
            claim: "activity".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    pub corpus: String,
    pub held_out: bool,
    pub true_positives: usize,
    pub false_positives: usize,
}

impl Policy {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=5).contains(&self.level) {
            return Err("evidence.level deve estar entre 1 e 5".into());
        }
        if self.rationale.trim().is_empty() {
            return Err("A regra precisa justificar seu nível de evidência".into());
        }
        if !["activity", "attempt", "execution", "effect"].contains(&self.claim.as_str()) {
            return Err("evidence.claim deve ser activity, attempt, execution ou effect".into());
        }
        Ok(())
    }
    pub fn assessed_level(&self) -> u8 {
        if self.maturity == "unassessed" {
            return 0;
        }
        self.level
    }
}

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
pub struct Evidence {
    pub evidence_level: u8,
    pub evidence_reasons: Vec<String>,
    #[serde(default)]
    pub conditions_satisfied: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub benign_alternatives: Vec<String>,
    pub rule_version: String,
    pub policy_version: String,
    pub normalization_version: String,
    pub maturity: String,
    pub outcome: String,
    pub evaluation: String,
    pub event_refs: Vec<String>,
    pub evidence_members: Vec<Member>,
    pub relationships: Vec<Relationship>,
    #[serde(default)]
    pub excerpts: Vec<Excerpt>,
    #[serde(default)]
    pub claim: String,
    #[serde(default)]
    pub validation: Option<Validation>,
    #[serde(default)]
    pub checks: Vec<Check>,
}

/// Evaluation facts, not generated explanations. Unknown is never a failed test.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Check {
    pub status: String,
    pub label: String,
    pub expected: String,
    pub observed: String,
    pub event_refs: Vec<String>,
    pub fields: Vec<String>,
}

/// Byte offsets refer to the named field after the declared transformation.
/// Only positive matched predicates may produce an excerpt.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Excerpt {
    pub event_ref: String,
    pub field: String,
    pub transformation: String,
    pub start: usize,
    pub end: usize,
    pub before: String,
    pub matched: String,
    pub after: String,
    pub prefix_omitted: bool,
    pub suffix_omitted: bool,
    pub match_truncated: bool,
}
impl Excerpt {
    pub fn new(
        reference: &str,
        field: &str,
        transformation: &str,
        value: &str,
        start: usize,
        end: usize,
    ) -> Option<Self> {
        if start >= end
            || end > value.len()
            || !value.is_char_boundary(start)
            || !value.is_char_boundary(end)
        {
            return None;
        }
        let a = value[..start]
            .char_indices()
            .rev()
            .nth(80)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let b = value[end..]
            .char_indices()
            .nth(80)
            .map(|(i, _)| end + i)
            .unwrap_or(value.len());
        let cut = value[start..end]
            .char_indices()
            .nth(400)
            .map(|(i, _)| start + i)
            .unwrap_or(end);
        Some(Self {
            event_ref: reference.into(),
            field: field.into(),
            transformation: transformation.into(),
            start,
            end,
            before: value[a..start].into(),
            matched: value[start..cut].into(),
            after: value[end..b].into(),
            prefix_omitted: a > 0,
            suffix_omitted: b < value.len(),
            match_truncated: cut < end,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Member {
    pub event_ref: String,
    pub event_id: usize,
    pub step: usize,
    pub fields: Vec<String>,
    #[serde(default)]
    pub provenance: std::collections::BTreeMap<String, crate::security_normalize::Provenance>,
    #[serde(default)]
    pub time: crate::security_normalize::TimeProvenance,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Relationship {
    pub kind: String,
    pub fields: Vec<String>,
    pub description: String,
}

impl Evidence {
    pub fn from_policy(policy: &Policy) -> Self {
        Self {
            evidence_level: policy.assessed_level(),
            evidence_reasons: vec![policy.rationale.clone()],
            missing_evidence: policy.missing.clone(),
            benign_alternatives: policy.benign.clone(),
            rule_version: policy.version.clone(),
            policy_version: POLICY_VERSION.into(),
            normalization_version: NORMALIZATION_VERSION.into(),
            maturity: policy.maturity.clone(),
            claim: policy.claim.clone(),
            validation: policy.validation.clone(),
            outcome: "unknown".into(),
            evaluation: "complete".into(),
            ..Default::default()
        }
    }
    pub fn limit(&mut self, maximum: u8, reason: &str) {
        self.evidence_level = self.evidence_level.min(maximum);
        if !self.missing_evidence.iter().any(|s| s == reason) {
            self.missing_evidence.push(reason.into());
            self.checks.push(Check { status: "unknown".into(), label: reason.into(),
                expected: format!("Contexto necessário para ultrapassar {}", label(maximum)),
                observed: format!("Nível limitado a {}", label(maximum)), event_refs: vec![], fields: vec![] });
        }
        self.evaluation = "partial".into();
    }
}

pub fn label(level: u8) -> &'static str {
    match level {
        5 => "Quase confirmado",
        4 => "Forte indício",
        3 => "Indício",
        2 => "Suspeita",
        1 => "Inconclusivo",
        _ => "Nível não avaliado",
    }
}

pub fn stable_id(prefix: &str, parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        let s = part.as_ref();
        hash.update((s.len() as u64).to_le_bytes());
        hash.update(s.as_bytes());
    }
    format!("{prefix}-{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strength_is_independent_of_validation_and_outcome() {
        let mut policy = Policy { level: 5, maturity: "experimental".into(), ..Default::default() };
        assert_eq!(policy.assessed_level(), 5);
        policy.validation = Some(Validation {
            corpus: "reserved-review".into(),
            held_out: true,
            true_positives: 199,
            false_positives: 1,
        });
        assert_eq!(policy.assessed_level(), 5);
        policy.validation.as_mut().unwrap().held_out = false;
        assert_eq!(policy.assessed_level(), 5);
        policy.maturity = "unassessed".into();
        assert_eq!(policy.assessed_level(), 0);
    }
}
