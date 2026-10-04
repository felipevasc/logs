//! Optional assisted synthesis is a client workflow. A draft must return through
//! this boundary so citations can be checked against the admitted population.
//! Citation membership is verified; factual wording still requires human review.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub claims: Vec<Claim>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub event_refs: Vec<String>,
    #[serde(default)]
    pub signal_ids: Vec<String>,
}
pub fn verify(
    draft: &Draft,
    mut event: impl FnMut(&str) -> Result<bool, String>,
    mut signal: impl FnMut(&str) -> Result<bool, String>,
) -> Result<Value, String> {
    if draft.claims.is_empty() || draft.claims.len() > 24 {
        return Err("A síntese exige de 1 a 24 afirmações".into());
    }
    let mut text_bytes = 0;
    for claim in &draft.claims {
        crate::operations::check()?;
        text_bytes += claim.text.len();
        if claim.text.trim().is_empty()
            || claim.text.len() > 1600
            || text_bytes > 6000
            || claim.event_refs.len() > 8
            || claim.signal_ids.len() > 4
        {
            return Err("A síntese excede o orçamento de texto ou citações".into());
        }
        if !matches!(
            claim.kind.as_str(),
            "observation" | "hypothesis" | "alternative" | "missing_telemetry"
        ) {
            return Err(
                "kind deve ser observation, hypothesis, alternative ou missing_telemetry".into(),
            );
        }
        if claim.kind == "observation" && claim.event_refs.is_empty() {
            return Err("Uma observação exige referência de um evento original".into());
        }
        if claim.kind == "hypothesis" && claim.event_refs.is_empty() && claim.signal_ids.is_empty()
        {
            return Err("Uma hipótese exige referência verificável".into());
        }
        for reference in &claim.event_refs {
            if reference.len() > 512 || !event(reference)? {
                return Err(format!("Citação fora do recorte admitido: {reference}"));
            }
        }
        for id in &claim.signal_ids {
            if id.len() > 256 || !signal(id)? {
                return Err(format!("Sinal fora do recorte admitido: {id}"));
            }
        }
    }
    Ok(
        json!({"version":"assisted-review-1","citation_membership":"verified_in_admitted_scope","semantic_validation":"human_review_required","classification_updated":false,"claims":draft.claims,"limitations":["A existência da citação não comprova que o texto a descreve corretamente.","Prioridade e síntese não promovem evidência; afirmações causais exigem corroboração."]}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_assisted_drafts_require_original_citations_reject_foreign_scope_and_never_change_classification(
    ) {
        let mut draft = Draft {
            claims: vec![Claim {
                kind: "observation".into(),
                text: "Processo observado".into(),
                event_refs: vec!["tenant-a:1".into()],
                signal_ids: vec![],
            }],
        };
        let check = |d: &Draft| verify(d, |r| Ok(r == "tenant-a:1"), |s| Ok(s == "signal-a"));
        let approved = check(&draft).unwrap();
        assert_eq!(approved["classification_updated"], false);
        assert_eq!(approved["semantic_validation"], "human_review_required");
        draft.claims[0].event_refs[0] = "tenant-b:1".into();
        assert!(check(&draft).is_err());
        draft.claims[0].event_refs.clear();
        assert!(check(&draft).is_err());
        draft.claims[0].kind = "hypothesis".into();
        draft.claims[0].signal_ids.push("signal-a".into());
        assert!(check(&draft).is_ok());
        draft.claims[0].kind = "E5".into();
        assert!(check(&draft).is_err());
        assert!(serde_json::from_value::<Draft>(json!({"claims":[],"evidence_level":5})).is_err());
    }
}
