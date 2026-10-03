//! Conservative, line-oriented HTTP log extraction. Positional custom columns
//! are never guessed; source spelling/escaping and the complete raw record stay
//! available. No vendor is inferred from generic JSON/logfmt HTTP field names.
//!
//! Format references (fixtures use synthetic addresses and messages):
//! https://nginx.org/en/docs/http/ngx_http_log_module.html#log_format
//! https://httpd.apache.org/docs/2.4/logs.html#common
//! https://github.com/nginx/nginx/blob/master/src/core/ngx_log.c
//! https://github.com/nginx/nginx/blob/master/src/http/ngx_http_request.c
use super::{normalize_level, parse_timestamp};
use crate::model::Event;
use serde_json::{Map, Value};

fn access_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(
        r#"^(\S+)[ \t]+(\S+)[ \t]+(\S+)[ \t]+\[([^\]\r\n]+)\][ \t]+"((?:\\.|[^"\\\r\n])*)"[ \t]+([0-9]{3})[ \t]+([0-9]+|-)(?:[ \t]+(.*))?$"#,
    ).unwrap())
}

fn error_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(
        r"^(\d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2}) \[(emerg|alert|crit|error|warn|notice|info|debug)\] ([0-9]+)#([0-9]+): (?:\*([0-9]+) )?(.*)$",
    ).unwrap())
}

pub(super) fn access_timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(value, "%d/%b/%Y:%H:%M:%S %z")
        .ok()
        .map(|time| time.timestamp_millis())
}

// Cheap dispatch guards run before any regex or key/value scanning.
pub(super) fn access_candidate(line: &str) -> bool {
    line.contains('[') && line.contains(']') && line.contains('"')
}

pub(super) fn error_candidate(line: &str) -> bool {
    line.as_bytes().get(4) == Some(&b'/')
        && line.as_bytes().get(7) == Some(&b'/')
        && line.contains("#")
        && line.contains("] ")
}

fn quoted(input: &str) -> Option<(&str, &str)> {
    let bytes = input.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some((&input[1..i], &input[i + 1..])),
            b'\n' | b'\r' => return None,
            _ => i += 1,
        }
    }
    None
}

fn http_status(value: &Value) -> Option<String> {
    let value = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) if number.is_u64() => number.to_string(),
        _ => return None,
    };
    (value.len() == 3 && value.bytes().all(|b| b.is_ascii_digit())).then_some(value)
}

fn status_level(status: &str) -> &'static str {
    match status.as_bytes().first() {
        Some(b'5') => "Erro",
        Some(b'4') => "Aviso",
        _ => "Informação",
    }
}

fn insert_once(fields: &mut Map<String, Value>, key: &str, value: &str) {
    fields
        .entry(key.to_string())
        .or_insert_with(|| Value::from(value));
}

fn insert_occurrence(fields: &mut Map<String, Value>, key: &str, value: &str) {
    match fields.get_mut(key) {
        None => {
            fields.insert(key.to_string(), Value::from(value));
        }
        Some(Value::Array(values)) => values.push(Value::from(value)),
        Some(existing) => *existing = Value::Array(vec![existing.clone(), Value::from(value)]),
    }
}

fn method_token(method: &str) -> bool {
    !method.is_empty()
        && method.bytes().any(|b| b.is_ascii_uppercase())
        && method.bytes().all(|b| b.is_ascii_uppercase() || b == b'-')
}

