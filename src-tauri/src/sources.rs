use crate::model::{
    label_class, CodesConfig, Event, LineMeta, LV_CRIT, LV_DEBUG, LV_ERR, LV_INFO, LV_OTHER,
    LV_TRACE, LV_WARN, STANDARD_COLUMNS,
};
use chrono::Datelike;
use serde_json::{Map, Value};

mod web_logs;

/// Scoped parsed-data revision; raw source and stable event identities never change.
pub(crate) fn parser_semantics_signature(format: &str) -> Option<&'static str> {
    matches!(format, "apache" | "nginx" | "nginx-error" | "jsonl" | "logfmt" | "auto" | "mixed")
        .then_some("web-logs-v1")
}

/// Interpreta data/hora "naive" (sem fuso) como horário LOCAL da máquina.
pub(crate) fn naive_to_ms(ndt: chrono::NaiveDateTime) -> i64 {
    ndt.and_local_timezone(chrono::Local)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| ndt.and_utc().timestamp_millis())
}

/// Tenta interpretar um texto como data/hora e devolve epoch em ms.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim().trim_matches(['[', ']']);
    if s.len() < 8 {
        return None;
    }
    if let Ok(number) = s.parse::<f64>() {
        if let Some(ms) = epoch_to_ms(number) {
            return Some(ms);
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    // Spreadsheets and exports often drop the seconds or use slashes.
    const FORMATS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y/%m/%d %H:%M:%S%.f",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
        "%d/%m/%Y %H:%M:%S%.f",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%d-%m-%Y %H:%M:%S",
        "%Y-%m-%d",
        "%Y/%m/%d",
        "%d/%m/%Y",
    ];
    for fmt in FORMATS {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive_to_ms(n));
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
            return d.and_hms_opt(0, 0, 0).map(naive_to_ms);
        }
    }
    None
}

pub fn normalize_level(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "1" | "fatal" | "critical" | "crit" | "crítico" | "emerg" | "alert" => "Crítico".into(),
        "2" | "error" | "err" | "erro" | "e" => "Erro".into(),
        "3" | "warn" | "warning" | "aviso" | "w" => "Aviso".into(),
        // Windows: nível 0 = "Log Always" (informativo); syslog 4 = info
        "0" | "4" | "info" | "information" | "informação" | "notice" | "i" => "Informação".into(),
        "5" | "debug" | "depuração" | "d" => "Depuração".into(),
        "trace" | "verbose" | "rastreio" => "Rastreio".into(),
        other if !other.is_empty() => other.to_string(),
        _ => "Informação".into(),
    }
}

// ---------------------------------------------------------------- arquivos

const TS_KEYS: &[&str] = &[
    "timestamp",
    "time",
    "ts",
    "@timestamp",
    "date",
    "datetime",
    "timeStamp",
    "event.created",
];
const LEVEL_KEYS: &[&str] = &[
    "level",
    "severity",
    "lvl",
    "log.level",
    "severityText",
    "severity_text",
];
const CODE_KEYS: &[&str] = &["code", "event_id", "eventid", "event.code", "id"];
const SOURCE_KEYS: &[&str] = &[
    "source",
    "provider",
    "logger",
    "service",
    "channel",
    "service.name",
    "host.name",
    "resource.service.name",
];
const MSG_KEYS: &[&str] = &["message", "msg", "log", "body", "text", "displayMessage"];

fn take_key(map: &mut Map<String, Value>, keys: &[&str]) -> Option<Value> {
    // An object/null in an alias must not hide a usable scalar alias.
    let key = keys.iter().find_map(|key| {
        map.iter()
            .find(|(k, v)| {
                k.eq_ignore_ascii_case(key)
                    && (v.is_string() || v.is_number() || v.is_boolean())
                    && v.as_str().is_none_or(|s| !s.trim().is_empty())
            })
            .map(|(k, _)| k.clone())
    })?;
    if key.contains('.') {
        map.get(&key).cloned()
    } else {
        map.remove(&key)
    }
}

fn epoch_to_ms(number: f64) -> Option<i64> {
    if !number.is_finite() {
        return None;
    }
    let magnitude = number.abs();
    let millis = if magnitude >= 1e17 {
        number / 1e6
    } else if magnitude >= 1e14 {
        number / 1e3
    } else if magnitude >= 1e11 {
        number
    } else if magnitude >= 1e8 {
        number * 1000.0
    } else {
        return None;
    };
    let ms = millis as i64;
    chrono::DateTime::from_timestamp_millis(ms).map(|_| ms)
}

fn value_to_ms(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => epoch_to_ms(n.as_f64()?),
        Value::String(s) => parse_timestamp(s),
        _ => None,
    }
}

fn flatten_json(
    map: Map<String, Value>,
    prefix: &str,
    depth: usize,
    flat: &mut Map<String, Value>,
) {
    // Explicit dotted keys win over the equivalent nested path.
    let mut leaves = Vec::new();
    for (key, value) in map {
        let path = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(inner) if depth < 12 && !inner.is_empty() => {
                flatten_json(inner, &path, depth + 1, flat)
            }
            value => leaves.push((path, value)),
        }
    }
    for (path, value) in leaves {
        flat.insert(path, value);
    }
}

fn event_from_json(input: Map<String, Value>, raw: &str) -> Event {
    let mut ev = Event::empty();
    ev.raw = raw.to_string();
    let mut map = Map::new();
    flatten_json(input, "", 0, &mut map);
    // Keep normalized aliases useful for filtering even when they also fill
    // a standard column (for example service.name and log.level).
    if let Some(v) = take_key(&mut map, TS_KEYS) {
        ev.timestamp = value_to_ms(&v);
    }
    let explicit_level = LEVEL_KEYS.iter().any(|alias| map.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case(alias) && (value.is_string() || value.is_number() || value.is_boolean())
            && value.as_str().is_none_or(|text| !text.trim().is_empty())
    }));
    if let Some(v) = take_key(&mut map, LEVEL_KEYS) {
        ev.level = normalize_level(
            &v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string()),
        );
    }
    if let Some(v) = take_key(&mut map, CODE_KEYS) {
        ev.code = match &v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    if let Some(v) = take_key(&mut map, SOURCE_KEYS) {
        ev.source = v
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string());
    }
    if let Some(v) = take_key(&mut map, MSG_KEYS) {
        ev.message = v
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| v.to_string());
    }
    ev.fields = map;
    web_logs::enrich_http(&mut ev, explicit_level);
    describe_known_json(&mut ev);
    if ev.message.is_empty() {
        ev.message = raw.chars().take(500).collect();
    }
    ev
}

fn field_text(ev: &Event, key: &str) -> Option<String> {
    ev.fields.get(key).and_then(|v| match v {
        Value::String(s) if !s.is_empty() && s != "-" => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

/// Readable message, code and level for well-known JSON families whose
/// records carry no message: CloudTrail, Suricata EVE, Zeek, Okta, GCP and
/// Kubernetes audit. Original fields are kept.
fn describe_known_json(ev: &mut Event) {
    let synthesized = ev.message.is_empty();
    // AWS CloudTrail: eventID is a UUID; the operation is eventName.
    if let (Some(source), Some(name)) = (field_text(ev, "eventSource"), field_text(ev, "eventName")) {
        if !ev.code.is_empty() && !ev.fields.contains_key("eventID") {
            ev.fields.insert("eventID".into(), Value::from(ev.code.clone()));
        }
        ev.code = name.clone();
        if ev.source.is_empty() {
            ev.source = source.trim_end_matches(".amazonaws.com").to_string();
        }
        let who = field_text(ev, "userIdentity.userName")
            .or_else(|| field_text(ev, "userIdentity.arn"))
            .or_else(|| field_text(ev, "userIdentity.type"))
            .unwrap_or_default();
        let from = field_text(ev, "sourceIPAddress").unwrap_or_default();
        if let Some(error) = field_text(ev, "errorCode") {
            ev.level = "Aviso".into();
            if synthesized {
                ev.message = format!("{name} negado ({error}) · {who} · {from}");
            }
        } else if synthesized {
            ev.message = format!("{name} · {who} · {from}");
        }
        return;
    }
    // Suricata EVE
    if let Some(kind) = field_text(ev, "event_type") {
        if let Some(signature) = field_text(ev, "alert.signature") {
            ev.message = signature;
            if let Some(id) = field_text(ev, "alert.signature_id") {
                ev.code = id;
            }
            ev.level = match field_text(ev, "alert.severity").as_deref() {
                Some("1") => "Erro",
                Some("2") => "Aviso",
                _ => "Informação",
            }
            .into();
        } else if synthesized {
            let src = field_text(ev, "src_ip").unwrap_or_default();
            let dst = field_text(ev, "dest_ip").unwrap_or_default();
            let detail = field_text(ev, "dns.rrname")
                .or_else(|| field_text(ev, "http.hostname").map(|h| format!("{h}{}", field_text(ev, "http.url").unwrap_or_default())))
                .or_else(|| field_text(ev, "tls.sni"))
                .unwrap_or_default();
            ev.message = format!("{kind} {src} → {dst} {detail}").trim().to_string();
        }
        if ev.source.is_empty() {
            ev.source = "suricata".into();
        }
        if ev.code.is_empty() {
            ev.code = kind;
        }
        return;
    }
    // Zeek (JSON or TSV converted to JSON)
    if let (Some(orig), Some(resp)) = (field_text(ev, "id.orig_h"), field_text(ev, "id.resp_h")) {
        if synthesized {
            let detail = field_text(ev, "query")
                .or_else(|| field_text(ev, "host").map(|h| format!("{h}{}", field_text(ev, "uri").unwrap_or_default())))
                .or_else(|| field_text(ev, "server_name"))
                .or_else(|| field_text(ev, "service"))
                .unwrap_or_default();
            let port = field_text(ev, "id.resp_p").map(|p| format!(":{p}")).unwrap_or_default();
            let proto = field_text(ev, "proto").unwrap_or_default();
            ev.message = format!("{orig} → {resp}{port} {proto} {detail}").trim().to_string();
        }
        if ev.source.is_empty() {
            ev.source = "zeek".into();
        }
        return;
    }
    // Okta System Log
    if let Some(kind) = field_text(ev, "eventType") {
        if ev.code.is_empty() || ev.fields.contains_key("uuid") {
            ev.code = kind.clone();
        }
        if ev.source.is_empty() {
            ev.source = "okta".into();
        }
        if field_text(ev, "outcome.result").is_some_and(|r| r.eq_ignore_ascii_case("FAILURE")) && ev.level == "Informação" {
            ev.level = "Aviso".into();
        }
        return;
    }
    // Google Cloud audit logs
    if let Some(method) = field_text(ev, "protoPayload.methodName") {
        if ev.code.is_empty() {
            ev.code = method.clone();
        }
        if synthesized {
            let who = field_text(ev, "protoPayload.authenticationInfo.principalEmail").unwrap_or_default();
            ev.message = format!("{method} · {who}");
        }
        return;
    }
    // Kubernetes audit
    if let (Some(verb), Some(stage)) = (field_text(ev, "verb"), field_text(ev, "stage")) {
        let _ = stage;
        if synthesized {
            let resource = field_text(ev, "objectRef.resource").unwrap_or_default();
            let name = field_text(ev, "objectRef.name").map(|n| format!("/{n}")).unwrap_or_default();
            let who = field_text(ev, "user.username").unwrap_or_default();
            ev.message = format!("{verb} {resource}{name} · {who}");
        }
        if ev.code.is_empty() {
            ev.code = verb;
        }
        if field_text(ev, "responseStatus.code").is_some_and(|c| c.starts_with('4') || c.starts_with('5')) {
            ev.level = "Aviso".into();
        }
    }
}

fn event_from_text(line: &str) -> Event {
    let mut ev = Event::empty();
    ev.parse_status = "text".into();
    ev.raw = line.to_string();
    ev.message = line.to_string();

    // Tenta extrair timestamp do início da linha (primeiros 40 caracteres).
    let head: String = line.chars().take(40).collect();
    let mut rest = line;
    if let Some((ms, len)) = extract_leading_ts(&head) {
        ev.timestamp = Some(ms);
        rest = &line[len.min(line.len())..];
    }

    // Tenta extrair nível logo em seguida ("ERROR ...", "[WARN] ...").
    let trimmed = rest.trim_start_matches([' ', '[', '-', ':']);
    for kw in [
        "CRITICAL", "FATAL", "ERROR", "WARN", "INFO", "DEBUG", "TRACE",
    ] {
        if trimmed
            .as_bytes()
            .get(..kw.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(kw.as_bytes()))
        {
            ev.level = normalize_level(kw);
            break;
        }
    }
    ev
}

/// Procura uma data/hora no começo da linha. Devolve (epoch_ms, bytes consumidos).
fn extract_leading_ts(head: &str) -> Option<(i64, usize)> {
    // Padrões comuns: "2024-01-31 10:00:00,123", "2024-01-31T10:00:00.123Z",
    // "[2024-01-31 10:00:00]". Testa prefixos decrescentes, sempre em
    // fronteiras de caractere para não quebrar texto não-ASCII.
    let start = if head.starts_with('[') { 1 } else { 0 };
    let ends: Vec<usize> = head
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(head.len()))
        .filter(|&i| i > start && i <= start + 35)
        .collect();
    for &end in ends.iter().rev() {
        let cand = &head[start..end];
        if let Some(ms) = parse_timestamp(cand) {
            let closing = usize::from(head[end..].starts_with(']'));
            return Some((ms, end + closing));
        }
    }
    None
}

// ==========================================================================
// Parsers de formatos conhecidos
// ==========================================================================

fn re_syslog3164() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(?:<(\d+)>)?([A-Z][a-z]{2})\s+(\d{1,2})\s+(\d{2}):(\d{2}):(\d{2})\s+(\S+)\s+(\S+?)(?:\[(\d+)\])?:\s+(.*)$").unwrap()
    })
}

fn re_syslog5424() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(?:<(\d+)>)?1\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+\S+\s+(.*)$")
            .unwrap()
    })
}

fn month_num(mon: &str) -> Option<u32> {
    Some(match mon {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn syslog_ts(mon: &str, day: &str, h: &str, mi: &str, s: &str, year: i32) -> Option<i64> {
    let (d, h, mi, s) = (
        day.parse().ok()?,
        h.parse().ok()?,
        mi.parse().ok()?,
        s.parse().ok()?,
    );
    chrono::NaiveDate::from_ymd_opt(year, month_num(mon)?, d)
        .and_then(|dt| dt.and_hms_opt(h, mi, s))
        .map(naive_to_ms)
}

fn parse_syslog3164(line: &str, year: i32) -> Option<Event> {
    let c = re_syslog3164().captures(line)?;
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = syslog_ts(&c[2], &c[3], &c[4], &c[5], &c[6], year);
    ev.source = c[7].to_string();
    let proc = c[8].to_string();
    ev.fields
        .insert("process".into(), Value::from(proc.clone()));
    if let Some(pid) = c.get(9) {
        ev.fields.insert("pid".into(), Value::from(pid.as_str()));
    }
    ev.message = c[10].to_string();
    ev.level = crate::model::class_label(text_level_class(c[10].as_bytes())).to_string();
    Some(ev)
}

fn parse_syslog5424(line: &str) -> Option<Event> {
    let c = re_syslog5424().captures(line)?;
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = parse_timestamp(&c[2]);
    ev.source = c[3].to_string();
    ev.fields
        .insert("app".into(), Value::from(c[4].to_string()));
    ev.fields
        .insert("pid".into(), Value::from(c[5].to_string()));
    ev.fields
        .insert("msgid".into(), Value::from(c[6].to_string()));
    ev.message = c[7].to_string();
    ev.level = crate::model::class_label(text_level_class(c[7].as_bytes())).to_string();
    Some(ev)
}

fn parse_apache(line: &str) -> Option<Event> {
    web_logs::parse_access(line)
}

fn parse_firewall(line: &str, year: i32) -> Option<Event> {
    // iptables/netfilter: prefixo estilo syslog + pares CHAVE=VALOR
    let c = re_syslog3164().captures(line)?;
    let msg = c[10].to_string();
    let kv = |key: &str| -> Option<String> {
        msg.split_whitespace()
            .find_map(|t| t.strip_prefix(key).map(|v| v.to_string()))
    };
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = syslog_ts(&c[2], &c[3], &c[4], &c[5], &c[6], year);
    ev.source = c[7].to_string();
    ev.fields
        .insert("process".into(), Value::from(c[8].to_string()));
    for (key, name) in [
        ("SRC=", "src"),
        ("DST=", "dst"),
        ("PROTO=", "proto"),
        ("SPT=", "spt"),
        ("DPT=", "dpt"),
        ("IN=", "iface_in"),
        ("OUT=", "iface_out"),
    ] {
        if let Some(v) = kv(key) {
            ev.fields.insert(name.into(), Value::from(v));
        }
    }
    for action in ["DROP", "ACCEPT", "REJECT"] {
        if msg.contains(action) {
            ev.code = action.to_string();
            break;
        }
    }
    ev.level = if ev.code == "DROP" || ev.code == "REJECT" {
        "Aviso".into()
    } else {
        "Informação".into()
    };
    ev.message = msg;
    Some(ev)
}

/// Formato customizado: regex com grupos nomeados. Nomes reservados:
/// timestamp, level, code, source, message — os demais viram campos.
fn parse_custom(line: &str, re: &regex::Regex) -> Option<Event> {
    let c = re.captures(line)?;
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    for name in re.capture_names().flatten() {
        let Some(m) = c.name(name) else { continue };
        let v = m.as_str();
        match name {
            "timestamp" | "ts" | "time" => ev.timestamp = parse_timestamp(v),
            "level" => ev.level = normalize_level(v),
            "code" => ev.code = v.to_string(),
            "source" | "host" => ev.source = v.to_string(),
            "message" | "msg" => ev.message = v.to_string(),
            other => {
                ev.fields.insert(other.into(), Value::from(v));
            }
        }
    }
    if ev.message.is_empty() {
        ev.message = line.to_string();
    }
    Some(ev)
}

/// Several objects on consecutive lines (JSONL) rather than one document.
fn looks_like_jsonl(bytes: &[u8]) -> bool {
    let Some(nl) = memchr::memchr(b'\n', bytes) else { return false };
    bytes[nl + 1..]
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'{')
        && serde_json::from_slice::<Value>(&bytes[..nl]).is_ok()
}

const ENVELOPE_KEYS: &[&str] = &[
    "records", "value", "data", "events", "items", "logs", "entries", "results", "hits", "rows",
    "messages", "alerts", "logevents", "findings", "activities", "signins", "detections", "auditlogs",
];

/// Offset of the records array inside a JSON document such as
/// `{"Records":[...]}` (CloudTrail), `{"value":[...]}` (Azure, Graph) or
/// `{"hits":{"hits":[...]}}` (Elasticsearch export).
pub(crate) fn json_envelope_array(bytes: &[u8]) -> Option<usize> {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace() && *b != 0xef && *b != 0xbb && *b != 0xbf)?;
    if bytes[start] != b'{' {
        return None;
    }
    envelope_in_object(bytes, start, 0)
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn read_json_string(bytes: &[u8], i: usize) -> Option<(String, usize)> {
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return Some((String::from_utf8_lossy(&bytes[i + 1..j]).into_owned(), j + 1)),
            _ => j += 1,
        }
    }
    None
}

