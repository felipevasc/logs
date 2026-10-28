//! Structure for any text record no dedicated parser claimed, read the way an
//! analyst reads a log line: the date and time wherever it is, then the usual
//! header items written before the message (level, `[thread]`, `(thread)`,
//! logger, `process[pid]:`, pid/tid, component, error codes), then what the
//! message itself carries (HTTP requests, `key=value` pairs, embedded JSON,
//! vendor mnemonics such as Cisco `%ASA-6-302013`). Also recognizes a few
//! widespread shapes outright: Apache error logs, Kubernetes klog, Android
//! logcat, Squid, AWS load balancers and container runtime (CRI) prefixes.
//!
//! The raw record is always kept unchanged; values keep their spelling.
use super::stamp::{self, Context, Inferred};
use crate::model::Event;
use serde_json::{Map, Value};
use std::borrow::Cow;

pub(super) const FORMAT: &str = "generic";

/// Canonical label for a level word in its usual spellings and languages.
pub(crate) fn level_word(word: &str) -> Option<&'static str> {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    if word.len() < 2 || word.len() > 16 {
        return None;
    }
    // Header words are checked for every record: no allocation for ASCII.
    let mut buffer = [0u8; 16];
    let lower_owned;
    let lower: &str = if word.is_ascii() {
        for (slot, b) in buffer.iter_mut().zip(word.bytes()) {
            *slot = b.to_ascii_lowercase();
        }
        std::str::from_utf8(&buffer[..word.len()]).unwrap_or("")
    } else {
        lower_owned = word.to_lowercase();
        &lower_owned
    };
    Some(match lower {
        "fatal" | "critical" | "crit" | "crt" | "emerg" | "emergency" | "alert" | "panic" | "ftl" | "crítico" | "critico" => {
            "Crítico"
        }
        "error" | "err" | "eror" | "erro" | "severe" => "Erro",
        "warn" | "warning" | "wrn" | "wrng" | "aviso" | "advertencia" | "advertência" | "atencao" | "atenção" => "Aviso",
        "info" | "information" | "informational" | "inf" | "informação" | "informacao" | "información" | "informacion"
        | "notice" | "note" | "log" | "config" | "system" => "Informação",
        "debug" | "dbg" | "debg" | "fine" | "depuração" | "depuracao" | "depuración" | "depuracion" => "Depuração",
        "trace" | "trc" | "verbose" | "vrb" | "finer" | "finest" | "rastreio" => "Rastreio",
        _ => return None,
    })
}

/// One-letter levels (klog, logcat, Rails, MongoDB, `[E]`).
fn letter_level(letter: &str) -> Option<&'static str> {
    let mut chars = letter.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    Some(match c.to_ascii_uppercase() {
        'F' | 'A' => "Crítico",
        'E' => "Erro",
        'W' => "Aviso",
        'I' => "Informação",
        'D' => "Depuração",
        'V' | 'T' => "Rastreio",
        _ => return None,
    })
}

/// Syslog severity 0-7 (also Cisco mnemonics and RFC 5424 PRI).
pub(super) fn severity_level(severity: u32) -> &'static str {
    match severity {
        0..=2 => "Crítico",
        3 => "Erro",
        4 => "Aviso",
        5 | 6 => "Informação",
        _ => "Depuração",
    }
}

pub(super) fn is_ip(text: &str) -> bool {
    text.parse::<std::net::IpAddr>().is_ok()
}

/// `1.2.3.4:5678` or `[2001:db8::1]:443`.
pub(super) fn ip_port(text: &str) -> Option<(&str, &str)> {
    let (host, port) = text.rsplit_once(':')?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    (is_ip(host) && !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit())).then_some((host, port))
}

/// Java/Python/.NET logger names: `com.example.App`, `o.e.n.Node`, `django.request`.
fn logger_like(text: &str) -> bool {
    const TLDS: &[&str] = &["com", "net", "org", "br", "io", "gov", "edu", "local", "lan", "internal", "info", "biz", "us", "uk", "de"];
    let text = text.trim_end_matches(':');
    if text.len() < 3 || text.len() > 200 || !text.contains('.') || is_ip(text) {
        return false;
    }
    let identifier = |s: &str| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '-'))
    };
    let last = text.rsplit('.').next().unwrap_or("");
    text.split('.').all(identifier)
        && !TLDS.iter().any(|tld| tld.eq_ignore_ascii_case(last))
        && !matches!(last, "log" | "txt" | "json" | "xml" | "py" | "go" | "java" | "js" | "rb" | "cs")
}

/// Source position written by Go, Python and Node loggers: `server.go:42`, `app/main.py:10`.
fn caller_like(text: &str) -> bool {
    let Some((file, line)) = text.trim_end_matches(':').rsplit_once(':') else { return false };
    let extension = file.rsplit_once('.').map_or("", |(_, ext)| ext);
    !line.is_empty()
        && line.len() <= 7
        && line.bytes().all(|b| b.is_ascii_digit())
        && !extension.is_empty()
        && extension.len() <= 5
        && extension.bytes().all(|b| b.is_ascii_alphabetic())
        && !file.contains(char::is_whitespace)
        && !is_ip(file)
}

/// Product error codes: `AH00126`, `MY-010116`, `ORA-00600`, `WFLYSRV0049`.
fn code_like(text: &str) -> bool {
    let text = text.trim_end_matches(':');
    let letters = text.bytes().take_while(|b| b.is_ascii_uppercase()).count();
    let rest = &text[letters..];
    let rest = rest.strip_prefix(['-', '_']).unwrap_or(rest);
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    (2..=12).contains(&letters) && digits >= 3 && rest.len() - digits <= 1 && rest[digits..].bytes().all(|b| b.is_ascii_uppercase())
}

/// Worker thread names written by application servers and HTTP stacks.
pub(super) fn thread_like(text: &str) -> bool {
    const PREFIXES: &[&str] = &["http-", "https-", "ajp-", "exec-", "default task-", "pool-", "thread-", "worker-", "nio-", "jsse-", "qtp", "scheduling-"];
    if text.len() > 80 || !text.bytes().last().is_some_and(|b| b.is_ascii_digit()) {
        return false;
    }
    let starts = |p: &str| text.len() >= p.len() && text.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    PREFIXES.iter().any(|p| starts(p)) || text.contains("-exec-") || text.contains("-thread-")
}

/// `name[pid]:`, `name[pid]` or `name:` as written by syslog and many daemons.
pub(super) fn process_token(text: &str) -> Option<(&str, Option<&str>, bool)> {
    let terminal = text.ends_with(':');
    let body = text.trim_end_matches(':');
    let (name, pid) = match body.split_once('[') {
        Some((name, rest)) => {
            let pid = rest.strip_suffix(']')?;
            if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (name, Some(pid))
        }
        None if terminal => (body, None),
        None => return None,
    };
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@' | '(' | ')'));
    valid.then_some((name, pid, terminal))
}

#[derive(Default)]
struct Parts {
    timestamp: Option<i64>,
    level: Option<&'static str>,
    source: Option<String>,
    code: Option<String>,
    message: Option<String>,
    fields: Map<String, Value>,
    structured: bool,
    http_client: Option<String>,
}

impl Parts {
    fn set(&mut self, key: &str, value: &str) {
        if !value.is_empty() {
            self.fields.entry(key.to_string()).or_insert_with(|| Value::from(value));
            self.structured = true;
        }
    }
    fn push(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        self.structured = true;
        match self.fields.get_mut(key) {
            None => {
                self.fields.insert(key.to_string(), Value::from(value));
            }
            Some(Value::Array(values)) => values.push(Value::from(value)),
            Some(existing) => *existing = Value::Array(vec![existing.clone(), Value::from(value)]),
        }
    }
    fn level(&mut self, level: &'static str) {
        if self.level.is_none() {
            self.level = Some(level);
            self.structured = true;
        }
    }
}