fn request_target(target: &str) -> bool {
    !target.is_empty()
        && !target
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn request_parts(request: &str) -> Option<(&str, &str, &str)> {
    let mut words = request.split_ascii_whitespace();
    let (method, target, protocol) = (words.next()?, words.next()?, words.next()?);
    if words.next().is_some() || !method_token(method) || !request_target(target) {
        return None;
    }
    let version = protocol.strip_prefix("HTTP/")?;
    let mut numbers = version.split('.');
    if !numbers
        .next()
        .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if let Some(minor) = numbers.next() {
        if minor.is_empty()
            || !minor.bytes().all(|b| b.is_ascii_digit())
            || numbers.next().is_some()
        {
            return None;
        }
    }
    Some((method, target, protocol))
}

pub(super) fn request_query_target(request: &str) -> Option<&str> {
    request_parts(request).map(|(_, target, _)| target)
}

fn add_request_fields(fields: &mut Map<String, Value>, request: &str) {
    if let Some((method, path, protocol)) = request_parts(request) {
        insert_once(fields, "method", method);
        insert_once(fields, "path", path);
        insert_once(fields, "protocol", protocol);
    }
}

pub(super) fn parse_access(line: &str) -> Option<Event> {
    let line_for_parse = line.trim();
    if !access_candidate(line_for_parse) {
        return None;
    }
    let captures = access_re().captures(line_for_parse)?;
    let timestamp = access_timestamp(&captures[4])?;
    let mut event = Event::empty();
    event.raw = line.to_string();
    event.timestamp = Some(timestamp);
    event.source = captures[1].to_string();
    event.code = captures[6].to_string();
    event.level = status_level(&event.code).into();
    event.message = format!("{} → {}", &captures[5], event.code);
    let fields = &mut event.fields;
    insert_once(fields, "time_local", &captures[4]);
    insert_once(fields, "request", &captures[5]);
    insert_once(fields, "status", &captures[6]);
    insert_once(fields, "size", &captures[7]); // legacy spelling/type remains stable
    if captures[1].parse::<std::net::IpAddr>().is_ok() {
        insert_once(fields, "remote_addr", &captures[1]);
        insert_once(fields, "client_ip", &captures[1]);
    } else {
        insert_once(fields, "remote_host", &captures[1]);
    }
    if &captures[2] != "-" {
        insert_once(fields, "ident", &captures[2]);
    }
    if &captures[3] != "-" {
        insert_once(fields, "user", &captures[3]);
    }
    add_request_fields(fields, &captures[5]);
    let mut tail = captures.get(8).map_or("", |value| value.as_str()).trim();
    let mut format = "access.common";
    if let Some((referer, after_referer)) = quoted(tail) {
        if after_referer.starts_with([' ', '\t']) {
            if let Some((agent, remaining)) = quoted(after_referer.trim_start()) {
                if remaining.is_empty() || remaining.starts_with([' ', '\t']) {
                    insert_once(fields, "referer", referer);
                    insert_once(fields, "agent", agent);
                    tail = remaining.trim();
                    format = "access.combined";
                }
            }
        }
    }
    insert_once(fields, "parser.format", format);
    if !tail.is_empty() {
        // Every suffix remains inspectable, including positional/invalid tails.
        insert_once(fields, "access.extra", tail);
        if let Some(pairs) = key_values(tail) {
            let mut extra = Map::new();
            for (key, value) in pairs {
                insert_occurrence(&mut extra, key, value);
            }
            for (key, value) in &extra {
                // Never let a custom suffix overwrite a structural field.
                fields.entry(key.clone()).or_insert_with(|| value.clone());
            }
            fields.insert("access.extra_fields".into(), Value::Object(extra));
        }
    }
    Some(event)
}

/// Strict whole-record key=value grammar. Unlike regex searches, this cannot
/// extract keys from a URL, prose, or the middle of a quoted user-agent. Exact
/// spelling and repeated values are preserved; no floats/IDs are coerced.
pub(super) fn key_values(mut input: &str) -> Option<Vec<(&str, &str)>> {
    let mut pairs = Vec::new();
    input = input.trim();
    while !input.is_empty() {
        let equal = input.find('=')?;
        let key = &input[..equal];
        if key.is_empty()
            || !key.bytes().enumerate().all(|(i, b)| {
                b.is_ascii_alphabetic()
                    || matches!(b, b'_' | b'@')
                    || (i > 0 && (b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'@')))
            })
        {
            return None;
        }
        let rest = &input[equal + 1..];
        let (value, tail) = if rest.starts_with('"') {
            quoted(rest)?
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let value = &rest[..end];
            if value.contains('"') {
                return None;
            }
            (value, &rest[end..])
        };
        if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
            return None;
        }
        pairs.push((key, value));
        input = tail.trim_start();
    }
    (!pairs.is_empty()).then_some(pairs)
}

pub(super) fn logfmt_candidate(line: &str) -> bool {
    let head = line.split_ascii_whitespace().next().unwrap_or("");
    head.contains('=') && !head.starts_with(['"', '{'])
}

