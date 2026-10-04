//! Historical Case display strings, matching EvidenceUI and CaseTimeline.
//!
//! Callers admit full input/output scratch before invoking these pure helpers.
//! Event title/detail/source and authored group/manual strings stay literal;
//! attached note text too. Search each displayed field
//! separately, before preview clipping; never join fields for substring search.
use serde_json::Value;

pub(crate) const POLICY_VERSION: u32 = 2;
pub(crate) const MAX_TEXT_BYTES: usize = 4 << 20;
pub(crate) const TEXT_LIMIT: &str = "CASE_DISPLAY_TEXT_LIMIT";
pub(crate) const UNSUPPORTED: &str = "CASE_HISTORY_DISPLAY_UNSUPPORTED";

// ECMAScript WhiteSpace + LineTerminator, shared by trim and regexp \s.
// In particular FEFF is included, while NEL (0085) and MVS (180E) are not.
fn js_space(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{00A0}' |
        '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' |
        '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

/// pt-BR has the default Unicode lowercase mapping, including contextual final
/// sigma. Use str::to_lowercase (not per-character char::to_lowercase), which
/// retains that context. No normalization/accent stripping/case folding occurs.
pub(crate) fn normalize_query(query: &str) -> Result<String, String> {
    if query.encode_utf16().take(201).count() > 200 {
        return Err(TEXT_LIMIT.into());
    }
    lower(query.trim_matches(js_space))
}
pub(crate) fn matches_normalized_query(text: &str, normalized_query: &str) -> Result<bool, String> {
    size(text.len())?;
    size(normalized_query.len())?;
    Ok(normalized_query.is_empty() || lower(text)?.contains(normalized_query))
}
fn size(bytes: usize) -> Result<usize, String> {
    if bytes > MAX_TEXT_BYTES {
        Err(TEXT_LIMIT.into())
    } else {
        Ok(bytes)
    }
}
fn add_size(total: usize, bytes: usize) -> Result<usize, String> {
    size(total.checked_add(bytes).ok_or(TEXT_LIMIT)?)
}
fn lower(text: &str) -> Result<String, String> {
    size(text.len())?;
    // Contextual final sigma changes the character, but not its byte length.
    // Count using the iterator first; materialize using str's contextual mapping.
    text.chars()
        .flat_map(char::to_lowercase)
        .try_fold(0, |bytes, ch| add_size(bytes, ch.len_utf8()))?;
    Ok(text.to_lowercase())
}

/// Historical Case display shows Event title/detail/source exactly as recorded,
/// like EvidenceUI.redact since policy 2. Masking is only an explicit export choice.
pub(crate) fn redact_event_text(value: &str) -> Result<String, String> {
    size(value.len())?;
    Ok(value.to_string())
}

pub(crate) fn evidence_label(level: Option<&Value>) -> &'static str {
    // Number.isInteger permits 1.0 as well as 1, but not strings or booleans.
    match level.and_then(Value::as_f64) {
        Some(1.0) => "Inconclusivo",
        Some(2.0) => "Suspeita",
        Some(3.0) => "Indício",
        Some(4.0) => "Forte indício",
        Some(5.0) => "Quase confirmado",
        _ => "Nível não avaliado",
    }
}

/// Saved detections retain declaration order and duplicate matching detections.
/// An absent Event reference corresponds to JS undefined, not an empty string.
/// Unknown non-string detection names/unsupported includes receivers must be
/// surfaced as unsupported display semantics, never silently coerced or lost.
pub(crate) fn event_context(
    detections: &[Value],
    event_ref: Option<&str>,
) -> Result<String, String> {
    let mut bytes = 0;
    for detection in detections {
        if let Some((label, name)) = matching_detection(detection, event_ref)? {
            bytes = add_size(bytes, if bytes == 0 { 0 } else { 2 })?;
            bytes = add_size(bytes, label.len() + 2)?;
            bytes = add_size(bytes, name.len())?;
        }
    }
    let mut output = String::with_capacity(bytes);
    for detection in detections {
        if let Some((label, name)) = matching_detection(detection, event_ref)? {
            if !output.is_empty() {
                output.push_str("; ");
            }
            output.push_str(label);
            output.push_str(": ");
            output.push_str(name);
        }
    }
    Ok(output)
}
fn matching_detection<'a>(
    detection: &'a Value,
    event_ref: Option<&str>,
) -> Result<Option<(&'static str, &'a str)>, String> {
    let object = detection.as_object().ok_or(UNSUPPORTED)?;
    let matches = match object.get("event_refs") {
        None | Some(Value::Null) => false,
        Some(Value::Array(refs)) => event_ref
            .is_some_and(|reference| refs.iter().any(|value| value.as_str() == Some(reference))),
        Some(Value::String(refs)) => refs.contains(event_ref.unwrap_or("undefined")),
        _ => return Err(UNSUPPORTED.into()),
    };
    if !matches {
        return Ok(None);
    }
    let name = match object.get("name") {
        Some(Value::String(name)) => name.as_str(),
        None => "undefined",
        Some(Value::Null) => "null",
        _ => return Err(UNSUPPORTED.into()),
    };
    Ok(Some((evidence_label(object.get("evidence_level")), name)))
}

