//! Suggestions from structured original fields, kept separate from semantic facts.
//! An address-shaped value never establishes source/destination or actor/target.
use crate::model::Event;
use serde_json::{json, Value};
use std::collections::BTreeMap;
pub const VERSION: &str = "schema-inference-1";
#[derive(Default)]
struct Field {
    present: u64,
    nonempty: u64,
    types: BTreeMap<&'static str, u64>,
    refs: Vec<String>,
}
#[derive(Default)]
pub struct Schema {
    fields: BTreeMap<(String, String, String), Field>,
    totals: BTreeMap<(String, String), u64>,
    omitted: u64,
}
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(s) if s.trim().is_empty() => "empty",
        Value::String(s) if s.parse::<std::net::IpAddr>().is_ok() => "ip_address",
        Value::String(s) if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) => {
            "sha256_shape"
        }
        Value::String(s) if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) => {
            "sha1_shape"
        }
        Value::String(s) if s.starts_with("https://") || s.starts_with("http://") => "http_url",
        Value::String(s) if s.len() < 128 && chrono::DateTime::parse_from_rfc3339(s).is_ok() => {
            "rfc3339_timestamp"
        }
        Value::String(s) if s.len() < 128 && s.parse::<f64>().is_ok_and(f64::is_finite) => {
            "numeric_text"
        }
        Value::String(_) => "text",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
impl Schema {
    pub fn observe(&mut self, event: &Event, namespace: &str) {
        let scope = (namespace.to_string(), event.source.clone());
        if !self.totals.contains_key(&scope) && self.totals.len() >= 4096 {
            self.omitted += 1;
            return;
        }
        *self.totals.entry(scope).or_default() += 1;
        let mut nodes = 0;
        for (path, value) in &event.fields {
            if path.starts_with("_sec.") {
                continue;
            }
            self.visit(event, namespace, path, value, 0, &mut nodes);
        }
    }
    fn visit(
        &mut self,
        event: &Event,
        namespace: &str,
        path: &str,
        value: &Value,
        depth: usize,
        nodes: &mut usize,
    ) {
        if depth > 8 || *nodes >= 256 || path.len() > 512 {
            self.omitted += 1;
            return;
        }
        *nodes += 1;
        if let Value::Object(object) = value {
            for (key, value) in object {
                self.visit(
                    event,
                    namespace,
                    &format!("{path}.{key}"),
                    value,
                    depth + 1,
                    nodes,
                );
            }
            return;
        }
        let key = (
            namespace.to_string(),
            event.source.clone(),
            path.to_string(),
        );
        if !self.fields.contains_key(&key) && self.fields.len() >= 4096 {
            self.omitted += 1;
            return;
        }
        let field = self.fields.entry(key).or_default();
        field.present += 1;
        let shape = kind(value);
        *field.types.entry(shape).or_default() += 1;
        if !matches!(shape, "null" | "empty") {
            field.nonempty += 1;
        }
        if let Err(index) = field.refs.binary_search(&event.event_ref) {
            field.refs.insert(index, event.event_ref.clone());
            field.refs.truncate(3);
        }
    }
    pub fn describe(&self) -> Value {
        json!({"version":VERSION,"status":if self.omitted==0{"complete_structured_field_scan"}else{"bounded_field_summary"},"omitted_nodes":self.omitted,"automatic_mapping":false,
            "meaning":"Format consistency is a support ratio, not calibrated semantic confidence. Review original fields before mapping. No direction, units or actor role is inferred from value shape.",
            "fields":self.fields.iter().map(|((ns,source,path),field)|{let dominant=field.types.iter().filter(|(kind,_)|!matches!(**kind,"null"|"empty")).max_by(|(a,an),(b,bn)|an.cmp(bn).then(b.cmp(a)));let total=self.totals[&(ns.clone(),source.clone())];
                json!({"namespace":ns,"source":source,"field":path,"population":total,"present":field.present,"nonempty":field.nonempty,"types":field.types,"suggested_type":dominant.map(|(name,_)|*name),"confidence":{"kind":"observed_format_consistency","value":dominant.map(|(_,n)|*n as f64/field.nonempty.max(1) as f64),"support":field.nonempty,"calibrated":false},"suggested_semantic_role":Value::Null,"sample_event_refs":field.refs})}).collect::<Vec<_>>()})
    }
    pub fn summary(&self) -> Value {
        json!({"version":VERSION,"fields":self.fields.len(),"sources_and_namespaces":self.totals.len(),"omitted_nodes":self.omitted,"automatic_mapping":false,"confidence":"observed format consistency; uncalibrated; no automatic direction or units"})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_schema_suggestions_preserve_tenant_scope_and_never_guess_direction_or_units() {
        let mut schema = Schema::default();
        for (ns, id, value) in [
            ("a", 1, json!("192.0.2.1")),
            ("a", 2, json!("not-an-ip")),
            ("b", 3, json!(42)),
        ] {
            let mut event = Event::empty();
            event.source = "generic".into();
            event.event_ref = id.to_string();
            event.fields.insert("unknown".into(), value);
            schema.observe(&event, ns);
        }
        let data = schema.describe();
        let fields = data["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0]["types"]["ip_address"], 1);
        assert_eq!(fields[0]["confidence"]["value"], 0.5);
        assert_eq!(fields[1]["suggested_type"], "number");
        for field in fields {
            assert!(field["suggested_semantic_role"].is_null());
        }
        assert_eq!(data["automatic_mapping"], false);
    }
}