pub(super) fn parse_logfmt(line: &str) -> Option<Event> {
    if !logfmt_candidate(line) {
        return None;
    }
    let pairs = key_values(line)?;
    // Two independent named fields are enough; a URL or lone expression isn't.
    if pairs.len() < 2 {
        return None;
    }
    let mut fields = Map::new();
    for (key, value) in &pairs {
        insert_occurrence(&mut fields, key, value);
    }
    // Legacy lowercase filters still work if there is one unambiguous spelling.
    // Case-distinct original keys are never merged or overwritten.
    let mut lower = std::collections::HashMap::<String, Option<Value>>::new();
    for (key, value) in &fields {
        lower
            .entry(key.to_lowercase())
            .and_modify(|value| *value = None)
            .or_insert_with(|| Some(value.clone()));
    }
    for (key, value) in lower {
        if let Some(value) = value {
            fields.entry(key).or_insert(value);
        }
    }
    let mut event = Event::empty();
    event.raw = line.to_string();
    let explicit_level = pairs.iter().any(|(key, _)| {
        ["level", "lvl", "severity"]
            .iter()
            .any(|alias| key.eq_ignore_ascii_case(alias))
    });
    // Do not normalize ambiguous duplicate aliases (including case variants).
    let single = |aliases: &[&str]| -> Option<&str> {
        let mut matches = pairs
            .iter()
            .filter(|(key, _)| aliases.iter().any(|alias| key.eq_ignore_ascii_case(alias)));
        let value = matches.next()?.1;
        if matches.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    if let Some(value) = single(&["ts", "time", "timestamp"]) {
        event.timestamp = parse_timestamp(value).or_else(|| {
            // Preserve legacy small epoch-second values, but reject NaN and
            // infinities rather than inventing a saturated timestamp.
            let seconds = value.parse::<f64>().ok()?;
            if !seconds.is_finite() || seconds.abs() >= 1e8 {
                return None;
            }
            let millis = (seconds * 1000.0) as i64;
            chrono::DateTime::from_timestamp_millis(millis).map(|_| millis)
        });
    }
    if let Some(value) = single(&["level", "lvl", "severity"]) {
        event.level = normalize_level(value);
    }
    if let Some(value) = single(&["msg", "message"]) {
        event.message = value.into();
    }
    if let Some(value) = single(&["code", "event"]) {
        event.code = value.into();
    }
    if let Some(value) = single(&["host", "source", "app", "service"]) {
        event.source = value.into();
    }
    event.fields = fields;
    enrich_http(&mut event, explicit_level);
    if event.message.is_empty() {
        event.message = line.into();
    }
    Some(event)
}

/// Names used by HTTP exporters, accepted only together with a request shape
/// and a literal three-digit status. Original JSON/logfmt values win over any
/// derived alias. Neither a product name nor a timezone is guessed.
fn http_field<'a>(fields: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    let value = fields.get(key)?;
    // Legacy case aliases are safe only when every spelling agrees.
    fields
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key))
        .all(|(_, candidate)| candidate == value)
        .then_some(value)
}