/// Skips one JSON value starting at `i`; returns the index after it.
fn skip_json_value(bytes: &[u8], i: usize) -> Option<usize> {
    let i = skip_ws(bytes, i);
    match bytes.get(i)? {
        b'"' => read_json_string(bytes, i).map(|(_, end)| end),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'"' => {
                        j = read_json_string(bytes, j)?.1;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < bytes.len() && !matches!(bytes[j], b',' | b'}' | b']') {
                j += 1;
            }
            Some(j)
        }
    }
}

fn envelope_in_object(bytes: &[u8], open: usize, depth: usize) -> Option<usize> {
    if depth > 2 {
        return None;
    }
    let mut i = skip_ws(bytes, open + 1);
    let limit = bytes.len().min(open + 64 * 1024 * 1024);
    while i < limit {
        if bytes[i] == b'}' {
            return None;
        }
        let (key, after) = read_json_string(bytes, i)?;
        i = skip_ws(bytes, after);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(bytes, i + 1);
        let known = ENVELOPE_KEYS.contains(&key.to_ascii_lowercase().as_str());
        match bytes.get(i)? {
            b'[' if known => {
                let first = skip_ws(bytes, i + 1);
                if bytes.get(first) == Some(&b'{') {
                    return Some(i);
                }
            }
            b'{' if known => {
                if let Some(found) = envelope_in_object(bytes, i, depth + 1) {
                    return Some(found);
                }
            }
            _ => {}
        }
        i = skip_ws(bytes, skip_json_value(bytes, i)?);
        if bytes.get(i) == Some(&b',') {
            i = skip_ws(bytes, i + 1);
        }
    }
    None
}

/// Zeek TSV: `#fields` header, `-` unset and `(empty)` values.
fn parse_zeek(line: &str, header: &[String]) -> Option<Event> {
    if header.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut map = Map::new();
    for (key, value) in header.iter().zip(line.split('\t')) {
        if value == "-" || value == "(empty)" || value.is_empty() {
            continue;
        }
        let v = match value.parse::<f64>() {
            Ok(n) if key == "ts" => serde_json::Number::from_f64(n).map(Value::Number).unwrap_or_else(|| Value::from(value)),
            _ => Value::from(value),
        };
        map.insert(key.clone(), v);
    }
    let mut ev = event_from_json(map, line);
    ev.raw = line.to_string();
    Some(ev)
}

fn decode_hex_text(value: &str) -> Option<String> {
    if value.len() < 4 || value.len() % 2 != 0 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes: Vec<u8> = (0..value.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .map(|b| if b == 0 { b' ' } else { b })
        .collect();
    let text = String::from_utf8(bytes).ok()?;
    text.chars().all(|c| !c.is_control()).then(|| text.trim().to_string())
}

/// Linux audit: `type=... msg=audit(epoch.ms:serial): key=value ... msg='k=v ...'`.
fn parse_auditd(line: &str) -> Option<Event> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static KV: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let head = RE.get_or_init(|| regex::Regex::new(r"^(?:node=(\S+) )?type=(\S+) msg=audit\((\d+)(?:\.(\d+))?:(\d+)\):\s?(.*)$").unwrap());
    let kv = KV.get_or_init(|| regex::Regex::new(r#"([A-Za-z0-9_\-]+)=("(?:[^"\\]|\\.)*"|'[^']*'|\S*)"#).unwrap());
    let c = head.captures(line)?;
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    let seconds: i64 = c[3].parse().ok()?;
    let millis: i64 = c.get(4).and_then(|m| m.as_str().get(..3)).and_then(|m| format!("{m:0<3}").parse().ok()).unwrap_or(0);
    ev.timestamp = Some(seconds * 1000 + millis);
    let kind = c[2].to_string();
    ev.code = kind.clone();
    ev.source = c.get(1).map(|m| m.as_str().to_string()).unwrap_or_else(|| "auditd".into());
    ev.fields.insert("type".into(), Value::from(kind.clone()));
    ev.fields.insert("audit_serial".into(), Value::from(c[5].to_string()));
    let body = c[6].to_string();
    let add = |text: &str, fields: &mut Map<String, Value>| {
        for cap in kv.captures_iter(text) {
            let key = cap[1].to_string();
            let raw_value = &cap[2];
            if key == "msg" && raw_value.starts_with('\'') {
                continue;
            }
            let value = raw_value.trim_matches(['"', '\'']).to_string();
            fields.entry(key).or_insert(Value::from(value));
        }
    };
    add(&body, &mut ev.fields);
    // USER_* records nest their details in msg='op=... acct=... res=...'.
    if let Some(start) = body.find("msg='") {
        if let Some(end) = body[start + 5..].find('\'') {
            let inner = body[start + 5..start + 5 + end].to_string();
            add(&inner, &mut ev.fields);
            ev.message = format!("{kind} {inner}");
        }
    }
    for key in ["proctitle", "cmd"] {
        if let Some(decoded) = ev.fields.get(key).and_then(Value::as_str).and_then(decode_hex_text) {
            ev.fields.insert("cmdline".into(), Value::from(decoded));
        }
    }
    let failed = ev.fields.get("res").and_then(Value::as_str).is_some_and(|r| r.starts_with("fail"))
        || ev.fields.get("success").and_then(Value::as_str) == Some("no");
    ev.level = if failed { "Aviso" } else { "Informação" }.into();
    if ev.message.is_empty() {
        let what = ["cmdline", "exe", "comm", "name", "key"]
            .iter()
            .find_map(|k| ev.fields.get(*k).and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        ev.message = format!("{kind} {what}").trim().to_string();
    }
    Some(ev)
}

/// Inferência automática do formato pela amostra inicial do arquivo.
pub(crate) fn detect_format(bytes: &[u8]) -> &'static str {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let trimmed = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|i| &bytes[i..])
        .unwrap_or(bytes);
    if trimmed.first() == Some(&b'[') {
        let next = trimmed[1..].iter().find(|b| !b.is_ascii_whitespace());
        if matches!(next, Some(b'{') | Some(b']')) {
            return "jsonl";
        }
    }
    if trimmed.first() == Some(&b'{') && !looks_like_jsonl(trimmed) && json_envelope_array(bytes).is_some() {
        return "jsonl";
    }
    if trimmed.starts_with(b"#separator") || trimmed.starts_with(b"#fields\t") {
        return "zeek";
    }
    {
        let head: Vec<&[u8]> = trimmed.split(|&b| b == b'\n').take(20).filter(|l| !l.is_empty()).collect();
        let audit = head
            .iter()
            .filter(|l| (l.starts_with(b"type=") || l.starts_with(b"node=")) && memchr::memmem::find(l, b"msg=audit(").is_some())
            .count();
        if !head.is_empty() && audit * 2 > head.len() {
            return "auditd";
        }
    }
    let mut n = 0usize;
    let mut nginx_error = 0usize;
    let (mut json, mut s3164, mut s5424, mut apache, mut fw, mut log4j, mut logfmt, mut wildfly) =
        (0, 0, 0, 0, 0, 0, 0, 0);
    // linhas de corpo de stacktrace Java contam como evidência de log4j/wildfly
    // (arquivos com muitos stacktraces teriam poucas linhas de cabeçalho)
    let mut stack = 0usize;
    let mut w3c_hint = false;
    for line_b in bytes.split(|&b| b == b'\n').take(60) {
        let line = std::str::from_utf8(line_b).unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if is_stacktrace_line(line) {
            stack += 1;
            continue;
        }
        if line.starts_with('#') {
            if line.to_lowercase().starts_with("#fields:") {
                w3c_hint = true;
            }
            continue;
        }
        if line.starts_with("CEF:") {
            return "cef";
        }
        if line.starts_with("LEEF:") {
            return "leef";
        }
        n += 1;
        if line.starts_with('{') {
            json += 1;
        } else if re_syslog5424().is_match(line) {
            s5424 += 1;
        } else if web_logs::parse_access(line).is_some() {
            apache += 1;
        } else if web_logs::parse_error(line).is_some() {
            nginx_error += 1;
        } else if re_log4j().is_match(line) {
            log4j += 1;
        } else if re_wildfly().is_match(line) || re_jboss().is_match(line) {
            wildfly += 1;
        } else if re_syslog3164().is_match(line) {
            if line.contains("SRC=") || line.contains("PROTO=") || line.contains("DPT=") {
                fw += 1;
            } else {
                s3164 += 1;
            }
        } else if web_logs::parse_logfmt(line).is_some() {
            logfmt += 1;
        }
    }
    if w3c_hint {
        return "w3c";
    }
    if let Some(format) = detect_delimited(bytes) {
        return format;
    }
    if n == 0 {
        return "text";
    }
    let half = n / 2;
    if json > half {
        "jsonl"
    } else if s5424 > half {
        "syslog5424"
    } else if apache > half {
        "apache"
    } else if nginx_error > half {
        "nginx-error"
    } else if fw > half {
        "firewall"
    } else if wildfly > half || (wildfly > 0 && wildfly + stack > half) {
        "wildfly"
    } else if log4j > half || (log4j > 0 && log4j + stack > half) {
        "log4j"
    } else if s3164 > half {
        "syslog3164"
    } else if logfmt > half {
        "logfmt"
    } else if json + apache + nginx_error + logfmt > 0 {
        "mixed"
    } else {
        "text"
    }
}

fn re_log4j() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}[,.]\d{3})\s+(\w+)\s+\[([^\]]*)\]\s+(\S+)\s+-\s+(.*)$").unwrap()
    })
}

fn re_kv_key() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(\w+)=").unwrap())
}

/// Extrai pares chave=valor onde valores podem conter espaços
/// (valor vai até o início da próxima chave).
fn parse_kv_pairs(s: &str) -> Vec<(String, String)> {
    let caps: Vec<_> = re_kv_key().captures_iter(s).collect();
    caps.iter()
        .enumerate()
        .map(|(i, cap)| {
            let key = cap[1].to_string();
            let start = cap.get(0).unwrap().end();
            let end = caps
                .get(i + 1)
                .map(|c| c.get(0).unwrap().start())
                .unwrap_or(s.len());
            (key, s[start..end].trim().to_string())
        })
        .collect()
}

fn re_wildfly() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(\d{2}:\d{2}:\d{2},\d{3})\s+(TRACE|DEBUG|INFO|WARN|ERROR|FATAL)\s+\[([^\]]+)\]\s+\(([^)]*)\)\s+(.*)$").unwrap()
    })
}

/// JBoss EAP 7 (com data): `2026-07-08 00:00:00,000 WARN  [br.app.Classe] (thread) mensagem`
fn re_jboss() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2},\d{3})\s+(TRACE|DEBUG|INFO|WARN|ERROR|FATAL)\s+\[([^\]]+)\]\s+\(([^)]*)\)\s+(.*)$").unwrap()
    })
}

/// Âncoras genéricas de "início de evento": timestamp no começo da linha.
/// Usadas quando o arquivo não casa nenhum regex estruturado conhecido.
fn re_anchor_datetime() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}").unwrap())
}

fn re_anchor_time() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^\d{2}:\d{2}:\d{2}[,.]\d{3}").unwrap())
}

fn re_anchor_date() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^\d{4}-\d{2}-\d{2}\s").unwrap())
}

/// Aprende o padrão de início de evento do próprio arquivo (formatos Java
/// multi-linha): tenta os regexes estruturados (log4j, JBoss com data,
/// WildFly só-hora) e, na falta deles, âncoras genéricas de timestamp no
/// começo da linha. Um candidato só é aceito se casar >= 2 linhas da
/// amostra — sem padrão confiável retorna None e cada linha vira um evento
/// (nunca cola o arquivo inteiro num evento único).
fn detect_entry_start(bytes: &[u8]) -> Option<regex::Regex> {
    let sample: Vec<&str> = bytes
        .split(|&b| b == b'\n')
        .take(2_000)
        .filter_map(|l| std::str::from_utf8(l).ok())
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    [
        re_log4j(),
        re_jboss(),
        re_wildfly(),
        re_anchor_datetime(),
        re_anchor_time(),
        re_anchor_date(),
    ]
    .into_iter()
    .find(|re| sample.iter().filter(|line| re.is_match(line)).count() >= 2)
    .cloned()
}

/// Linha típica de corpo de stacktrace Java (continuação de evento log4j/wildfly).
/// Recebe a linha já trimada.
fn is_stacktrace_line(t: &str) -> bool {
    t.starts_with("at ")
        || t.starts_with("Caused by:")
        || t.starts_with("Suppressed:")
        || (t.starts_with("...") && t.ends_with(" more"))
}

/// Extrai evidências do corpo multi-linha de um evento Java (stacktrace):
/// - frames `at ...` viram fields["stacktrace"] (array de strings, sem indentação)
/// - a primeira linha não-`at` que parece exceção vira fields["exception"]
///   (cobre "java.lang.X: msg" e "Caused by: java.lang.X: msg")
fn extract_java_body(ev: &mut Event, body: &str) {
    if body.is_empty() {
        return;
    }
    let mut frames: Vec<Value> = Vec::new();
    let mut exception: Option<String> = None;
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("at ") {
            frames.push(Value::from(t));
        } else if exception.is_none() && (t.contains("Exception") || t.contains("Error")) {
            exception = Some(t.to_string());
        }
    }
    if !frames.is_empty() {
        ev.fields.insert("stacktrace".into(), Value::Array(frames));
    }
    if let Some(ex) = exception {
        ev.fields.insert("exception".into(), Value::from(ex));
    }
}

/// Interpret only the already-framed Java block. Existing evidence fields and
/// byte framing are unchanged. This infallible source parser uses the strictly
/// bounded pure parser; outer indexing/query operation checks own cancellation.
fn enrich_java_trace(ev: &mut Event, block: &str, body_start: usize) {
    let Ok(trace) = crate::java_stacktrace::parse(block, body_start, Default::default()) else {
        return;
    };
    ev.fields.extend(trace.scalar_fields());
}

/// Rich details are request-local metadata, not an indexed Event field. Missing
/// historical raw or an unrecognized Java log header is explicitly unavailable.
/// Header inspection is capped at64KiB; the pure trace parser inspects at most
/// its256KiB suffix budget. Callers retain their outer operation checks.
pub(crate) fn java_trace_for_event(event: &Event) -> Result<Option<crate::java_stacktrace::Trace>, String> {
    if event.raw.is_empty() { return Ok(None); }
    let mut end = event.raw.len().min(64 << 10);
    while !event.raw.is_char_boundary(end) { end -= 1; }
    let inspected = &event.raw[..end];
    let head = inspected.split('\n').next().unwrap_or("").trim_end_matches('\r');
    let body_start = [re_log4j(), re_jboss(), re_wildfly()].into_iter()
        .find_map(|re| re.captures(head).and_then(|captures| captures.get(5).map(|body| body.start())));
    let Some(body_start) = body_start else {
        if end < event.raw.len() && !inspected.contains('\n') {
            return Err("O cabeçalho excede o limite de inspeção da estrutura Java.".into());
        }
        return Ok(None);
    };
    let trace = crate::java_stacktrace::parse(&event.raw, body_start, Default::default())
        .map_err(|error| error.to_string())?;
    let limited = trace.diagnostics.iter().any(|d| d.code == crate::java_stacktrace::Code::InputLimit);
    Ok((trace.observed() || limited).then_some(trace))
}

/// WildFly/JBoss: `00:00:00,001 WARN  [br.app.Classe] (EJB default - 4) mensagem`
/// Só tem hora — a data costuma estar no nome do arquivo (server.log.2026-06-24),
/// resolvida pela configuração de data/hora (TsConfig).
/// `block` pode ser multi-linha (cabeçalho + stacktrace): os campos vêm da
/// primeira linha; o corpo alimenta `stacktrace`/`exception`; `raw` é o bloco.
fn parse_wildfly(block: &str) -> Option<Event> {
    let (head, body) = block.split_once('\n').unwrap_or((block, ""));
    let c = re_wildfly().captures(head.trim_end_matches('\r'))?;
    let mut ev = Event::empty();
    ev.raw = block.to_string();
    ev.fields
        .insert("hora_linha".into(), Value::from(c[1].to_string()));
    ev.level = normalize_level(&c[2]);
    ev.source = c[3].to_string();
    ev.fields
        .insert("logger".into(), Value::from(c[3].to_string()));
    ev.fields
        .insert("thread".into(), Value::from(c[4].to_string()));
    ev.message = c[5].to_string();
    extract_java_body(&mut ev, body);
    enrich_java_trace(&mut ev, block, c.get(5)?.start());
    Some(ev)
}

/// JBoss EAP 7 com data completa: mesmos campos do WildFly, mas o
/// timestamp da linha já tem data — preenche `timestamp` direto.
fn parse_jboss(block: &str) -> Option<Event> {
    let (head, body) = block.split_once('\n').unwrap_or((block, ""));
    let c = re_jboss().captures(head.trim_end_matches('\r'))?;
    let mut ev = Event::empty();
    ev.raw = block.to_string();
    ev.timestamp = parse_timestamp(&c[1].replace(',', "."));
    ev.level = normalize_level(&c[2]);
    ev.source = c[3].to_string();
    ev.fields
        .insert("logger".into(), Value::from(c[3].to_string()));
    ev.fields
        .insert("thread".into(), Value::from(c[4].to_string()));
    ev.message = c[5].to_string();
    extract_java_body(&mut ev, body);
    enrich_java_trace(&mut ev, block, c.get(5)?.start());
    Some(ev)
}

/// Log4j/Logback: `2024-01-31 08:00:01,123 INFO [thread] com.app.Classe - mensagem`
/// Mesmo tratamento multi-linha de `parse_wildfly`.
fn parse_log4j(block: &str) -> Option<Event> {
    let (head, body) = block.split_once('\n').unwrap_or((block, ""));
    let c = re_log4j().captures(head.trim_end_matches('\r'))?;
    let mut ev = Event::empty();
    ev.raw = block.to_string();
    ev.timestamp = parse_timestamp(&c[1].replace(',', "."));
    ev.level = normalize_level(&c[2]);
    ev.fields
        .insert("thread".into(), Value::from(c[3].to_string()));
    ev.fields
        .insert("logger".into(), Value::from(c[4].to_string()));
    ev.source = c[4].to_string();
    ev.message = c[5].to_string();
    extract_java_body(&mut ev, body);
    enrich_java_trace(&mut ev, block, c.get(5)?.start());
    Some(ev)
}

