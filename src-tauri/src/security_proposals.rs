//! Deterministic proposals require labelled controls and deliberate acceptance.
//! Evaluation in one Case is not representative validation or an evidence grade.
use crate::detections::RuleDef;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub const VERSION: &str = "proposals-1";
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Controls {
    pub positive: Vec<String>,
    pub negative: Vec<String>,
    pub holdout_declaration: String,
}
impl Controls {
    pub fn validate(&self) -> Result<(), String> {
        if self.positive.is_empty()
            || self.negative.is_empty()
            || self.positive.len() + self.negative.len() > 2000
            || self.holdout_declaration.trim().is_empty()
            || self.holdout_declaration.len() > 1024
        {
            return Err("Avaliação exige positivos e negativos rotulados (até 2000 referências) e declaração da separação por origem/tempo".into());
        }
        let mut refs = std::collections::HashSet::new();
        for reference in self.positive.iter().chain(&self.negative) {
            if reference.is_empty() || reference.len() > 512 || !refs.insert(reference) {
                return Err(
                    "Referências vazias, repetidas ou compartilhadas entre controles".into(),
                );
            }
        }
        Ok(())
    }
}
pub fn propose(signal: &Value, analysis_id: &str) -> Result<Value, String> {
    let kind = signal["kind"].as_str().ok_or("Sinal sem tipo")?;
    let value = signal["value"].as_str().ok_or("Sinal sem entidade")?;
    let namespace = signal["namespace"].as_str().ok_or("Sinal sem namespace")?;
    let quoted = serde_json::to_string(value).map_err(|e| e.to_string())?;
    let mut candidate = json!({"id":crate::evidence::stable_id("proposal",[analysis_id,signal["id"].as_str().ok_or("Sinal sem ID")?]),"name":format!("Proposta revisável: {kind}"),"description":"Seletor derivado de observações, sujeito a avaliação de controles e revisão humana.","severity":"medium","kind":"single","where":"","by":[],"enabled":false,"evidence":{"level":1,"maturity":"unassessed","version":VERSION,"claim":"activity","rationale":"Proposta sem calibração representativa; correspondências são observações a revisar.","missing":["Validação independente e análise do contexto"],"benign":signal["benign_alternatives"]},"provenance":{"proposal_version":VERSION,"proposal_namespace":namespace,"origin_analysis":analysis_id,"origin_signal":signal["id"]}});
    match kind {
        "password_spraying" => {
            candidate["kind"] = json!("distinct");
            candidate["where"] = json!(format!(
                "_sec.action:logon _sec.outcome:failure @src_ip={quoted}"
            ));
            candidate["by"] = json!(["@src_ip"]);
            candidate["distinct"] = json!("@user");
            candidate["count"] = json!(20);
            candidate["window"] = json!("24h");
        }
        "authentication_chain" => {
            candidate["kind"] = json!("sequence");
            candidate["by"] = json!(["_sec.actor", "@src_ip"]);
            candidate["window"] = json!("30m");
            candidate["steps"] = json!([{"where":format!("_sec.action:logon _sec.outcome:failure _sec.actor={quoted}"),"count":10},{"where":format!("_sec.action:logon _sec.outcome:success _sec.actor={quoted}"),"count":1}]);
        }
        "lolbin_context" => {
            candidate["where"]=json!(format!("_sec.action:(process_start OR script_execution) _sec.suspicious_command:true @process={quoted}"));
        }
        _ => {
            return Ok(
                json!({"version":VERSION,"analysis_id":analysis_id,"signal_id":signal["id"],"status":"requires_manual_design","reason":"Este sinal depende de referência estatística, políticas, validade de IOC ou vínculos que não podem ser convertidos em seletor isolado sem alterar o significado.","automatic_activation":false}),
            )
        }
    }
    let _: RuleDef = serde_json::from_value(candidate.clone()).map_err(|e| e.to_string())?;
    Ok(
        json!({"version":VERSION,"analysis_id":analysis_id,"signal_id":signal["id"],"status":"draft","rule":candidate,"rule_hash":crate::evidence::stable_id(VERSION,[candidate.to_string()]),"automatic_activation":false,"evaluation_required":true,"evidence_level":0,"limitation":"Um seletor aprendido desta entidade pode memorizar o incidente. Separe controles por origem/tempo; a ativação permanece sem nível avaliado."}),
    )
}
pub fn accepted(proposal: &Value, evaluation: &Value, rationale: &str) -> Result<RuleDef, String> {
    if rationale.trim().len() < 12 || rationale.len() > 2000 {
        return Err("Registre o motivo da revisão (12–2000 bytes)".into());
    }
    if proposal["rule_hash"] != evaluation["rule_hash"]
        || proposal["analysis_id"] != evaluation["analysis_id"]
        || evaluation["complete"] != true
        || evaluation["activation_gate"] != true
    {
        return Err("Avaliação ausente, desatualizada ou com controles não satisfeitos; proposta não ativada".into());
    }
    let mut rule: RuleDef =
        serde_json::from_value(proposal["rule"].clone()).map_err(|e| e.to_string())?;
    rule.enabled = true;
    rule.provenance["review_rationale"] = json!(rationale);
    rule.provenance["evaluation"] = evaluation.clone();
    rule.evidence.maturity = "unassessed".into();
    rule.evidence.validation = None;
    Ok(rule)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_proposals_require_exact_current_evaluation_and_never_promote_evidence() {
        let signal = json!({"id":"s","kind":"password_spraying","value":"192.0.2.1","namespace":"A","benign_alternatives":[]});
        let proposal = propose(&signal, "a").unwrap();
        assert_eq!(proposal["rule"]["enabled"], false);
        let evaluation = json!({"analysis_id":"a","rule_hash":proposal["rule_hash"],"complete":true,"activation_gate":true});
        let accepted =
            accepted(&proposal, &evaluation, "Revisão dos controles e contexto").unwrap();
        assert!(accepted.enabled);
        assert_eq!(accepted.evidence.assessed_level(), 0);
        assert!(super::accepted(
            &proposal,
            &json!({"analysis_id":"old"}),
            "Motivo de revisão suficiente"
        )
        .is_err());
        assert!(Controls {
            positive: vec!["ref".into()],
            negative: vec!["ref".into()],
            holdout_declaration: "tempo".into()
        }
        .validate()
        .is_err());
    }
}
