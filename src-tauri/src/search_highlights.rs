use crate::search_regex::{Matcher, Pattern};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Rule {
    pub id: String,
    pub label: String,
    pub color: String,
    pub pattern: Pattern,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Mark {
    pub start: usize,
    pub end: usize,
    pub rule_id: String,
    pub label: String,
    pub color: String,
}
pub(crate) struct Compiled {
    rules: Vec<(Rule, Matcher)>,
}
impl Compiled {
    pub(crate) fn new(rules: &[Rule]) -> Result<Self, String> {
        if rules.len() > 16 {
            return Err("Use até 16 regras visuais por Caso.".into());
        }
        let mut output = Vec::new();
        let limits = crate::analysis_context::regex_limits(rules.len())?;
        let mut ids = std::collections::HashSet::new();
        for rule in rules {
            if rule.id.is_empty()
                || rule.id.len() > 128
                || rule.label.len() > 128
                || !ids.insert(&rule.id)
                || rule.color.len() != 7
                || !rule.color.starts_with('#')
                || !rule.color[1..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(
                    "Regra visual inválida: use id único, rótulo e cor hexadecimal.".into(),
                );
            }
            if rule.enabled {
                output.push((rule.clone(), Matcher::with_limits(&rule.pattern, limits)?));
            }
        }
        Ok(Self { rules: output })
    }
    pub(crate) fn apply(&self, text: &str) -> Result<Vec<Mark>, String> {
        let mut marks = Vec::new();
        for (rule, matcher) in &self.rules {
            let matches = matcher.find_spans(text)?;
            if !matches.complete {
                return Err(format!(
                    "Destaques incompletos para {}: mais de 512 ocorrências por registro.",
                    rule.label
                ));
            }
            for span in matches.spans {
                marks.push(Mark {
                    start: span.start,
                    end: span.end,
                    rule_id: rule.id.clone(),
                    label: rule.label.clone(),
                    color: rule.color.clone(),
                });
            }
        }
        Ok(marks)
    }
}