/// Logfmt (Heroku/estilo key=value): `ts=... level=info msg="..." k=v`
fn parse_logfmt(line: &str) -> Option<Event> {
    web_logs::parse_logfmt(line)
}

/// Only strict, self-describing single-line formats participate in fallback.
/// Explicit text, snapshots, custom schemas and multiline framing stay intact.
fn parse_structured_line(line: &str) -> Option<Event> {
    let trimmed = line.trim();
    if trimmed.starts_with('{') {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
            return Some(event_from_json(map, line));
        }
        return None;
    }
    if web_logs::error_candidate(trimmed) {
        return web_logs::parse_error(line);
    }
    if web_logs::access_candidate(trimmed) {
        if let Some(event) = parse_apache(line) { return Some(event); }
    }
    if web_logs::logfmt_candidate(trimmed) { return parse_logfmt(line); }
    None
}

/// CEF (ArcSight): `CEF:0|Vendor|Product|Version|SignatureID|Name|Severity|extensão`
fn parse_cef(line: &str) -> Option<Event> {
    if !line.starts_with("CEF:") {
        return None;
    }
    let parts: Vec<&str> = line.splitn(8, '|').collect();
    if parts.len() < 8 {
        return None;
    }
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.fields.insert("vendor".into(), Value::from(parts[1]));
    ev.source = parts[2].to_string();
    ev.fields
        .insert("product_version".into(), Value::from(parts[3]));
    ev.code = parts[4].to_string();
    ev.message = parts[5].to_string();
    let sev: u32 = parts[6].trim().parse().unwrap_or(0);
    ev.level = match sev {
        8..=10 => "Erro",
        4..=7 => "Aviso",
        _ => "Informação",
    }
    .into();
    for (k, v) in parse_kv_pairs(parts[7]) {
        let k = k.as_str();
        let v = v.as_str();
        match k {
            "rt" | "start" | "end" => {
                ev.timestamp = parse_timestamp(v).or_else(|| {
                    v.parse::<f64>().ok().map(|f| {
                        if f > 1e12 {
                            f as i64
                        } else {
                            (f * 1000.0) as i64
                        }
                    })
                });
            }
            "msg" => ev.message = format!("{} — {}", ev.message, v),
            other => {
                ev.fields.insert(other.into(), Value::from(v));
            }
        }
    }
    Some(ev)
}

/// LEEF (IBM QRadar): `LEEF:1.0|Vendor|Product|Version|EventID|chave=valor...`
fn parse_leef(line: &str) -> Option<Event> {
    if !line.starts_with("LEEF:") {
        return None;
    }
    let parts: Vec<&str> = line.splitn(6, '|').collect();
    if parts.len() < 6 {
        return None;
    }
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.fields.insert("vendor".into(), Value::from(parts[1]));
    ev.source = parts[2].to_string();
    ev.fields
        .insert("product_version".into(), Value::from(parts[3]));
    ev.code = parts[4].to_string();
    for (k, v) in parse_kv_pairs(parts[5]) {
        let k = k.as_str();
        let v = v.as_str();
        match k {
            "devTime" | "rt" | "start" => {
                ev.timestamp = parse_timestamp(v).or_else(|| {
                    v.parse::<f64>().ok().map(|f| {
                        if f > 1e12 {
                            f as i64
                        } else {
                            (f * 1000.0) as i64
                        }
                    })
                });
            }
            "msg" => ev.message = v.to_string(),
            "sev" => {
                let s: u32 = v.parse().unwrap_or(1);
                ev.level = match s {
                    8..=10 => "Erro",
                    4..=7 => "Aviso",
                    _ => "Informação",
                }
                .into();
            }
            other => {
                ev.fields.insert(other.into(), Value::from(v));
            }
        }
    }
    if ev.message.is_empty() {
        ev.message = format!("{} ({})", parts[2], parts[4]);
    }
    Some(ev)
}

//// Formatos tabulares com cabeçalho e o separador de cada um: CSV com vírgula,
/// CSV com ponto e vírgula (Excel em português e outros idiomas com vírgula
/// decimal), TSV e barra vertical.
const DELIMITED: [(&str, char); 4] = [("csv", ','), ("csv-semicolon", ';'), ("tsv", '\t'), ("csv-pipe", '|')];

pub(crate) fn delimiter_of(format: &str) -> Option<char> {
    DELIMITED.iter().find(|(id, _)| *id == format).map(|(_, delimiter)| *delimiter)
}

/// Divide uma linha pelo separador, respeitando aspas duplas com escape "".
fn split_delimited(line: &str, delimiter: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_q = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_q = true;
        } else if c == delimiter {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

#[cfg(test)]
fn split_csv(line: &str) -> Vec<String> {
    split_delimited(line, ',')
}

/// Nome de coluna plausível num cabeçalho: texto curto com letras, que não é
/// data, JSON nem chave=valor.
fn plausible_column_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.chars().count() <= 64
        && name.chars().any(char::is_alphabetic)
        && !name.chars().any(|c| c.is_control() || matches!(c, '{' | '}' | '[' | ']' | '=' | '"'))
        && name.split_whitespace().count() <= 6
        && parse_timestamp(name).is_none()
}

/// Texto tabular com cabeçalho: o separador que divide a primeira linha em
/// nomes de coluna plausíveis e a maioria das linhas seguintes no mesmo número
/// de valores (ou um a mais, como a mensagem sem nome das exportações do
/// Visualizador de Eventos do Windows).
fn detect_delimited(bytes: &[u8]) -> Option<&'static str> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let lines: Vec<&str> = bytes
        .split(|&b| b == b'\n')
        .take(60)
        .filter_map(|line| std::str::from_utf8(line.strip_suffix(b"\r").unwrap_or(line)).ok())
        .filter(|line| !line.trim().is_empty())
        .take(30)
        .collect();
    let (header, rows) = lines.split_first()?;
    if rows.is_empty() || header.trim_start().starts_with(['{', '[', '#']) {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    for (format, delimiter) in DELIMITED {
        let names = split_delimited(header, delimiter);
        let minimum = if matches!(delimiter, ',' | '|') { 3 } else { 2 };
        if names.len() < minimum || !names.iter().all(|name| plausible_column_name(name)) {
            continue;
        }
        let consistent = rows
            .iter()
            .filter(|row| {
                let count = split_delimited(row, delimiter).len();
                count == names.len() || count == names.len() + 1
            })
            .count();
        if consistent * 10 >= rows.len() * 8 && best.is_none_or(|(_, columns)| names.len() > columns) {
            best = Some((format, names.len()));
        }
    }
    best.map(|(format, _)| format)
}

/// Significado de um nome de coluna em fontes tabulares (CSV, TSV, IIS e
/// planilhas), em inglês ou português.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColumnRole {
    Timestamp,
    Date,
    Time,
    Level,
    Code,
    Source,
    Message,
    Other,
}

/// Minúsculas sem acentos e com separadores uniformes ("Data/Hora" → "data hora").
fn column_key(name: &str) -> String {
    let folded: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            '_' | '-' | '/' | '.' | ':' => ' ',
            other => other,
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn column_role(name: &str) -> ColumnRole {
    let key = column_key(name);
    let is = |names: &[&str]| names.contains(&key.as_str());
    if is(&[
        "timestamp", "@timestamp", "ts", "datetime", "date time", "time stamp", "event created", "event time",
        "eventtime", "time generated", "timegenerated", "time created", "timecreated", "created", "created at",
        "data hora", "data e hora", "datahora", "carimbo de data hora", "momento", "registrado em", "ocorrido em",
    ]) {
        ColumnRole::Timestamp
    } else if is(&["date", "data", "dia", "day"]) {
        ColumnRole::Date
    } else if is(&["time", "hora", "horario", "hour"]) {
        ColumnRole::Time
    } else if is(&[
        "level", "severity", "lvl", "log level", "loglevel", "severity text", "severitytext", "nivel", "severidade",
        "criticidade", "gravidade",
    ]) {
        ColumnRole::Level
    } else if is(&[
        "code", "id", "event id", "eventid", "event code", "status", "sc status", "codigo", "codigo do evento",
        "id do evento", "identificacao do evento",
    ]) {
        ColumnRole::Code
    } else if is(&[
        "source", "provider", "logger", "service", "channel", "service name", "host name", "s ip", "c ip", "origem",
        "fonte", "servidor", "host", "hostname", "computador", "computer", "maquina", "sistema", "servico",
        "aplicacao", "aplicativo", "equipamento", "dispositivo",
    ]) {
        ColumnRole::Source
    } else if is(&[
        "message", "msg", "log", "body", "text", "displaymessage", "description", "details", "detail", "mensagem",
        "descricao", "detalhes", "detalhe", "evento", "ocorrencia", "historico", "observacao", "texto", "conteudo",
        "resumo",
    ]) {
        ColumnRole::Message
    } else {
        ColumnRole::Other
    }
}

/// Data e hora vindas de uma coluna só ou de uma coluna de data e outra de hora.
pub(crate) fn date_time_ms(date: Option<&str>, time: Option<&str>) -> Option<i64> {
    match (date, time) {
        (Some(date), Some(time)) => parse_timestamp(&format!("{date} {time}"))
            .or_else(|| parse_timestamp(date))
            .or_else(|| parse_timestamp(time)),
        (Some(value), None) | (None, Some(value)) => parse_timestamp(value),
        (None, None) => None,
    }
}

/// Mapeia colunas nomeadas (cabeçalho CSV/TSV ou #Fields do IIS) para o evento.
/// Valores além do cabeçalho recebem o nome da posição ("coluna 6").
fn event_from_columns(vals: Vec<String>, header: &[String], raw: &str) -> Event {
    let mut ev = Event::empty();
    ev.raw = raw.to_string();
    let mut level_set = false;
    // Resolved after every column is seen: a timestamp column wins, a date and a time are combined.
    let (mut stamp, mut date, mut time): (Option<(String, String)>, Option<(String, String)>, Option<(String, String)>) =
        (None, None, None);
    let mut unnamed: Option<(String, String)> = None;
    for (i, value) in vals.iter().enumerate() {
        let v = value.trim();
        if v.is_empty() || v == "-" {
            continue;
        }
        let Some(name) = header.get(i).map(|h| h.trim()).filter(|h| !h.is_empty()).map(str::to_string) else {
            // Windows Event Viewer exports leave the message column unnamed.
            let name = format!("coluna {}", i + 1);
            match unnamed {
                None => unnamed = Some((name, v.to_string())),
                Some(_) => {
                    ev.fields.insert(name, Value::from(v));
                }
            }
            continue;
        };
        match column_role(&name) {
            ColumnRole::Timestamp if stamp.is_none() => stamp = Some((name, v.to_string())),
            ColumnRole::Date if date.is_none() => date = Some((name, v.to_string())),
            ColumnRole::Time if time.is_none() => time = Some((name, v.to_string())),
            ColumnRole::Level if !level_set => {
                ev.level = normalize_level(v);
                level_set = true;
            }
            ColumnRole::Code if ev.code.is_empty() => ev.code = v.to_string(),
            ColumnRole::Source if ev.source.is_empty() => ev.source = v.to_string(),
            ColumnRole::Message if ev.message.is_empty() => ev.message = v.to_string(),
            _ => {
                ev.fields.insert(name, Value::from(v));
            }
        }
    }
    let text = |pair: &Option<(String, String)>| pair.as_ref().map(|(_, v)| v.clone());
    ev.timestamp = text(&stamp).and_then(|v| parse_timestamp(&v));
    let stamp_used = ev.timestamp.is_some();
    let parts_used = !stamp_used && {
        ev.timestamp = date_time_ms(text(&date).as_deref(), text(&time).as_deref());
        ev.timestamp.is_some()
    };
    // Columns that did not become the event time remain available as fields.
    for (used, pair) in [(stamp_used, stamp), (parts_used, date), (parts_used, time)] {
        if let (false, Some((name, value))) = (used, pair) {
            ev.fields.insert(name, Value::from(value));
        }
    }
    if let Some((name, value)) = unnamed {
        if ev.message.is_empty() {
            ev.message = value;
        } else {
            ev.fields.insert(name, Value::from(value));
        }
    }
    if ev.message.is_empty() {
        ev.message = raw.chars().take(500).collect();
    }
    ev
}

fn parse_w3c(line: &str, header: &[String]) -> Option<Event> {
    if header.is_empty() || line.starts_with('#') {
        return None;
    }
    let vals: Vec<String> = line.split(' ').map(|s| s.to_string()).collect();
    Some(event_from_columns(vals, header, line))
}

fn parse_delimited(line: &str, header: &[String], delimiter: char) -> Option<Event> {
    if header.is_empty() {
        return None;
    }
    Some(event_from_columns(split_delimited(line, delimiter), header, line))
}

#[cfg(test)]
fn parse_csv_line(line: &str, header: &[String]) -> Option<Event> {
    parse_delimited(line, header, ',')
}

// ==========================================================================
// Arquivos grandes: índice em memória sobre mmap
// ==========================================================================

/// Configuração de data/hora: como montar o timestamp do evento.
/// `sources`: campos a concatenar (colunas, "arquivo", "caminho", "linha").
/// `regex`: opcional, extrai o texto da data (2 grupos = data + hora).
/// `format`: padrão chrono, "epoch_ms" ou "epoch_s".
/// `complement`: data literal ("2026-06-24") quando o formato só tem hora.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
pub struct TsConfig {
    #[serde(default)]
    pub timezone_offset_minutes: Option<i32>,
    #[serde(default)]
    pub clock_adjustment_ms: i64,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub regex: Option<String>,
    /// Montagem do texto final a partir dos grupos: \1 \2 (ou $1 $2).
    /// Ex.: regex `(\d{2})/(\d{2})/(\d{4})` + template `$3-$1-$2` → "2024-31-01"→"2024-01-31".
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub complement: Option<String>,
    /// Regras alternativas (OU): a primeira que produzir um timestamp vence.
    /// Vazio = usa `regex`/`template` da raiz (formato legado).
    #[serde(default)]
    pub rules: Vec<TsRule>,
}

/// Uma alternativa de extração de data/hora (regex + montagem opcional).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
pub struct TsRule {
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
}

/// TsConfig com as regexes já compiladas.
#[derive(Clone)]
pub struct CompiledTsConfig {
    /// Pin an omitted date once, rather than changing semantics across midnight.
    pub reference_date: chrono::NaiveDate,
    pub timezone_offset_minutes: Option<i32>,
    pub clock_adjustment_ms: i64,
    pub sources: Vec<String>,
    pub format: String,
    pub complement: Option<String>,
    /// (regex compilada, template) por regra, na ordem do OU
    pub rules: Vec<(Option<regex::Regex>, Option<String>)>,
}

impl CompiledTsConfig {
    /// Stable cache fingerprint shared by metadata and columnar stores.
    pub(crate) fn signature(&self) -> String {
        let rules: Vec<_> = self.rules.iter()
            .map(|(re, template)| (re.as_ref().map(|r| r.as_str()), template.as_ref())).collect();
        let fallback_date = self.complement.as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
            .is_none()
            && !self.format.starts_with("epoch")
            && chrono::format::StrftimeItems::new(&self.format).any(|item| {
                use chrono::format::{Fixed, Item, Numeric};
                matches!(item, Item::Numeric(Numeric::Hour | Numeric::Hour12 | Numeric::Minute | Numeric::Second | Numeric::Timestamp, _)
                    | Item::Fixed(Fixed::RFC2822 | Fixed::RFC3339))
            });
        let mut signature = format!("{:?}|{}|{:?}|{:?}|{}|{:?}", self.sources, self.format,
            self.complement, self.timezone_offset_minutes, self.clock_adjustment_ms, rules);
        if fallback_date { signature.push_str(&format!("|reference-date:{}", self.reference_date)); }
        signature
    }
}

impl TsConfig {
    pub fn compile(&self) -> Result<CompiledTsConfig, String> {
        if self
            .timezone_offset_minutes
            .is_some_and(|v| !(-840..=840).contains(&v))
        {
            return Err("Fuso inválido.".into());
        }
        let raw: Vec<TsRule> = if self.rules.is_empty() {
            vec![TsRule {
                regex: self.regex.clone(),
                template: self.template.clone(),
            }]
        } else {
            self.rules.clone()
        };
        let mut rules = Vec::with_capacity(raw.len());
        for r in raw {
            let re = r
                .regex
                .as_ref()
                .map(|p| regex::Regex::new(p))
                .transpose()
                .map_err(|e| format!("Regex da config de data inválida: {e}"))?;
            rules.push((re, r.template));
        }
        Ok(CompiledTsConfig {
            reference_date: chrono::Utc::now().date_naive(),
            timezone_offset_minutes: self.timezone_offset_minutes,
            clock_adjustment_ms: self.clock_adjustment_ms,
            sources: self.sources.clone(),
            format: self.format.clone(),
            complement: self.complement.clone(),
            rules,
        })
    }
}

/// Uma regra de extração: regex + modelo + condição.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct DerivedRule {
    pub pattern: String,
    /// modelo do valor com grupos da regex ("$1 - $2"); vazio = 1º grupo
    #[serde(default)]
    pub template: Option<String>,
    /// condição: a regra só vale quando o filtro é atendido
    #[serde(default)]
    pub filter: Option<crate::query::Filter>,
}

/// Campo derivado: regras tentadas em ordem (OU) — a primeira que extrai
/// um valor não vazio vence, pois linhas diferentes podem ter formatos diferentes.
#[derive(Clone, Debug, serde::Serialize)]
pub struct DerivedField {
    pub name: String,
    pub source: String,
    pub rules: Vec<DerivedRule>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<crate::field_transform::Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup: Option<crate::reference_lookup::Definition>,
}

/// Formato de leitura tolerante ao legado (regra única na raiz do objeto).
#[derive(serde::Deserialize)]
pub struct DerivedFieldCompat {
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub filter: Option<crate::query::Filter>,
    #[serde(default)]
    pub rules: Vec<DerivedRule>,
    #[serde(default)]
    pub steps: Vec<crate::field_transform::Step>,
    #[serde(default)]
    pub lookup: Option<crate::reference_lookup::Definition>,
}

impl DerivedFieldCompat {
    pub fn normalize(self) -> DerivedField {
        let mut rules = self.rules;
        if rules.is_empty() && !self.pattern.is_empty() {
            rules.push(DerivedRule {
                pattern: self.pattern,
                template: self.template,
                filter: self.filter,
            });
        }
        DerivedField {
            name: self.name,
            source: self.source,
            rules,
            steps: self.steps,
            lookup: self.lookup,
        }
    }
}