/// Removes ANSI color sequences and control characters other than tabs.
pub(super) fn clean(line: &str) -> Cow<'_, str> {
    if !line.bytes().any(|b| b < 0x20 && b != b'\t' || b == 0x7f) {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ params final; OSC: ESC ] ... BEL/ST; other: ESC + one char.
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' || n == '\u{1b}' {
                            if n == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if c == '\t' || !(c.is_control()) {
            out.push(c);
        } else if c != '\0' && c != '\r' && c != '\n' {
            out.push(' ');
        }
    }
    Cow::Owned(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Bracket,
    Paren,
    Angle,
    Word,
}

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    kind: Kind,
    /// Content without its brackets.
    inner: &'a str,
    raw: &'a str,
    start: usize,
    end: usize,
}

fn token(s: &str, from: usize) -> Option<Token<'_>> {
    let b = s.as_bytes();
    let mut i = from;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let group = |open: u8, close: u8, limit: usize, kind: Kind| -> Option<Token> {
        if b[i] != open {
            return None;
        }
        let end = b[i + 1..].iter().take(limit).position(|&c| c == close)? + i + 1;
        Some(Token { kind, inner: &s[i + 1..end], raw: &s[i..=end], start: i, end: end + 1 })
    };
    if let Some(t) = group(b'[', b']', 160, Kind::Bracket)
        .or_else(|| group(b'(', b')', 120, Kind::Paren))
        .or_else(|| group(b'<', b'>', 40, Kind::Angle).filter(|t| !t.inner.contains(' ')))
    {
        return Some(t);
    }
    if b[i] == b'|' {
        return Some(Token { kind: Kind::Word, inner: "|", raw: "|", start: i, end: i + 1 });
    }
    let mut j = i;
    while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'|') {
        j += 1;
    }
    Some(Token { kind: Kind::Word, inner: &s[i..j], raw: &s[i..j], start: i, end: j })
}

fn separator(text: &str) -> bool {
    matches!(text, "-" | "--" | ":" | "|" | "::" | "->" | "=>" | "»" | "—" | "–" | ">" | "-:" | "*")
}

fn filler(text: &str) -> bool {
    matches!(text, "---" | "," | ";" | "..." | "~")
}

/// Bracketed or parenthesized header items.
fn group(inner: &str, kind: Kind, p: &mut Parts) {
    let t = inner.trim();
    if t.is_empty() {
        return;
    }
    if let Some(level) = level_word(t).or_else(|| letter_level(t)) {
        if !(kind == Kind::Paren && letter_level(t).is_some()) {
            p.level(level);
            return;
        }
    }
    if t.bytes().all(|b| b.is_ascii_digit()) && t.len() <= 10 {
        if !p.fields.contains_key("pid") {
            p.set("pid", t);
        } else {
            p.set("tid", t);
        }
        return;
    }
    if let Some(rest) = t.strip_prefix('#').filter(|r| r.bytes().all(|b| b.is_ascii_digit()) && !r.is_empty()) {
        p.set("pid", rest);
        return;
    }
    // Apache: "pid 1234:tid 5678", "client 1.2.3.4:5678", "core:error".
    if let Some(rest) = t.strip_prefix("pid ").or_else(|| t.strip_prefix("pid:")).or_else(|| t.strip_prefix("pid=")) {
        let (pid, tid) = rest.split_once(":tid ").unwrap_or((rest, ""));
        p.set("pid", pid.trim());
        p.set("tid", tid.trim());
        return;
    }
    for prefix in ["client ", "remote "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            match ip_port(rest.trim()) {
                Some((ip, port)) => {
                    p.set("client_ip", ip);
                    p.set("client_port", port);
                }
                None => p.set(if is_ip(rest.trim()) { "client_ip" } else { "client" }, rest.trim()),
            }
            return;
        }
    }
    if code_like(t) {
        p.code.get_or_insert_with(|| t.to_string());
        p.set("code", t);
        return;
    }
    if logger_like(t) {
        if !p.fields.contains_key("logger") {
            p.set("logger", t);
            return;
        }
    }
    if let Some((module, level)) = t.split_once(':') {
        if let Some(level) = level_word(level).filter(|_| !module.contains(' ')) {
            p.level(level);
            p.set("module", module);
            return;
        }
    }
    if let Some((file, line)) = t.rsplit_once(':') {
        if file.contains('.') && !file.contains(' ') && !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) && !is_ip(file) {
            p.set("caller", t);
            return;
        }
    }
    let pairs = super::web_logs::key_values(t);
    if let Some(pairs) = pairs.filter(|pairs| pairs.len() >= 2) {
        for (key, value) in pairs {
            p.set(key, value);
        }
        return;
    }
    if !p.fields.contains_key("thread") {
        p.set("thread", t);
    } else {
        p.push("context", t);
    }
}

/// Header items after (or around) the timestamp; returns where the message
/// starts. After a separator only a level continues the header, as in
/// Python's `name - LEVEL - message`.
fn header(s: &str, p: &mut Parts, first_after_stamp: bool) -> usize {
    let mut pos = 0;
    let mut accepted = 0usize;
    let mut after_separator = false;
    let mut first = first_after_stamp;
    let mut previous_level = false;
    loop {
        let Some(t) = token(s, pos) else { return s.len() };
        if accepted >= 8 {
            return t.start;
        }
        if t.kind == Kind::Word && separator(t.raw) {
            // A pipe delimits every field (NLog, Serilog layouts); other
            // separators end the header unless a level follows.
            if accepted > 0 && t.raw != "|" {
                after_separator = true;
            }
            pos = t.end;
            continue;
        }
        if t.kind == Kind::Word && filler(t.raw) {
            pos = t.end;
            continue;
        }
        let wide = s[t.end..].starts_with("  ") || s[t.end..].starts_with('\t');
        let next = token(s, t.end);
        let level = match t.kind {
            Kind::Bracket | Kind::Angle => level_word(t.inner).or_else(|| letter_level(t.inner.trim())),
            Kind::Paren => level_word(t.inner),
            Kind::Word => {
                let word = t.raw.trim_end_matches(':');
                let enclosed = t.raw.ends_with(':') && !word.contains(':');
                let upper = word.bytes().any(|b| b.is_ascii_alphabetic()) && !word.bytes().any(|b| b.is_ascii_lowercase());
                // Column-aligned layouts (tabs or padding) mark the level in any case: "info<TAB>", "Info   ".
                let title = first && wide;
                let next_is_component = next.is_some_and(|n| {
                    n.kind == Kind::Bracket || (n.raw.len() >= 2 && n.raw.bytes().all(|b| b.is_ascii_uppercase()))
                });
                if enclosed || upper || title {
                    level_word(word).or_else(|| (first && next_is_component).then(|| letter_level(word)).flatten())
                } else {
                    None
                }
            }
        };
        if let Some(level) = level {
            p.level(level);
            accepted += 1;
            first = false;
            after_separator = false;
            previous_level = true;
            pos = t.end;
            if t.kind == Kind::Word && t.raw.ends_with(':') {
                return skip_space(s, t.end);
            }
            continue;
        }
        if after_separator {
            return t.start;
        }
        match t.kind {
            Kind::Bracket | Kind::Paren | Kind::Angle => {
                if t.kind == Kind::Paren && accepted == 0 && t.inner.contains(' ') && !thread_like(t.inner) {
                    // "(prose in parentheses) message": not a header item.
                    return t.start;
                }
                group(t.inner, t.kind, p);
            }
            Kind::Word => {
                let word = t.raw;
                if word.contains('=') || word.starts_with(['{', '"', '\'']) {
                    return t.start;
                }
                // Python's default "LEVEL:logger:message".
                if let Some((name, rest)) = word.split_once(':') {
                    if name.bytes().all(|b| b.is_ascii_uppercase()) {
                        if let (Some(level), Some((logger, _))) = (level_word(name), rest.split_once(':')) {
                            p.level(level);
                            p.set("logger", logger);
                            return t.start + name.len() + 1 + logger.len() + 1;
                        }
                    }
                }
                if code_like(word) && word.ends_with(':') {
                    p.code.get_or_insert_with(|| word.trim_end_matches(':').to_string());
                    p.structured = true;
                    return t.start;
                }
                // An address in the header may be the client or the host: neutral name.
                if let Some((ip, port)) = ip_port(word).or_else(|| is_ip(word).then_some((word, ""))) {
                    p.set("ip", ip);
                    p.set("port", port);
                    accepted += 1;
                    pos = t.end;
                    first = false;
                    previous_level = false;
                    continue;
                }
                if let Some((name, pid, terminal)) = process_token(word) {
                    if pid.is_some() || terminal {
                        if logger_like(name) {
                            p.set("logger", name);
                        } else if pid.is_some() {
                            p.set("process", name);
                        } else {
                            p.set("component", name);
                        }
                        if let Some(pid) = pid {
                            p.set("pid", pid);
                        }
                        if terminal {
                            return skip_space(s, t.end);
                        }
                        accepted += 1;
                        pos = t.end;
                        first = false;
                        previous_level = false;
                        continue;
                    }
                }
                let bare = word.trim_end_matches([']', ')']);
                if bare.len() <= 10 && !bare.is_empty() && bare.bytes().all(|b| b.is_ascii_digit()) {
                    if !p.fields.contains_key("pid") {
                        p.set("pid", bare);
                    } else if !p.fields.contains_key("tid") {
                        p.set("tid", bare);
                    } else {
                        return t.start;
                    }
                } else if let Some(pid) = bare.strip_prefix('#').filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())) {
                    p.set("pid", pid);
                } else if caller_like(word) {
                    p.set("caller", word.trim_end_matches(':'));
                } else if logger_like(word) && word.len() > 3 {
                    p.set("logger", word.trim_end_matches(':'));
                    if word.ends_with(':') {
                        return skip_space(s, t.end);
                    }
                } else if previous_level && word.len() >= 2 && word.len() <= 16 && word.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
                    p.set("component", word);
                } else {
                    return t.start;
                }
            }
        }
        accepted += 1;
        first = false;
        previous_level = false;
        pos = t.end;
    }
}

