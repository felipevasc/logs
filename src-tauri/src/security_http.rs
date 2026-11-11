//! HTTP facts derived from any web log: status, response size, method, path,
//! server-side scripts, upload directories, attack payloads and session tokens.
//! They only add `_sec.http.*`, `_sec.url.path`, `_sec.web.*` fields; nothing
//! existing is changed, so every other format keeps its interpretation.
use crate::model::Event;
use crate::security_normalize::{self as norm, Normalized};
use serde_json::Value;

const STATUS_FIELDS: &[&str] = &[
    "http.response.status_code",
    "status",
    "status_code",
    "statusCode",
    "response_code",
    "response.status",
    "sc-status",
    "sc_status",
];
const BYTES_FIELDS: &[&str] = &[
    "http.response.body.bytes",
    "http.response.bytes",
    "size",
    "bytes",
    "body_bytes_sent",
    "bytes_sent",
    "response_size",
    "response.bytes",
    "sc-bytes",
    "sc_bytes",
];
const METHOD_FIELDS: &[&str] = &[
    "http.request.method",
    "method",
    "request_method",
    "cs-method",
    "cs_method",
    "http_method",
];
const PATH_FIELDS: &[&str] = &["url.path", "path", "cs-uri-stem", "cs_uri_stem", "uri_path"];
const URL_FIELDS: &[&str] = &["url.original", "url.full", "request_uri", "uri", "http.url", "url"];

/// Server-side script extensions: a request to one of these runs code on the server.
const SCRIPT_EXTENSIONS: &[&str] = &[
    "php", "php3", "php4", "php5", "php7", "php8", "phtml", "pht", "phar", "jsp", "jspx", "jspf", "asp",
    "aspx", "ashx", "asmx", "cer", "cfm", "cfml", "cgi", "pl",
];
/// Directories that normally hold uploaded or static content, never server code.
const UPLOAD_DIRECTORIES: &[&str] = &[
    "upload", "uploads", "uploaded", "userfiles", "user_uploads", "files", "file", "images", "image", "img",
    "media", "tmp", "temp", "attachments", "attachment", "documents", "avatars", "assets", "static", "storage",
    "pictures", "photos",
];
/// Session identifiers recorded by web servers, frameworks and token decoders.
const SESSION_FIELDS: &[&str] = &[
    "session_id",
    "sessionid",
    "sessionId",
    "session.id",
    "jsessionid",
    "phpsessid",
    "token.payload.sid",
    "token.payload.jti",
    "authorization.payload.sid",
    "authorization.payload.jti",
    "http.request.session",
];
const SESSION_COOKIES: &[&str] = &[
    "phpsessid",
    "jsessionid",
    "asp.net_sessionid",
    "aspsessionid",
    "connect.sid",
    "sessionid",
    "session",
    "sid",
    "laravel_session",
    "_session_id",
];
const COOKIE_FIELDS: &[&str] = &["http.request.cookie", "cookie", "cookies", "http.cookie", "cs(cookie)"];
/// Content signals that describe a request carrying an attack payload.
const ATTACK_SIGNALS: &[&str] = &[
    "_sec.content.sensitive_request",
    "_sec.content.traversal_request",
    "_sec.content.read_request",
    "_sec.content.ambiguous_request",
    "_sec.content.backup_request",
    "_sec.content.xxe_request",
    "_sec.content.xss_request",
    "_sec.content.sqli_request",
    "_sec.suspicious_request",
    "_sec.reverse_shell_request",
];

fn text(ev: &Event, field: &str) -> Option<String> {
    match norm::field_value(ev, field)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty() && s != "-"),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn first(ev: &Event, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|f| text(ev, f))
}

pub(crate) fn status(ev: &Event) -> Option<u16> {
    STATUS_FIELDS
        .iter()
        .filter_map(|f| text(ev, f))
        .find_map(|v| v.parse::<u16>().ok().filter(|s| (100..=599).contains(s)))
}

/// Request line `GET /path?x HTTP/1.1` recorded as one field.
fn request_line(ev: &Event) -> Option<(String, String)> {
    let line = text(ev, "request")?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method.len() <= 16 && method.chars().all(|c| c.is_ascii_alphabetic()) && (target.starts_with('/') || target.contains("://")))
        .then(|| (method.to_ascii_uppercase(), target.to_string()))
}

/// Path of a URL or request target, without query, fragment or `;param`.
pub(crate) fn path_of(target: &str) -> Option<String> {
    let mut rest = target.trim();
    if let Some(index) = rest.find("://") {
        rest = &rest[index + 3..];
        rest = &rest[rest.find('/')?..];
    }
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = &rest[..end];
    let path = path.split(';').next().unwrap_or(path);
    (path.starts_with('/') && path.len() <= 2048).then(|| path.to_string())
}

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (_, extension) = name.rsplit_once('.')?;
    Some(extension.to_ascii_lowercase())
}

pub(crate) fn is_script(path: &str) -> bool {
    extension(&crate::threats::percent_decode(path)).is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e.as_str()))
}

pub(crate) fn in_upload_directory(path: &str) -> bool {
    let decoded = crate::threats::percent_decode(path).to_ascii_lowercase();
    let mut segments: Vec<&str> = decoded.split('/').filter(|s| !s.is_empty()).collect();
    segments.pop();
    segments.iter().any(|s| UPLOAD_DIRECTORIES.contains(s))
}

fn session(ev: &Event) -> Option<String> {
    if let Some(value) = SESSION_FIELDS.iter().find_map(|f| text(ev, f)) {
        return Some(value);
    }
    let cookie = first(ev, COOKIE_FIELDS)?;
    cookie.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        let value = value.trim();
        (SESSION_COOKIES.contains(&name.trim().to_ascii_lowercase().as_str()) && value.len() >= 8 && value != "deleted")
            .then(|| value.to_string())
    })
}