#[derive(Clone)]
pub struct CompiledRule {
    pub re: regex::Regex,
    pub template: Option<String>,
    pub filter: Option<crate::query::Filter>,
    /// Frozen condition, including any Case-local detection/threat catalog.
    /// Hydration must never re-resolve a raw filter on a Rayon/background thread.
    prepared_filter: Option<std::sync::Arc<crate::query::PreparedFilter>>,
    pub(crate) filter_security_signature: Option<String>,
}
impl CompiledRule {
    pub(crate) fn new(re: regex::Regex, template: Option<String>, filter: Option<crate::query::Filter>) -> Result<Self, String> {
        let (prepared_filter, filter_security_signature) = match &filter {
            Some(filter) => {
                crate::operations::check()?;
                crate::workspace::validate(std::slice::from_ref(filter))?;
                let prepared = crate::query::prepare(std::slice::from_ref(filter)).pop().ok_or("Condição derivada não compilada.")?;
                crate::operations::check()?;
                (Some(std::sync::Arc::new(prepared)), Some(crate::detections::fingerprint()))
            }
            None => (None, None),
        };
        Ok(Self { re, template, filter, prepared_filter, filter_security_signature })
    }
    pub(crate) fn filter_reads_id(&self) -> bool {
        let Some(filter) = &self.filter else { return false; };
        // Catalog detections may themselves read id. Keep such fields on the
        // authoritative event path rather than bake segment-relative ids.
        if filter.column == "id" || filter.op == "detection" { return true; }
        if filter.op != "query" { return false; }
        let Some(expr) = self.prepared_filter.as_ref().and_then(|prepared| prepared.expr.as_ref()) else { return true; };
        let mut names = Vec::new(); expr.field_names(&mut names);
        names.iter().any(|name| matches!(name.as_str(), "id" | "deteccao" | "detecção" | "detection"))
    }
}

#[derive(Clone)]
pub struct CompiledDerived {
    pub name: String,
    pub source: String,
    pub rules: Vec<CompiledRule>,
    pub steps: Vec<crate::field_transform::Step>,
    pub(crate) lookup: Option<crate::reference_lookup::Compiled>,
}