fn skip_space(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

/// Items written before the timestamp: Rails `I, [`, Redis `1234:M`, levels,
/// addresses. Returns true for Redis' `pid:role` prefix.
fn before_stamp(before: &str, p: &mut Parts) -> bool {
    let mut redis = false;
    for word in before.split_whitespace() {
        let w = word.trim_matches(|c: char| matches!(c, '[' | '(' | '<' | ',' | ']' | ')' | '>'));
        if w.is_empty() {
            continue;
        }
        if let Some(level) = letter_level(w).filter(|_| word.ends_with(',')).or_else(|| level_word(w)) {
            p.level(level);
        } else if let Some((pid, role)) = w.split_once(':').filter(|(pid, role)| {
            !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) && matches!(*role, "M" | "S" | "C" | "X")
        }) {
            p.set("pid", pid);
            p.set("role", role);
            redis = true;
        } else if is_ip(w) {
            p.set("ip", w);
        } else if w.bytes().all(|b| b.is_ascii_digit()) && w.len() <= 10 {
            p.set("pid", w);
        } else if w.contains('.') || w.contains('-') {
            p.set("host", w);
        }
    }
    redis
}

const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "HEAD", "OPTIONS", "PATCH", "CONNECT", "TRACE", "PROPFIND", "PROPPATCH", "MKCOL",
    "COPY", "MOVE", "LOCK", "UNLOCK", "REPORT", "SEARCH", "PURGE", "MERGE", "SUBSCRIBE", "NOTIFY",
];

struct Request<'a> {
    method: &'a str,
    target: &'a str,
    protocol: Option<&'a str>,
    end: usize,
}

fn protocol(word: &str) -> bool {
    let Some(version) = word.strip_prefix("HTTP/") else { return false };
    let mut parts = version.split('.');
    parts.next().is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none()
}

/// `METHOD target HTTP/x.y` (quoted or not) anywhere in a message, or a known
/// method followed by a path or URL.
fn http_request(s: &str) -> Option<Request<'_>> {
    let limit = s.len().min(4096);
    let words: Vec<(usize, &str)> = {
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < limit && out.len() < 96 {
            while i < limit && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i > start {
                out.push((start, &s[start..i]));
            }
        }
        out
    };
    for (k, &(_, word)) in words.iter().enumerate() {
        let method = word.trim_start_matches(['"', '\'']);
        let known = METHODS.contains(&method);
        if !known && !(method.len() >= 3 && method.len() <= 12 && method.bytes().all(|b| b.is_ascii_uppercase())) {
            continue;
        }
        let Some(&(target_start, target)) = words.get(k + 1) else { continue };
        let quote = word.starts_with('"');
        let target_clean = target.trim_end_matches('"');
        let looks_like_target = target_clean.starts_with('/')
            || target_clean.starts_with("http://")
            || target_clean.starts_with("https://")
            || target_clean == "*"
            || (method == "CONNECT" && target_clean.contains(':'));
        if !looks_like_target {
            continue;
        }
        let after_target = target_start + target.len();
        match words.get(k + 2) {
            Some(&(p_start, word)) if protocol(word.trim_end_matches(['"', ','])) && !target.ends_with('"') => {
                let proto = word.trim_end_matches(['"', ',']);
                let mut end = p_start + proto.len();
                if quote && s[end..].starts_with('"') {
                    end += 1;
                }
                return Some(Request { method, target: target_clean, protocol: Some(proto), end });
            }
            _ if known => {
                return Some(Request { method, target: target_clean, protocol: None, end: after_target });
            }
            _ => continue,
        }
    }
    None
}

fn http(message: &str, line: &str, p: &mut Parts) -> bool {
    // Cheap guards keep the word scan off ordinary messages.
    if !(message.contains("HTTP/") || message.contains(" /") || message.contains("\"/") || message.contains(" http")) {
        return false;
    }
    let Some(request) = http_request(message) else { return false };
    let text = match request.protocol {
        Some(protocol) => format!("{} {} {}", request.method, request.target, protocol),
        None => format!("{} {}", request.method, request.target),
    };
    p.set("request", &text);
    p.set("method", request.method);
    p.set("path", request.target);
    if let Some(protocol) = request.protocol {
        p.set("protocol", protocol);
    }
    // Status and size right after the request, as in access-log layouts.
    let mut after = message[request.end..].split_ascii_whitespace();
    let status = after.next().map(|w| w.trim_matches(['"', ',']));
    if let Some(status) = status.filter(|s| s.len() == 3 && s.parse::<u16>().is_ok_and(|n| (100..600).contains(&n))) {
        p.set("status", status);
        p.code.get_or_insert_with(|| status.to_string());
        if let Some(size) = after.next().filter(|s| *s == "-" || s.bytes().all(|b| b.is_ascii_digit())) {
            p.set("size", size);
        }
    }
    // The client is the first address before the request.
    if !p.fields.contains_key("client_ip") {
        let head = line.find(request.method).map_or(line, |i| &line[..i]);
        if let Some(ip) = head.split(|c: char| c.is_whitespace() || matches!(c, '"' | ',' | '[' | ']' | '(' | ')')).find_map(|w| {
            if is_ip(w) {
                Some(w)
            } else {
                ip_port(w).map(|(ip, _)| ip)
            }
        }) {
            p.set("client_ip", ip);
            p.http_client = Some(ip.to_string());
        }
    }
    true
}