/// Adds the HTTP facts of one event. Values come from fields only; no text is guessed.
pub(crate) fn annotate(ev: &Event, derived: &mut Event, n: &Normalized) {
    let attack = ATTACK_SIGNALS
        .iter()
        .any(|key| derived.fields.get(*key).and_then(Value::as_str) == Some("true"));
    let mut put = |key: &str, value: String| {
        derived.fields.insert(key.to_string(), Value::from(value));
    };
    let line = request_line(ev);
    let status = status(ev);
    if let Some(status) = status {
        put("_sec.http.status", status.to_string());
        put("_sec.http.status_class", format!("{}xx", status / 100));
    }
    if let Some(bytes) = BYTES_FIELDS
        .iter()
        .filter_map(|f| text(ev, f))
        .find_map(|v| v.parse::<u64>().ok())
    {
        put("_sec.http.bytes", bytes.to_string());
    }
    let method = first(ev, METHOD_FIELDS)
        .filter(|m| m.len() <= 16 && m.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|m| m.to_ascii_uppercase())
        .or_else(|| line.as_ref().map(|(m, _)| m.clone()));
    if let Some(method) = method {
        put("_sec.http.method", method);
    }
    let path = first(ev, PATH_FIELDS)
        .and_then(|p| path_of(&p))
        .or_else(|| first(ev, URL_FIELDS).and_then(|u| path_of(&u)))
        .or_else(|| n.get("url").and_then(path_of))
        .or_else(|| line.as_ref().and_then(|(_, t)| path_of(t)));
    let web = status.is_some() || line.is_some() || n.product == "web";
    if let Some(path) = path.filter(|_| web) {
        if is_script(&path) {
            put("_sec.web.script", "true".into());
        }
        if in_upload_directory(&path) {
            put("_sec.web.upload_dir", "true".into());
        }
        put("_sec.url.path", path);
    }
    if attack {
        put("_sec.web.attack", "true".into());
    }
    if let Some(token) = session(ev).filter(|v| v.len() <= 1024) {
        let namespace = n.get("namespace").unwrap_or("");
        put("_sec.web.session", format!("{namespace}\u{1f}{token}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(fields: serde_json::Value) -> Event {
        let mut ev = Event::empty();
        ev.fields = fields.as_object().unwrap().clone().into_iter().collect();
        ev
    }

    #[test]
    fn access_log_fields_become_http_facts() {
        let ev = event(serde_json::json!({"request":"POST /uploads/2026/x.php?cmd=id HTTP/1.1","status":"200","size":"5120"}));
        let (derived, _) = crate::security_normalize::normalize(&ev, &[]);
        assert_eq!(derived.fields["_sec.http.status"], "200");
        assert_eq!(derived.fields["_sec.http.status_class"], "2xx");
        assert_eq!(derived.fields["_sec.http.bytes"], "5120");
        assert_eq!(derived.fields["_sec.http.method"], "POST");
        assert_eq!(derived.fields["_sec.url.path"], "/uploads/2026/x.php");
        assert_eq!(derived.fields["_sec.web.script"], "true");
        assert_eq!(derived.fields["_sec.web.upload_dir"], "true");
    }

    #[test]
    fn non_web_events_and_ordinary_paths_get_no_web_flags() {
        let (derived, _) = crate::security_normalize::normalize(&event(serde_json::json!({"path":"/var/log/x.php"})), &[]);
        assert!(!derived.fields.contains_key("_sec.web.script"));
        let (derived, _) = crate::security_normalize::normalize(&event(serde_json::json!({"request":"GET /index.php HTTP/1.1","status":404})), &[]);
        assert_eq!(derived.fields["_sec.web.script"], "true");
        assert!(!derived.fields.contains_key("_sec.web.upload_dir"));
        assert_eq!(derived.fields["_sec.http.status_class"], "4xx");
        let (derived, _) = crate::security_normalize::normalize(&event(serde_json::json!({"request":"GET /img/logo.png HTTP/1.1","status":200,"size":"-"})), &[]);
        assert!(!derived.fields.contains_key("_sec.web.script"));
        assert!(!derived.fields.contains_key("_sec.http.bytes"));
    }

    #[test]
    fn session_tokens_come_from_fields_or_session_cookies_only() {
        let ev = event(serde_json::json!({"request":"GET / HTTP/1.1","status":200,"cookie":"theme=dark; PHPSESSID=abcdef123456"}));
        let (derived, _) = crate::security_normalize::normalize(&ev, &[]);
        assert!(derived.fields["_sec.web.session"].as_str().unwrap().ends_with("abcdef123456"));
        let ev = event(serde_json::json!({"request":"GET / HTTP/1.1","status":200,"cookie":"theme=dark"}));
        let (derived, _) = crate::security_normalize::normalize(&ev, &[]);
        assert!(!derived.fields.contains_key("_sec.web.session"));
    }

    #[test]
    fn paths_drop_query_fragment_and_matrix_parameters() {
        assert_eq!(path_of("https://a.example/x/y.jsp;jsessionid=1?z#w").as_deref(), Some("/x/y.jsp"));
        assert_eq!(path_of("/a%2Fb.php?x").as_deref(), Some("/a%2Fb.php"));
        assert!(path_of("relative/x.php").is_none());
        assert!(is_script("/shell.PhP"));
        assert!(!is_script("/download.php.txt"));
        assert!(in_upload_directory("/wp-content/uploads/2026/a.php"));
        assert!(!in_upload_directory("/uploads"));
    }
}