pub(super) fn enrich_http(event: &mut Event, explicit_level: bool) -> bool {
    let Some(status) = http_field(&event.fields, "status").and_then(http_status) else {
        return false;
    };
    let request = http_field(&event.fields, "request")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let method = http_field(&event.fields, "request_method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let uri = http_field(&event.fields, "request_uri")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if !request
        .as_deref()
        .is_some_and(|value| request_parts(value).is_some() || value == "-")
        && !(method.as_deref().is_some_and(method_token)
            && uri.as_deref().is_some_and(request_target))
    {
        return false;
    }
    if event.timestamp.is_none() {
        event.timestamp = event
            .fields
            .get("time_iso8601")
            .and_then(Value::as_str)
            .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
            .map(|time| time.timestamp_millis())
            .or_else(|| {
                event
                    .fields
                    .get("time_local")
                    .and_then(Value::as_str)
                    .and_then(access_timestamp)
            });
    }
    if event.code.is_empty() {
        event.code = status.clone();
    }
    if !explicit_level {
        event.level = status_level(&status).into();
    }
    if event.source.is_empty() {
        if let Some(remote) = event.fields.get("remote_addr").and_then(Value::as_str) {
            if remote != "-" {
                event.source = remote.to_string();
            }
        }
    }
    if let Some(value) = &method {
        insert_once(&mut event.fields, "method", value);
    }
    if let Some(value) = &uri {
        insert_once(&mut event.fields, "path", value);
    }
    if let Some(value) = &request {
        add_request_fields(&mut event.fields, value);
    }
    if event.message.is_empty() {
        if let Some(value) = request {
            event.message = format!("{value} → {status}");
        } else if let (Some(method), Some(uri)) = (method, uri) {
            event.message = format!("{method} {uri} → {status}");
        }
    }
    true
}

// Error-log context uses explicit comma-delimited labels, with quoted request,
// host and upstream strings. Split outside quotes; do not mine arbitrary words
// from the message. Keep context in the message too, preserving old searches.
fn error_context(body: &str) -> Vec<(&str, &str)> {
    let mut pieces = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (offset, ch) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quoted {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
        }
        if ch == ',' && !quoted {
            pieces.push(&body[start..offset]);
            start = offset + 1;
        }
    }
    pieces.push(&body[start..]);
    pieces
        .into_iter()
        .skip(1)
        .filter_map(|piece| {
            let (key, value) = piece.trim().split_once(':')?;
            let value = value.trim_start();
            if !matches!(
                key,
                "client" | "server" | "request" | "subrequest" | "upstream" | "host" | "referrer"
            ) {
                return None;
            }
            if value.starts_with('"') {
                let (value, tail) = self::quoted(value)?;
                if !tail.trim().is_empty() {
                    return None;
                }
                Some((key, value))
            } else if matches!(key, "client" | "server") && !value.contains('"') {
                Some((key, value))
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn parse_error(line: &str) -> Option<Event> {
    let trimmed = line.trim();
    if !error_candidate(trimmed) {
        return None;
    }
    let captures = error_re().captures(trimmed)?;
    let timestamp = parse_timestamp(&captures[1])?;
    let mut event = Event::empty();
    event.raw = line.into();
    event.timestamp = Some(timestamp);
    event.level = normalize_level(&captures[2]);
    event.source = "nginx".into();
    event.message = captures[6].to_string();
    insert_once(&mut event.fields, "parser.format", "nginx.error");
    insert_once(&mut event.fields, "pid", &captures[3]);
    insert_once(&mut event.fields, "tid", &captures[4]);
    if let Some(value) = captures.get(5) {
        insert_once(&mut event.fields, "connection", value.as_str());
    }
    for (key, value) in error_context(&captures[6]) {
        insert_occurrence(&mut event.fields, key, value);
    }
    if let Some(request) = event
        .fields
        .get("request")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        add_request_fields(&mut event.fields, &request);
    }
    if let Some(client) = event
        .fields
        .get("client")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        if client.parse::<std::net::IpAddr>().is_ok() {
            insert_once(&mut event.fields, "client_ip", &client);
        }
    }
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{detect_format, parse_line, parser_semantics_signature};

    #[test]
    fn synthetic_format_corpus_preserves_raw_and_exact_fields() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/import/http-records.json"
        ))
        .unwrap();
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let line = case["line"].as_str().unwrap();
            let event = parse_line(line.as_bytes(), "mixed", None, &[]);
            assert_eq!(event.raw, line, "raw: {name}");
            assert_eq!(
                event.parse_status,
                case["parse_status"].as_str().unwrap_or("parsed"),
                "status: {name}"
            );
            let value = serde_json::to_value(&event).unwrap();
            if let Some(expected) = case["expected"].as_object() {
                for (key, expected) in expected {
                    assert_eq!(&value[key], expected, "{name}: {key}");
                }
            }
            if let Some(expected) = case["fields"].as_object() {
                for (key, expected) in expected {
                    assert_eq!(event.fields.get(key), Some(expected), "{name}: {key}");
                }
            }
            if let Some(absent) = case["absent"].as_array() {
                for key in absent {
                    assert!(
                        !event.fields.contains_key(key.as_str().unwrap()),
                        "{name}: unexpected {key}"
                    );
                }
            }
        }
    }

    #[test]
    fn mixed_rows_are_detected_without_guessing_text_or_reframing_java() {
        let access = "203.0.113.1 - - [02/Oct/2026:13:20:30 +0000] \"GET / HTTP/1.1\" 200 1";
        let error = "2026/10/02 13:20:30 [error] 1#2: *3 failed, client: 203.0.113.1";
        let json = "{\"message\":\"ok\",\"nested\":{\"key\":\"value\"}}";
        assert_eq!(
            detect_format(format!("{access}\n{json}\n").as_bytes()),
            "mixed"
        );
        assert_eq!(
            detect_format(format!("{error}\n{error}\n").as_bytes()),
            "nginx-error"
        );
        for format in ["apache", "jsonl", "logfmt", "nginx-error", "mixed", "auto"] {
            for line in [access, error, json, "key=one other=two"] {
                assert_eq!(
                    parse_line(line.as_bytes(), format, None, &[]).parse_status,
                    "parsed",
                    "{format}: {line}"
                );
            }
        }
        let plain = parse_line(access.as_bytes(), "text", None, &[]);
        assert_eq!(plain.parse_status, "text");
        assert!(plain.fields.is_empty());
        let custom = parse_line(access.as_bytes(), "custom", None, &[]);
        assert_eq!(custom.parse_status, "unparsed");
        assert!(custom.fields.is_empty());
        let snapshot = parse_line(access.as_bytes(), "snapshot", None, &[]);
        assert_eq!(snapshot.parse_status, "unparsed");
        assert!(snapshot.fields.is_empty());
        let java = "2026-10-02 13:20:30,000 INFO [worker] a.Logger - key=one other=two\n\tat a.Service.run(Service.java:1)";
        let event = parse_line(java.as_bytes(), "log4j", None, &[]);
        assert!(!event.fields.contains_key("key"));
        assert_eq!(event.raw, java);
        assert_eq!(parser_semantics_signature("text"), None);
        assert_eq!(parser_semantics_signature("log4j"), None);
        assert_eq!(parser_semantics_signature("csv"), None);
        assert_eq!(parser_semantics_signature("custom"), None);
    }

    #[test]
    fn request_separators_and_leading_whitespace_match_detection() {
        let raw = " \t203.0.113.1\t-  -\t[02/Oct/2026:13:20:30 +0000]\t\"GET / HTTP/1.1\"\t200  1\t\"-\"  \"client\"\r";
        assert_eq!(detect_format(raw.as_bytes()), "apache");
        let event = parse_line(raw.as_bytes(), "apache", None, &[]);
        assert_eq!(event.raw, raw);
        assert_eq!(event.fields["agent"], "client");
        assert_eq!(event.fields["path"], "/");
        let error = " 2026/10/02 13:20:30 [notice] 1#2: started\r";
        assert_eq!(
            parse_line(error.as_bytes(), "nginx-error", None, &[]).fields["pid"],
            "1"
        );
    }

    #[test]
    fn timestamps_honor_offsets_and_explicit_columns() {
        let epoch = chrono::DateTime::parse_from_rfc3339("2026-10-02T13:20:30Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(access_timestamp("02/Oct/2026:10:20:30 -0300"), Some(epoch));
        assert_eq!(access_timestamp("02/Oct/2026:13:20:30 +0000"), Some(epoch));
        assert_eq!(access_timestamp("02/Oct/2026:13:20:30"), None);
        for (value, expected) in [
            ("0", Some(0)),
            ("1", Some(1000)),
            ("1.125", Some(1125)),
            ("NaN", None),
            ("inf", None),
        ] {
            assert_eq!(
                parse_logfmt(&format!("ts={value} msg=test"))
                    .unwrap()
                    .timestamp,
                expected,
                "{value}"
            );
        }
        let json =
            r#"{"time_local":"02/Oct/2026:13:20:30","status":503,"request":"GET / HTTP/1.1"}"#;
        assert_eq!(
            parse_line(json.as_bytes(), "jsonl", None, &[]).timestamp,
            None
        );
        let explicit = r#"{"level":true,"status":503,"request":"GET / HTTP/1.1"}"#;
        assert_eq!(
            parse_line(explicit.as_bytes(), "jsonl", None, &[]).level,
            "true"
        );
    }

    #[test]
    fn original_keys_repetitions_and_ambiguous_aliases_are_not_overwritten() {
        let event =
            parse_logfmt("User=Alice trace.ID=001 tag=a tag=b level=warn level=error").unwrap();
        assert_eq!(event.fields["User"], "Alice");
        assert_eq!(event.fields["user"], "Alice");
        assert_eq!(event.fields["trace.ID"], "001");
        assert_eq!(event.fields["trace.id"], "001");
        assert_eq!(event.fields["tag"], serde_json::json!(["a", "b"]));
        assert_eq!(event.fields["level"], serde_json::json!(["warn", "error"]));
        assert_eq!(event.level, "Informação");
        let conflict = parse_logfmt("User=Alice user=Bob Key=one KEY=two").unwrap();
        assert_eq!(conflict.fields["User"], "Alice");
        assert_eq!(conflict.fields["user"], "Bob");
        assert!(!conflict.fields.contains_key("key"));
        assert!(parse_logfmt("msg=\"a=1 b=2 c=3\"").is_none());
        assert!(parse_logfmt("msg=\"a\"tail level=info").is_none());
        assert!(parse_logfmt("https://example.test/?a=1&b=2 c=3").is_none());
        let severity =
            parse_logfmt("level=warn level=error status=503 request=\"GET / HTTP/1.1\"").unwrap();
        assert_eq!(severity.level, "Informação");
        let status = parse_logfmt("status=503 Status=200 request=\"GET / HTTP/1.1\"").unwrap();
        assert_eq!(status.code, "");
        assert_eq!(status.fields["status"], "503");
        assert_eq!(status.fields["Status"], "200");
    }

    #[test]
    fn invalid_http_aliases_do_not_create_http_semantics() {
        for (method, uri) in [
            ("", "/"),
            ("whatever", "/"),
            ("GET", ""),
            ("GET", "two words"),
        ] {
            let json =
                serde_json::json!({"request_method": method, "request_uri": uri, "status": 503});
            let event = parse_line(json.to_string().as_bytes(), "jsonl", None, &[]);
            assert_eq!(event.code, "");
            assert_eq!(event.level, "Informação");
            assert!(!event.fields.contains_key("path"));
        }
        let event =
            parse_access("host.example - - [02/Oct/2026:13:20:30 +0000] \"GET / HTTP/1.1\" 200 1")
                .unwrap();
        assert_eq!(event.fields["remote_host"], "host.example");
        assert!(!event.fields.contains_key("client_ip"));
    }
}