/// `key=value` pairs anywhere in free text: `user=bob`, `src=1.2.3.4,`,
/// `msg="quoted value"`. Quoted segments and URLs are skipped, so query strings
/// and prose in quotes never produce pairs. Returns the pairs and whether the
/// text consisted of nothing else.
pub(super) fn loose_pairs(s: &str) -> (Vec<(&str, String)>, bool) {
    let b = s.as_bytes();
    let mut pairs = Vec::new();
    let mut only_pairs = true;
    let mut i = 0;
    let key_start = |c: u8| c.is_ascii_alphabetic() || c == b'_' || c == b'@';
    let key_char = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'@');
    while i < b.len() && pairs.len() < 64 {
        while i < b.len() && (b[i].is_ascii_whitespace() || matches!(b[i], b',' | b';')) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if matches!(b[i], b'"' | b'\'') {
            let quote = b[i];
            i += 1;
            while i < b.len() && b[i] != quote {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            only_pairs = false;
            continue;
        }
        let mut j = i;
        if key_start(b[j]) {
            j += 1;
            while j < b.len() && key_char(b[j]) && j - i < 64 {
                j += 1;
            }
        }
        if j > i && b.get(j) == Some(&b'=') && b.get(j + 1) != Some(&b'=') {
            let key = &s[i..j];
            let start = j + 1;
            let (value, next) = match b.get(start) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let mut k = start + 1;
                    let mut value = String::new();
                    let mut closed = false;
                    while k < b.len() {
                        if b[k] == b'\\' && quote == b'"' && k + 1 < b.len() {
                            value.push_str(&s[k..k + 2]);
                            k += 2;
                            continue;
                        }
                        if b[k] == quote {
                            closed = true;
                            break;
                        }
                        let ch = s[k..].chars().next().unwrap_or(' ');
                        value.push(ch);
                        k += ch.len_utf8();
                    }
                    (value, if closed { k + 1 } else { k })
                }
                _ => {
                    let mut k = start;
                    while k < b.len() && !b[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    let mut value = &s[start..k];
                    if !value.contains('(') {
                        value = value.trim_end_matches(')');
                    }
                    if !value.contains('[') {
                        value = value.trim_end_matches(']');
                    }
                    (value.trim_end_matches([',', ';']).to_string(), k)
                }
            };
            pairs.push((key, value));
            i = next;
            continue;
        }
        only_pairs = false;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    (pairs, only_pairs)
}

/// Pairs found in a message become fields; a level or time they name explicitly
/// is used, and when the message is nothing but pairs its code, source and
/// message keys fill the standard columns too.
fn pairs(message: &str, p: &mut Parts) -> Option<String> {
    if !message.contains('=') {
        return None;
    }
    let (pairs, only) = loose_pairs(message);
    if pairs.is_empty() || (pairs.len() < 2 && !only) {
        return None;
    }
    let mut found = Map::new();
    for (key, value) in &pairs {
        match found.get_mut(*key) {
            None => {
                found.insert(key.to_string(), Value::from(value.as_str()));
            }
            Some(Value::Array(values)) => values.push(Value::from(value.as_str())),
            Some(existing) => *existing = Value::Array(vec![existing.clone(), Value::from(value.as_str())]),
        }
    }
    let single = |aliases: &[&str]| {
        let mut matches = pairs.iter().filter(|(key, _)| aliases.iter().any(|a| key.eq_ignore_ascii_case(a)));
        let value = matches.next().map(|(_, v)| v.clone());
        if matches.next().is_some() { None } else { value }
    };
    if let Some(level) = single(&["level", "lvl", "severity", "loglevel", "log_level"]) {
        if let Some(label) = crate::model::label_class(&super::normalize_level(&level)).map(crate::model::class_label) {
            p.level(label);
        }
    }
    if p.timestamp.is_none() {
        if let Some(ts) = single(&["ts", "time", "timestamp", "@timestamp", "datetime", "date_time", "eventtime", "logtime"]) {
            p.timestamp = super::text_to_ms(&ts).or_else(|| stamp::exact(&ts));
        }
        if p.timestamp.is_none() {
            if let (Some(date), Some(time)) = (single(&["date"]), single(&["time"])) {
                p.timestamp = stamp::exact(&format!("{date} {time}"));
            }
        }
    }
    if only && p.code.is_none() {
        p.code = single(&["code", "event_id", "eventid", "error_code", "errorcode", "logid"]);
    }
    if only && p.source.is_none() {
        p.source = single(&["source", "host", "hostname", "app", "service", "devname", "logger"]);
    }
    for (key, value) in found {
        if !p.fields.contains_key(&key) {
            p.fields.insert(key, value);
        }
    }
    p.structured = true;
    let message = single(&["msg", "message", "logdesc", "description"]);
    if only { message } else { None }
}

/// A JSON object inside the message (`... {"user":"bob"}`): its members become
/// fields (explicit ones win), and its time/level/message fill what is missing.
fn embedded_json(message: &str, p: &mut Parts) -> Option<String> {
    let start = message.find('{')?;
    if start > 2048 || message.len() - start > 256 * 1024 {
        return None;
    }
    let candidate = message[start..].trim_end();
    let end = candidate.rfind('}')?;
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&candidate[..=end]) else { return None };
    if object.is_empty() {
        return None;
    }
    let mut flat = Map::new();
    super::flatten_json(object, "", 0, &mut flat);
    let text = |keys: &[&str]| {
        keys.iter().find_map(|k| flat.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()))
    };
    if p.timestamp.is_none() {
        p.timestamp = text(&["@timestamp", "timestamp", "time", "ts", "@t", "datetime"]).and_then(|v| super::value_to_ms(&v));
    }
    if let Some(Value::String(level)) = text(&["level", "severity", "lvl", "loglevel", "levelname", "@l"]) {
        if let Some(label) = crate::model::label_class(&super::normalize_level(&level)).map(crate::model::class_label) {
            p.level(label);
        }
    }
    let inner_message = text(&["message", "msg", "@m", "short_message"]).and_then(|v| v.as_str().map(str::to_string));
    for (key, value) in flat.into_iter().take(512) {
        if key.len() <= 512 && !p.fields.contains_key(&key) {
            p.fields.insert(key, value);
        }
    }
    p.structured = true;
    if message[..start].trim().is_empty() { inner_message } else { None }
}

/// Cisco and similar `%FACILITY-SEVERITY-MNEMONIC:` tags.
fn mnemonic(message: &str, p: &mut Parts) {
    let Some(start) = message.find('%') else { return };
    if start > 160 {
        return;
    }
    let tag: &str = message[start + 1..].split([':', ' ']).next().unwrap_or("");
    let parts: Vec<&str> = tag.split('-').collect();
    if parts.len() < 3 || tag.len() > 64 {
        return;
    }
    let Some(severity_index) = parts.iter().position(|part| part.len() == 1 && part.as_bytes()[0].is_ascii_digit()) else { return };
    if severity_index == 0 || severity_index + 1 >= parts.len() || !parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')) {
        return;
    }
    let severity = u32::from(parts[severity_index].as_bytes()[0] - b'0');
    p.set("mnemonic", tag);
    p.set("facility", &parts[..severity_index].join("-"));
    p.set("severity", parts[severity_index]);
    p.code.get_or_insert_with(|| tag.to_string());
    p.level(severity_level(severity));
}

/// Level words in the message when nothing structural named one.
fn level_in_text(message: &str) -> Option<&'static str> {
    let head = &message[..message.len().min(160)];
    let mut start = None;
    for (i, c) in head.char_indices().chain(std::iter::once((head.len(), ' '))) {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
            continue;
        }
        let Some(s) = start.take() else { continue };
        let word = &head[s..i];
        let upper = word.bytes().all(|b| b.is_ascii_uppercase());
        let labeled = c == ':' || (s > 0 && matches!(head.as_bytes()[s - 1], b'[' | b'<'));
        if upper || labeled {
            if let Some(level) = level_word(word) {
                return Some(level);
            }
        }
    }
    if message.contains("Traceback (most recent call last)") || message.contains("Exception") && message.contains(['.', ':']) {
        return Some("Erro");
    }
    None
}

