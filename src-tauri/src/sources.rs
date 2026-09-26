use crate::model::{
    label_class, CodesConfig, Event, LineMeta, LV_CRIT, LV_DEBUG, LV_ERR, LV_INFO, LV_OTHER,
    LV_TRACE, LV_WARN, STANDARD_COLUMNS,
};
use chrono::Datelike;
use serde_json::{Map, Value};

/// Interpreta data/hora "naive" (sem fuso) como horário LOCAL da máquina.
fn naive_to_ms(ndt: chrono::NaiveDateTime) -> i64 {
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
    const FORMATS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%d/%m/%Y %H:%M:%S%.f",
        "%d/%m/%Y %H:%M:%S",
        "%Y-%m-%d",
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
const MSG_KEYS: &[&str] = &["message", "msg", "log", "body", "text"];

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
    if ev.message.is_empty() {
        ev.message = raw.chars().take(500).collect();
    }
    ev.fields = map;
    ev
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

fn re_apache() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r#"^(\S+)\s+(\S+)\s+(\S+)\s+\[([^\]]+)\]\s+"(\S+)\s+(\S+)\s+(\S+)"\s+(\d{3})\s+(\S+)(?:\s+"([^"]*)"\s+"([^"]*)")?"#).unwrap()
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

fn syslog_ts(mon: &str, day: &str, h: &str, mi: &str, s: &str) -> Option<i64> {
    let year = chrono::Utc::now().year();
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

fn parse_syslog3164(line: &str) -> Option<Event> {
    let c = re_syslog3164().captures(line)?;
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = syslog_ts(&c[2], &c[3], &c[4], &c[5], &c[6]);
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
    let c = re_apache().captures(line)?;
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    // [10/Oct/2000:13:55:36 -0700]
    if let Ok(dt) = chrono::DateTime::parse_from_str(&c[4], "%d/%b/%Y:%H:%M:%S %z") {
        ev.timestamp = Some(dt.timestamp_millis());
    }
    ev.source = c[1].to_string(); // IP do cliente
    if &c[3] != "-" {
        ev.fields
            .insert("user".into(), Value::from(c[3].to_string()));
    }
    ev.fields
        .insert("method".into(), Value::from(c[5].to_string()));
    ev.fields
        .insert("path".into(), Value::from(c[6].to_string()));
    ev.fields
        .insert("protocol".into(), Value::from(c[7].to_string()));
    ev.code = c[8].to_string(); // status HTTP
    ev.level = match c[8].chars().next() {
        Some('5') => "Erro".into(),
        Some('4') => "Aviso".into(),
        _ => "Informação".into(),
    };
    ev.fields
        .insert("size".into(), Value::from(c[9].to_string()));
    if let Some(r) = c.get(10) {
        ev.fields.insert("referer".into(), Value::from(r.as_str()));
    }
    if let Some(a) = c.get(11) {
        ev.fields.insert("agent".into(), Value::from(a.as_str()));
    }
    ev.message = format!("{} {} {} → {}", &c[5], &c[6], &c[7], &c[8]);
    Some(ev)
}

fn parse_firewall(line: &str) -> Option<Event> {
    // iptables/netfilter: prefixo estilo syslog + pares CHAVE=VALOR
    let c = re_syslog3164().captures(line)?;
    let msg = c[10].to_string();
    let kv = |key: &str| -> Option<String> {
        msg.split_whitespace()
            .find_map(|t| t.strip_prefix(key).map(|v| v.to_string()))
    };
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = syslog_ts(&c[2], &c[3], &c[4], &c[5], &c[6]);
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

/// Inferência automática do formato pela amostra inicial do arquivo.
fn detect_format(bytes: &[u8]) -> &'static str {
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
    let mut n = 0usize;
    let (mut json, mut s3164, mut s5424, mut apache, mut fw, mut log4j, mut logfmt, mut wildfly) =
        (0, 0, 0, 0, 0, 0, 0, 0);
    // linhas de corpo de stacktrace Java contam como evidência de log4j/wildfly
    // (arquivos com muitos stacktraces teriam poucas linhas de cabeçalho)
    let mut stack = 0usize;
    let mut csv_hint = false;
    let mut csv_checked = false;
    let mut w3c_hint = false;
    for (li, line_b) in bytes.split(|&b| b == b'\n').take(60).enumerate() {
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
        if !csv_checked {
            csv_checked = true;
            if looks_like_csv_header(line) {
                // próxima linha precisa ter o mesmo número de vírgulas
                csv_hint = bytes
                    .split(|&b| b == b'\n')
                    .nth(li + 1)
                    .map(|l2| {
                        l2.iter().filter(|&&b| b == b',').count() == line.matches(',').count()
                    })
                    .unwrap_or(false);
                continue;
            }
        }
        n += 1;
        if line.starts_with('{') {
            json += 1;
        } else if re_syslog5424().is_match(line) {
            s5424 += 1;
        } else if re_apache().is_match(line) {
            apache += 1;
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
        } else if re_logfmt().captures_iter(line).count() >= 3 {
            logfmt += 1;
        }
    }
    if w3c_hint {
        return "w3c";
    }
    if csv_hint {
        return "csv";
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

fn re_logfmt() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#"(\w+)=("([^"]*)"|\S+)"#).unwrap())
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
    Some(ev)
}

/// Logfmt (Heroku/estilo key=value): `ts=... level=info msg="..." k=v`
fn parse_logfmt(line: &str) -> Option<Event> {
    let kvs: Vec<_> = re_logfmt().captures_iter(line).collect();
    if kvs.len() < 3 {
        return None;
    }
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    for cap in kvs {
        let key = cap[1].to_lowercase();
        let val = cap
            .get(3)
            .map(|m| m.as_str())
            .unwrap_or_else(|| cap[2].trim_matches('"'));
        match key.as_str() {
            "ts" | "time" | "timestamp" => {
                ev.timestamp = parse_timestamp(val).or_else(|| {
                    val.parse::<f64>().ok().map(|f| {
                        if f > 1e12 {
                            f as i64
                        } else {
                            (f * 1000.0) as i64
                        }
                    })
                });
            }
            "level" | "lvl" | "severity" => ev.level = normalize_level(val),
            "msg" | "message" => ev.message = val.to_string(),
            "code" | "event" => ev.code = val.to_string(),
            "host" | "source" | "app" | "service" => ev.source = val.to_string(),
            other => {
                ev.fields.insert(other.into(), Value::from(val));
            }
        }
    }
    if ev.message.is_empty() {
        ev.message = line.to_string();
    }
    Some(ev)
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

/// Divide linha CSV respeitando aspas duplas com escape "".
fn split_csv(line: &str) -> Vec<String> {
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
        } else if c == ',' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Mapeia colunas (de cabeçalho CSV ou #Fields do W3C) para o evento.
fn event_from_columns(vals: Vec<String>, header: &[String], raw: &str) -> Event {
    let mut ev = Event::empty();
    ev.raw = raw.to_string();
    for (i, h) in header.iter().enumerate() {
        let v = vals.get(i).map(|s| s.trim()).unwrap_or("");
        if v.is_empty() || v == "-" {
            continue;
        }
        let hl = h.to_lowercase();
        if TS_KEYS.contains(&hl.as_str()) {
            if ev.timestamp.is_none() {
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
        } else if hl == "date" {
            // W3C: combina date + time
            let time = header
                .iter()
                .position(|x| x.eq_ignore_ascii_case("time"))
                .and_then(|p| vals.get(p))
                .map(|s| s.trim());
            ev.timestamp = parse_timestamp(&format!("{} {}", v, time.unwrap_or("00:00:00")));
        } else if hl == "time" {
            // tratado junto com date
        } else if LEVEL_KEYS.contains(&hl.as_str()) {
            ev.level = normalize_level(v);
        } else if CODE_KEYS.contains(&hl.as_str()) || hl == "status" || hl == "sc-status" {
            ev.code = v.to_string();
        } else if SOURCE_KEYS.contains(&hl.as_str()) || hl == "s-ip" || hl == "c-ip" {
            if ev.source.is_empty() {
                ev.source = v.to_string();
            } else {
                ev.fields.insert(h.clone(), Value::from(v));
            }
        } else if MSG_KEYS.contains(&hl.as_str()) {
            ev.message = v.to_string();
        } else {
            ev.fields.insert(h.clone(), Value::from(v));
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

fn parse_csv_line(line: &str, header: &[String]) -> Option<Event> {
    if header.is_empty() {
        return None;
    }
    Some(event_from_columns(split_csv(line), header, line))
}

/// Parece um cabeçalho CSV? (≥3 colunas, tokens simples, sem espaços longos)
fn looks_like_csv_header(line: &str) -> bool {
    let cols: Vec<&str> = line.split(',').collect();
    cols.len() >= 3
        && cols.iter().all(|c| {
            let c = c.trim();
            !c.is_empty()
                && c.len() <= 32
                && c.chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | ' '))
                && c.chars().filter(|ch| ch.is_whitespace()).count() <= 2
        })
}

// ==========================================================================
// Arquivos grandes: índice em memória sobre mmap
// ==========================================================================

/// Configuração de data/hora: como montar o timestamp do evento.
/// `sources`: campos a concatenar (colunas, "arquivo", "caminho", "linha").
/// `regex`: opcional, extrai o texto da data (2 grupos = data + hora).
/// `format`: padrão chrono, "epoch_ms" ou "epoch_s".
/// `complement`: data literal ("2026-06-24") quando o formato só tem hora.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
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
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TsRule {
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
}

/// TsConfig com as regexes já compiladas.
pub struct CompiledTsConfig {
    pub timezone_offset_minutes: Option<i32>,
    pub clock_adjustment_ms: i64,
    pub sources: Vec<String>,
    pub format: String,
    pub complement: Option<String>,
    /// (regex compilada, template) por regra, na ordem do OU
    pub rules: Vec<(Option<regex::Regex>, Option<String>)>,
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
}

/// Formato de leitura tolerante ao legado (regra única na raiz do objeto).
#[derive(serde::Deserialize)]
pub struct DerivedFieldCompat {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub filter: Option<crate::query::Filter>,
    #[serde(default)]
    pub rules: Vec<DerivedRule>,
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
        }
    }
}

pub struct CompiledRule {
    pub re: regex::Regex,
    pub template: Option<String>,
    pub filter: Option<crate::query::Filter>,
}

pub struct CompiledDerived {
    pub name: String,
    pub source: String,
    pub rules: Vec<CompiledRule>,
}

/// Aplica os campos derivados a um evento. As regras de cada campo são
/// tentadas em ordem (OU): a primeira que extrai valor não vazio vence.
pub fn apply_derived(ev: &mut Event, derived: &[CompiledDerived]) {
    for d in derived {
        let Some(val) = ev.col_str(&d.source) else {
            continue;
        }; // owned: liberado para inserir o campo
        for r in &d.rules {
            if let Some(f) = &r.filter {
                if !crate::query::matches_filter(ev, f) {
                    continue; // condição não atendida: tenta a próxima regra
                }
            }
            if let Some(cap) = r.re.captures(&val) {
                let extracted = match &r.template {
                    Some(t) if !t.is_empty() => {
                        let mut out = String::new();
                        cap.expand(t, &mut out);
                        out
                    }
                    _ => cap
                        .get(1)
                        .or_else(|| cap.get(0))
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default(),
                };
                if !extracted.is_empty() {
                    ev.fields.insert(d.name.clone(), Value::from(extracted));
                    break;
                }
            }
        }
    }
}

/// Como interpretar um formato customizado.
#[derive(Clone)]
pub enum CustomParse {
    Regex(regex::Regex),
    Delimited { sep: char, fields: Vec<String> },
}

pub struct FilePart {
    pub path: String,
    pub file_name: String,
    pub format: String,
    pub custom: Option<CustomParse>,
    pub ts_config: Option<CompiledTsConfig>,
    pub header: Vec<String>,
    pub mmap: memmap2::Mmap,
    pub base: u64,
    pub identity: String,
}

pub struct FileIndex {
    pub parts: Vec<FilePart>,
    pub lines: Vec<LineMeta>,
    pub columns: Vec<String>,
    pub time_order: std::sync::OnceLock<Vec<usize>>,
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
        let offset = self.lines[i].offset;
        &self.parts[self
            .parts
            .partition_point(|p| p.base <= offset)
            .saturating_sub(1)]
    }
    pub fn append(&mut self, mut other: FileIndex) {
        let base = self
            .parts
            .last()
            .map(|p| p.base + p.mmap.len() as u64 + 1)
            .unwrap_or(0);
        for p in &mut other.parts {
            p.base += base;
        }
        for m in &mut other.lines {
            m.offset += base;
        }
        self.parts.append(&mut other.parts);
        self.lines.append(&mut other.lines);
        for c in other.columns {
            if !self.columns.contains(&c) {
                self.columns.push(c);
            }
        }
        self.time_order.take();
    }
    pub fn bytes_len(&self) -> u64 {
        self.parts.iter().map(|p| p.mmap.len() as u64).sum()
    }
    pub fn ordered(&self) -> &[usize] {
        self.time_order.get_or_init(|| {
            let mut order: Vec<usize> = (0..self.lines.len()).collect();
            order.sort_unstable_by_key(|&i| (self.lines[i].ts, i));
            order
        })
    }
}

pub fn line_bytes(idx: &FileIndex, i: usize) -> &[u8] {
    let m = &idx.lines[i];
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

fn parse_with_format(s: &str, fmt: &str, complement: Option<&str>) -> Option<i64> {
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
                .unwrap_or_else(|| chrono::Utc::now().date_naive());
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
        if let Some(mut ts) = parse_with_format(&candidate, &cc.format, cc.complement.as_deref()) {
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
    let total = idx.lines.len();
    let mut timestamps = Vec::with_capacity(total);
    for i in 0..total {
        if i % 2048 == 0 {
            crate::operations::check()?;
            if let Some(cb) = progress {
                cb(i, total);
            }
        }
        let part = idx.part_at(i);
        let bytes = line_bytes(idx, i);
        let mut ev = parse_line(bytes, &part.format, part.custom.as_ref(), &part.header);
        if let Some(cc) = &part.ts_config {
            apply_ts_config(&mut ev, cc, part, &String::from_utf8_lossy(bytes));
        }
        timestamps.push(ev.timestamp.unwrap_or(0));
    }
    crate::operations::check()?;
    for (line, timestamp) in idx.lines.iter_mut().zip(timestamps) {
        line.ts = timestamp;
    }
    idx.time_order.take();
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
    let mut ev = parse_line(bytes, &part.format, part.custom.as_ref(), &part.header);
    ev.id = i;
    if ev.event_ref.is_empty() {
        ev.event_ref = format!("{}:{}", part.identity, idx.lines[i].offset - part.base);
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
        "syslog3164" => parse_syslog3164(&text).unwrap_or_else(|| event_from_text(&text)),
        "syslog5424" => parse_syslog5424(&text).unwrap_or_else(|| event_from_text(&text)),
        "apache" => parse_apache(&text).unwrap_or_else(|| event_from_text(&text)),
        "firewall" => parse_firewall(&text).unwrap_or_else(|| event_from_text(&text)),
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
        "csv" => parse_csv_line(&text, header).unwrap_or_else(|| event_from_text(&text)),
        "w3c" => parse_w3c(&text, header).unwrap_or_else(|| event_from_text(&text)),
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

fn meta_for_line(
    line: &[u8],
    offset: u64,
    format: &str,
    custom: Option<&CustomParse>,
    header: &[String],
) -> LineMeta {
    let mut m = LineMeta {
        offset,
        len: line.len() as u32,
        ..Default::default()
    };
    let ev = parse_line(line, format, custom, header);
    m.ts = ev.timestamp.unwrap_or(0);
    m.level = label_class(&ev.level).unwrap_or(LV_OTHER);
    // Offset is an optimization only; non-JSON codes are resolved from the event.
    if format == "jsonl" {
        if let Some((a, b)) = find_json_value(line, CODE_KEYS) {
            if line.get(a..b) == Some(ev.code.as_bytes()) && b - a <= u16::MAX as usize {
                m.code_off = a as u32;
                m.code_len = (b - a) as u16;
            }
        }
    }
    m
}

/// Adiciona uma linha ao índice. Em formatos multi-linha (`start_re`), uma
/// linha que NÃO casa o padrão de início é continuação (corpo de stacktrace):
/// estende o `len` do evento anterior em vez de virar evento próprio.
/// Continuações antes do primeiro evento viram linhas soltas (o comportamento
/// normal de uma linha qualquer).
#[allow(clippy::too_many_arguments)]
fn push_meta(
    lines: &mut Vec<LineMeta>,
    line: &[u8],
    offset: u64,
    fmt: &str,
    custom: Option<&CustomParse>,
    header: &[String],
    start_re: Option<&regex::Regex>,
) {
    if let Some(re) = start_re {
        let text = std::str::from_utf8(line).unwrap_or("");
        if !re.is_match(text) {
            if let Some(last) = lines.last_mut() {
                let end = offset as usize + line.len();
                last.len = (end - last.offset as usize) as u32;
                return;
            }
        }
    }
    lines.push(meta_for_line(line, offset, fmt, custom, header));
}

/// Index JSON array objects directly in the mapped file, including pretty JSON.
/// Only positions and nesting are kept; the array is never deserialized as a Vec.
fn index_json_array(
    bytes: &[u8],
    start: usize,
    lines: &mut Vec<LineMeta>,
    custom: Option<&CustomParse>,
    header: &[String],
    progress: Option<&dyn Fn(usize, usize)>,
) -> Result<(), String> {
    let mut stack = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut record_start = None;
    let mut need_separator = false;
    let mut after_comma = false;
    for (i, &byte) in bytes.iter().enumerate().skip(start + 1) {
        if i % 65_536 == 0 {
            crate::operations::check()?;
        }
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        if let Some(begin) = record_start {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    if stack.len() >= 128 {
                        return Err("JSON excede 128 níveis de aninhamento.".into());
                    }
                    stack.push(byte);
                }
                b'}' | b']' => {
                    let opening = if byte == b'}' { b'{' } else { b'[' };
                    if stack.pop() != Some(opening) {
                        return Err(format!("Estrutura JSON inválida no byte {i}."));
                    }
                    if stack.is_empty() {
                        if i - begin >= u32::MAX as usize {
                            return Err("Um registro JSON excede 4 GB.".into());
                        }
                        lines.push(meta_for_line(
                            &bytes[begin..=i],
                            begin as u64,
                            "jsonl",
                            custom,
                            header,
                        ));
                        record_start = None;
                        need_separator = true;
                        if lines.len().is_multiple_of(2048) {
                            if let Some(cb) = progress {
                                cb(i, bytes.len());
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        match byte {
            b']' if !after_comma => {
                if bytes[i + 1..].iter().any(|b| !b.is_ascii_whitespace()) {
                    return Err("Há conteúdo após o fim do array JSON.".into());
                }
                return Ok(());
            }
            b',' if need_separator => {
                need_separator = false;
                after_comma = true;
            }
            b'{' if !need_separator => {
                record_start = Some(i);
                stack.push(byte);
                after_comma = false;
            }
            _ => {
                return Err(format!(
                    "Esperado um objeto ou separador no array JSON (byte {i})."
                ))
            }
        }
    }
    Err("Array JSON incompleto: falta fechar um objeto ou o array.".into())
}

/// Indexa um arquivo inteiro em uma única passada, guardando apenas
/// metadados compactos por linha (~32 bytes/linha).
pub fn index_file(
    path: &str,
    format: &str,
    custom: Option<CustomParse>,
    saved_ts: Option<CompiledTsConfig>,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Result<FileIndex, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("Não foi possível abrir '{path}': {e}"))?;
    if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
        return Err("Arquivo vazio.".into());
    }
    let mmap = unsafe { memmap2::MmapOptions::new().map(&file) }
        .map_err(|e| format!("Falha ao mapear '{path}': {e}"))?;

    let fmt = if format == "auto" {
        detect_format(&mmap).to_string()
    } else if format == "custom" || format.starts_with("custom:") {
        "custom".to_string()
    } else {
        format.to_string()
    };
    let fmt = fmt.as_str();

    // Cabeçalho (csv: primeira linha; w3c: linha "#Fields:")
    let mut header: Vec<String> = Vec::new();
    if fmt == "csv" {
        if let Some(first) = mmap.split(|&b| b == b'\n').next() {
            let first = String::from_utf8_lossy(first).trim().to_string();
            header = split_csv(&first)
                .iter()
                .map(|s| s.trim().to_string())
                .collect();
        }
    } else if fmt == "w3c" {
        for line in mmap.split(|&b| b == b'\n').take(20) {
            let l = String::from_utf8_lossy(line);
            let l = l.trim();
            if let Some(rest) = l.to_lowercase().strip_prefix("#fields:") {
                header = rest.split_whitespace().map(|s| s.to_string()).collect();
                break;
            }
        }
    }

    // total de linhas por contagem rápida (memchr é SIMD, custo desprezível)
    let total_lines = progress
        .map(|_| memchr::memchr_iter(b'\n', &mmap).count() + 1)
        .unwrap_or(0);
    let mut lines: Vec<LineMeta> = Vec::with_capacity((mmap.len() / 160).min(1_000_000) + 16);
    let mut offset = 0usize;
    let mut first_line = true;
    let mut last_report = 0usize;
    // Formatos multi-linha (stacktrace Java): linha que casa o padrão do
    // formato inicia um evento; as demais estendem o evento anterior.
    let start_re: Option<regex::Regex> = match fmt {
        "log4j" | "wildfly" => detect_entry_start(&mmap),
        _ => None,
    };
    let content_start = if mmap.starts_with(&[0xef, 0xbb, 0xbf]) {
        3
    } else {
        0
    };
    let array_start = (content_start..mmap.len())
        .find(|&i| !mmap[i].is_ascii_whitespace())
        .filter(|&i| fmt == "jsonl" && mmap[i] == b'[');
    if let Some(start) = array_start {
        index_json_array(&mmap, start, &mut lines, custom.as_ref(), &header, progress)?;
    } else {
        for (physical_line, nl) in memchr::memchr_iter(b'\n', &mmap).enumerate() {
            if physical_line % 2048 == 0 {
                crate::operations::check()?;
            }
            let raw = &mmap[offset..nl];
            let line = if raw.last() == Some(&b'\r') {
                &raw[..raw.len() - 1]
            } else {
                raw
            };
            if !line.is_empty() {
                let skip = (fmt == "csv" && first_line) || (fmt == "w3c" && line[0] == b'#');
                if !skip {
                    push_meta(
                        &mut lines,
                        line,
                        offset as u64,
                        fmt,
                        custom.as_ref(),
                        &header,
                        start_re.as_ref(),
                    );
                }
            }
            first_line = false;
            offset = nl + 1;
            if let Some(cb) = progress {
                if lines.len() >= last_report + 2048 {
                    last_report = lines.len();
                    cb(lines.len(), total_lines);
                }
            }
        }
        if offset < mmap.len() {
            let line = &mmap[offset..];
            if !line.is_empty() {
                push_meta(
                    &mut lines,
                    line,
                    offset as u64,
                    fmt,
                    custom.as_ref(),
                    &header,
                    start_re.as_ref(),
                );
            }
        }
    }

    // Descoberta de colunas: amostra das primeiras 2.000 linhas.
    let mut columns: Vec<String> = STANDARD_COLUMNS.iter().map(|s| s.to_string()).collect();
    let mut extra = std::collections::HashSet::new();
    let mut sampled = Vec::with_capacity(lines.len().min(4_000));
    for sample in 0..lines.len().min(4_000) {
        let i = sample * lines.len() / lines.len().min(4_000);
        let m = &lines[i];
        let ev = parse_line(
            &mmap[m.offset as usize..(m.offset as usize + m.len as usize)],
            fmt,
            custom.as_ref(),
            &header,
        );
        for k in ev.fields.keys() {
            extra.insert(k.clone());
        }
        sampled.push(ev);
    }
    let mut extra: Vec<String> = extra.into_iter().collect();
    extra.sort();
    columns.extend(extra);
    // Canonical entities (@user, @src_ip…) observed in the sample.
    for column in crate::entities::observed_columns(sampled.iter()) {
        if !columns.contains(&column) {
            columns.push(column);
        }
    }
    drop(sampled);

    // campos de origem sempre disponíveis como colunas
    for extra_col in ["arquivo", "caminho"] {
        if !columns.iter().any(|c| c == extra_col) {
            columns.push(extra_col.to_string());
        }
    }

    let identity = crate::index_cache::identity(path, &mmap);
    Ok(FileIndex {
        parts: vec![FilePart {
            path: path.into(),
            file_name: file_name_of(path),
            format: fmt.into(),
            custom,
            ts_config: saved_ts,
            header,
            mmap,
            base: 0,
            identity,
        }],
        lines,
        columns,
        time_order: std::sync::OnceLock::new(),
    })
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
        let query = Handle(
            EvtQuery(None, PCWSTR(path.as_ptr()), PCWSTR::null(), flags).map_err(|e| {
                if e.code().0 as u32 == 0x80070005 {
                    "ELEVATION_REQUIRED".into()
                } else {
                    e.message()
                }
            })?,
        );
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
    _channel: &str,
    _max_events: usize,
    _visit: impl FnMut(Event) -> Result<(), String>,
) -> Result<usize, String> {
    Err("Leitura do Event Log só está disponível no Windows.".into())
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

#[cfg(windows)]
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
        );
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
        );
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
mod ts_tests {
    #[test]
    fn parse_data_com_virgula() {
        let r = super::parse_with_format("2026-06-24 00:00:00,001", "%Y-%m-%d %H:%M:%S%.f", None);
        eprintln!("resultado: {:?}", r);
        assert!(r.is_some(), "falhou ao parsear com vírgula");
    }
}
