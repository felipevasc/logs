//! Native support for the assisted expression editor: a bounded sample test
//! that reports groups with the engine's own ranges, and the full text of an
//! event field for use as an editable sample.
use crate::{
    model::Event,
    search_regex::{Matcher, Pattern},
    AppState,
};
use serde_json::{json, Value};
/// Samples are interactive input, not records: the editor never sends more.
pub(crate) const PREVIEW_LIMIT: usize = 1 << 20;
/// Text fetched from an event to seed a sample.
const SAMPLE_LIMIT: usize = 256 << 10;
/// Test `pattern` against `text`. A pattern that does not compile, or whose run
/// hits an engine limit, is a diagnostic in the payload; cancellation and stale
/// ownership remain transport errors so a replaced request never looks like a result.
pub(crate) fn preview(pattern: Pattern, text: String, lines: bool) -> Result<Value, String> {
    if text.len() > PREVIEW_LIMIT {
        return Err("A amostra excede 1 MiB. Use um trecho menor para testar.".into());
    }
    let version = crate::search_regex::pcre2_version();
    let matcher = match Matcher::diagnose(&pattern) {
        Ok(matcher) => matcher,
        Err(diagnostic) => {
            crate::operations::check()?;
            return Ok(json!({
                "ok": false,
                "stage": "compile",
                "error": diagnostic,
                "engine": pattern.engine,
                "pcre2Version": version,
            }));
        }
    };
    let outcome = if lines { matcher.find_lines_with(&text, true) } else { matcher.find_detailed(&text) };
    let found = match outcome {
        Ok(found) => found,
        Err(message) => {
            crate::operations::check()?;
            return Ok(json!({
                "ok": false,
                "stage": "run",
                "error": { "message": message },
                "engine": pattern.engine,
                "groups": matcher.groups(),
                "pcre2Version": version,
            }));
        }
    };
    Ok(json!({
        "ok": true,
        "engine": pattern.engine,
        "pcre2Version": version,
        "groups": matcher.groups(),
        "spans": found.spans,
        "complete": found.complete,
        "limit": found.limit,
        "bytes": text.len(),
        "length": text.encode_utf16().count(),
    }))
}
/// One field of one event, whole when it fits the sample limit. `truncated`
/// says that the editable sample is a prefix of a longer value.
pub(crate) fn sample_text(
    state: &AppState,
    id: usize,
    event_ref: &str,
    field: &str,
) -> Result<Value, String> {
    if field.is_empty() || field.len() > 512 {
        return Err("Informe o campo da amostra.".into());
    }
    let event = crate::workspace::with_selection(state, &[], |s| s.event(id))?
        .ok_or("Evento ausente ou excluído.")?;
    if event.event_ref != event_ref {
        return Err("A referência do evento mudou.".into());
    }
    let value = field_text(&event, field).ok_or("O evento não possui esse campo.")?;
    let mut end = value.len().min(SAMPLE_LIMIT);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Ok(json!({
        "field": field,
        "text": &value[..end],
        "bytes": value.len(),
        "truncated": end < value.len(),
    }))
}
fn field_text(event: &Event, field: &str) -> Option<String> {
    if field == "raw" && event.raw.is_empty() {
        return Some(event.message.clone());
    }
    event.col_ref(field).map(|v| v.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pattern(text: &str, engine: &str) -> Pattern {
        Pattern {
            engine: engine.into(),
            text: text.into(),
            case_sensitive: true,
            multiline: false,
            dot_all: false,
        }
    }
    fn run(text: &str, engine: &str, sample: &str) -> Value {
        preview(pattern(text, engine), sample.into(), false).unwrap()
    }
    #[test]
    fn lookbehind_value_has_exact_ranges_and_groups_are_described() {
        let out = run(r"(?<=user=)(?<name>\w+)", "pcre2", "Falha user=ana origem=10.0.0.5");
        assert_eq!(out["ok"], true);
        let span = &out["spans"][0];
        assert_eq!(span["start"], 11);
        assert_eq!(span["end"], 14);
        assert_eq!(span["groups"][0]["names"][0], "name");
        assert_eq!(span["groups"][0]["start"], 11);
        assert_eq!(span["groups"][0]["value"], "ana");
        assert!(out["pcre2Version"].as_str().unwrap().starts_with("10."));
    }
    #[test]
    fn compile_errors_carry_utf16_positions() {
        let out = run("ação(", "pcre2", "x");
        assert_eq!(out["ok"], false);
        assert_eq!(out["stage"], "compile");
        // Five UTF-16 units precede the open parenthesis' failure point at the end.
        assert_eq!(out["error"]["offset"], 5);
        assert_eq!(out["error"]["byteOffset"], 7);
        let unbounded = run(r"(?<=a+)b", "pcre2", "ab");
        assert_eq!(unbounded["ok"], false);
    }
    #[test]
    fn unicode_emoji_crlf_nested_optional_and_repeated_values_use_engine_ranges() {
        let text = "😀 id=ação\r\nid=ação id=x";
        let out = run(r"id=((?<w>\w+)(?:-(\d+))?)", "pcre2", text);
        let spans = out["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 3);
        for span in spans {
            let group = &span["groups"][0];
            let byte = |k: &str| group[k].as_u64().unwrap() as usize;
            assert_eq!(&text[byte("byteStart")..byte("byteEnd")], group["value"].as_str().unwrap());
            let units: Vec<u16> = text.encode_utf16().collect();
            let slice = String::from_utf16(&units[byte_start_units(group)..end_units(group)]).unwrap();
            assert_eq!(slice, group["value"].as_str().unwrap());
        }
        // Group 3 never took part: no range and no empty value invented.
        let optional = &spans[0]["groups"][2];
        assert_eq!(optional["participated"], false);
        assert!(optional.get("value").is_none());
        // Identical values are separate occurrences with separate ranges.
        let again = run(r"(?<v>ação)", "pcre2", "ação ação");
        assert_ne!(again["spans"][0]["groups"][0]["start"], again["spans"][1]["groups"][0]["start"]);
    }
    fn byte_start_units(group: &Value) -> usize {
        group["start"].as_u64().unwrap() as usize
    }
    fn end_units(group: &Value) -> usize {
        group["end"].as_u64().unwrap() as usize
    }
    #[test]
    fn empty_captures_and_lookaround_captures_are_reported_as_matched() {
        let out = run(r"a(b*)c", "pcre2", "ac");
        let group = &out["spans"][0]["groups"][0];
        assert_eq!(group["participated"], true);
        assert_eq!(group["value"], "");
        let ahead = run(r"x(?=(?<next>\d+))", "pcre2", "x42");
        assert_eq!(ahead["spans"][0]["end"], 1);
        assert_eq!(ahead["spans"][0]["groups"][0]["value"], "42");
        // Repetition keeps only what the engine returns: the last iteration.
        let repeated = run(r"(?:(\d)-)+", "pcre2", "1-2-3-");
        assert_eq!(repeated["spans"][0]["groups"][0]["value"], "3");
    }
    #[test]
    fn fast_engine_reports_the_same_shape_and_duplicate_names_are_listed() {
        let out = run(r"(?P<k>\w+)=(?P<v>\w+)", "rust", "a=1 b=2");
        assert_eq!(out["spans"][1]["groups"][1]["names"][0], "v");
        assert_eq!(out["spans"][1]["groups"][1]["value"], "2");
        let duplicate = run(r"(?J)(?<n>a)|(?<n>b)", "pcre2", "b");
        let groups = duplicate["groups"].as_array().unwrap();
        assert_eq!(groups[1]["names"][0], "n");
        assert_eq!(groups[2]["names"][0], "n");
    }
    #[test]
    fn group_range_budget_marks_the_result_incomplete() {
        let out = run(&"(a)".repeat(20), "pcre2", &"a".repeat(20 * 600));
        assert_eq!(out["ok"], true);
        assert_eq!(out["complete"], false);
        assert_eq!(out["limit"], "groups");
        assert!(out["spans"].as_array().unwrap().len() < 512);
    }
    #[test]
    fn runtime_limits_are_diagnostics_not_zero_matches() {
        let out = run(r"(*NO_START_OPT)(a+)+$", "pcre2", &format!("{}!", "a".repeat(20000)));
        assert_eq!(out["ok"], false);
        assert_eq!(out["stage"], "run");
    }
}
