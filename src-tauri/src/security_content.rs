//! Contextual file-disclosure signals. A request is not a response or execution.
use crate::{
    evidence::Excerpt,
    model::Event,
    security_normalize::{self as norm, Normalized},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;
#[path = "security_text.rs"]
mod original;
#[path = "security_web.rs"]
mod web;
pub(crate) use web::infer_request;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signal {
    pub key: String,
    pub excerpt: Excerpt,
}
struct Patterns {
    path: Regex,
    read: Regex,
    passwd: Regex,
    shadow: Regex,
    pem: Regex,
    env: Regex,
}
fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        path: Regex::new(r#"(?i)(?:/(?:etc/(?:passwd|shadows?|gshadow)|proc/(?:self|[0-9]+)/environ)(?:[/?&#\s'\"<>]|$)|(?:/|\\|^)(?:\.env(?:[._-][a-z0-9_-]+)*|wp-config\.php(?:\.(?:bak|old|save))?|\.aws/credentials|(?:\.ssh/)?id_(?:rsa|ed25519|ecdsa)(?:_[0-9]+)?|\.(?:bash_history|zsh_history|mysql_history|pgpass|esmtprc|htpasswds?)|(?:mysql|database|db|backup|dump|mysqldump)(?:[_-][a-z0-9]+)?\.(?:sql(?:\.gz)?|ini)|\.git/config|(?:config|settings|application)(?:[._-](?:dev|prod|production|local|development|test|backup))*\.(?:yml|yaml|ini|json|php))(?:[/?&#\s'\"<>]|$)|(?:windows|winnt)[/\\]system32[/\\]config[/\\](?:sam|security|system)(?:[/?&#\s'\"<>]|$))"#).unwrap(),
        read: Regex::new(r#"(?:^|[;&|=\n]\s*|\s-c\s+["']?)(?:sudo\s+)?(?:/usr/bin/|/bin/)?(?:cat|head|tail|less|more|strings)\s+(?:-[a-zA-Z0-9]+\s+)*["']?(?:/etc/(?:passwd|shadows?|gshadow)|/proc/(?:self|[0-9]+)/environ|(?:/[^\s;|]+)?/\.(?:aws/credentials|ssh/id_(?:rsa|ed25519|ecdsa)))(?:\b|$)"#).unwrap(),
        passwd: Regex::new(r"(?m)^[a-zA-Z_][a-zA-Z0-9_.-]*:[x*!]?:[0-9]{1,10}:[0-9]{1,10}:[^:\r\n]*:/[^:\r\n]*:(?:/[^:\r\n]*)?\r?$").unwrap(),
        shadow: Regex::new(r"(?m)^[a-zA-Z_][a-zA-Z0-9_.-]*:!?(?:\$1\$[./A-Za-z0-9]{1,8}\$[./A-Za-z0-9]{22}|\$5\$(?:rounds=[0-9]+\$)?[./A-Za-z0-9]{1,16}\$[./A-Za-z0-9]{43}|\$6\$(?:rounds=[0-9]+\$)?[./A-Za-z0-9]{1,16}\$[./A-Za-z0-9]{86}|\$y\$[./A-Za-z0-9]+\$[./A-Za-z0-9]+\$[./A-Za-z0-9]{43}|\$2[aby]\$[0-9]{2}\$[./A-Za-z0-9]{53}):[0-9]*:[0-9]*:[0-9]*:[0-9]*:[0-9]*:[0-9]*:[0-9]*\r?$").unwrap(),
        pem: Regex::new(r"(?m)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----\r?\n(?:[A-Za-z0-9+/=]{16,}\r?\n){2,}-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----").unwrap(),
        env: Regex::new(r"(?m)^(?:AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|DB_PASSWORD|DATABASE_PASSWORD|DB_HOST|DATABASE_HOST)\s*=\s*[^\r\n]+$").unwrap(),
    })
}
fn bounded(text: &str) -> &str {
    let mut end = text.len().min(65536);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn add(
    ev: &Event,
    out: &mut Vec<Signal>,
    key: &str,
    field: &str,
    transform: &str,
    value: &str,
    a: usize,
    b: usize,
) {
    if out.len() >= 24
        || out
            .iter()
            .any(|s| s.key == format!("_sec.content.{key}") && s.excerpt.field == field)
    {
        return;
    }
    let original = if field == "message" {
        Some(ev.message.as_str())
    } else if field == "raw" {
        Some(ev.raw.as_str())
    } else {
        text(ev, field)
    };
    let excerpt = if transform.starts_with("URI extraída") {
        original.and_then(|raw| {
            raw.find(value).and_then(|offset| {
                Excerpt::new(
                    &norm::event_ref(ev),
                    field,
                    "original",
                    raw,
                    offset + a,
                    offset + b,
                )
            })
        })
    } else {
        None
    }
    .or_else(|| Excerpt::new(&norm::event_ref(ev), field, transform, value, a, b));
    if let Some(excerpt) = excerpt {
        out.push(Signal {
            key: format!("_sec.content.{key}"),
            excerpt,
        });
    }
}
fn text<'a>(ev: &'a Event, key: &str) -> Option<&'a str> {
    norm::field_value(ev, key)?.as_str()
}
fn kind(path: &str) -> &str {
    if path.contains("/etc/passwd") {
        "passwd"
    } else if path.contains("/etc/shadow") || path.contains("/etc/gshadow") {
        "shadow"
    } else if path.contains("id_rsa") || path.contains("id_ed25519") || path.contains("id_ecdsa") {
        "private_key"
    } else if path.contains(".env") || path.contains("credentials") || path.contains("environ") {
        "secrets"
    } else {
        "config"
    }
}
pub fn apply(ev: &Event, derived: &mut Event, n: &mut Normalized) {
    if derived.fields.contains_key("_sec.literal_output") {
        return;
    }
    let p = patterns();
    let mut signals = Vec::new();
    web::apply(ev, derived, n, &mut signals);
    if derived.fields.contains_key("_sec.literal_output") {
        return;
    }
    let mut requests = Vec::new();
    let mut fields: Vec<(String, String, bool, String)> = Vec::new();
    for role in ["url", "request_command", "request_body"] {
        if let Some(v) = n.values.get(role) {
            fields.push((v.field.clone(), v.value.clone(), true, v.method.clone()));
        }
    }
    for field in [
        "url.original",
        "url.full",
        "url.path",
        "http.request.uri",
        "request.uri",
        "request_uri",
        "uri",
        "http.request.body.content",
        "request.body",
        "http.request.body",
    ] {
        if let Some(value) = text(ev, field) {
            if !fields.iter().any(|(k, ..)| k == field) {
                fields.push((field.into(), value.into(), true, "original".into()));
            }
        }
    }
    let (segments, clipped) = original::segments(ev);
    n.content_clipped |= clipped;
    for segment in segments {
        if fields.iter().any(|(k, ..)| k == &segment.field) {
            continue;
        }
        if !web::can_infer_request(&segment.field, n) {
            continue;
        }
        if let Some(uri) = web::request_uri(segment.value) {
            fields.push((
                segment.field,
                uri.into(),
                true,
                "URI extraída de registro HTTP".into(),
            ));
        }
    }
    if let Some(v) = n.values.get("command") {
        fields.push((v.field.clone(), v.value.clone(), false, v.method.clone()));
    }
    for (field, raw, request, method) in fields {
        n.content_clipped |= raw.len() > 65536;
        let original = bounded(&raw);
        let mut value = original.to_string();
        if request {
            for _ in 0..2 {
                value = crate::threats::percent_decode(&value);
            }
        }
        let method = method.as_str();
        let transform = if method.contains("decod")
            || value == original && method.starts_with("URI extraída")
        {
            method
        } else if value == original {
            "original"
        } else {
            "percent-decode (até 2 passagens)"
        };
        let transformed = if method.starts_with("URI extraída") && value != original {
            format!("{method}; {transform}")
        } else {
            transform.to_string()
        };
        let transform = transformed.as_str();
        if request {
            // Only a URI/body field participates; documentation and arbitrary messages do not.
            if let Some(m) = p.path.find(&value) {
                let lower = m.as_str().to_ascii_lowercase();
                let k = kind(&lower);
                requests.push((
                    k.to_string(),
                    field.clone(),
                    transform.to_string(),
                    value.clone(),
                    m.start(),
                    m.end(),
                ));
                add(
                    ev,
                    &mut signals,
                    if lower.contains(".env")
                        || lower.contains("wp-config")
                        || lower.contains(".git/config")
                        || ["/config.", "/settings.", "/application."]
                            .iter()
                            .any(|v| lower.contains(v))
                    {
                        "ambiguous_request"
                    } else if lower.contains(".sql") || lower.contains(".ini") {
                        "backup_request"
                    } else {
                        "sensitive_request"
                    },
                    &field,
                    transform,
                    &value,
                    m.start(),
                    m.end(),
                );
                add(
                    ev,
                    &mut signals,
                    &format!("{k}_request"),
                    &field,
                    transform,
                    &value,
                    m.start(),
                    m.end(),
                );
                if value.contains("../")
                    || value.contains("..\\")
                    || value.contains("file://")
                    || value.contains("php://filter")
                {
                    add(
                        ev,
                        &mut signals,
                        "traversal_request",
                        &field,
                        transform,
                        &value,
                        0,
                        value.len(),
                    );
                }
            }
        }
        if let Some(m) = p.read.find(&value) {
            add(
                ev,
                &mut signals,
                if request {
                    "read_request"
                } else {
                    "sensitive_read"
                },
                &field,
                transform,
                &value,
                m.start(),
                m.end(),
            );
            if !request
                && matches!(n.get("action"), Some("process_start"))
                && [
                    "process.parent.name",
                    "ParentImage",
                    "process.parent.executable",
                ]
                .iter()
                .filter_map(|f| text(ev, f))
                .any(|v| {
                    let name = v
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(v)
                        .to_ascii_lowercase();
                    ["nginx", "apache2", "httpd", "php-fpm", "w3wp.exe"].contains(&name.as_str())
                })
            {
                add(
                    ev,
                    &mut signals,
                    "web_process_read",
                    &field,
                    transform,
                    &value,
                    m.start(),
                    m.end(),
                );
            }
        }
    }
    let denied = matches!(n.get("outcome"), Some("blocked" | "failure"))
        || ["http.response.status_code", "status", "status_code"]
            .iter()
            .filter_map(|f| norm::field_value(ev, f))
            .any(|v| {
                v.as_u64()
                    .or_else(|| v.as_str()?.parse().ok())
                    .is_some_and(|v| v >= 400)
            });
    let mut response_fields = vec![
        "http.response.body.content".to_string(),
        "http.response.body".into(),
        "response.body".into(),
        "response_body".into(),
    ];
    if let Some(p) = n.values.get("response_body") {
        if !response_fields.contains(&p.field) {
            response_fields.push(p.field.clone());
        }
    }
    for field in response_fields {
        let Some(raw) = text(ev, &field) else {
            continue;
        };
        let body = bounded(raw);
        n.content_clipped |= raw.len() > 65536;
        // Reflected submitted content cannot corroborate disclosure. No HTML/example snippets.
        let reflected = n
            .get("request_body")
            .is_some_and(|r| !r.trim().is_empty() && body.contains(r.trim()))
            || [
                "http.request.body.content",
                "http.request.body",
                "request.body",
            ]
            .iter()
            .filter_map(|f| text(ev, f))
            .any(|r| !r.trim().is_empty() && body.contains(r.trim()));
        let lower = body.to_ascii_lowercase();
        if reflected
            || denied
            || lower.contains("<html")
            || lower.contains("<pre")
            || lower.contains("<code")
            || body.contains("```")
        {
            continue;
        }
        let rows: Vec<_> = p.passwd.find_iter(body).take(3).collect();
        let passwd = rows.len() >= 2 && rows.iter().any(|m| m.as_str().starts_with("root:"));
        let shadow = p.shadow.find(body);
        let pem = p.pem.find(body).filter(|m| {
            let s = m.as_str();
            let begin = s.lines().next().unwrap_or("").replace("BEGIN", "END");
            s.lines().last() == Some(begin.as_str())
        });
        let env: Vec<_> = p.env.find_iter(body).collect();
        let valid_env = env.iter().all(|m| {
            let v = m
                .as_str()
                .split_once('=')
                .unwrap()
                .1
                .trim_matches([' ', '\'', '"']);
            v.len() >= 8
                && ![
                    "example",
                    "changeme",
                    "redacted",
                    "placeholder",
                    "your_",
                    "${",
                ]
                .iter()
                .any(|s| v.to_ascii_lowercase().contains(s))
        });
        let has = |key: &str| env.iter().any(|m| m.as_str().starts_with(key));
        let secrets = valid_env
            && ((has("AWS_ACCESS_KEY_ID") && has("AWS_SECRET_ACCESS_KEY"))
                || ((has("DB_PASSWORD") || has("DATABASE_PASSWORD"))
                    && (has("DB_HOST") || has("DATABASE_HOST"))));
        for (k, hit) in [
            ("passwd", if passwd { rows.first().copied() } else { None }),
            ("shadow", shadow),
            ("private_key", pem),
            ("secrets", if secrets { env.first().copied() } else { None }),
        ] {
            let Some(hit) = hit else { continue };
            add(
                ev,
                &mut signals,
                &format!("{k}_response"),
                &field,
                "original",
                body,
                hit.start(),
                hit.end(),
            );
            if let Some((_, rf, rt, rv, ra, rb)) = requests.iter().find(|(rk, ..)| rk == k) {
                add(
                    ev,
                    &mut signals,
                    &format!("{k}_disclosure"),
                    &field,
                    "original",
                    body,
                    hit.start(),
                    hit.end(),
                );
                add(
                    ev,
                    &mut signals,
                    &format!("{k}_disclosure"),
                    rf,
                    rt,
                    rv,
                    *ra,
                    *rb,
                );
            }
        }
    }
    n.content_clipped |= signals.len() >= 24;
    web::probe_context(ev, derived, n, &mut signals);
    for signal in &signals {
        derived
            .fields
            .entry(signal.key.clone())
            .or_insert(Value::from("true"));
    }
    n.signals = signals;
    if n.content_clipped {
        n.limitations.push(
            "Inspeção textual limitada: 64 KiB por campo; varredura genérica de até 256 KiB, 128 segmentos e 12 níveis por evento"
                .into(),
        );
    }
}