#[cfg(test)]
mod wrapped_tests {
    use super::*;
    use crate::sources::{expand_query_param_fields, extract_query_params, parse_line};
    use serde_json::json;
    const LINE: &str = "192.0.2.80 - - [16/Sep/2026:20:55:04 -0300] \"GET /api/items?page=1&isAssigned=false&tags=one%2C+two&next=%2Ffind%3Fx%3D1%26y%3D2 HTTP/1.1\" 204 0 \"https://example.org/?ref=a\" \"client/1.0 (one, two)\"";
    const STAMP: &str = "2026-09-16T20:55:04.550942594-03:00";

    #[test]
    fn wrapped_access_preserves_envelope_and_exact_timestamp_with_namespaced_http_fields() {
        let raw = json!({"labels":{},"line":LINE,"timestamp":STAMP,"enabled":false,
            "count":9007199254740993u64,"items":[1,"two"],"empty":null}).to_string();
        let mut event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
        assert_eq!(event.raw, raw);
        assert_eq!(event.message, LINE);
        assert_eq!(event.timestamp, Some(1789602904550));
        assert_eq!(event.fields["timestamp"], STAMP);
        assert_eq!(event.fields["line"], LINE);
        assert_eq!(event.fields["labels"], json!({}));
        assert_eq!(event.fields["enabled"], false);
        assert_eq!(event.fields["count"], 9007199254740993u64);
        assert_eq!(event.fields["items"], json!([1,"two"]));
        assert_eq!(event.fields["empty"], Value::Null);
        for prefix in ["", "line."] {
            for (key, expected) in [("method","GET"),("status","204"),("client_ip","192.0.2.80"),
                ("request.page","1"),("path.isAssigned","false"),("path.tags","one, two"),
                ("path.next","/find?x=1&y=2"),("referer.ref","a")] {
                assert_eq!(event.fields[&format!("{prefix}{key}")], expected);
            }
        }
        assert!(!event.fields.contains_key("line.page"));
        assert!(!event.fields.contains_key("path.next.x"));
        let before = event.fields.clone();
        for _ in 0..3 { expand_query_param_fields(&mut event); }
        assert_eq!(event.fields, before);
    }