/// The complete message-or-description plus saved-detection suffix is redacted
/// together, so a secret beginning in one part cannot leak across the newline.
pub(crate) fn event_detail(
    message: &str,
    description: &str,
    context: &str,
) -> Result<String, String> {
    size(message.len())?;
    size(description.len())?;
    size(context.len())?;
    let body = if message.is_empty() {
        description
    } else {
        message
    };
    if body.is_empty() {
        return redact_event_text(context);
    }
    if context.is_empty() {
        return redact_event_text(body);
    }
    let bytes = add_size(add_size(body.len(), 1)?, context.len())?;
    let mut full = String::with_capacity(bytes);
    full.push_str(body);
    full.push('\n');
    full.push_str(context);
    redact_event_text(&full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle() -> Value {
        serde_json::from_str(include_str!(
            "../../scripts/tests/fixtures/native-display-policy.json"
        ))
        .unwrap()
    }
    #[test]
    fn historical_event_redaction_matches_actual_javascript_oracle() {
        for case in oracle()["redaction"].as_array().unwrap() {
            assert_eq!(
                redact_event_text(case["input"].as_str().unwrap()).unwrap(),
                case["expected"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
    #[test]
    fn historical_search_matches_javascript_whitespace_and_contextual_lowercase() {
        for case in oracle()["search"].as_array().unwrap() {
            let query = normalize_query(case["query"].as_str().unwrap()).unwrap();
            assert_eq!(
                query,
                case["normalized"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            let matched =
                case["fields"].as_array().unwrap().iter().any(|field| {
                    matches_normalized_query(field.as_str().unwrap(), &query).unwrap()
                });
            assert_eq!(
                matched,
                case["expected"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
    #[test]
    fn display_text_growth_and_utf16_query_admission_are_bounded() {
        assert!(normalize_query(&"😀".repeat(100)).is_ok());
        assert_eq!(normalize_query(&"😀".repeat(101)).unwrap_err(), TEXT_LIMIT);
        assert_eq!(normalize_query(&" ".repeat(201)).unwrap_err(), TEXT_LIMIT);
        assert_eq!(
            matches_normalized_query(&"İ".repeat(MAX_TEXT_BYTES / 2), "x").unwrap_err(),
            TEXT_LIMIT
        );
        assert_eq!(
            redact_event_text(&"token=a ".repeat(MAX_TEXT_BYTES / 8 + 1)).unwrap_err(),
            TEXT_LIMIT
        );
        assert_eq!(
            event_detail(&"x".repeat(MAX_TEXT_BYTES), "", "suffix").unwrap_err(),
            TEXT_LIMIT
        );
        assert_eq!(
            event_context(
                &[serde_json::json!({"event_refs":["e"],"name":"x".repeat(MAX_TEXT_BYTES)})],
                Some("e")
            )
            .unwrap_err(),
            TEXT_LIMIT
        );
    }
    #[test]
    fn historical_saved_detection_suffix_matches_javascript() {
        for case in oracle()["context"].as_array().unwrap() {
            let context = event_context(
                case["detections"].as_array().unwrap(),
                case["eventRef"].as_str(),
            )
            .unwrap();
            assert_eq!(
                context,
                case["expected"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                event_detail(
                    case["message"].as_str().unwrap(),
                    case["description"].as_str().unwrap(),
                    &context
                )
                .unwrap(),
                case["detail"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
        assert_eq!(
            event_context(
                &[serde_json::json!({"event_refs":["e"],"name":{}})],
                Some("e")
            )
            .unwrap_err(),
            UNSUPPORTED
        );
        assert_eq!(
            event_context(&[serde_json::json!({"event_refs":{}})], Some("e")).unwrap_err(),
            UNSUPPORTED
        );
    }
}