/// Message-borne structure for events read by dedicated parsers: vendor
/// mnemonics, HTTP requests, `key=value` pairs and JSON. Existing fields, code
/// and timestamp win; `level_known` keeps a level the record stated itself.
pub(super) fn enrich(ev: &mut Event, message: &str, level_known: bool) {
    if message.is_empty() || message.len() > 64 * 1024 {
        return;
    }
    let mut p = Parts { timestamp: ev.timestamp, ..Parts::default() };
    mnemonic(message, &mut p);
    let http_found = http(message, message, &mut p);
    pairs(message, &mut p);
    embedded_json(message, &mut p);
    for (key, value) in p.fields {
        ev.fields.entry(key).or_insert(value);
    }
    if ev.timestamp.is_none() {
        ev.timestamp = p.timestamp;
    }
    if ev.code.is_empty() {
        if let Some(code) = p.code {
            ev.code = code;
        }
    }
    if !level_known {
        let status = || {
            ev.fields.get("status").and_then(Value::as_str).filter(|_| http_found).map(|status| match status.as_bytes()[0] {
                b'5' => "Erro",
                b'4' => "Aviso",
                _ => "Informação",
            })
        };
        if let Some(level) = p.level.or_else(status) {
            ev.level = level.into();
        }
    }
}

/// Container runtime prefix after the timestamp: `stdout F message`.
fn cri(after: &str) -> Option<(&str, &str, &str)> {
    let rest = after.trim_start();
    let (stream, rest) = rest.split_once(' ')?;
    if stream != "stdout" && stream != "stderr" {
        return None;
    }
    let (tag, message) = rest.split_once(' ').unwrap_or((rest, ""));
    matches!(tag, "F" | "P").then_some((stream, tag, message))
}

fn merge(outer: &mut Parts, inner: Event) {
    if outer.level.is_none() && inner.parse_status == "parsed" {
        outer.level = crate::model::label_class(&inner.level).map(crate::model::class_label);
    }
    if outer.code.is_none() && !inner.code.is_empty() {
        outer.code = Some(inner.code.clone());
    }
    if outer.source.is_none() && !inner.source.is_empty() {
        outer.source = Some(inner.source.clone());
    }
    if outer.timestamp.is_none() {
        outer.timestamp = inner.timestamp;
    }
    for (key, value) in inner.fields {
        if key != "parser.format" {
            outer.fields.entry(key).or_insert(value);
        }
    }
    if !inner.message.is_empty() {
        outer.message = Some(inner.message);
    }
    outer.structured = true;
}

/// Lines whose whole layout is known.
fn families(line: &str, ctx: &Context) -> Option<Event> {
    klog(line, ctx).or_else(|| logcat(line, ctx)).or_else(|| squid(line)).or_else(|| apache_error(line)).or_else(|| aws_load_balancer(line))
}

fn regex(pattern: &'static str, cell: &'static std::sync::OnceLock<regex::Regex>) -> &'static regex::Regex {
    cell.get_or_init(|| regex::Regex::new(pattern).expect("generic pattern"))
}

fn event(line: &str, format: &str) -> Event {
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.fields.insert("parser.format".into(), Value::from(format));
    ev
}

fn put(ev: &mut Event, key: &str, value: &str) {
    if !value.is_empty() && value != "-" {
        ev.fields.entry(key.to_string()).or_insert_with(|| Value::from(value));
    }
}