    #[test]
    fn wrapped_collisions_keep_explicit_fields_types_and_standard_columns() {
        let raw = json!({"line":LINE,"timestamp":STAMP,"source":"collector","code":"outer-code",
            "message":"explicit message","level":"debug","method":false,"status":201,
            "path":["external"],"request.page":7,"line.path.page":"authored","labels":{"host":"edge"}}).to_string();
        let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
        assert_eq!(event.source, "collector"); assert_eq!(event.code, "outer-code");
        assert_eq!(event.message, "explicit message"); assert_eq!(event.level, "Depuração");
        assert_eq!(event.fields["method"], false); assert_eq!(event.fields["status"], 201);
        assert_eq!(event.fields["path"], json!(["external"]));
        assert_eq!(event.fields["request.page"], 7); assert_eq!(event.fields["line.path.page"], "authored");
        assert_eq!(event.fields["line.method"], "GET"); assert_eq!(event.fields["line.status"], "204");
        assert_eq!(event.fields["labels.host"], "edge");
        for key in ["timestamp","source","code","message","level"] {
            assert_eq!(event.fields[key], serde_json::from_str::<Value>(&raw).unwrap()[key]);
        }
    }

    #[test]
    fn wrapped_recognition_is_conservative_and_snapshot_evidence_stays_captured() {
        for line in ["request failed for /api/items?page=1 HTTP/1.1\" 204 0", "GET /api/items?page=1 HTTP/1.1", "arbitrary prose", "{\"line\":\"nested\"}"] {
            let raw = json!({"line":line}).to_string();
            let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
            assert_eq!(event.raw, raw); assert!(!event.fields.contains_key("method"));
        }
        let raw = json!({"line":LINE}).to_string();
        assert_eq!(parse_line(raw.as_bytes(), "jsonl", None, &[]).timestamp, Some(1789602904000));
        let mut captured = Event::empty(); captured.raw = raw; captured.event_ref = "preserved:17".into();
        captured.fields.insert("line".into(), Value::from(LINE));
        let restored = parse_line(&serde_json::to_vec(&captured).unwrap(), "snapshot", None, &[]);
        assert_eq!(restored.raw, captured.raw); assert_eq!(restored.event_ref, captured.event_ref);
        assert!(!restored.fields.contains_key("method")); assert!(!restored.fields.contains_key("line.page"));
    }

