use crate::{
    model::Event,
    search_regex::{Matcher, Pattern},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
/// Bump when map/extraction interpretation changes; legacy overlays retain their key.
pub(crate) const VERSION: &str = "advanced-fields-1";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Definition {
    Captures {
        pattern: Pattern,
        capture: String,
    },
    Map {
        map: crate::enrichment_maps::Definition,
    },
}
impl Definition {
    /// Identity of what the definition computes (see `enrichment_maps::semantic_signature`).
    pub(crate) fn signature(&self) -> String {
        match self {
            Definition::Map { map } => crate::enrichment_maps::semantic_signature(map),
            other => serde_json::to_string(other).unwrap_or_default(),
        }
    }
}
pub(crate) struct Compiled {
    pub definition: Definition,
    matcher: Option<Matcher>,
    /// A map is compiled once (patterns, conditions, extractions) and reused for every record.
    map: Option<Arc<crate::enrichment_maps::Compiled>>,
}
impl Compiled {
    pub(crate) fn new(definition: Definition) -> Result<Arc<Self>, String> {
        Self::with_limits(definition, crate::analysis_context::regex_limits(1)?)
    }
    pub(crate) fn with_limits(
        definition: Definition,
        limits: crate::analysis_context::RegexLimits,
    ) -> Result<Arc<Self>, String> {
        let mut map = None;
        let matcher = match &definition {
            Definition::Captures { pattern, capture } => {
                if capture.is_empty() || capture.len() > 128 {
                    return Err("Informe a captura a extrair.".into());
                }
                let matcher = Matcher::with_limits(pattern, limits)?;
                if !matcher.capture_exists(capture) {
                    return Err("A captura informada não existe no padrão.".into());
                }
                Some(matcher)
            }
            Definition::Map { map: definition } => {
                map = Some(crate::enrichment_maps::Compiled::new(definition.clone())?);
                None
            }
        };
        Ok(Arc::new(Self {
            definition,
            matcher,
            map,
        }))
    }
    pub(crate) fn evaluate(&self, event: &Event, source: &str) -> Result<Option<Value>, String> {
        match &self.definition {
            Definition::Map { .. } => self.map.as_ref().expect("compiled map").evaluate(event, None),
            Definition::Captures { capture, .. } => {
                let Some(value) = event.col_ref(source) else {
                    return Ok(None);
                };
                let result = self.matcher.as_ref().unwrap().extract(&value, capture)?;
                Ok(result
                    .spans
                    .first()
                    .and_then(|span| span.captures.get(capture))
                    .map(|value| Value::String(value.clone())))
            }
        }
    }
}
