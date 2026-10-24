//! The `search` filter operation: one field tested with the same matcher the
//! editor previews with (literal, Rust regex or PCRE2). The engine, case and
//! multiline options travel inside the filter, so a stored selection, a count,
//! an export and an SQL scope always evaluate the exact expression the user ran.
//! `column` names one field; `_all` is refused because joining fields would let
//! an expression cross the boundary between them.
use crate::{
    query::Filter,
    search_regex::{Matcher, Pattern},
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct Options {
    pub engine: String,
    pub case_sensitive: bool,
    pub multiline: bool,
    pub dot_all: bool,
    /// `event`: the whole field value; `line`: each physical line separately.
    pub scope: String,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            engine: "rust".into(),
            case_sensitive: false,
            multiline: false,
            dot_all: false,
            scope: "event".into(),
        }
    }
}
pub(crate) struct Predicate {
    matcher: Matcher,
    lines: bool,
}
impl Predicate {
    pub(crate) fn is_match(&self, text: &str) -> Result<bool, String> {
        self.matcher.is_match(text, self.lines)
    }
    /// PCRE2 can fail while matching (limits, deadline), so it is verified on the
    /// calling thread where a failure can abort the query instead of vanishing.
    pub(crate) fn can_fail(&self) -> bool {
        self.matcher.is_pcre2()
    }
}
fn options(filter: &Filter) -> Result<Options, String> {
    let raw = filter.value2.as_deref().filter(|v| !v.trim().is_empty());
    let options: Options = match raw {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|e| format!("Opções da busca por expressão inválidas: {e}"))?,
        None => Options::default(),
    };
    if !["literal", "rust", "pcre2"].contains(&options.engine.as_str()) {
        return Err("Motor desconhecido: use literal, rust ou pcre2.".into());
    }
    if !["event", "line"].contains(&options.scope.as_str()) {
        return Err("Escopo inválido: use o valor completo ou a linha física.".into());
    }
    Ok(options)
}
pub(crate) fn compile(filter: &Filter) -> Result<Predicate, String> {
    let options = options(filter)?;
    let matcher = Matcher::new(&Pattern {
        engine: options.engine,
        text: filter.value.clone(),
        case_sensitive: options.case_sensitive,
        multiline: options.multiline,
        dot_all: options.dot_all,
    })?;
    Ok(Predicate {
        matcher,
        lines: options.scope == "line",
    })
}
/// Boundary validation shared by every entry point that accepts filters.
pub(crate) fn validate(filter: &Filter) -> Result<(), String> {
    if filter.column.trim().is_empty() || filter.column == "_all" {
        return Err("A busca por expressão exige um campo específico (mensagem, bruto ou outro). Campos não são concatenados.".into());
    }
    compile(filter)
        .map(|_| ())
        .map_err(|e| format!("Expressão inválida: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn filter(column: &str, value: &str, options: &str) -> Filter {
        Filter {
            column: column.into(),
            op: "search".into(),
            value: value.into(),
            value2: Some(options.into()),
        }
    }
    #[test]
    fn options_select_the_engine_and_scope_explicitly() {
        let p = compile(&filter("message", r"(?<=user=)\w+", r#"{"engine":"pcre2","caseSensitive":true}"#)).unwrap();
        assert!(p.can_fail());
        assert!(p.is_match("Falha user=ana").unwrap());
        // The same text is not silently reinterpreted by another engine.
        let rust = filter("message", r"(?<=user=)\w+", r#"{"engine":"rust"}"#);
        assert!(validate(&rust).is_err());
        let literal = compile(&filter("message", "a+b(1).log", r#"{"engine":"literal","caseSensitive":true}"#)).unwrap();
        assert!(literal.is_match("x a+b(1).log y").unwrap());
        assert!(!literal.is_match("x aab1log y").unwrap());
        assert!(!literal.can_fail());
    }
    #[test]
    fn physical_lines_do_not_cross_line_boundaries_and_all_is_refused() {
        let event = compile(&filter("raw", r"a\s+b", r#"{"engine":"pcre2","scope":"event"}"#)).unwrap();
        let line = compile(&filter("raw", r"a\s+b", r#"{"engine":"pcre2","scope":"line"}"#)).unwrap();
        assert!(event.is_match("a\nb").unwrap());
        assert!(!line.is_match("a\nb").unwrap());
        assert!(line.is_match("x\r\na  b\r\n").unwrap());
        assert!(validate(&filter("_all", "x", "{}")).is_err());
        assert!(validate(&filter("message", "x", r#"{"engine":"sed"}"#)).is_err());
        assert!(validate(&filter("message", "x", r#"{"surprise":1}"#)).is_err());
    }
    #[test]
    fn matching_failures_are_errors_not_misses() {
        let p = compile(&filter("raw", r"(*NO_START_OPT)(a+)+$", r#"{"engine":"pcre2","caseSensitive":true}"#)).unwrap();
        assert!(p.is_match(&format!("{}!", "a".repeat(20000))).is_err());
    }
}