    #[test]
    fn query_expansion_limits_preserve_source_values_and_remain_idempotent() {
        let many = (0..257).map(|i| format!("k{i}=v")).collect::<Vec<_>>().join("&");
        assert!(extract_query_params("query", &many).is_none());
        assert!(extract_query_params("query", &format!("x={}", "v".repeat(64 * 1024))).is_none());
        let values: serde_json::Map<String, Value> = (0..80).map(|i| (format!("query{i:02}"),
            Value::from("first=%2Fx%3Fp%3D1%26q%3D2&second=one%2C+two"))).collect();
        let mut event = parse_line(&serde_json::to_vec(&values).unwrap(), "jsonl", None, &[]);
        let first = event.fields.clone();
        for _ in 0..3 { expand_query_param_fields(&mut event); }
        assert_eq!(event.fields, first); assert!(event.fields.len() <= values.len() + 512);
        for (key, value) in values { assert_eq!(event.fields[&key], value); }
    }

    #[test]
    fn query_expansion_has_request_boundaries_and_is_idempotent_for_commas_and_nested_urls() {
        let mut event = parse_line(br#"{"query":"a=one%2C+two&a=three&a=one%2C+two&next=%2Fx%3Fp%3D1%26q%3D2","query.explicit":false,"url":"/x?page=1#ignored=two"}"#, "jsonl", None, &[]);
        assert_eq!(event.fields["query.a"], "one, two, three");
        assert_eq!(event.fields["query.next"], "/x?p=1&q=2");
        let before = event.fields.clone();
        for _ in 0..3 { expand_query_param_fields(&mut event); }
        assert_eq!(event.fields, before);
        for text in [LINE,"see /x?page=1","/x?page=1 HTTP/1.1\" 200 0","/x#fragment?page=1","/x#fragment?page=1&z=2"] {
            assert!(extract_query_params("line", text).is_none(), "{text}");
        }
        assert_eq!(extract_query_params("request", "GET /x?page=1 HTTP/1.1").unwrap(), vec![("page".into(), "1".into())]);
        assert_eq!(extract_query_params("query", "redirect=/x?p=1&count=2").unwrap(), vec![("redirect".into(), "/x?p=1".into()), ("count".into(), "2".into())]);
        assert_eq!(extract_query_params("query", "x=%€").unwrap(), vec![("x".into(), "%€".into())]);
    }
}

#[cfg(test)]
mod generic_envelope_tests {
    use crate::sources::{parse_line, parse_line_at};
    use serde_json::{json, Value};