/// Kubernetes/glog: `I1012 00:00:00.123456    1234 main.go:42] message`.
fn klog(line: &str, ctx: &Context) -> Option<Event> {
    let b = line.as_bytes();
    if b.len() < 22 || !matches!(b[0], b'I' | b'W' | b'E' | b'F') || !b[1..5].iter().all(u8::is_ascii_digit) || b[5] != b' ' {
        return None;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let c = regex(r"^([IWEF])(\d{2})(\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+(\d+) ([^\s\]]+:\d+)\] ?(.*)$", &RE).captures(line)?;
    let ts = stamp::at(&format!("{}-{} {}", &c[2], &c[3], &c[4]), 0, ctx)?;
    let mut ev = event(line, "klog");
    ev.timestamp = Some(ts.ms);
    ev.level = letter_level(&c[1])?.into();
    put(&mut ev, "pid", &c[5]);
    put(&mut ev, "caller", &c[6]);
    ev.source = c[6].split(':').next().unwrap_or("").to_string();
    ev.message = c[7].to_string();
    Some(ev)
}

/// Android logcat (`threadtime` and `time` layouts).
fn logcat(line: &str, ctx: &Context) -> Option<Event> {
    let b = line.as_bytes();
    if b.len() < 20 || !(b[2] == b'-' && b[5] == b' ' && b[8] == b':') {
        return None;
    }
    static THREADTIME: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static TIME: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let (stamp_text, pid, tid, letter, tag, message) = if let Some(c) =
        regex(r"^(\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3})\s+(\d+)\s+(\d+) ([VDIWEFA]) (.*?)\s*: (.*)$", &THREADTIME).captures(line)
    {
        (c.get(1)?, Some(c.get(2)?), Some(c.get(3)?), c.get(4)?, c.get(5)?, c.get(6)?)
    } else {
        let c = regex(r"^(\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}) ([VDIWEFA])/(.*?)\(\s*(\d+)\): (.*)$", &TIME).captures(line)?;
        (c.get(1)?, Some(c.get(4)?), None, c.get(2)?, c.get(3)?, c.get(5)?)
    };
    let ts = stamp::at(stamp_text.as_str(), 0, ctx)?;
    let mut ev = event(line, "logcat");
    ev.timestamp = Some(ts.ms);
    ev.level = letter_level(letter.as_str())?.into();
    ev.source = tag.as_str().trim().to_string();
    put(&mut ev, "tag", tag.as_str().trim());
    if let Some(pid) = pid {
        put(&mut ev, "pid", pid.as_str());
    }
    if let Some(tid) = tid {
        put(&mut ev, "tid", tid.as_str());
    }
    ev.message = message.as_str().to_string();
    Some(ev)
}

/// Squid native access log.
fn squid(line: &str) -> Option<Event> {
    let b = line.as_bytes();
    if b.len() < 40 || !b[..10].iter().all(u8::is_ascii_digit) || b.get(10) != Some(&b'.') {
        return None;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let c = regex(
        r"^(\d{9,10}\.\d{3})\s+(\d+) (\S+) ([A-Z_]+)/(\d{3}) (\d+) (\S+) (\S+) (\S+) ([A-Z_]+)/(\S+) ?(\S*)",
        &RE,
    )
    .captures(line)?;
    let mut ev = event(line, "squid");
    ev.timestamp = super::text_to_ms(&c[1]);
    ev.code = c[5].to_string();
    ev.level = match c[5].as_bytes()[0] {
        b'5' => "Erro",
        b'4' => "Aviso",
        _ => "Informação",
    }
    .into();
    ev.source = c[3].to_string();
    for (key, index) in [("duration_ms", 2), ("client_ip", 3), ("cache_result", 4), ("status", 5), ("size", 6), ("method", 7), ("url", 8), ("user", 9), ("hierarchy", 10), ("peer", 11), ("content_type", 12)] {
        put(&mut ev, key, &c[index]);
    }
    ev.message = format!("{} {} → {}", &c[7], &c[8], &c[5]);
    Some(ev)
}

pub(super) fn apache_error_line(line: &str) -> bool {
    apache_error(line).is_some()
}

/// Apache httpd error log (2.2 and 2.4 layouts).
pub(super) fn apache_error(line: &str) -> Option<Event> {
    if !line.starts_with('[') || !line.contains("] [") {
        return None;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let c = regex(
        r"^\[([^\]]+)\] \[(?:([\w.\-]+):)?(emerg|alert|crit|error|warn|notice|info|debug|trace\d)\] (?:\[pid (\d+)(?::tid (\d+))?\] )?(?:\[(?:client|remote) ([^\]]+)\] )?(.*)$",
        &RE,
    )
    .captures(line)?;
    let timestamp = stamp::exact(&c[1])?;
    let mut ev = event(line, "apache.error");
    ev.timestamp = Some(timestamp);
    ev.level = super::normalize_level(if c[3].starts_with("trace") { "trace" } else { &c[3] });
    ev.source = c.get(2).map_or("apache", |m| m.as_str()).to_string();
    if let Some(module) = c.get(2) {
        put(&mut ev, "module", module.as_str());
    }
    for (key, index) in [("pid", 4), ("tid", 5)] {
        if let Some(value) = c.get(index) {
            put(&mut ev, key, value.as_str());
        }
    }
    if let Some(client) = c.get(6) {
        match ip_port(client.as_str()) {
            Some((ip, port)) => {
                put(&mut ev, "client_ip", ip);
                put(&mut ev, "client_port", port);
            }
            None => put(&mut ev, if is_ip(client.as_str()) { "client_ip" } else { "client" }, client.as_str()),
        }
    }
    let message = c[7].to_string();
    if let Some(code) = message.split(':').next().filter(|code| code_like(code)) {
        ev.code = code.to_string();
    }
    ev.message = message;
    Some(ev)
}

/// AWS Application and Classic Load Balancer access logs.
fn aws_load_balancer(line: &str) -> Option<Event> {
    if !line.contains("Z ") || !line.contains(" \"") {
        return None;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let c = regex(
        r#"^(?:(https?|h2|grpcs?|wss?) )?(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z) (\S+) (\S+) (\S+) (-?[\d.]+) (-?[\d.]+) (-?[\d.]+) (\d{3}|-) (\d{3}|-) (\d+) (\d+) "([^"]*)" "([^"]*)" ?(.*)$"#,
        &RE,
    )
    .captures(line)?;
    let mut ev = event(line, "aws.lb");
    ev.timestamp = super::parse_timestamp(&c[2]);
    if let Some(kind) = c.get(1) {
        put(&mut ev, "lb.type", kind.as_str());
    }
    put(&mut ev, "elb", &c[3]);
    match ip_port(&c[4]) {
        Some((ip, port)) => {
            put(&mut ev, "client_ip", ip);
            put(&mut ev, "client_port", port);
            ev.source = ip.to_string();
        }
        None => put(&mut ev, "client", &c[4]),
    }
    put(&mut ev, "target", &c[5]);
    for (key, index) in [("request_processing_time", 6), ("target_processing_time", 7), ("response_processing_time", 8), ("status", 9), ("target_status", 10), ("received_bytes", 11), ("sent_bytes", 12), ("request", 13), ("agent", 14)] {
        put(&mut ev, key, &c[index]);
    }
    if let Some((method, rest)) = c[13].split_once(' ') {
        let (target, protocol) = rest.rsplit_once(' ').unwrap_or((rest, ""));
        put(&mut ev, "method", method);
        put(&mut ev, "path", target);
        put(&mut ev, "protocol", protocol);
    }
    let tail = c[15].trim();
    if !tail.is_empty() {
        put(&mut ev, "lb.extra", tail);
    }
    ev.code = c[9].to_string();
    ev.level = match c[9].as_bytes()[0] {
        b'5' => "Erro",
        b'4' => "Aviso",
        _ => "Informação",
    }
    .into();
    ev.message = format!("{} → {}", &c[13], &c[9]);
    Some(ev)
}

fn split_block(text: &str) -> (&str, &str) {
    match text.find('\n') {
        Some(nl) => (text[..nl].trim_end_matches('\r'), &text[nl + 1..]),
        None => (text.trim_end_matches('\r'), ""),
    }
}

/// Structure of any record (a line or a multi-line block).
pub(super) fn parse(text: &str, ctx: &Context) -> Event {
    let mut ev = parse_depth(text, ctx, 0);
    // Header and comment lines (`# Nmap 7.94 scan initiated…`, `#Comando que gerou: …`) stay records,
    // so line numbers and offsets do not move, but they say what they are and can be filtered out.
    if text.trim_start().starts_with('#') {
        ev.fields.entry("comentario").or_insert(Value::Bool(true));
    }
    ev
}

fn parse_depth(text: &str, ctx: &Context, depth: usize) -> Event {
    let (first, body) = split_block(text);
    let cleaned = clean(first);
    let line = cleaned.trim();
    if let Some(mut ev) = families(line, ctx) {
        ev.raw = text.to_string();
        return ev;
    }
    let mut p = Parts::default();
    let mut rest = line;
    // RFC 3164/5424 priority without a recognized syslog header.
    if let Some(after) = rest.strip_prefix('<') {
        if let Some((pri, tail)) = after.split_once('>') {
            if let Ok(pri) = pri.parse::<u32>().map_err(|_| ()).and_then(|v| if v <= 191 && !pri.is_empty() && pri.len() <= 3 { Ok(v) } else { Err(()) }) {
                p.set("syslog.facility", &(pri / 8).to_string());
                p.set("syslog.severity", &(pri % 8).to_string());
                p.level(severity_level(pri % 8));
                rest = tail.trim_start();
            }
        }
    }
    let mut message = rest;
    let mut message_offset_in_line = None;
    match stamp::find(rest, ctx) {
        Some(found) => {
            p.timestamp = Some(found.ms);
            p.structured = true;
            if found.inferred == Inferred::Date {
                if let Some(date) = ctx.reference {
                    p.set("data_inferida", &date.to_string());
                }
            }
            let before = rest[..found.start].trim();
            if before.split_whitespace().count() <= 3 && !before.contains(['=', '"']) {
                let mut after = &rest[found.end..];
                if let Some(open) = before.chars().last().filter(|c| matches!(c, '[' | '(' | '<')) {
                    let close = match open {
                        '[' => ']',
                        '(' => ')',
                        _ => '>',
                    };
                    if let Some(position) = after.find(close).filter(|&i| i <= 24 && !after[..i].contains(['[', '('])) {
                        let between = after[..position].trim();
                        if !between.is_empty() {
                            group(between, Kind::Bracket, &mut p);
                        }
                        after = &after[position + 1..];
                    }
                }
                let redis = before_stamp(before, &mut p);
                if redis {
                    let trimmed = after.trim_start();
                    let level = match trimmed.chars().next() {
                        Some('.') => Some("Depuração"),
                        Some('-') => Some("Rastreio"),
                        Some('*') => Some("Informação"),
                        Some('#') => Some("Aviso"),
                        _ => None,
                    };
                    if let Some(level) = level {
                        p.level(level);
                        after = &trimmed[1..];
                    }
                }
                if let Some((stream, tag, inner)) = cri(after) {
                    p.set("stream", stream);
                    p.set("logtag", tag);
                    let inner_event = if depth < 2 { Some(parse_depth(inner, ctx, depth + 1)) } else { None };
                    if let Some(inner_event) = inner_event.filter(|e| e.parse_status == "parsed") {
                        merge(&mut p, inner_event);
                        p.timestamp = Some(found.ms);
                        let mut ev = finish(p, text, inner);
                        ev.raw = text.to_string();
                        return ev;
                    }
                    message = inner;
                } else {
                    let start = header(after, &mut p, true);
                    message = after[start..].trim();
                    // A record wrapped by another prefix (kubectl --timestamps).
                    if depth < 2 && stamp::at(message, 0, ctx).is_some_and(|f| f.has_time) {
                        let inner = parse_depth(message, ctx, depth + 1);
                        if inner.parse_status == "parsed" {
                            let outer = p.timestamp;
                            merge(&mut p, inner);
                            p.timestamp = outer;
                        }
                    }
                }
            }
        }
        None => {
            let start = header(rest, &mut p, true);
            message = rest[start..].trim();
        }
    }
    if let Some(offset) = (message.as_ptr() as usize).checked_sub(line.as_ptr() as usize).filter(|o| *o <= line.len()) {
        message_offset_in_line = Some(offset);
    }
    mnemonic(message, &mut p);
    let http_found = http(message, line, &mut p);
    let mut replaced = pairs(message, &mut p);
    if let Some(json_message) = embedded_json(message, &mut p) {
        replaced.get_or_insert(json_message);
    }
    if p.level.is_none() {
        if let Some(level) = level_in_text(message) {
            p.level = Some(level);
        } else if let Some(status) = p.fields.get("status").and_then(Value::as_str).filter(|_| http_found) {
            p.level = Some(match status.as_bytes()[0] {
                b'5' => "Erro",
                b'4' => "Aviso",
                _ => "Informação",
            });
        }
    }
    if http_found && p.source.is_none() {
        p.source = p.http_client.clone();
    }
    let mut final_message = replaced.unwrap_or_else(|| message.to_string());
    // A timestamp alone on its line heads the lines below it (Oracle alert log).
    if final_message.is_empty() {
        final_message = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(line).to_string();
    }
    // Multi-line records: Java/Python traces and java.util.logging's level line.
    if !body.trim().is_empty() {
        let second = body.lines().next().unwrap_or("").trim();
        if p.level.is_none() || p.fields.get("logger").is_some() && message.split_whitespace().count() <= 1 {
            for name in ["SEVERE", "WARNING", "INFO", "CONFIG", "FINE", "FINER", "FINEST"] {
                if let Some(rest) = second.strip_prefix(name).and_then(|r| r.strip_prefix(':')) {
                    if let Some(level) = level_word(name) {
                        p.level = Some(level);
                    }
                    if !message.is_empty() {
                        p.set("logger.method", message);
                    }
                    final_message = rest.trim().to_string();
                    p.structured = true;
                    break;
                }
            }
        }
        if let Some(exception) = body.lines().rev().map(str::trim).find(|l| python_exception(l)) {
            if body.contains("Traceback (most recent call last)") || text.contains("Traceback (most recent call last)") {
                p.set("exception", exception);
                p.level.get_or_insert("Erro");
            }
        }
    }
    let mut ev = finish(p, text, &final_message);
    if !body.trim().is_empty() && body.lines().any(|l| l.trim_start().starts_with("at ")) {
        super::extract_java_body(&mut ev, body);
        if let Some(offset) = message_offset_in_line.filter(|_| matches!(cleaned, Cow::Borrowed(_))) {
            let start = (line.as_ptr() as usize - first.as_ptr() as usize) + offset;
            super::enrich_java_trace(&mut ev, text, start);
        }
    }
    ev
}

/// `ValueError: message`, `requests.exceptions.HTTPError: ...`.
fn python_exception(line: &str) -> bool {
    let name = line.split(':').next().unwrap_or("");
    !name.contains(' ')
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && ["Error", "Exception", "Warning", "Exit", "Interrupt"].iter().any(|suffix| name.ends_with(suffix))
}

fn finish(p: Parts, raw: &str, message: &str) -> Event {
    let mut ev = Event::empty();
    ev.raw = raw.to_string();
    ev.timestamp = p.timestamp;
    ev.level = p.level.unwrap_or("Informação").to_string();
    ev.code = p.code.unwrap_or_default();
    let field = |key: &str| p.fields.get(key).and_then(Value::as_str).map(str::to_string);
    ev.source = p
        .source
        .or_else(|| field("logger"))
        .or_else(|| field("process"))
        .or_else(|| field("component"))
        .or_else(|| field("host"))
        .unwrap_or_default();
    ev.message = p.message.unwrap_or_else(|| message.to_string());
    ev.fields = p.fields;
    if p.structured {
        ev.fields.entry("parser.format").or_insert_with(|| Value::from(FORMAT));
        ev.parse_status = "parsed".into();
    } else {
        ev.parse_status = "text".into();
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn ctx() -> Context {
        Context { year: 2026, reference: NaiveDate::from_ymd_opt(2026, 10, 14), day_first: None }
    }
    fn parse_line(line: &str) -> Event {
        parse(line, &ctx())
    }
    fn field<'a>(ev: &'a Event, key: &str) -> &'a str {
        ev.fields.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("{key} missing in {:?}", ev.fields))
    }

    #[test]
    fn headers_of_common_application_layouts() {
        let ev = parse_line("2026-10-12 00:00:00.123  INFO 1234 --- [nio-8080-exec-1] c.e.d.Controller         : Request handled");
        assert_eq!((ev.level.as_str(), ev.message.as_str()), ("Informação", "Request handled"));
        assert_eq!((field(&ev, "pid"), field(&ev, "thread"), field(&ev, "logger")), ("1234", "nio-8080-exec-1", "c.e.d.Controller"));
        assert!(ev.timestamp.is_some());

        let ev = parse_line("2026-10-12 00:00:00,123 - myapp.module - WARNING - Disk almost full");
        assert_eq!((ev.level.as_str(), ev.message.as_str(), field(&ev, "logger")), ("Aviso", "Disk almost full", "myapp.module"));

        let ev = parse_line("[2026-10-12T00:00:00,123][INFO ][o.e.n.Node               ] [node-1] started");
        assert_eq!((ev.level.as_str(), field(&ev, "logger"), field(&ev, "thread"), ev.message.as_str()), ("Informação", "o.e.n.Node", "node-1", "started"));

        let ev = parse_line("12-Oct-2026 00:00:00.123 SEVERE [main] org.apache.catalina.core.StandardContext.startInternal One or more listeners failed");
        assert_eq!((ev.level.as_str(), field(&ev, "thread"), ev.message.as_str()), ("Erro", "main", "One or more listeners failed"));

        let ev = parse_line("[2026-10-12 00:00:00 -0300] [1234] [INFO] Starting gunicorn 21.2.0");
        assert_eq!((ev.level.as_str(), field(&ev, "pid"), ev.message.as_str()), ("Informação", "1234", "Starting gunicorn 21.2.0"));

        let ev = parse_line("2026-10-12 00:00:00.123 -03:00 [ERR] Unhandled exception in request");
        assert_eq!((ev.level.as_str(), ev.message.as_str()), ("Erro", "Unhandled exception in request"));

        let ev = parse_line("2026-10-12 00:00:00.1234|WARN|MyApp.Services.Mailer|Retrying delivery");
        assert_eq!((ev.level.as_str(), field(&ev, "logger"), ev.message.as_str()), ("Aviso", "MyApp.Services.Mailer", "Retrying delivery"));

        let ev = parse_line("I, [2026-10-12T00:00:00.123456 #1234]  INFO -- : Completed 200 OK in 5ms");
        assert_eq!((ev.level.as_str(), field(&ev, "pid"), ev.message.as_str()), ("Informação", "1234", "Completed 200 OK in 5ms"));

        let ev = parse_line("2026-10-12 00:00:00.123 -03 [4321] LOG:  database system is ready to accept connections");
        assert_eq!((ev.level.as_str(), field(&ev, "pid"), ev.message.as_str()), ("Informação", "4321", "database system is ready to accept connections"));
        let ev = parse_line("2026-10-12 00:00:00.123 -03 [4321] ERROR:  relation \"x\" does not exist");
        assert_eq!(ev.level, "Erro");

        let ev = parse_line("2026-10-12T00:00:00.123456Z 0 [System] [MY-010116] [Server] /usr/sbin/mysqld starting");
        assert_eq!((ev.code.as_str(), ev.message.as_str()), ("MY-010116", "/usr/sbin/mysqld starting"));

        let ev = parse_line("1234:M 12 Oct 2026 00:00:00.123 # WARNING overcommit_memory is set to 0");
        assert_eq!((ev.level.as_str(), field(&ev, "pid"), field(&ev, "role")), ("Aviso", "1234", "M"));

        let ev = parse_line("2026-10-12T00:00:00.123+0000 I NETWORK  [conn123] end connection 10.0.0.1:5000");
        assert_eq!((ev.level.as_str(), field(&ev, "component"), field(&ev, "thread"), ev.message.as_str()), ("Informação", "NETWORK", "conn123", "end connection 10.0.0.1:5000"));

        let ev = parse_line("2026-10-12 00:00:00, Info                  CBS    Loaded Servicing Stack v10.0");
        assert_eq!((ev.level.as_str(), field(&ev, "component")), ("Informação", "CBS"));

        let ev = parse_line("INFO:root:Connected to broker");
        assert_eq!((ev.level.as_str(), field(&ev, "logger"), ev.message.as_str()), ("Informação", "root", "Connected to broker"));
        assert!(ev.timestamp.is_none());
        assert_eq!(ev.parse_status, "parsed");
    }

    #[test]
    fn messages_keep_their_words_and_yield_pairs_requests_and_json() {
        let ev = parse_line("2026-10-12 10:00:00 Error connecting to db host=db01 port=5432");
        assert_eq!(ev.message, "Error connecting to db host=db01 port=5432");
        assert_eq!((field(&ev, "host"), field(&ev, "port")), ("db01", "5432"));

        let ev = parse_line("something happened a=1 b=2 c=3");
        assert_eq!((ev.parse_status.as_str(), field(&ev, "a"), field(&ev, "c")), ("parsed", "1", "3"));
        assert!(ev.timestamp.is_none());

        let ev = parse_line("visit https://example.test/?a=1&b=2&c=3");
        assert_eq!(ev.parse_status, "text");
        assert!(ev.fields.is_empty());

        let ev = parse_line("level=info msg=\"not finished");
        assert_eq!((ev.level.as_str(), field(&ev, "msg")), ("Informação", "not finished"));

        let ev = parse_line("2026-10-12T00:00:00Z stdout F {\"level\":\"error\",\"msg\":\"boom\",\"user\":{\"id\":7}}");
        assert_eq!((ev.level.as_str(), ev.message.as_str(), field(&ev, "stream")), ("Erro", "boom", "stdout"));
        assert_eq!(ev.fields["user.id"], 7);

        let ev = parse_line("2026-10-12T03:00:00.123Z app-1 handled {\"status\":\"ok\",\"elapsed\":12}");
        assert_eq!((field(&ev, "status"), ev.fields["elapsed"].as_i64()), ("ok", Some(12)));

        let ev = parse_line("Oct 12 2026 00:00:00: %ASA-4-106023: Deny tcp src outside:10.1.1.1/1234 dst inside:10.2.2.2/80");
        assert_eq!((ev.code.as_str(), ev.level.as_str(), field(&ev, "facility")), ("ASA-4-106023", "Aviso", "ASA"));

        let ev = parse_line("2026-10-12T00:00:00.000Z [Envoy] \"GET /api/v1/items?x=1 HTTP/1.1\" 503 UF 0 91 2 - \"10.0.0.7\" \"curl/8.0\"");
        assert_eq!((field(&ev, "method"), field(&ev, "path"), field(&ev, "status"), ev.level.as_str()), ("GET", "/api/v1/items?x=1", "503", "Erro"));
    }

    #[test]
    fn whole_layouts_klog_logcat_squid_apache_error_and_load_balancers() {
        let ev = parse_line("E1012 00:00:00.123456    4321 controller.go:114] error syncing pod");
        assert_eq!((ev.level.as_str(), field(&ev, "caller"), ev.message.as_str()), ("Erro", "controller.go:114", "error syncing pod"));
        let ev = parse_line("10-12 00:00:00.123  1234  5678 W ActivityManager: Slow operation");
        assert_eq!((ev.level.as_str(), ev.source.as_str(), field(&ev, "tid")), ("Aviso", "ActivityManager", "5678"));
        let ev = parse_line("1286536309.586    921 192.168.0.68 TCP_MISS/200 507 POST http://www.example.com/x - DIRECT/1.2.3.4 text/html");
        assert_eq!((ev.timestamp, field(&ev, "method"), field(&ev, "cache_result"), ev.code.as_str()), (Some(1286536309586), "POST", "TCP_MISS", "200"));
        let ev = parse_line("[Mon Oct 12 00:00:00.123456 2026] [core:error] [pid 1234:tid 5678] [client 192.0.2.1:5000] AH00126: Invalid URI in request");
        assert_eq!((ev.level.as_str(), field(&ev, "module"), field(&ev, "tid"), field(&ev, "client_ip"), ev.code.as_str()), ("Erro", "core", "5678", "192.0.2.1", "AH00126"));
        let ev = parse_line("[Mon Oct 12 00:00:00 2026] [error] [client 192.0.2.1] File does not exist: /var/www/favicon.ico");
        assert_eq!((ev.level.as_str(), field(&ev, "client_ip")), ("Erro", "192.0.2.1"));
        let ev = parse_line(r#"https 2026-10-12T00:00:00.123456Z app/my-lb/50dc6c495c0c9188 192.0.2.5:4321 10.0.0.1:80 0.000 0.001 0.000 502 502 34 366 "GET https://example.test:443/ HTTP/1.1" "curl/7.46.0" ECDHE-RSA-AES128-GCM-SHA256 TLSv1.2"#);
        assert_eq!((field(&ev, "client_ip"), field(&ev, "status"), field(&ev, "method"), ev.level.as_str()), ("192.0.2.5", "502", "GET", "Erro"));
    }

    #[test]
    fn multi_line_blocks_and_inferred_dates() {
        let ev = parse_line("Oct 12, 2026 12:00:00 AM org.apache.catalina.startup.Catalina start\nINFO: Server startup in 1234 ms");
        assert_eq!((ev.level.as_str(), ev.message.as_str(), field(&ev, "logger")), ("Informação", "Server startup in 1234 ms", "org.apache.catalina.startup.Catalina"));
        let ev = parse_line("2026-10-12 00:00:00,123 ERROR Failed\nTraceback (most recent call last):\n  File \"x.py\", line 1, in <module>\nValueError: bad value");
        assert_eq!((ev.level.as_str(), field(&ev, "exception")), ("Erro", "ValueError: bad value"));
        let ev = parse_line("2026-10-12 00:00:00.123 ERROR 1 --- [main] o.s.boot.App : Boom\njava.lang.IllegalStateException: x\n\tat a.B.c(B.java:1)");
        assert_eq!(ev.fields["exception"], "java.lang.IllegalStateException: x");
        let ev = parse_line("00:00:00,001 WARN  [a.b.C] (default task-1) late");
        assert_eq!((ev.level.as_str(), field(&ev, "data_inferida")), ("Aviso", "2026-10-14"));
    }

    #[test]
    fn ansi_colors_and_control_characters_are_ignored_for_structure() {
        let raw = "\u{1b}[32m2026-10-12 00:00:00\u{1b}[0m \u{1b}[1;31mERROR\u{1b}[0m failed\0";
        let ev = parse_line(raw);
        assert_eq!((ev.level.as_str(), ev.message.as_str(), ev.raw.as_str()), ("Erro", "failed", raw));
    }
}