/// Aplica os campos derivados a um evento. As regras de cada campo são
/// tentadas em ordem (OU): a primeira que extrai valor não vazio vence.
pub fn apply_derived(ev: &mut Event, derived: &[CompiledDerived]) {
    const BUDGET: usize = 2 << 20;
    for (name, original) in std::mem::take(&mut ev.derived_originals) {
        match original {
            crate::model::DerivedOriginal::Missing => { ev.fields.remove(&name); }
            crate::model::DerivedOriginal::Present(value) => { ev.fields.insert(name, value); }
        }
    }
    ev.derived_diagnostics.clear();
    let mut remaining = BUDGET;
    fn diagnostic(ev: &mut Event, field: &str, code: &str, message: String, warning: bool) {
        if ev.derived_diagnostics.len() < 31 {
            ev.derived_diagnostics.push(crate::model::DerivedDiagnostic {
                field: field.chars().take(128).collect(), code: code.into(), message, warning,
            });
        } else if ev.derived_diagnostics.len() == 31 {
            ev.derived_diagnostics.push(crate::model::DerivedDiagnostic {
                field: String::new(), code: "diagnostic_limit".into(), message: "Outros diagnósticos de campos foram omitidos neste registro.".into(), warning: true,
            });
        }
    }
    for d in derived {
        if let Some(lookup) = &d.lookup {
            if !crate::field_transform::valid_typed_target(&d.name) {
                diagnostic(ev, &d.name, "reserved_target", crate::field_transform::TARGET_NAME_ERROR.into(), false);
                continue;
            }
            let value = match lookup.evaluate(ev) {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(error) => { diagnostic(ev, &d.name, "lookup_error", error, false); continue; }
            };
            let limits = crate::field_transform::Limits { output_bytes: (1 << 20).min(remaining), ..Default::default() };
            let fields = match crate::field_transform::expanded_fields(&d.name, &value, limits, 512) {
                Ok(fields) => fields,
                Err(error) => { diagnostic(ev, &d.name, "lookup_error", error.to_string(), false); continue; }
            };
            if fields.keys().any(|name| ev.fields.contains_key(name)) {
                diagnostic(ev, &d.name, "target_conflict", "O nome do campo consultado ou de um subcampo já existe no registro original.".into(), false);
                continue;
            }
            let bytes = fields.iter().try_fold(0usize, |size, (key, value)| {
                Ok::<_, crate::field_transform::Error>(size.saturating_add(key.len().saturating_mul(2)).saturating_add(crate::field_transform::payload_bytes(value, remaining)?).saturating_add(32))
            });
            let bytes = match bytes {
                Ok(bytes) if bytes <= remaining => bytes,
                _ => { diagnostic(ev, &d.name, "lookup_error", crate::field_transform::Error::OutputLimit.to_string(), false); continue; }
            };
            remaining -= bytes;
            for name in fields.keys() { ev.derived_originals.insert(name.clone(), crate::model::DerivedOriginal::Missing); }
            ev.fields.extend(fields);
            continue;
        }
        if !d.steps.is_empty() {
            if !crate::field_transform::valid_typed_target(&d.name) {
                diagnostic(ev, &d.name, "reserved_target", crate::field_transform::TARGET_NAME_ERROR.into(), false);
                continue;
            }
            let limits = crate::field_transform::Limits { output_bytes: (512 << 10).min(remaining), ..Default::default() };
            let canonical = matches!(d.source.as_str(), "id" | "event_ref" | "timestamp" | "source" | "level" | "code" | "name" | "description" | "message" | "raw");
            let source = match (!canonical).then(|| ev.fields.get(&d.source)).flatten() {
                Some(value) => std::borrow::Cow::Borrowed(value),
                None => match ev.col_ref(&d.source) {
                    Some(value) => {
                        if value.len() > limits.input_bytes {
                            diagnostic(ev, &d.name, "transform_error", crate::field_transform::Error::InputLimit.to_string(), false);
                            continue;
                        }
                        std::borrow::Cow::Owned(Value::String(value.into_owned()))
                    }
                    None => continue,
                },
            };
            // Validate before regex stringification or capture expansion. A
            // structured source can otherwise allocate before pipeline limits.
            let source = match crate::field_transform::transform(source.as_ref(), &[], limits) {
                Ok(output) => output.value,
                Err(error) => { diagnostic(ev, &d.name, "transform_error", error.to_string(), false); continue; }
            };
            let extracted;
            let input = if d.rules.is_empty() { &source } else {
                let value = source.as_str().map(std::borrow::Cow::Borrowed)
                    .unwrap_or_else(|| std::borrow::Cow::Owned(source.to_string()));
                let found = d.rules.iter().filter(|r| r.prepared_filter.as_ref().is_none_or(|f| crate::query::matches(ev, f))).find_map(|r| {
                    let captures = r.re.captures(&value)?;
                    let output = match &r.template {
                        Some(template) if !template.is_empty() => {
                            match crate::field_transform::expand_capture(&captures, template, limits.output_bytes) {
                                Ok(value) => value,
                                Err(error) => return Some(Err(error)),
                            }
                        }
                        _ => captures.get(1).or_else(|| captures.get(0)).map(|m| m.as_str().to_owned()).unwrap_or_default(),
                    };
                    (!output.is_empty()).then_some(Ok(Value::String(output)))
                });
                let Some(value) = found else { continue };
                extracted = match value {
                    Ok(value) => value,
                    Err(error) => { diagnostic(ev, &d.name, "transform_error", error.to_string(), false); continue; }
                }; &extracted
            };
            let result = crate::field_transform::transform(input, &d.steps, limits).and_then(|output| {
                let expanded = crate::field_transform::expanded_fields(&d.name, &output.value,
                    crate::field_transform::Limits { output_bytes: (1 << 20).min(remaining), ..limits }, 512)?;
                Ok((expanded, output.notices))
            });
            match result {
                Ok((fields, notices)) => {
                    // A new transform creates another field; a raw same-named
                    // field or child must never be silently overwritten.
                    if fields.keys().any(|name| ev.fields.contains_key(name)) {
                        diagnostic(ev, &d.name, "target_conflict", "O nome do campo transformado ou de um subcampo já existe no registro original.".into(), false);
                        continue;
                    }
                    let bytes = fields.iter().try_fold(0usize, |size, (key, value)| {
                        Ok::<_, crate::field_transform::Error>(size.saturating_add(key.len().saturating_mul(2)).saturating_add(crate::field_transform::payload_bytes(value, remaining)?).saturating_add(32))
                    });
                    let bytes = match bytes {
                        Ok(bytes) if bytes <= remaining => bytes,
                        _ => { diagnostic(ev, &d.name, "transform_error", crate::field_transform::Error::OutputLimit.to_string(), false); continue; }
                    };
                    remaining -= bytes;
                    for name in fields.keys() { ev.derived_originals.insert(name.clone(), crate::model::DerivedOriginal::Missing); }
                    ev.fields.extend(fields);
                    for notice in notices { diagnostic(ev, &d.name, notice, "Payload JWT decodificado. A assinatura e as declarações não foram verificadas.".into(), true); }
                }
                Err(error) => diagnostic(ev, &d.name, "transform_error", error.to_string(), false),
            }
            continue;
        }
        let Some(val) = ev.col_ref(&d.source) else {
            continue;
        }; // owned: liberado para inserir o campo
        for r in &d.rules {
            if let Some(f) = &r.prepared_filter {
                if !crate::query::matches(ev, f) {
                    continue; // condição não atendida: tenta a próxima regra
                }
            }
            if let Some(cap) = r.re.captures(&val) {
                let extracted = match &r.template {
                    Some(t) if !t.is_empty() => {
                        match crate::field_transform::expand_capture(&cap, t, remaining) {
                            Ok(value) => value,
                            Err(error) => { diagnostic(ev, &d.name, "transform_error", error.to_string(), false); break; }
                        }
                    }
                    _ => {
                        let value = cap.get(1).or_else(|| cap.get(0));
                        if value.is_some_and(|m| m.len() > remaining) {
                            diagnostic(ev, &d.name, "transform_error", crate::field_transform::Error::OutputLimit.to_string(), false);
                            break;
                        }
                        value.map(|m| m.as_str().to_owned()).unwrap_or_default()
                    },
                };
                if !extracted.is_empty() {
                    let previous = if ev.derived_originals.contains_key(&d.name) { None } else { ev.fields.get(&d.name) };
                    let bytes = previous.map(|value| crate::field_transform::payload_bytes(value, remaining)).transpose()
                        .map(|n| n.unwrap_or(0).saturating_add(extracted.len()).saturating_add(d.name.len().saturating_mul(2)).saturating_add(32));
                    let bytes = match bytes {
                        Ok(bytes) if bytes <= remaining => bytes,
                        _ => { diagnostic(ev, &d.name, "transform_error", crate::field_transform::Error::OutputLimit.to_string(), false); break; }
                    };
                    let original = previous.cloned().map(crate::model::DerivedOriginal::Present).unwrap_or(crate::model::DerivedOriginal::Missing);
                    ev.derived_originals.entry(d.name.clone()).or_insert(original);
                    remaining -= bytes;
                    ev.fields.insert(d.name.clone(), Value::from(extracted));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod derived_transform_tests {
    use super::*;
    use crate::field_transform::Step;
    use serde_json::json;
    fn definition(name: &str, source: &str, steps: Vec<Step>) -> CompiledDerived {
        CompiledDerived { name: name.into(), source: source.into(), rules: Vec::new(), steps, lookup: None }
    }
    #[test]
    fn transformed_parent_and_children_keep_types_and_query_visibility() {
        let mut event = Event::empty();
        event.fields.insert("encoded".into(), json!("eyJjb2RlIjo1MDMsIm9rIjpmYWxzZSwiZW1wdHkiOiIifQ=="));
        let original = event.fields.clone();
        apply_derived(&mut event, &[definition("decoded", "encoded", vec![Step::Base64Decode, Step::ParseJson])]);
        assert_eq!(event.fields["encoded"], original["encoded"]);
        assert_eq!(event.fields["decoded.code"], 503);
        assert_eq!(event.fields["decoded.ok"], false);
        assert_eq!(event.fields["decoded.empty"], "");
        assert!(event.fields["decoded"].is_object());
        let filter: crate::query::Filter = serde_json::from_value(json!({"column":"decoded.code","op":"gte","value":"500"})).unwrap();
        assert!(crate::query::matches_filter(&event, &filter));
        assert!(event.derived_diagnostics.is_empty());
    }
    #[test]
    fn transform_failure_keeps_original_and_other_fields_queryable() {
        let mut event = Event::empty();
        event.message = "visible original".into();
        event.fields.insert("token".into(), json!("invalid-secret"));
        event.fields.insert("path".into(), json!("%2Fapi"));
        apply_derived(&mut event, &[
            definition("decoded", "token", vec![Step::Base64Decode]),
            definition("url", "path", vec![Step::UrlDecode]),
        ]);
        assert_eq!(event.message, "visible original");
        assert_eq!(event.fields["token"], "invalid-secret");
        assert!(!event.fields.contains_key("decoded"));
        assert_eq!(event.fields["url"], "/api");
        assert_eq!(event.derived_diagnostics.len(), 1);
        assert!(!event.derived_diagnostics[0].message.contains("secret"));
    }
    #[test]
    fn transform_children_never_overwrite_an_original_field() {
        let mut event = Event::empty();
        event.fields.insert("json".into(), json!("{\"code\":503}"));
        event.fields.insert("decoded.code".into(), json!(200));
        apply_derived(&mut event, &[definition("decoded", "json", vec![Step::ParseJson])]);
        assert_eq!(event.fields["decoded.code"], 200);
        assert!(!event.fields.contains_key("decoded"), "collision rejects the entire field, not just one child");
        assert_eq!(event.derived_diagnostics[0].code, "target_conflict");
    }
    #[test]
    fn regex_then_transform_preserves_existing_extraction_order() {
        let mut event = Event::empty(); event.message = "value=%2Fapi".into();
        let mut field = definition("decoded", "message", vec![Step::UrlDecode]);
        field.rules.push(CompiledRule::new(regex::Regex::new("value=(.*)").unwrap(), None, None).unwrap());
        apply_derived(&mut event, &[field]);
        assert_eq!(event.fields["decoded"], "/api");
    }
    #[test]
    fn transform_limits_and_diagnostics_are_bounded_per_record() {
        let mut event = Event::empty(); event.message = "x".repeat((256 << 10) + 1);
        let fields = (0..64).map(|n| definition(&format!("decoded{n}"), "message", vec![Step::Base64Decode])).collect::<Vec<_>>();
        apply_derived(&mut event, &fields);
        assert!(event.fields.is_empty());
        assert_eq!(event.derived_diagnostics.len(), 32);
        assert_eq!(event.derived_diagnostics.last().unwrap().code, "diagnostic_limit");
        assert_eq!(event.message.len(), (256 << 10) + 1);
    }
    #[test]
    fn transform_source_uses_canonical_columns_before_raw_aliases() {
        let mut event = Event::empty(); event.level = "Erro".into(); event.id = 42;
        event.fields.insert("level".into(), json!("ERROR"));
        event.fields.insert("id".into(), json!("raw-id"));
        event.fields.insert("timestamp".into(), json!("raw-time"));
        apply_derived(&mut event, &[
            definition("level_copy", "level", vec![Step::Base64Encode, Step::Base64Decode]),
            definition("id_copy", "id", vec![Step::Base64Encode, Step::Base64Decode]),
            definition("time_copy", "timestamp", vec![Step::Base64Encode, Step::Base64Decode]),
        ]);
        assert_eq!(event.fields["level_copy"], "Erro");
        assert_eq!(event.fields["id_copy"], "42");
        assert!(!event.fields.contains_key("time_copy"));
        apply_derived(&mut event, &[definition("LEVEL", "id", vec![Step::Base64Encode])]);
        assert!(!event.fields.contains_key("LEVEL"));
        assert_eq!(event.derived_diagnostics[0].code, "reserved_target");
    }
    #[test]
    fn case_overlay_recomputes_from_provenance_and_empty_config_restores_original() {
        let mut original = Event::empty(); original.fields.insert("input".into(), json!("%2Fapi"));
        original.fields.insert("native_null".into(), Value::Null);
        let first = definition("decoded", "input", vec![Step::UrlDecode]);
        let mut captured = original.clone(); apply_derived(&mut captured, &[first.clone()]);
        let serialized = serde_json::to_string(&captured).unwrap();
        let evidence: Event = serde_json::from_str(&serialized).unwrap();
        let mut overlay = evidence.clone(); apply_derived(&mut overlay, &[first]);
        assert_eq!(overlay.fields["decoded"], "/api"); assert!(overlay.derived_diagnostics.is_empty());
        let mut second = definition("native_null", "input", Vec::new());
        second.rules.push(CompiledRule::new(regex::Regex::new("(.*)").unwrap(), Some("replacement".into()), None).unwrap());
        apply_derived(&mut overlay, &[second]);
        assert!(!overlay.fields.contains_key("decoded"));
        assert_eq!(overlay.fields["native_null"], "replacement");
        let mut restored: Event = serde_json::from_str(&serde_json::to_string(&overlay).unwrap()).unwrap();
        apply_derived(&mut restored, &[]);
        assert_eq!(restored.fields, original.fields);
        assert!(restored.derived_originals.is_empty());
        assert_eq!(serde_json::to_string(&evidence).unwrap(), serialized, "saved evidence was never changed");
        let mut legacy = original.clone(); legacy.fields.insert("historical_unmarked".into(), json!("keep"));
        apply_derived(&mut legacy, &[]);
        assert_eq!(legacy.fields["historical_unmarked"], "keep", "never guess that an unmarked captured field was generated");
    }
}

/// Como interpretar um formato customizado.
#[derive(Clone)]
pub enum CustomParse {
    Regex(regex::Regex),
    Delimited { sep: char, fields: Vec<String> },
}

/// Calendar-sensitive parsers infer their year once per source load. Local
/// timezone configuration remains part of the cache identity; the mapped
/// source and system timezone must stay stable while a load is in progress.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct ParserCalendar {
    pub year: i32,
    pub timezone: String,
}
impl ParserCalendar {
    pub(crate) fn current() -> Result<Self, String> {
        Ok(Self { year: chrono::Utc::now().year(), timezone: Self::timezone_signature()? })
    }
    fn timezone_signature() -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"timezone-configuration-v1");
        hash.update(chrono::Local::now().offset().to_string());
        let tz = std::env::var_os("TZ").unwrap_or_default();
        hash.update(tz.to_string_lossy().as_bytes());
        #[cfg(unix)]
        {
            use std::io::Read;
            let mut paths = vec![std::path::PathBuf::from("/etc/localtime")];
            let name = tz.to_string_lossy();
            let name = name.trim_start_matches(':');
            let tzdir = std::env::var_os("TZDIR");
            if let Some(dir) = &tzdir { hash.update(dir.to_string_lossy().as_bytes()); }
            if !name.is_empty() {
                if std::path::Path::new(name).is_absolute() { paths.push(name.into()); }
                else {
                    if let Some(dir) = tzdir { paths.push(std::path::PathBuf::from(dir).join(name)); }
                    for dir in ["/usr/share/zoneinfo", "/share/zoneinfo", "/etc/zoneinfo"] { paths.push(std::path::Path::new(dir).join(name)); }
                }
            }
            for path in paths {
                if let Ok(bytes) = std::fs::read(&path) {
                    hash.update(path.to_string_lossy().as_bytes()); hash.update(bytes);
                }
            }
            // The relevant TZif bytes are hashed above. Also include the tzdb
            // version header when the distribution makes it available.
            if let Ok(file) = std::fs::File::open("/usr/share/zoneinfo/tzdata.zi") {
                let mut version = Vec::new(); file.take(1024).read_to_end(&mut version).map_err(|e| e.to_string())?;
                hash.update(version.split(|&b| b == b'\n').next().unwrap_or_default());
            }
        }
        #[cfg(windows)]
        {
            use windows::Win32::System::Time::{GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION, TIME_ZONE_ID_INVALID};
            let mut zone = DYNAMIC_TIME_ZONE_INFORMATION::default();
            if unsafe { GetDynamicTimeZoneInformation(&mut zone) } == TIME_ZONE_ID_INVALID {
                return Err("Não foi possível identificar a configuração de fuso horário do Windows.".into());
            }
            // Includes dynamic zone key, enabled/disabled DST and current rules.
            // This is not a hash of the entire historical Windows rule database.
            hash.update(format!("{zone:?}"));
        }
        Ok(format!("{:x}", hash.finalize()))
    }
    pub(crate) fn validate_timezone(&self) -> Result<(), String> {
        if self.timezone != Self::timezone_signature()? {
            return Err("O fuso horário mudou durante a indexação. Reabra a fonte para preservar horários consistentes.".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct FilePart {
    pub path: String,
    /// Actual immutable input backing the mapping (display paths may be virtual).
    pub physical_path: String,
    pub physical_file_id: Option<(u64, u64)>,
    pub(crate) calendar: ParserCalendar,
    pub(crate) event_identity: Option<String>,
    /// Complete raw source/parser/calendar identity, fixed before publication.
    pub(crate) metadata_identity: String,
    /// Protects a canonical expanded file from another process's pruning.
    pub(crate) canonical_lease: Option<std::sync::Arc<std::fs::File>>,
    pub file_name: String,
    pub format: String,
    pub custom: Option<CustomParse>,
    pub ts_config: Option<CompiledTsConfig>,
    pub header: Vec<String>,
    /// Shared so the query engine can read lines while it builds its store.
    pub mmap: std::sync::Arc<memmap2::Mmap>,
    pub base: u64,
    pub identity: String,
}

pub struct FileIndex {
    pub parts: Vec<FilePart>,
    pub lines: std::sync::Arc<crate::metadata_store::LineStore>,
    pub columns: Vec<String>,
    pub time_order: std::sync::Arc<std::sync::OnceLock<Vec<usize>>>,
}

// Single-source configuration remains available to the existing format editor.
impl std::ops::Deref for FileIndex {
    type Target = FilePart;
    fn deref(&self) -> &FilePart {
        &self.parts[0]
    }
}
impl std::ops::DerefMut for FileIndex {
    fn deref_mut(&mut self) -> &mut FilePart {
        &mut self.parts[0]
    }
}
impl FileIndex {
    pub fn part_at(&self, i: usize) -> &FilePart {
        let offset = self.lines.at(i).offset;
        &self.parts[self
            .parts
            .partition_point(|p| p.base <= offset)
            .saturating_sub(1)]
    }
    pub fn append(&mut self, mut other: FileIndex) -> Result<(), String> {
        let base = self.parts.last().map(|p| p.base.checked_add(p.mmap.len() as u64).and_then(|n| n.checked_add(1)).ok_or("Posição de fonte excedida."))
            .transpose()?.unwrap_or(0);
        let bases = other.parts.iter().map(|p| {
            let relocated = p.base.checked_add(base).ok_or("Posição de fonte excedida.")?;
            relocated.checked_add(p.mmap.len() as u64).ok_or("Tamanho de fonte excedido.")?;
            Ok(relocated)
        }).collect::<Result<Vec<_>, &str>>()?;
        // Build cheap immutable views before changing either source. This also
        // checks all row/offset arithmetic before a staged import can publish.
        let mut lines = (*self.lines).clone();
        lines.append_relocated(&other.lines, base)?;
        for (part, relocated) in other.parts.iter_mut().zip(bases) { part.base = relocated; }
        self.parts.append(&mut other.parts);
        self.lines = std::sync::Arc::new(lines);
        for c in other.columns {
            if !self.columns.contains(&c) { self.columns.push(c); }
        }
        self.time_order = std::sync::Arc::new(std::sync::OnceLock::new());
        Ok(())
    }
    pub fn bytes_len(&self) -> u64 {
        self.parts.iter().map(|p| p.mmap.len() as u64).sum()
    }
    pub fn ordered(&self) -> &[usize] {
        self.time_order.get_or_init(|| {
            let mut order: Vec<usize> = (0..self.lines.len()).collect();
            order.sort_unstable_by_key(|&i| (self.lines.at(i).ts, i));
            order
        })
    }
}

/// Identity of the opened file object, distinct from a reusable path. This
/// catches atomic replacement even if byte length and modification time match.
#[cfg(unix)]
pub(crate) fn file_identity(file: &std::fs::File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().ok()?; Some((m.dev(), m.ino()))
}
#[cfg(windows)]
pub(crate) fn file_identity(file: &std::fs::File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    Some((u64::from(info.dwVolumeSerialNumber), (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
}
#[cfg(not(any(unix, windows)))]
pub(crate) fn file_identity(_file: &std::fs::File) -> Option<(u64, u64)> { None }

pub(crate) fn validate_source(part: &FilePart) -> Result<(), String> {
    crate::operations::check()?;
    let file = crate::case_archive_format::open_regular(std::path::Path::new(&part.physical_path)).map_err(|e| format!("Fonte indisponível: {e}"))?;
    let metadata = file.metadata().map_err(|e| format!("Fonte indisponível: {e}"))?;
    part.calendar.validate_timezone()?;
    crate::workspace::validate_canonical_origin(std::path::Path::new(&part.physical_path))?;
    // Reject changes observed before touching the mapping. This is not an
    // atomic snapshot: external writers must not modify/truncate the backing
    // file during use, including between this guard and subsequent mmap reads.
    if metadata.len() != part.mmap.len() as u64
        || part.physical_file_id.is_some_and(|id| file_identity(&file) != Some(id))
        || crate::index_cache::identity(&part.physical_path, &part.mmap) != part.identity {
        return Err("A fonte foi alterada durante a preparação. Reabra o arquivo para indexar uma versão consistente; checkpoints anteriores permanecem separados.".into());
    }
    Ok(())
}

pub fn line_bytes(idx: &FileIndex, i: usize) -> &[u8] {
    let m = &idx.lines.at(i);
    let part = idx.part_at(i);
    let start = (m.offset - part.base) as usize;
    let end = (start + m.len as usize).min(part.mmap.len());
    &part.mmap[start..end]
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
fn parse_with_format(s: &str, fmt: &str, complement: Option<&str>) -> Option<i64> {
    parse_with_format_at(s, fmt, complement, chrono::Utc::now().date_naive())
}
fn parse_with_format_at(s: &str, fmt: &str, complement: Option<&str>, reference_date: chrono::NaiveDate) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    match fmt {
        "epoch_ms" => return s.parse::<i64>().ok(),
        "epoch_s" => return s.parse::<i64>().ok().map(|v| v * 1000),
        _ => {}
    }
    // chrono não aceita vírgula na fração de segundos: tenta também com ponto
    let alt;
    let candidates: &[&str] = if s.contains(',') {
        alt = s.replacen(',', ".", 1);
        &[s, &alt]
    } else {
        &[s]
    };
    for cand in candidates {
        if let Ok(dt) = chrono::DateTime::parse_from_str(cand, fmt) {
            return Some(dt.timestamp_millis());
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(cand, fmt) {
            return Some(naive_to_ms(dt));
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(cand, fmt) {
            return d.and_hms_opt(0, 0, 0).map(naive_to_ms);
        }
        if let Ok(t) = chrono::NaiveTime::parse_from_str(cand, fmt) {
            let date = complement
                .and_then(|c| chrono::NaiveDate::parse_from_str(c.trim(), "%Y-%m-%d").ok())
                .unwrap_or(reference_date);
            return Some(naive_to_ms(date.and_time(t)));
        }
    }
    None
}

/// Monta o candidato de uma regra (regex + template) sobre o texto concatenado.
fn ts_candidate(joined: &str, re: &regex::Regex, tpl: Option<&String>) -> Option<String> {
    let c = re.captures(joined)?;
    if let Some(tpl) = tpl {
        // monta a partir dos grupos: \1..\9 ou $1..$9
        let mut out = tpl.clone();
        for gi in 1..c.len() {
            let g = c.get(gi).map(|m| m.as_str()).unwrap_or("");
            out = out
                .replace(&format!("\\{gi}"), g)
                .replace(&format!("${gi}"), g);
        }
        Some(out)
    } else if c.len() >= 3 {
        Some(format!("{} {}", &c[1], &c[2]))
    } else {
        Some(
            c.get(1)
                .or_else(|| c.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
        )
    }
}

/// Extrai o timestamp de um texto já concatenado, tentando as regras em ordem (OU):
/// a primeira que casar E produzir um timestamp válido vence.
fn ts_from_joined(joined: &str, cc: &CompiledTsConfig) -> Option<i64> {
    if cc.sources.is_empty() || cc.format.is_empty() {
        return None;
    }
    for (re, tpl) in &cc.rules {
        let candidate = match re {
            Some(r) => match ts_candidate(joined, r, tpl.as_ref()) {
                Some(c) => c,
                None => continue, // regra não casou: próxima
            },
            None => joined.trim().to_string(),
        };
        if let Some(mut ts) = parse_with_format_at(&candidate, &cc.format, cc.complement.as_deref(), cc.reference_date) {
            if let Some(minutes) = cc.timezone_offset_minutes {
                if !cc.format.contains("%z")
                    && !cc.format.contains("%:z")
                    && !cc.format.starts_with("epoch")
                {
                    use chrono::TimeZone;
                    let local_offset = chrono::Local
                        .timestamp_millis_opt(ts)
                        .single()
                        .map(|d| d.offset().local_minus_utc())
                        .unwrap_or(0);
                    ts += (local_offset as i64 - minutes as i64 * 60) * 1000;
                }
            }
            return ts.checked_add(cc.clock_adjustment_ms);
        }
        // parse falhou: próxima regra
    }
    None
}

/// Aplica a TsConfig usando apenas os dados do próprio evento (fonte em memória,
/// ex.: arquivos unidos ou Event Log). "linha" = raw; "arquivo"/"caminho" vêm
/// dos campos injetados na materialização.
pub fn apply_ts_config_event(ev: &mut Event, cc: &CompiledTsConfig) {
    if cc.sources.is_empty() {
        return;
    }
    let joined = cc
        .sources
        .iter()
        .map(|s| match s.as_str() {
            "linha" => ev.raw.clone(),
            other => ev.col_str(other).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(ts) = ts_from_joined(&joined, cc) {
        ev.timestamp = Some(ts);
    }
}

pub fn apply_ts_config(ev: &mut Event, cc: &CompiledTsConfig, idx: &FilePart, line: &str) {
    if cc.sources.is_empty() {
        return;
    }
    let joined = cc
        .sources
        .iter()
        .map(|s| match s.as_str() {
            "arquivo" => idx.file_name.clone(),
            "caminho" => idx.path.clone(),
            "linha" => line.to_string(),
            other => ev.col_str(other).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(ts) = ts_from_joined(&joined, cc) {
        ev.timestamp = Some(ts);
    }
}

/// Recalcula o timestamp de todas as linhas do índice com a TsConfig atual.
/// Chamado ao aplicar/alterar a configuração de data/hora.
pub fn retimestamp_index(
    idx: &mut FileIndex,
    progress: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Result<(), String> {
    for part in &idx.parts { validate_source(part)?; }
    let total = idx.lines.len();
    let mut timestamps = crate::metadata_store::TimestampWriter::new()?;
    let mut completed = 0;
    let cancellation = crate::operations::current_token();
    let wave = crate::resources::workers() * 8192;
    let index: &FileIndex = idx;
    while completed < total {
        crate::operations::check()?;
        if let Some(cb) = progress {
            cb(completed, total);
        }
        let start = completed;
        let end = (start + wave).min(total);
        let parts = crate::global_scheduler::map((start..end).step_by(1024), |from| {
            (from..(from + 1024).min(end)).map(|i| {
                if cancellation.cancelled() {
                    return 0;
                }
                let part = index.part_at(i);
                let bytes = line_bytes(index, i);
                let mut ev = parse_part_line(part, bytes);
                if let Some(cc) = &part.ts_config {
                    apply_ts_config(&mut ev, cc, part, &String::from_utf8_lossy(bytes));
                }
                ev.timestamp.unwrap_or(0)
            }).collect::<Vec<_>>()
        });
        for timestamp in parts.into_iter().flatten() { timestamps.push(timestamp)?; }
        completed = end;
    }
    crate::operations::check()?;
    for part in &idx.parts { validate_source(part)?; }
    let timestamps = timestamps.finish()?;
    crate::operations::check()?;
    for part in &idx.parts { validate_source(part)?; }
    idx.lines = std::sync::Arc::new(idx.lines.with_timestamps(timestamps)?);
    idx.time_order = std::sync::Arc::new(std::sync::OnceLock::new());
    if let Some(cb) = progress {
        cb(total, total);
    }
    Ok(())
}

/// Materializa um evento completo a partir da linha indexada.
pub fn event_at(
    idx: &FileIndex,
    i: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Event {
    let part = idx.part_at(i);
    let bytes = line_bytes(idx, i);
    let mut ev = parse_part_line(part, bytes);
    ev.id = i;
    if ev.event_ref.is_empty() {
        ev.event_ref = format!("{}:{}", part.event_identity.as_deref().unwrap_or(&part.identity), idx.lines.at(i).offset - part.base);
    }
    ev.enrich(codes, system);
    ev.fields
        .insert("arquivo".into(), Value::from(part.file_name.clone()));
    ev.fields
        .insert("caminho".into(), Value::from(part.path.clone()));
    if let Some(cc) = &part.ts_config {
        apply_ts_config(&mut ev, cc, part, &String::from_utf8_lossy(bytes));
    }
    normalize_fields(&mut ev);
    apply_derived(&mut ev, derived);
    expand_query_param_fields(&mut ev);
    ev
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned())
}

pub fn extract_query_params(field_name: &str, raw_val: &str) -> Option<Vec<(String, String)>> {
    let s = raw_val.trim();
    if s.len() < 3 || !s.contains('=') {
        return None;
    }

    let qs = if let Some(pos) = s.find('?') {
        let after = &s[pos + 1..];
        after.split('#').next().unwrap_or(after)
    } else {
        let lower_field = field_name.to_ascii_lowercase();
        let field_suggests_params = lower_field.contains("query")
            || lower_field.contains("param")
            || lower_field.contains("qs")
            || lower_field.contains("search");
        if !s.contains('&') && !field_suggests_params {
            return None;
        }
        s
    };

    if qs.is_empty() || !qs.contains('=') {
        return None;
    }

    let parts: Vec<&str> = qs.split('&').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }

    let lower_field = field_name.to_ascii_lowercase();
    if !s.contains('?') && !lower_field.contains("query") && !lower_field.contains("param") && parts.len() < 2 {
        return None;
    }

    let mut pairs = Vec::new();
    for part in parts {
        let (raw_k, raw_v) = part.split_once('=')?;
        let key = percent_decode(raw_k.trim());
        let val = percent_decode(raw_v.trim());

        if key.is_empty() || key.len() > 100 {
            return None;
        }
        if key.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || c == '='
                || c == '&'
                || c == '<'
                || c == '>'
                || c == '"'
                || c == '\''
        }) {
            return None;
        }
        pairs.push((key, val));
    }

    if pairs.is_empty() {
        None
    } else {
        Some(pairs)
    }
}

pub fn expand_query_param_fields(ev: &mut Event) {
    let mut additions: Vec<(String, String)> = Vec::new();

    for (k, v) in &ev.fields {
        if k.matches('.').count() >= 8 {
            continue;
        }
        if let serde_json::Value::String(s) = v {
            if let Some(params) = extract_query_params(k, s) {
                for (sub_k, sub_v) in params {
                    additions.push((format!("{k}.{sub_k}"), sub_v));
                }
            }
        }
    }

    for (sub_field, sub_val) in additions {
        match ev.fields.get_mut(&sub_field) {
            Some(serde_json::Value::String(existing)) => {
                if !existing.is_empty() && !existing.split(", ").any(|part| part == sub_val) {
                    existing.push_str(", ");
                    existing.push_str(&sub_val);
                }
            }
            Some(_) => {}
            None => {
                ev.fields.insert(sub_field, serde_json::Value::String(sub_val));
            }
        }
    }
}

pub fn normalize_fields(ev: &mut Event) {
    for (canonical, aliases) in [
        ("host", &["host.name", "hostname", "computer"][..]),
        ("service", &["service.name", "app", "application"][..]),
        (
            "client_ip",
            &["ip_cliente", "src", "c-ip", "remote_addr"][..],
        ),
        ("trace_id", &["trace.id", "traceId", "TraceId"][..]),
        (
            "request_id",
            &["requestId", "request.id", "correlation_id"][..],
        ),
    ] {
        if !ev.fields.contains_key(canonical) {
            if let Some(v) = aliases.iter().find_map(|key| ev.fields.get(*key)).cloned() {
                ev.fields.insert(canonical.into(), v);
            }
        }
    }
    expand_query_param_fields(ev);
}

pub fn parse_line(
    bytes: &[u8],
    format: &str,
    custom: Option<&CustomParse>,
    header: &[String],
) -> Event {
    parse_line_at(bytes, format, custom, header, chrono::Utc::now().year())
}

pub(crate) fn parse_part_line(part: &FilePart, bytes: &[u8]) -> Event {
    parse_line_at(bytes, &part.format, part.custom.as_ref(), &part.header, part.calendar.year)
}

pub(crate) fn parse_line_at(bytes: &[u8], format: &str, custom: Option<&CustomParse>, header: &[String], year: i32) -> Event {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    let mut ev = match format {
        "snapshot" => {
            serde_json::from_str::<Event>(&text).unwrap_or_else(|_| event_from_text(&text))
        }
        "jsonl" => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => event_from_json(map, &text),
            _ => event_from_text(&text),
        },
        "syslog3164" => parse_syslog3164(&text, year).unwrap_or_else(|| event_from_text(&text)),
        "syslog5424" => parse_syslog5424(&text).unwrap_or_else(|| event_from_text(&text)),
        "apache" => parse_apache(&text).unwrap_or_else(|| event_from_text(&text)),
        "nginx-error" => web_logs::parse_error(&text).unwrap_or_else(|| event_from_text(&text)),
        "nginx" | "auto" | "mixed" => parse_structured_line(&text).unwrap_or_else(|| event_from_text(&text)),
        "firewall" => parse_firewall(&text, year).unwrap_or_else(|| event_from_text(&text)),
        "wildfly" => parse_wildfly(&text)
            .or_else(|| parse_jboss(&text))
            .or_else(|| parse_log4j(&text))
            .unwrap_or_else(|| event_from_text(&text)),
        "cef" => parse_cef(&text).unwrap_or_else(|| event_from_text(&text)),
        "leef" => parse_leef(&text).unwrap_or_else(|| event_from_text(&text)),
        "log4j" => parse_log4j(&text)
            .or_else(|| parse_jboss(&text))
            .or_else(|| parse_wildfly(&text))
            .unwrap_or_else(|| event_from_text(&text)),
        "logfmt" => parse_logfmt(&text).unwrap_or_else(|| event_from_text(&text)),
        tabular if delimiter_of(tabular).is_some() => {
            parse_delimited(&text, header, delimiter_of(tabular).unwrap_or(','))
                .unwrap_or_else(|| event_from_text(&text))
        }
        "w3c" => parse_w3c(&text, header).unwrap_or_else(|| event_from_text(&text)),
        "zeek" => parse_zeek(&text, header).unwrap_or_else(|| event_from_text(&text)),
        "auditd" => parse_auditd(&text).unwrap_or_else(|| event_from_text(&text)),
        "custom" => match custom {
            Some(CustomParse::Regex(re)) => {
                parse_custom(&text, re).unwrap_or_else(|| event_from_text(&text))
            }
            Some(CustomParse::Delimited { sep, fields }) => {
                let vals: Vec<String> = text.split(*sep).map(|s| s.trim().to_string()).collect();
                event_from_columns(vals, fields, &text)
            }
            None => event_from_text(&text),
        },
        _ => event_from_text(&text),
    };
    if ev.parse_status == "text" && matches!(format, "apache" | "nginx-error" | "jsonl" | "logfmt") {
        if let Some(recognized) = parse_structured_line(&text) { ev = recognized; }
    }
    if ev.parse_status == "text" && format != "text" {
        ev.parse_status = "unparsed".into();
    }
    normalize_fields(&mut ev);
    ev
}

/// Extrai a fatia (início, fim) do valor de uma chave JSON de primeiro nível
/// via varredura de bytes, sem parse completo. Strings vêm sem aspas.
fn find_json_value(line: &[u8], keys: &[&str]) -> Option<(usize, usize)> {
    for key in keys {
        let needle = key.as_bytes();
        let mut from = 0usize;
        while let Some(rel) = memchr::memmem::find(&line[from..], needle) {
            let mut i = from + rel + needle.len();
            // a chave termina com aspas de fechamento: "chave":valor
            if i >= line.len() || line[i] != b'"' {
                from = from + rel + 1;
                continue;
            }
            i += 1;
            while i < line.len() && matches!(line[i], b' ' | b'\t') {
                i += 1;
            }
            if i >= line.len() || line[i] != b':' {
                from = from + rel + 1;
                continue;
            }
            i += 1;
            while i < line.len() && matches!(line[i], b' ' | b'\t') {
                i += 1;
            }
            if i >= line.len() {
                return None;
            }
            if line[i] == b'"' {
                let start = i + 1;
                let mut j = start;
                while j < line.len() {
                    if line[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if line[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                return Some((start, j.min(line.len())));
            }
            let start = i;
            let mut j = i;
            while j < line.len() && !matches!(line[j], b',' | b'}' | b' ' | b'\t') {
                j += 1;
            }
            return Some((start, j));
        }
    }
    None
}

fn text_level_class(line: &[u8]) -> u8 {
    let head = String::from_utf8_lossy(&line[..line.len().min(48)]).to_uppercase();
    for (word, class) in [
        ("CRITICAL", LV_CRIT),
        ("FATAL", LV_CRIT),
        ("ERROR", LV_ERR),
        ("WARN", LV_WARN),
        ("INFO", LV_INFO),
        ("DEBUG", LV_DEBUG),
        ("TRACE", LV_TRACE),
    ] {
        if head.contains(word) {
            return class;
        }
    }
    LV_INFO
}

fn meta_for_line(line: &[u8], offset: u64, part: &FilePart) -> Result<LineMeta, String> {
    let mut m = LineMeta { offset, len: u32::try_from(line.len()).map_err(|_| "Um registro excede 4 GB.")?, ..Default::default() };
    let ev = parse_part_line(part, line);
    m.ts = ev.timestamp.unwrap_or(0);
    m.level = label_class(&ev.level).unwrap_or(LV_OTHER);
    if part.format == "jsonl" {
        if let Some((a, b)) = find_json_value(line, CODE_KEYS) {
            if line.get(a..b) == Some(ev.code.as_bytes()) && b - a <= u16::MAX as usize {
                m.code_off = u32::try_from(a).map_err(|_| "Posição de código excede 4 GB.")?;
                m.code_len = (b - a) as u16;
            }
        }
    }
    Ok(m)
}

fn push_meta(lines: &mut Vec<LineMeta>, line: &[u8], offset: u64, part: &FilePart, start_re: Option<&regex::Regex>) -> Result<(), String> {
    if let Some(re) = start_re {
        if !re.is_match(std::str::from_utf8(line).unwrap_or("")) {
            if let Some(last) = lines.last_mut() {
                let end = offset.checked_add(line.len() as u64).ok_or("Posição de registro inválida.")?;
                last.len = u32::try_from(end - last.offset).map_err(|_| "Um registro multilinha excede 4 GB.")?;
                return Ok(());
            }
        }
    }
    lines.push(meta_for_line(line, offset, part)?);
    Ok(())
}

type CheckpointSink<'a> = dyn FnMut(&mut crate::metadata_store::LineBuilder, usize, bool, Option<&[String]>) -> Result<(), String> + 'a;

/// At most 4 MiB or 65,536 physical lines per worker, whichever comes first.
/// This bounds temporary metadata even for very short records. A single long
/// physical/logical record is still an indivisible parsing unit.
const INDEX_CHUNK: usize = 4 << 20;
const INDEX_CHUNK_LINES: usize = 65_536;
struct ChunkLines {
    lines: Vec<LineMeta>,
    leading: Option<(usize, usize)>,
}

fn next_line_bound(bytes: &[u8], from: usize, limits: &IndexLimits) -> Result<usize, String> {
    let target = from.saturating_add(limits.chunk_bytes);
    let mut line_count = 0usize;
    for (block, bytes) in bytes[from..].chunks(65_536).enumerate() {
        crate::operations::check()?;
        let base = from + block * 65_536;
        for nl in memchr::memchr_iter(b'\n', bytes) {
            line_count += 1;
            let end = base + nl + 1;
            if end >= target || line_count >= limits.chunk_lines { return Ok(end); }
        }
    }
    Ok(bytes.len())
}

fn index_lines(prepared: &PreparedIndex, lines: &mut crate::metadata_store::LineBuilder, mut cursor: usize, sink: &mut CheckpointSink<'_>, reporter: crate::metadata_checkpoint::Reporter<'_>) -> Result<(), String> {
    use crate::metadata_checkpoint::{report, Progress};
    let part = &prepared.part;
    let bytes: &[u8] = &part.mmap;
    let start_re = prepared.descriptor.start_pattern.as_deref().map(regex::Regex::new).transpose().map_err(|e| e.to_string())?;
    let comments = matches!(part.format.as_str(), "w3c" | "zeek");
    let keep = |line: &[u8], offset: usize, terminated: bool| {
        !line.is_empty() && (!terminated || (Some(offset) != prepared.descriptor.header_at && !(comments && line[0] == b'#')))
    };
    let cancellation = crate::operations::current_token();
    while cursor < bytes.len() {
        prepared.validate()?;
        crate::operations::check()?;
        let mut bounds = vec![cursor];
        for _ in 0..prepared.limits.wave_chunks {
            let from = *bounds.last().unwrap();
            if from == bytes.len() { break; }
            bounds.push(next_line_bound(bytes, from, &prepared.limits)?);
        }
        let chunk = |k: usize| -> Result<ChunkLines, String> {
            let (from, to) = (bounds[k], bounds[k + 1]);
            let mut out = ChunkLines { lines: Vec::new(), leading: None };
            let visit = |line: &[u8], offset: usize, terminated: bool, out: &mut ChunkLines| -> Result<(), String> {
                if !keep(line, offset, terminated) { return Ok(()); }
                if let Some(re) = start_re.as_ref() {
                    if from > 0 && out.lines.is_empty() && !re.is_match(std::str::from_utf8(line).unwrap_or("")) {
                        let end = offset + line.len();
                        out.leading = Some((out.leading.map_or(offset, |(start, _)| start), end));
                        return Ok(());
                    }
                }
                push_meta(&mut out.lines, line, offset as u64, part, start_re.as_ref())
            };
            let mut offset = from;
            for (n, nl) in memchr::memchr_iter(b'\n', &bytes[from..to]).enumerate() {
                if n % 4096 == 0 && cancellation.cancelled() { return Err("Operação cancelada.".into()); }
                let end = from + nl;
                let raw = &bytes[offset..end];
                visit(raw.strip_suffix(b"\r").unwrap_or(raw), offset, true, &mut out)?;
                offset = end + 1;
            }
            if offset < to { visit(&bytes[offset..to], offset, false, &mut out)?; }
            Ok(out)
        };
        report(reporter, Progress::new("metadata-scan", "Indexando metadados", cursor, bytes.len(), "bytes"));
        let parts: Result<Vec<ChunkLines>, String> = crate::global_scheduler::map(0..bounds.len() - 1, chunk).into_iter().collect();
        crate::operations::check()?;
        let previous_rows = lines.len();
        for chunk in parts? {
            if let Some((start, stop)) = chunk.leading {
                match lines.last_mut() {
                    Some(entry) => entry.len = u32::try_from(stop as u64 - entry.offset).map_err(|_| "Um registro multilinha excede 4 GB.")?,
                    None => {
                        let mut initial = Vec::new();
                        let mut offset = start;
                        for raw in bytes[start..stop].split(|&b| b == b'\n') {
                            let line = raw.strip_suffix(b"\r").unwrap_or(raw);
                            if keep(line, offset, true) { push_meta(&mut initial, line, offset as u64, part, start_re.as_ref())?; }
                            offset += raw.len() + 1;
                        }
                        lines.extend(initial)?;
                    }
                }
            }
            lines.extend(chunk.lines)?;
        }
        cursor = *bounds.last().unwrap();
        let parsed = prepared.parsed_rows.fetch_add(lines.len() - previous_rows, std::sync::atomic::Ordering::Relaxed) + lines.len() - previous_rows;
        let mut progress = Progress::new("metadata-scan", "Indexando metadados", cursor, bytes.len(), "bytes");
        progress.parsed_rows = parsed;
        report(reporter, progress);
        if cursor < bytes.len() { sink(lines, cursor, false, None)?; }
    }
    Ok(())
}

/// Scanner checkpoints only between complete objects, never inside a string,
/// escape, or nesting stack. The array itself is never collected in memory.
struct JsonScanner<'a> {
    bytes: &'a [u8],
    cursor: usize,
    need_separator: bool,
    after_comma: bool,
    envelope: bool,
    done: bool,
    stack: Vec<u8>,
    last_report: std::time::Instant,
}
impl<'a> JsonScanner<'a> {
    fn new(bytes: &'a [u8], start: usize, cursor: usize, envelope: bool) -> Self {
        Self { bytes, cursor, need_separator: cursor != start + 1, after_comma: false, envelope, done: false, stack: Vec::with_capacity(32), last_report: std::time::Instant::now() }
    }
    fn next(&mut self, reporter: crate::metadata_checkpoint::Reporter<'_>) -> Result<Option<(usize, usize)>, String> {
        use crate::metadata_checkpoint::{report, Progress};
        if self.done { return Ok(None); }
        let mut quoted = false;
        let mut escaped = false;
        let mut record_start = None;
        while self.cursor < self.bytes.len() {
            let i = self.cursor;
            let byte = self.bytes[i];
            self.cursor += 1;
            if i % 65_536 == 0 {
                crate::operations::check()?;
                if self.last_report.elapsed() >= std::time::Duration::from_millis(150) {
                    report(reporter, Progress::new("metadata-json-boundaries", "Localizando registros JSON", i, self.bytes.len(), "bytes"));
                    self.last_report = std::time::Instant::now();
                }
            }
            if quoted {
                if escaped { escaped = false; }
                else if byte == b'\\' { escaped = true; }
                else if byte == b'"' { quoted = false; }
                continue;
            }
            if let Some(begin) = record_start {
                match byte {
                    b'"' => quoted = true,
                    b'{' | b'[' => {
                        if self.stack.len() >= 128 { return Err("JSON excede 128 níveis de aninhamento.".into()); }
                        self.stack.push(byte);
                    }
                    b'}' | b']' => {
                        let opening = if byte == b'}' { b'{' } else { b'[' };
                        if self.stack.pop() != Some(opening) { return Err(format!("Estrutura JSON inválida no byte {i}.")); }
                        if self.stack.is_empty() {
                            if i - begin >= u32::MAX as usize { return Err("Um registro JSON excede 4 GB.".into()); }
                            self.need_separator = true;
                            return Ok(Some((begin, i + 1)));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if byte.is_ascii_whitespace() { continue; }
            match byte {
                b']' if !self.after_comma => {
                    if !self.envelope {
                        for suffix in self.bytes[self.cursor..].chunks(65_536) {
                            crate::operations::check()?;
                            if suffix.iter().any(|b| !b.is_ascii_whitespace()) { return Err("Há conteúdo após o fim do array JSON.".into()); }
                        }
                    }
                    self.done = true;
                    return Ok(None);
                }
                b',' if self.need_separator => { self.need_separator = false; self.after_comma = true; }
                b'{' if !self.need_separator => { record_start = Some(i); self.stack.push(byte); self.after_comma = false; }
                _ => return Err(format!("Esperado um objeto ou separador no array JSON (byte {i}).")),
            }
        }
        Err("Array JSON incompleto: falta fechar um objeto ou o array.".into())
    }
}

fn index_json_array(prepared: &PreparedIndex, start: usize, envelope: bool, cursor: usize, lines: &mut crate::metadata_store::LineBuilder, sink: &mut CheckpointSink<'_>, reporter: crate::metadata_checkpoint::Reporter<'_>) -> Result<(), String> {
    use crate::metadata_checkpoint::{report, Progress};
    let bytes: &[u8] = &prepared.part.mmap;
    let mut scanner = JsonScanner::new(bytes, start, cursor, envelope);
    let cancellation = crate::operations::current_token();
    let max_records = prepared.limits.json_records;
    while !scanner.done {
        prepared.validate()?;
        crate::operations::check()?;
        let wave_start = scanner.cursor;
        let mut records = Vec::with_capacity(max_records);
        while records.len() < max_records {
            let Some(record) = scanner.next(reporter)? else { break; };
            records.push(record);
            if scanner.cursor - wave_start >= prepared.limits.json_bytes { break; }
        }
        let metas: Result<Vec<LineMeta>, String> = crate::global_scheduler::map(&records, |&(begin, end)| {
            if cancellation.cancelled() { return Err("Operação cancelada.".into()); }
            meta_for_line(&bytes[begin..end], begin as u64, &prepared.part)
        }).into_iter().collect();
        crate::operations::check()?;
        lines.extend(metas?)?;
        let parsed = prepared.parsed_rows.fetch_add(records.len(), std::sync::atomic::Ordering::Relaxed) + records.len();
        let mut progress = Progress::new("metadata-scan", "Indexando metadados JSON", scanner.cursor, bytes.len(), "bytes");
        progress.parsed_rows = parsed;
        report(reporter, progress);
        if !scanner.done { sink(lines, scanner.cursor, false, None)?; }
    }
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
struct SourceStamp {
    canonical_path: String,
    bytes: u64,
    modified: Option<(u64, u32)>,
    file_id: Option<(u64, u64)>,
}
impl SourceStamp {
    fn read(file: &std::fs::File, path: &str) -> Result<Self, String> {
        let meta = file.metadata().map_err(|e| e.to_string())?;
        Ok(Self {
            canonical_path: std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path)).to_string_lossy().into_owned(),
            bytes: meta.len(),
            modified: meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|t| (t.as_secs(), t.subsec_nanos())),
            file_id: file_identity(file),
        })
    }
}

#[derive(serde::Serialize)]
struct IndexDescriptor {
    requested_format: String,
    format: String,
    custom: serde_json::Value,
    header: Vec<String>,
    header_at: Option<usize>,
    start_pattern: Option<String>,
    array: Option<(usize, bool)>,
}

/// Chunking affects work scheduling only, never the cache identity or parser
/// semantics. Integration tests use tiny waves to exercise every boundary.
pub(crate) struct IndexLimits {
    pub chunk_bytes: usize,
    pub chunk_lines: usize,
    pub wave_chunks: usize,
    pub json_records: usize,
    pub json_bytes: usize,
}
impl Default for IndexLimits {
    fn default() -> Self {
        let workers = crate::resources::workers();
        Self { chunk_bytes: INDEX_CHUNK, chunk_lines: INDEX_CHUNK_LINES, wave_chunks: workers * 2,
            json_records: workers * 4 * 1024, json_bytes: INDEX_CHUNK * workers }
    }
}

pub(crate) struct PreparedIndex {
    pub(crate) limits: IndexLimits,
    pub(crate) parsed_rows: std::sync::atomic::AtomicUsize,
    pub part: FilePart,
    descriptor: IndexDescriptor,
    stamp: SourceStamp,
}
impl PreparedIndex {
    pub(crate) fn key(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(crate::index_cache::INDEX_DIR, crate::metadata_checkpoint::VERSION, &self.stamp, &self.part.identity, &self.descriptor, &self.part.calendar)).map_err(|e| e.to_string())?;
        // Keep unaffected formats byte-for-byte compatible with prior keys.
        let mut hash = Sha256::new();
        hash.update(bytes);
        if let Some(revision) = parser_semantics_signature(&self.part.format) {
            hash.update(b"|structured-parser:");
            hash.update(revision.as_bytes());
        }
        Ok(format!("{:x}", hash.finalize()))
    }
    pub(crate) fn initial_cursor(&self) -> usize { self.descriptor.array.map_or(0, |(start, _)| start + 1) }
    pub(crate) fn multiline(&self) -> bool { self.descriptor.start_pattern.is_some() }
    pub(crate) fn validate(&self) -> Result<(), String> {
        crate::operations::check()?;
        let file = crate::case_archive_format::open_regular(std::path::Path::new(&self.part.physical_path))?;
        if SourceStamp::read(&file, &self.part.physical_path)? != self.stamp {
            return Err("A fonte foi alterada durante a indexação. Reabra o arquivo para usar uma versão consistente.".into());
        }
        validate_source(&self.part)
    }
}

pub(crate) fn prepare_index(path: &str, format: &str, custom: Option<CustomParse>, saved_ts: Option<CompiledTsConfig>) -> Result<PreparedIndex, String> {
    crate::operations::check()?;
    let canonical_lease = crate::workspace::canonical_source_lease(std::path::Path::new(path))?;
    let event_identity = crate::workspace::canonical_event_identity(std::path::Path::new(path))?;
    let file = crate::case_archive_format::open_regular(std::path::Path::new(path)).map_err(|e| format!("Não foi possível abrir '{path}': {e}"))?;
    let stamp = SourceStamp::read(&file, path)?;
    if stamp.bytes == 0 { return Err("Arquivo vazio.".into()); }
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| format!("Falha ao mapear '{path}': {e}"))?;
    let fmt = if format == "auto" {
        // An inconclusive prefix must not disable recognition for later rows.
        // Explicitly requested plain text still keeps its literal semantics.
        match detect_format(&mmap) { "text" => "mixed", detected => detected }.to_string()
    }
        else if format == "custom" || format.starts_with("custom:") { "custom".to_string() }
        else if format == "csv" { detect_delimited(&mmap).unwrap_or("csv").to_string() }
        else { format.to_string() };
    let mut header: Vec<String> = Vec::new();
    if let Some(delimiter) = delimiter_of(&fmt) {
        let body = mmap.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&mmap);
        if let Some(first) = body.split(|&b| b == b'\n').find(|line| !line.trim_ascii().is_empty()) {
            let first = String::from_utf8_lossy(first).trim().to_string();
            header = split_delimited(&first, delimiter).iter().map(|s| s.trim().to_string()).collect();
        }
    } else if fmt == "zeek" {
        for line in mmap.split(|&b| b == b'\n').take(40) {
            let l = String::from_utf8_lossy(line);
            if let Some(rest) = l.trim_end().strip_prefix("#fields") {
                header = rest.split('\t').filter(|f| !f.is_empty()).map(|f| f.to_string()).collect(); break;
            }
        }
    } else if fmt == "w3c" {
        for line in mmap.split(|&b| b == b'\n').take(20) {
            let l = String::from_utf8_lossy(line);
            if let Some(rest) = l.trim().to_lowercase().strip_prefix("#fields:") {
                header = rest.split_whitespace().map(|s| s.to_string()).collect(); break;
            }
        }
    }
    let header_at = if delimiter_of(&fmt).is_some() {
        let mut offset = 0usize; let mut found = None;
        for line in mmap.split(|&b| b == b'\n') {
            if !line.trim_ascii().is_empty() { found = Some(offset); break; }
            offset += line.len() + 1;
        }
        found
    } else { None };
    let start_pattern = match fmt.as_str() { "log4j" | "wildfly" => detect_entry_start(&mmap).map(|re| re.as_str().to_string()), _ => None };
    let content_start = if mmap.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 };
    let first_byte = (content_start..mmap.len()).find(|&i| !mmap[i].is_ascii_whitespace());
    let array = first_byte.filter(|&i| fmt == "jsonl" && mmap[i] == b'[').map(|start| (start, false))
        .or_else(|| first_byte.filter(|&i| fmt == "jsonl" && mmap[i] == b'{' && !looks_like_jsonl(&mmap[i..]))
            .and_then(|_| json_envelope_array(&mmap)).map(|start| (start, true)));
    let custom_descriptor = match &custom {
        Some(CustomParse::Regex(re)) => serde_json::json!(["regex", re.as_str()]),
        Some(CustomParse::Delimited { sep, fields }) => serde_json::json!(["delimited", sep, fields]),
        None => serde_json::Value::Null,
    };
    let identity = crate::index_cache::identity(path, &mmap);
    let prepared = PreparedIndex {
        limits: IndexLimits::default(), parsed_rows: std::sync::atomic::AtomicUsize::new(0),
        descriptor: IndexDescriptor { requested_format: format.into(), format: fmt.clone(), custom: custom_descriptor, header: header.clone(), header_at, start_pattern, array },
        stamp,
        part: FilePart { path: path.into(), physical_path: path.into(), physical_file_id: file_identity(&file), calendar: ParserCalendar::current()?, canonical_lease, event_identity, metadata_identity: String::new(),
            file_name: file_name_of(path), format: fmt, custom, ts_config: saved_ts, header, mmap: std::sync::Arc::new(mmap), base: 0, identity },
    };
    prepared.validate()?;
    Ok(prepared)
}

pub(crate) fn index_prepared(prepared: &PreparedIndex, mut resume: crate::metadata_checkpoint::Resume, sink: &mut CheckpointSink<'_>, reporter: crate::metadata_checkpoint::Reporter<'_>) -> Result<FileIndex, String> {
    use crate::metadata_checkpoint::{report, Progress};
    prepared.validate()?;
    let bytes: &[u8] = &prepared.part.mmap;
    let safe_cursor = resume.cursor == bytes.len() || match prepared.descriptor.array {
        Some((start, _)) => resume.cursor == start + 1 || resume.cursor > start + 1 && bytes.get(resume.cursor - 1) == Some(&b'}'),
        None => resume.cursor == 0 || bytes.get(resume.cursor - 1) == Some(&b'\n'),
    };
    if !safe_cursor { return Err("Limite de retomada dos metadados inválido.".into()); }
    if !resume.scan_complete {
        match prepared.descriptor.array {
            Some((start, envelope)) => index_json_array(&prepared, start, envelope, resume.cursor, &mut resume.lines, sink, reporter)?,
            None => index_lines(&prepared, &mut resume.lines, resume.cursor, sink, reporter)?,
        }
        prepared.validate()?;
        resume.lines.flush()?;
        sink(&mut resume.lines, bytes.len(), true, None)?;
    }
    let mut columns = match resume.columns {
        Some(columns) => columns,
        None => {
            let mut columns: Vec<String> = STANDARD_COLUMNS.iter().map(|s| s.to_string()).collect();
            let mut extra = std::collections::HashSet::new();
            let count = resume.lines.len().min(4_000);
            let mut sampled = Vec::with_capacity(count);
            for sample in 0..count {
                if sample % 32 == 0 {
                    crate::operations::check()?;
                    report(reporter, Progress::new("metadata-columns", "Descobrindo colunas", sample, count, "amostras"));
                }
                let i = sample * resume.lines.len() / count;
                let m = resume.lines.at(i)?;
                let ev = parse_part_line(&prepared.part, &bytes[m.offset as usize..m.offset as usize + m.len as usize]);
                extra.extend(ev.fields.keys().cloned()); sampled.push(ev);
            }
            let mut extra: Vec<String> = extra.into_iter().filter(|name| !STANDARD_COLUMNS.contains(&name.as_str())).collect(); extra.sort(); columns.extend(extra);
            for column in crate::entities::observed_columns(sampled.iter()) {
                if !columns.contains(&column) { columns.push(column); }
            }
            for extra in ["arquivo", "caminho"] {
                if !columns.iter().any(|c| c == extra) { columns.push(extra.into()); }
            }
            prepared.validate()?;
            sink(&mut resume.lines, bytes.len(), true, Some(&columns))?;
            report(reporter, Progress::new("metadata-columns", "Colunas identificadas", count, count, "amostras"));
            columns
        }
    };
    // Supported parser fields remain discoverable even when an unchanged
    // metadata journal predates Java structural enrichment. This does not
    // rewrite its record offsets, saved catalog, or durable framing identity.
    crate::java_stacktrace::extend_columns(&prepared.part.format, &mut columns);
    prepared.validate()?;
    crate::operations::check()?;
    let mut part = prepared.part.clone();
    part.metadata_identity = prepared.key()?;
    Ok(FileIndex { parts: vec![part], lines: std::sync::Arc::new(resume.lines.finish()?), columns, time_order: std::sync::Arc::new(std::sync::OnceLock::new()) })
}

/// Uncached entry point, also used as the semantic oracle in recovery tests.
pub fn index_file(path: &str, format: &str, custom: Option<CustomParse>, saved_ts: Option<CompiledTsConfig>, progress: Option<&dyn Fn(usize, usize)>) -> Result<FileIndex, String> {
    let prepared = prepare_index(path, format, custom, saved_ts)?;
    let resume = crate::metadata_checkpoint::Resume { cursor: prepared.initial_cursor(), ..Default::default() };
    let report = |p: &crate::metadata_checkpoint::Progress| {
        if p.phase_id == "metadata-scan" { if let Some(cb) = progress { cb(p.completed, p.total); } }
    };
    index_prepared(&prepared, resume, &mut |_, _, _, _| Ok(()), Some(&report))
}

// ------------------------------------------------------- Windows Event Log

#[cfg(windows)]
pub fn list_channels() -> Result<Vec<String>, String> {
    use windows::Win32::System::EventLog::*;
    unsafe {
        let h_enum = EvtOpenChannelEnum(None, 0).map_err(|e| e.message())?;
        let mut out = Vec::new();
        let mut buf = vec![0u16; 512];
        loop {
            let mut used: u32 = 0;
            if EvtNextChannelPath(h_enum, Some(&mut buf), &mut used).is_err() {
                break;
            }
            let len = (used as usize).saturating_sub(1).min(buf.len());
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        let _ = EvtClose(h_enum);
        out.sort();
        Ok(out)
    }
}

#[cfg(not(windows))]
pub fn list_channels() -> Result<Vec<String>, String> {
    Err("Leitura de canais só está disponível no Windows.".into())
}

#[cfg(windows)]
pub fn visit_channel(
    channel: &str,
    max_events: usize,
    mut visit: impl FnMut(Event) -> Result<(), String>,
) -> Result<usize, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::EventLog::*;
    struct Handle(EVT_HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = EvtClose(self.0);
            }
        }
    }
    unsafe {
        let path = HSTRING::from(channel);
        let file = std::path::Path::new(channel).is_file();
        let flags = if file {
            EvtQueryFilePath.0
        } else {
            EvtQueryChannelPath.0
        } | EvtQueryReverseDirection.0;
        let query = match EvtQuery(None, PCWSTR(path.as_ptr()), PCWSTR::null(), flags) {
            Ok(handle) => Handle(handle),
            // Damaged or foreign exports are still readable by the portable parser.
            Err(_) if file => return visit_evtx_file(channel, max_events, visit),
            Err(e) => {
                return Err(if e.code().0 as u32 == 0x80070005 {
                    "ELEVATION_REQUIRED".into()
                } else {
                    e.message()
                })
            }
        };
        let mut count = 0;
        let mut batch = [0isize; 64];
        while count < max_events {
            crate::operations::check()?;
            let mut returned = 0;
            if let Err(error) = EvtNext(query.0, &mut batch, 0, 0, &mut returned) {
                if error.code().0 as u32 == 0x80070103 {
                    break;
                }
                return Err(error.message());
            }
            if returned == 0 {
                break;
            }
            // Own every returned handle before parsing, so errors and cancellation close the batch.
            let handles: Vec<Handle> = batch[..returned as usize]
                .iter()
                .map(|h| Handle(EVT_HANDLE(*h)))
                .collect();
            for handle in handles {
                if count >= max_events {
                    continue;
                }
                crate::operations::check()?;
                let xml = render_event_xml(handle.0)
                    .ok_or("Não foi possível ler um evento do Windows.")?;
                let event = parse_event_xml(&xml).ok_or("Evento Windows com XML inválido.")?;
                visit(event)?;
                count += 1;
            }
        }
        Ok(count)
    }
}
#[cfg(not(windows))]
pub fn visit_channel(
    channel: &str,
    max_events: usize,
    visit: impl FnMut(Event) -> Result<(), String>,
) -> Result<usize, String> {
    if std::path::Path::new(channel).is_file() {
        return visit_evtx_file(channel, max_events, visit);
    }
    Err("Canais do Event Log só podem ser lidos no Windows. Arquivos .evtx exportados funcionam em qualquer sistema.".into())
}

/// Exported .evtx files through a pure Rust parser (any operating system).
/// Records that cannot be decoded are skipped and counted.
pub fn visit_evtx_file(
    path: &str,
    max_events: usize,
    mut visit: impl FnMut(Event) -> Result<(), String>,
) -> Result<usize, String> {
    // EVTX parsing runs in the caller's globally admitted lane; its native
    // chunk workers must not multiply the application's shared CPU budget.
    let mut parser = evtx::EvtxParser::from_path(path)
        .map_err(|e| format!("Não foi possível abrir o EVTX: {e}"))?
        .with_configuration(evtx::ParserSettings::default().num_threads(1));
    let mut count = 0usize;
    let mut skipped = 0usize;
    for record in parser.records() {
        crate::operations::check()?;
        if count >= max_events {
            break;
        }
        match record {
            Ok(record) => match parse_event_xml(&record.data) {
                Some(mut event) => {
                    event.fields.insert("EventRecordID".into(), Value::from(record.event_record_id));
                    visit(event)?;
                    count += 1;
                }
                None => skipped += 1,
            },
            Err(_) => skipped += 1,
        }
    }
    if count == 0 && skipped > 0 {
        return Err(format!("Nenhum evento legível no EVTX ({skipped} registros com erro)."));
    }
    Ok(count)
}

#[cfg(windows)]
unsafe fn render_event_xml(h: windows::Win32::System::EventLog::EVT_HANDLE) -> Option<String> {
    use windows::Win32::System::EventLog::*;
    let mut used: u32 = 0;
    let mut props: u32 = 0;
    let _ = EvtRender(None, h, EvtRenderEventXml.0, 0, None, &mut used, &mut props);
    if used == 0 {
        return None;
    }
    let mut buf = vec![0u16; (used / 2 + 2) as usize];
    let ok = EvtRender(
        None,
        h,
        EvtRenderEventXml.0,
        used,
        Some(buf.as_mut_ptr() as *mut _),
        &mut used,
        &mut props,
    );
    if ok.is_err() {
        return None;
    }
    let len = (used as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn parse_event_xml(xml: &str) -> Option<Event> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let system = root.children().find(|n| n.has_tag_name("System"))?;

    let mut ev = Event::empty();
    ev.raw = xml.to_string();

    if let Some(p) = system.children().find(|n| n.has_tag_name("Provider")) {
        ev.source = p.attribute("Name").unwrap_or_default().to_string();
    }
    if let Some(n) = system.children().find(|n| n.has_tag_name("EventID")) {
        ev.code = n.text().unwrap_or_default().trim().to_string();
    }
    if let Some(n) = system.children().find(|n| n.has_tag_name("Level")) {
        ev.level = normalize_level(n.text().unwrap_or_default());
    }
    if let Some(n) = system.children().find(|n| n.has_tag_name("TimeCreated")) {
        if let Some(st) = n.attribute("SystemTime") {
            ev.timestamp = parse_timestamp(st);
        }
    }
    if let Some(n) = system.children().find(|n| n.has_tag_name("Computer")) {
        ev.fields
            .insert("computer".into(), Value::from(n.text().unwrap_or_default()));
    }
    if let Some(n) = system.children().find(|n| n.has_tag_name("Channel")) {
        ev.fields
            .insert("channel".into(), Value::from(n.text().unwrap_or_default()));
    }

    // Corpo do evento: EventData (Data name=valor) ou UserData.
    let mut parts: Vec<String> = Vec::new();
    if let Some(data) = root.children().find(|n| n.has_tag_name("EventData")) {
        for d in data.children().filter(|n| n.has_tag_name("Data")) {
            let text = d.text().unwrap_or_default().trim();
            if let Some(name) = d.attribute("Name") {
                parts.push(format!("{name}={text}"));
                ev.fields.insert(name.to_string(), Value::from(text));
            } else if !text.is_empty() {
                parts.push(text.to_string());
            }
        }
    } else if let Some(ud) = root.children().find(|n| n.has_tag_name("UserData")) {
        for d in ud
            .descendants()
            .filter(|n| n.is_element() && n.children().all(|c| c.is_text()))
        {
            let text = d.text().unwrap_or_default().trim();
            if !text.is_empty() {
                parts.push(format!("{}={text}", d.tag_name().name()));
            }
        }
    }
    ev.message = parts.join("; ");
    if ev.message.is_empty() {
        ev.message = "(evento sem dados)".into();
    }
    Some(ev)
}

// --------------------------------------------- catálogo do sistema (harvest)

/// Deriva um nome curto a partir do modelo de mensagem oficial do evento:
/// primeira frase da primeira linha, sem os placeholders (%1, %2...).
#[cfg(windows)]
fn short_name(msg: &str) -> String {
    let first = msg.lines().next().unwrap_or(msg);
    let first = first.split(". ").next().unwrap_or(first).trim();
    let mut out = String::with_capacity(first.len());
    let mut chars = first.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            // pula placeholder numérico (%1, %2, ...); %% vira %
            if matches!(chars.peek(), Some('%')) {
                out.push('%');
                chars.next();
            } else {
                while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                    chars.next();
                }
            }
        } else {
            out.push(c);
        }
    }
    let out = out.trim().trim_end_matches('.').trim().to_string();
    let mut out = if out.is_empty() {
        "Evento do sistema".to_string()
    } else {
        out
    };
    if out.chars().count() > 72 {
        out = out
            .chars()
            .take(72)
            .collect::<String>()
            .trim_end()
            .to_string()
            + "…";
    }
    out
}

#[cfg(windows)]
unsafe fn event_prop_u32(
    em: windows::Win32::System::EventLog::EVT_HANDLE,
    prop: windows::Win32::System::EventLog::EVT_EVENT_METADATA_PROPERTY_ID,
) -> Option<u32> {
    use windows::Win32::System::EventLog::*;
    let mut variant = EVT_VARIANT::default();
    let mut used: u32 = 0;
    EvtGetEventMetadataProperty(
        em,
        prop,
        0,
        std::mem::size_of::<EVT_VARIANT>() as u32,
        Some(&mut variant as *mut _),
        &mut used,
    )
    .ok()?;
    if variant.Type == EvtVarTypeUInt32.0 as u32 {
        Some(variant.Anonymous.UInt32Val)
    } else {
        None
    }
}

#[cfg(windows)]
unsafe fn event_id_of(em: windows::Win32::System::EventLog::EVT_HANDLE) -> Option<u32> {
    event_prop_u32(em, windows::Win32::System::EventLog::EventMetadataEventID)
}

#[cfg(windows)]
unsafe fn event_message(
    meta: windows::Win32::System::EventLog::EVT_HANDLE,
    em: windows::Win32::System::EventLog::EVT_HANDLE,
) -> Option<String> {
    use windows::Win32::System::EventLog::*;
    let msg_id = event_prop_u32(em, EventMetadataEventMessageID)?;
    if msg_id == u32::MAX {
        return None; // evento sem mensagem associada
    }
    let mut used: u32 = 0;
    let _ = EvtFormatMessage(
        meta,
        None,
        msg_id,
        None,
        EvtFormatMessageId.0 as u32,
        None,
        &mut used,
    );
    if used == 0 {
        return None;
    }
    let mut buf = vec![0u16; used as usize + 2];
    EvtFormatMessage(
        meta,
        None,
        msg_id,
        None,
        EvtFormatMessageId.0 as u32,
        Some(&mut buf),
        &mut used,
    )
    .ok()?;
    let len = (used as usize).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(windows)]
unsafe fn harvest_publisher(publisher: &str, cfg: &mut crate::model::CodesConfig) -> usize {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::EventLog::*;

    let mut count = 0;
    let pid = HSTRING::from(publisher);
    let meta = match EvtOpenPublisherMetadata(None, PCWSTR(pid.as_ptr()), PCWSTR::null(), 0, 0) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let em_enum = match EvtOpenEventMetadataEnum(meta, 0) {
        Ok(e) => e,
        Err(_) => {
            let _ = EvtClose(meta);
            return 0;
        }
    };
    loop {
        let em = match EvtNextEventMetadata(em_enum, 0) {
            Ok(h) => h,
            Err(_) => break,
        };
        if let Some(id) = event_id_of(em) {
            if let Some(msg) = event_message(meta, em) {
                let msg = msg.trim().to_string();
                if !msg.is_empty() {
                    cfg.sources
                        .entry(publisher.to_string())
                        .or_default()
                        .entry(id.to_string())
                        .or_insert(crate::model::CodeInfo {
                            name: short_name(&msg),
                            description: msg,
                        });
                    count += 1;
                }
            }
        }
        let _ = EvtClose(em);
    }
    let _ = EvtClose(em_enum);
    let _ = EvtClose(meta);
    count
}

/// Extrai o catálogo completo de eventos registrados no sistema operacional:
/// todos os providers e seus eventos, com a mensagem oficial localizada.
#[cfg(windows)]
pub fn harvest_system_codes() -> Result<(crate::model::CodesConfig, usize), String> {
    use windows::Win32::System::EventLog::*;

    unsafe {
        let mut cfg = crate::model::CodesConfig::default();
        let mut total = 0usize;
        let h_enum = EvtOpenPublisherEnum(None, 0).map_err(|e| e.message())?;
        let mut buf = vec![0u16; 256];
        loop {
            let mut used: u32 = 0;
            if EvtNextPublisherId(h_enum, Some(&mut buf), &mut used).is_err() {
                break;
            }
            let len = (used as usize).saturating_sub(1).min(buf.len());
            let publisher = String::from_utf16_lossy(&buf[..len]);
            total += harvest_publisher(&publisher, &mut cfg);
        }
        let _ = EvtClose(h_enum);
        Ok((cfg, total))
    }
}

#[cfg(not(windows))]
pub fn harvest_system_codes() -> Result<(crate::model::CodesConfig, usize), String> {
    Err("Extração de catálogo só está disponível no Windows.".into())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    #[ignore = "Requires installed Windows providers; run explicitly as an integration check."]
    fn harvest_extracts_events() {
        let (cfg, total) = super::harvest_system_codes().expect("harvest falhou");
        eprintln!("fontes: {}, eventos: {}", cfg.sources.len(), total);
        assert!(total > 0, "nenhum evento extraído");
    }

    #[test]
    #[ignore = "Set BENCH_FILE to a local workload and run explicitly."]
    fn bench_index_e_consulta() {
        use crate::model::CodesConfig;
        use crate::query;
        let path = std::env::var("BENCH_FILE")
            .unwrap_or_else(|_| r"C:\dev\logs\exemplos\grande.jsonl".into());
        let t0 = std::time::Instant::now();
        let idx = super::index_file(&path, "auto", None, None, None).expect("index falhou");
        eprintln!(
            "INDEX: {} linhas em {:?} ({:.1} MB/s)",
            idx.lines.len(),
            t0.elapsed(),
            idx.mmap.len() as f64 / 1e6 / t0.elapsed().as_secs_f64()
        );
        let codes = CodesConfig::default();
        let mk = |col: &str, op: &str, val: &str| query::Filter {
            column: col.into(),
            op: op.into(),
            value: val.into(),
            value2: None,
        };
        let t1 = std::time::Instant::now();
        let r = query::query_indexed(
            &idx,
            &[mk("message", "contains", "timeout")],
            "timestamp",
            "desc",
            0,
            50,
            &codes,
            &codes,
            &[],
        ).unwrap();
        eprintln!(
            "QUERY contains(message=timeout): {} resultados em {:?}",
            r.total,
            t1.elapsed()
        );
        let t2 = std::time::Instant::now();
        let r2 = query::query_indexed(
            &idx,
            &[mk("_all", "regex", r"empresa-3")],
            "timestamp",
            "desc",
            0,
            50,
            &codes,
            &codes,
            &[],
        ).unwrap();
        eprintln!(
            "QUERY regex(_all): {} resultados em {:?}",
            r2.total,
            t2.elapsed()
        );
        let t3 = std::time::Instant::now();
        let a = query::aggregate_indexed(
            &idx,
            &[],
            "level",
            &[query::AggSpec {
                func: "count".into(),
                column: "*".into(),
                alias: "n".into(),
            }],
            &codes,
            &codes,
            &[],
        );
        eprintln!(
            "AGG group by level: {} grupos em {:?}",
            a.rows.len(),
            t3.elapsed()
        );
        assert!(idx.lines.len() > 0);
    }

    #[test]
    fn parse_todos_os_formatos() {
        // CEF
        let ev = super::parse_cef("CEF:0|Fortinet|FortiGate|7.2|000123|SSH login failed|8|src=45.90.1.2 dst=10.0.0.1 spt=51022 dpt=22 msg=Failed password").unwrap();
        assert_eq!(ev.code, "000123");
        assert_eq!(ev.level, "Erro");
        assert_eq!(ev.fields.get("src").unwrap(), "45.90.1.2");
        // LEEF
        let ev = super::parse_leef(
            "LEEF:1.0|Microsoft|MSExchange|2016|15345|src=10.1.1.9 sev=6 msg=Mail delivered",
        )
        .unwrap();
        assert_eq!(ev.code, "15345");
        assert_eq!(ev.level, "Aviso");
        assert_eq!(ev.message, "Mail delivered");
        // log4j
        let ev = super::parse_log4j("2024-01-31 08:00:01,123 ERROR [http-nio-8080-exec-3] com.app.PagamentoService - Falha ao processar pagamento").unwrap();
        assert_eq!(ev.level, "Erro");
        assert!(ev.timestamp.unwrap() > 0);
        assert_eq!(ev.fields.get("thread").unwrap(), "http-nio-8080-exec-3");
        // logfmt
        let ev = super::parse_logfmt(
            "ts=2024-01-31T08:00:01Z level=error code=42 host=srv-01 msg=\"falha no disco\" io=123",
        )
        .unwrap();
        assert_eq!(ev.level, "Erro");
        assert_eq!(ev.code, "42");
        assert_eq!(ev.source, "srv-01");
        assert_eq!(ev.message, "falha no disco");
        // csv
        let header = super::split_csv("timestamp,level,code,message,latency_ms");
        let ev = super::parse_csv_line("2024-01-31 08:00:01,error,500,falha geral,820", &header)
            .unwrap();
        assert_eq!(ev.code, "500");
        assert_eq!(ev.fields.get("latency_ms").unwrap(), "820");
        // w3c
        let header: Vec<String> = "date time s-ip cs-method cs-uri-stem sc-status"
            .split(' ')
            .map(|s| s.into())
            .collect();
        let ev = super::parse_w3c("2024-01-31 08:00:01 10.0.0.9 GET /api/x 500", &header).unwrap();
        assert_eq!(ev.source, "10.0.0.9");
        assert_eq!(ev.code, "500");
        assert!(ev.timestamp.unwrap() > 0);
        // detecção
        assert_eq!(super::detect_format(b"CEF:0|V|P|1|2|n|3|k=v\n"), "cef");
        assert_eq!(super::detect_format(b"LEEF:1.0|V|P|1|2|k=v\n"), "leef");
        assert_eq!(super::detect_format(b"2024-01-31 08:00:01,123 INFO [t] com.x.Y - msg\n2024-01-31 08:00:02,123 WARN [t] com.x.Y - msg2\n2024-01-31 08:00:03,123 ERROR [t] com.x.Y - msg3\n"), "log4j");
        assert_eq!(super::detect_format(b"ts=1 level=info msg=a x=1\nts=2 level=info msg=b x=2\nts=3 level=info msg=c x=3\n"), "logfmt");
        assert_eq!(
            super::detect_format(b"timestamp,level,message\n2024-01-31,info,ok\n"),
            "csv"
        );
        assert_eq!(super::detect_format(b"#Software: IIS\n#Fields: date time s-ip cs-method\n2024-01-31 08:00:01 10.0.0.1 GET\n"), "w3c");
    }
}

#[cfg(test)]
mod java_enrichment_tests {
    use super::*;

    const BODY: &str = "a.TopException: café\n\tat a.Top.run(Top.java:1)\n\tSuppressed: a.CloseException\n\t\tat a.Close.close(Native Method)\nCaused by: a.BottomException: actual cause\n\tat a.Bottom.run(Unknown Source)\n\t... 1 more\n";

    #[test]
    fn existing_java_parsers_preserve_legacy_evidence_and_add_real_typed_fields() {
        for (head, parse) in [
            ("00:00:00,001 ERROR [a.Logger] (worker) failed", parse_wildfly as fn(&str) -> Option<Event>),
            ("2024-01-31 08:00:01,123 ERROR [a.Logger] (worker) failed", parse_jboss),
            ("2024-01-31 08:00:01,123 ERROR [worker] a.Logger - failed", parse_log4j),
        ] {
            let raw = format!("{head}\n{BODY}");
            let event = parse(&raw).unwrap();
            let mut legacy = Event::empty();
            extract_java_body(&mut legacy, BODY);
            assert_eq!(event.raw, raw);
            assert_eq!(event.message, "failed");
            assert_eq!(event.source, "a.Logger");
            assert_eq!(event.level, "Erro");
            assert_eq!(event.fields["exception"], legacy.fields["exception"]);
            assert_eq!(event.fields["stacktrace"], legacy.fields["stacktrace"]);
            assert_eq!(event.fields["java.exception.class"], "a.TopException");
            assert_eq!(event.fields["java.exception.message"], "café");
            assert_eq!(event.fields["java.root_cause.class"], "a.BottomException");
            assert_eq!(event.fields["java.trace.complete"], true);
            assert!(!event.fields.contains_key("java.trace"), "ordinary Events keep only scalar summaries");
            let detail = java_trace_for_event(&event).unwrap().unwrap();
            assert!(detail.complete);
            assert_eq!(detail.nodes[0].header.start, head.len() + 1);
            assert_eq!(detail.root_cause_class(), Some("a.BottomException"));
            let incomplete = parse(&format!("{head}\na.TopException\n\tat malformed frame\n")).unwrap();
            assert_eq!(incomplete.fields["java.trace.complete"], false);
            assert!(!incomplete.fields.contains_key("java.root_cause.class"));
            assert!(!incomplete.fields.contains_key("java.trace.fingerprint"));
            let ordinary = parse(head).unwrap();
            assert!(!ordinary.fields.contains_key("java.trace.complete"));
            assert!(java_trace_for_event(&ordinary).unwrap().is_none());
        }
    }

    #[test]
    fn header_exception_spans_use_raw_coordinates_and_old_snapshots_remain_unavailable() {
        let raw = "2024-01-31 08:00:01,123 ERROR [worker] a.Logger - a.TopException: 日\n\tat a.Top.run(Top.java:1)\n";
        let event = parse_log4j(raw).unwrap();
        assert_eq!(event.message, "a.TopException: 日");
        assert_eq!(event.fields["java.exception.class"], "a.TopException");
        assert_eq!(java_trace_for_event(&event).unwrap().unwrap().nodes[0].header.start, raw.find("a.TopException").unwrap());
        for preserved_raw in ["", raw] {
            let mut legacy = Event::empty();
            legacy.raw = preserved_raw.into();
            legacy.fields.insert("exception".into(), Value::String("a.TopException".into()));
            legacy.fields.insert("stacktrace".into(), serde_json::json!(["at a.Top.run(Top.java:1)"]));
            let bytes = serde_json::to_vec(&legacy).unwrap();
            let restored = parse_line_at(&bytes, "snapshot", None, &[], 2026);
            assert_eq!(restored.raw, preserved_raw);
            assert_eq!(restored.fields["stacktrace"], legacy.fields["stacktrace"]);
            assert!(!restored.fields.contains_key("java.trace"), "historical evidence is not silently reconstructed");
            assert!(!restored.fields.contains_key("java.exception.class"));
            let detail = java_trace_for_event(&restored).unwrap();
            assert_eq!(detail.is_some(), !preserved_raw.is_empty());
            if let Some(detail) = detail { assert_eq!(detail.exception_class(), Some("a.TopException")); }
        }
        let json = serde_json::json!({"message": raw});
        let event = parse_line_at(&serde_json::to_vec(&json).unwrap(), "jsonl", None, &[], 2026);
        assert!(!event.fields.contains_key("java.trace"));
        assert!(java_trace_for_event(&event).unwrap().is_none());
    }

    #[test]
    fn explicit_trace_details_preserve_limits_and_leave_the_event_unchanged() {
        let prefix = "2024-01-31 08:00:01,123 ERROR [worker] a.Logger - ";
        let mut event = Event::empty();
        event.raw = format!("{prefix}a.FailureException: {}", "x".repeat(8192));
        let before = serde_json::to_value(&event).unwrap();
        let trace = java_trace_for_event(&event).unwrap().unwrap();
        assert!(!trace.complete);
        assert!(trace.nodes.is_empty());
        assert!(trace.diagnostics.iter().any(|d| d.code == crate::java_stacktrace::Code::StringLimit));
        assert_eq!(before, serde_json::to_value(&event).unwrap());
        event.raw = format!("{prefix}{}", "x".repeat(300 << 10));
        let trace = java_trace_for_event(&event).unwrap().unwrap();
        assert!(!trace.complete);
        assert!(trace.diagnostics.iter().any(|d| d.code == crate::java_stacktrace::Code::InputLimit));
        event.raw = "x".repeat(70 << 10);
        assert!(java_trace_for_event(&event).unwrap_err().contains("limite"));
        event.raw.clear();
        assert!(java_trace_for_event(&event).unwrap().is_none());
    }
}

#[cfg(test)]
mod ts_tests {
    #[test]
    fn parse_data_com_virgula() {
        let r = super::parse_with_format("2026-06-24 00:00:00,001", "%Y-%m-%d %H:%M:%S%.f", None);
        eprintln!("resultado: {:?}", r);
        assert!(r.is_some(), "falhou ao parsear com vírgula");
    }
}

#[cfg(test)]
mod metadata_calendar_tests {
    use super::*;

    fn configured(format: &str, complement: Option<&str>) -> CompiledTsConfig {
        TsConfig { sources: vec!["linha".into()], format: format.into(), complement: complement.map(str::to_string), ..Default::default() }.compile().unwrap()
    }
    fn old_signature(cc: &CompiledTsConfig) -> String {
        let rules: Vec<_> = cc.rules.iter().map(|(re, template)| (re.as_ref().map(|re| re.as_str()), template.as_ref())).collect();
        format!("{:?}|{}|{:?}|{:?}|{}|{:?}", cc.sources, cc.format, cc.complement, cc.timezone_offset_minutes, cc.clock_adjustment_ms, rules)
    }

    #[test]
    fn fallback_date_is_pinned_and_part_of_only_semantically_relevant_signatures() {
        let day = chrono::NaiveDate::from_ymd_opt(2030, 1, 2).unwrap();
        let next = day.succ_opt().unwrap();
        for format in ["%H:%M:%S", "%H:%M:%S %%Y", "%Y-%m-%d %H:%M:%S"] {
            let mut cc = configured(format, None); cc.reference_date = day;
            let first = cc.signature(); cc.reference_date = next;
            assert_ne!(first, cc.signature(), "{format} can use an omitted/invalid date");
        }
        let mut cc = configured("%H:%M:%S", None); cc.reference_date = day;
        assert_eq!(ts_from_joined("12:30:00", &cc), Some(naive_to_ms(day.and_hms_opt(12, 30, 0).unwrap())));
        cc.reference_date = next;
        assert_eq!(ts_from_joined("12:30:00", &cc), Some(naive_to_ms(next.and_hms_opt(12, 30, 0).unwrap())));
        for (format, complement) in [("epoch_ms", None), ("epoch_s", None), ("%Y-%m-%d", None), ("%H:%M:%S", Some("2020-02-03"))] {
            let mut cc = configured(format, complement); cc.reference_date = day;
            let first = cc.signature(); assert_eq!(first, old_signature(&cc));
            cc.reference_date = next; assert_eq!(first, cc.signature());
        }
    }

    #[test]
    fn explicit_year_parser_covers_syslog_and_firewall_without_clock_reads() {
        for (format, line) in [
            ("syslog3164", "Sep 30 12:30:00 host app: hello"),
            ("firewall", "Sep 30 12:30:00 host kernel: SRC=10.0.0.1 DST=10.0.0.2"),
        ] {
            let event = parse_line_at(line.as_bytes(), format, None, &[], 2032);
            let expected = chrono::NaiveDate::from_ymd_opt(2032, 9, 30).unwrap().and_hms_opt(12, 30, 0).unwrap();
            assert_eq!(event.timestamp, Some(naive_to_ms(expected)), "{format}");
        }
    }

    #[test]
    fn overflowing_multiline_length_is_rejected_instead_of_wrapped() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("source.log");
        std::fs::write(&path, "first\n").unwrap();
        let prepared = prepare_index(path.to_str().unwrap(), "text", None, None).unwrap();
        let mut lines = vec![LineMeta { offset: 0, len: 1, ..Default::default() }];
        let re = regex::Regex::new("^START").unwrap();
        assert!(push_meta(&mut lines, b"continuation", u64::from(u32::MAX), &prepared.part, Some(&re)).is_err());
    }
}