    #[test]
    fn embedded_formats_are_recognized_in_arbitrary_and_nested_fields() {
        for (field, line, key, expected) in [
            ("payload", "192.0.2.1 - alice [16/Sep/2026:20:55:04 -0300] \"GET /apache HTTP/1.1\" 200 42", "user", "alice"),
            ("message", "<34>1 2026-09-16T23:55:04Z host app 123 ID47 - connection accepted", "app", "app"),
            ("body", "Sep 16 23:55:04 host sshd[123]: connection accepted", "process", "sshd"),
            ("value", "2026-09-16 23:55:04,123 ERROR [worker] example.Service - failure", "thread", "worker"),
            ("custom", "time=2026-09-16T23:55:04Z level=error msg=failed request_id=0042", "request_id", "0042"),
            ("record", "CEF:0|Example|Product|1|42|Example event|5|src=192.0.2.1", "vendor", "Example"),
        ] {
            let raw = json!({"container":{field:line},"timestamp":"2026-09-16T23:55:04.987654321Z","enabled":false}).to_string();
            let event = parse_line_at(raw.as_bytes(), "jsonl", None, &[], 2026);
            assert_eq!(event.raw, raw); assert_eq!(event.fields[&format!("container.{field}")], line);
            assert_eq!(event.fields[&format!("container.{field}.{key}")], expected, "{field}");
            assert_eq!(event.fields[key], expected, "canonical {field}");
            assert_eq!(event.fields["enabled"], false); assert_eq!(event.timestamp, Some(1789602904987));
        }
    }

    #[test]
    fn embedded_json_preserves_standard_column_source_types() {
        let inner = json!({"timestamp":"2026-09-16T23:55:04.987654321Z","code":500,"source":12,"message":false,"level":true});
        let raw = json!({"payload":inner.to_string()}).to_string();
        let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
        for key in ["timestamp","code","source","message","level"] {
            assert_eq!(event.fields[&format!("payload.{key}")], inner[key], "{key}");
        }
    }

    #[test]
    fn oversized_field_names_do_not_amplify_embedded_or_query_outputs() {
        let name = "x".repeat(2048);
        for text in ["time=2026-09-16T23:55:04Z msg=hello", "/x?page=1&tags=two"] {
            let raw = json!({name.clone():text}).to_string();
            let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
            assert_eq!(event.fields.len(), 1); assert!(event.fields.contains_key(&name));
        }
    }

    #[test]
    fn embedded_sibling_records_are_namespaced_without_arbitrary_canonical_choice() {
        let raw = json!({"first":"<34>1 2026-09-16T23:55:04Z a first-app 123 ID47 - one",
            "second":"<34>1 2026-09-16T23:55:05Z b second-app 456 ID48 - two"}).to_string();
        let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
        assert_eq!(event.fields["first.app"], "first-app"); assert_eq!(event.fields["second.app"], "second-app");
        assert!(!event.fields.contains_key("app")); assert!(event.timestamp.is_none()); assert!(event.source.is_empty());
    }

    #[test]
    fn encoded_json_recurses_with_limits_and_does_not_guess_prose_or_csv() {
        let inner = json!({"line":"192.0.2.1 - - [16/Sep/2026:20:55:04 -0300] \"GET /nested?page=1 HTTP/1.1\" 200 2"}).to_string();
        let raw = json!({"payload":inner}).to_string();
        let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
        assert_eq!(event.fields["payload.line.method"], "GET"); assert_eq!(event.fields["method"], "GET");
        let mut deep = raw;
        for _ in 0..4 { deep = json!({"payload":deep}).to_string(); }
        let event = parse_line(deep.as_bytes(), "jsonl", None, &[]);
        assert_eq!(event.raw, deep); assert!(!event.fields.contains_key("method")); assert!(event.fields.len() < 128);
        for value in ["ordinary prose key=value tail","alpha,beta,gamma","request /x?page=1 failed",
            "prefix CEF:0|Example|Product|1|42|Event|5|src=192.0.2.1","1 not-a-time host app pid msg - prose"] {
            let raw = json!({"payload":value}).to_string();
            let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
            assert!(!event.fields.contains_key("payload.parser.format"), "{value}");
        }
        let huge = format!("time=2026-09-16T23:55:04Z msg={} id=42", "a".repeat(70 * 1024));
        let raw = json!({"payload":huge}).to_string();
        let event = parse_line(raw.as_bytes(), "jsonl", None, &[]);
        assert_eq!(event.fields["payload"], serde_json::from_str::<Value>(&raw).unwrap()["payload"]);
        assert!(!event.fields.contains_key("payload.parser.format"));
    }
}
