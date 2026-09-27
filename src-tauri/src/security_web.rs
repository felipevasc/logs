//! Request syntax in application logs and specific web payloads. No execution inference.
use super::{add, bounded, Signal};
use crate::{
    model::Event,
    security_normalize::{self as norm, Normalized},
};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

struct Patterns {
    uri: Regex,
    access: Regex,
    short_access: Regex,
    traversal: Regex,
    xxe: Regex,
    entity_denied: Regex,
    xss: Regex,
    sql: Regex,
    sql_boolean: Regex,
}
fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        uri: Regex::new(r#"(?i)\b(?:HTTP request with URI|request URI|request URL)\s*\[([^\r\n]+)\]"#).unwrap(),
        access: Regex::new(r#"(?i)(?:^|[\s"])(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+([^\r\n]+?)\s+HTTP/[123](?:\.[0-9])?\b"#).unwrap(),
        short_access: Regex::new(r"(?i)^\s*(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+(/[^\r\n]+)$").unwrap(),
        traversal: Regex::new(r"(?:\.\.[/\\]){2,}[^\s<>]*").unwrap(),
        xxe: Regex::new(r#"(?is)<!ENTITY\s+(?:%\s*)?[A-Za-z_][\w.-]*\s+(?:SYSTEM\s+|PUBLIC\s+["'][^"']*["']\s+)["'](?:file:/+(?:etc/(?:passwd|shadows?|gshadow)|proc/(?:self|[0-9]+)/environ)(?:[?#][^"']*)?|php://filter[^"']*resource=(?:/etc/(?:passwd|shadows?)|[^"']*(?:wp-config\.php|\.env))|https?://(?:169\.254\.169\.254|127\.0\.0\.1|localhost)(?::[0-9]+)?(?:/[^"']*)?)["']\s*>"#).unwrap(),
        entity_denied: Regex::new(r#"(?i)\b(?:Entity resolution disallowed for|external entity (?:resolution )?(?:blocked|denied)(?: for)?)\s+["']?(file:/+(?:etc/(?:passwd|shadows?|gshadow)|proc/(?:self|[0-9]+)/environ))(?:[\s"'<>)]|$)"#).unwrap(),
        xss: Regex::new(r#"(?is)<script\b[^>]*>\s*(?:alert|prompt|confirm|eval|fetch)\s*\([^<]*?(?:</script\s*>|$)|<(?:img|svg|body|iframe|input|video)\b[^>]*\bon(?:error|load|focus|mouseover)\s*=\s*["']?\s*(?:alert|prompt|confirm|eval|fetch)\s*\([^>]*>?|javascript\s*:\s*(?:alert|prompt|confirm|eval)\s*\([^\s<]*"#).unwrap(),
        sql: Regex::new(r#"(?is)\bunion(?:\s|/\*[^*]*\*/)+(?:all(?:\s|/\*[^*]*\*/)+)?select\b\s+[^&#\r\n]+|["');]\s*;?\s*(?:select\s+(?:pg_sleep|sleep)\s*\(\s*[1-9][0-9]*\s*\)|waitfor\s+delay\s+'[0-9:]+')"#).unwrap(),
        sql_boolean: Regex::new(r#"(?i)["']\s*(?:or|and)\s+(?:([0-9]{1,8})\s*=\s*([0-9]{1,8})|'([^']{1,20})'\s*=\s*'([^']{1,20})')\s*(?:--|#|/\*)"#).unwrap(),
    })
}
fn documented(ev: &Event) -> bool {
    norm::field_text(ev, "event.kind")
        .is_some_and(|s| matches!(s.as_str(), "documentation" | "example"))
        || documented_text(&ev.message)
}
fn documented_text(value: &str) -> bool {
    let message = bounded(value).trim_start().to_lowercase();
    [
        "example:",
        "documentation:",
        "sample payload:",
        "exemplo:",
        "documentação:",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
}
pub(crate) fn infer_request(ev: &Event, n: &mut Normalized) {
    if documented(ev) {
        return;
    }
    let p = patterns();
    let (segments, clipped) = super::original::segments(ev);
    n.content_clipped |= clipped;
    for segment in segments {
        let msg = segment.value;
        if documented_text(msg) || !can_infer_request(&segment.field, n) {
            continue;
        }
        if let Some(uri) = request_uri(msg) {
            if n.get("url").is_none() {
                n.put(
                    "url",
                    &segment.field,
                    uri.into(),
                    "URI extraída de registro HTTP",
                );
            }
            n.put(
                "action",
                &segment.field,
                "http_request".into(),
                "inference: sintaxe HTTP",
            );
            if msg.contains("No mapping found for HTTP request") {
                n.put(
                    "outcome",
                    &segment.field,
                    "failure".into(),
                    "inference: rota não encontrada",
                );
            }
        }
        if p.entity_denied.is_match(msg) {
            n.put(
                "action",
                &segment.field,
                "xml_entity_resolution".into(),
                "inference: diagnóstico do parser XML",
            );
            n.put(
                "outcome",
                &segment.field,
                "blocked".into(),
                "inference: resolução explicitamente negada",
            );
        }
    }
}
pub(super) fn request_uri(value: &str) -> Option<&str> {
    if documented_text(value) {
        return None;
    }
    let p = patterns();
    p.uri
        .captures(value)
        .or_else(|| p.access.captures(value))
        .or_else(|| p.short_access.captures(value))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
}
pub(super) fn can_infer_request(field: &str, n: &Normalized) -> bool {
    !n.values
        .get("response_body")
        .is_some_and(|p| p.field == field || field == "raw")
        && !n.values.get("command").is_some_and(|p| p.field == field)
        && !["response", "template", "documentation", "example"]
            .iter()
            .any(|s| field.to_ascii_lowercase().split('.').any(|part| part == *s))
}
pub(super) fn apply(ev: &Event, derived: &mut Event, n: &mut Normalized, out: &mut Vec<Signal>) {
    if documented(ev) {
        derived
            .fields
            .insert("_sec.literal_output".into(), Value::from("true"));
        return;
    }
    let p = patterns();
    let mut fields = Vec::new();
    for role in ["url", "request_body"] {
        if let Some(v) = n.values.get(role) {
            fields.push((v.field.clone(), v.value.clone(), v.method.clone(), true));
        }
    }
    for field in [
        "url.path",
        "http.request.uri",
        "request.uri",
        "http.request.body.content",
        "http.request.body",
        "request.body",
    ] {
        if let Some(v) = norm::field_text(ev, field) {
            if !fields.iter().any(|(f, ..)| f == field) {
                fields.push((field.into(), v, "original".into(), true));
            }
        }
    }
    // Every original scalar is a candidate, retaining its boundary and meaning.
    let (segments, clipped) = super::original::segments(ev);
    n.content_clipped |= clipped;
    for segment in segments {
        if documented_text(segment.value) {
            continue;
        }
        if fields.iter().any(|(f, ..)| f == &segment.field) {
            continue;
        }
        if !can_infer_request(&segment.field, n) {
            continue;
        }
        let (value, method, request) = if let Some(uri) = request_uri(segment.value) {
            (uri, "URI extraída de registro HTTP", true)
        } else {
            (segment.value, "original", false)
        };
        fields.push((segment.field, value.into(), method.into(), request));
    }
    for (field, raw, method, request) in fields {
        n.content_clipped |= raw.len() > 65536;
        let input = bounded(&raw);
        let mut value = input.to_string();
        for _ in 0..2 {
            value = crate::threats::percent_decode(&value);
        }
        let transform = if value == input {
            if method.starts_with("URI extraída") {
                method.as_str()
            } else {
                "original"
            }
        } else {
            "percent-decode (até 2 passagens)"
        };
        let transformed = if method.starts_with("URI extraída") && value != input {
            format!("{method}; {transform}")
        } else {
            transform.to_string()
        };
        let transform = transformed.as_str();
        if !request && norm::literal_output(&value) {
            continue;
        }
        if request {
            if let Some(hit) = p.traversal.find(&value) {
                add(
                    ev,
                    out,
                    "path_traversal",
                    &field,
                    transform,
                    &value,
                    hit.start(),
                    hit.end(),
                );
            }
            if let Some(hit) = p.xss.find(&value) {
                add(
                    ev,
                    out,
                    "xss_request",
                    &field,
                    transform,
                    &value,
                    hit.start(),
                    hit.end(),
                );
            }
            let sql_value = value.replace('+', " ");
            let boolean = p
                .sql_boolean
                .captures(&sql_value)
                .filter(|c| {
                    c.get(1)
                        .zip(c.get(2))
                        .is_some_and(|(a, b)| a.as_str() == b.as_str())
                        || c.get(3)
                            .zip(c.get(4))
                            .is_some_and(|(a, b)| a.as_str() == b.as_str())
                })
                .and_then(|c| c.get(0));
            if let Some(hit) = p.sql.find(&sql_value).or(boolean) {
                let sql_transform = if sql_value != value {
                    format!("{transform}; form-urlencoded (+ → espaço)")
                } else {
                    transform.to_string()
                };
                add(
                    ev,
                    out,
                    "sqli_request",
                    &field,
                    &sql_transform,
                    &sql_value,
                    hit.start(),
                    hit.end(),
                );
            }
        }
        if let Some(hit) = p.xxe.find(&value) {
            // A declaration of an external entity aimed at credentials/metadata is specific;
            // the DOCTYPE alone or an ordinary external DTD is not an indication.
            add(
                ev,
                out,
                if request {
                    "xxe_request"
                } else {
                    "xxe_payload"
                },
                &field,
                transform,
                &value,
                hit.start(),
                hit.end(),
            );
        }
        if !request {
            if let Some(hit) = p.xss.find(&value) {
                add(
                    ev,
                    out,
                    "xss_payload",
                    &field,
                    transform,
                    &value,
                    hit.start(),
                    hit.end(),
                );
            }
            let boolean = p
                .sql_boolean
                .captures(&value)
                .filter(|c| {
                    c.get(1)
                        .zip(c.get(2))
                        .is_some_and(|(a, b)| a.as_str() == b.as_str())
                        || c.get(3)
                            .zip(c.get(4))
                            .is_some_and(|(a, b)| a.as_str() == b.as_str())
                })
                .and_then(|c| c.get(0));
            if let Some(hit) = boolean {
                add(
                    ev,
                    out,
                    "sqli_payload",
                    &field,
                    transform,
                    &value,
                    hit.start(),
                    hit.end(),
                );
            }
            if let Some(hit) = p
                .traversal
                .find(&value)
                .filter(|m| super::patterns().path.is_match(m.as_str()))
            {
                add(
                    ev,
                    out,
                    "traversal_payload",
                    &field,
                    transform,
                    &value,
                    hit.start(),
                    hit.end(),
                );
            }
            if n.get("command").is_none() && norm::reverse_shell(&value) {
                add(
                    ev,
                    out,
                    "reverse_payload",
                    &field,
                    transform,
                    &value,
                    0,
                    value.len(),
                );
            }
            if let Some(hit) = p.entity_denied.find(&value) {
                add(
                    ev,
                    out,
                    "xxe_blocked",
                    &field,
                    transform,
                    &value,
                    hit.start(),
                    hit.end(),
                );
            }
        }
    }
    // Copies in raw/message/parsed fields are one fact; the contextual version wins.
    n.content_clipped |= out.len() >= 24;
    let preferred: Vec<_> = out
        .iter()
        .filter(|s| s.key.ends_with("_request"))
        .map(|s| {
            (
                s.key.replace("_request", "_payload"),
                s.excerpt.matched.clone(),
            )
        })
        .collect();
    out.retain(|s| {
        !preferred
            .iter()
            .any(|(key, value)| key == &s.key && value == &s.excerpt.matched)
    });
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert((s.key.clone(), s.excerpt.matched.clone())));
}

/// Counting categories prevents retries and cosmetic file suffixes from corroborating a scan.
pub(super) fn probe_context(
    ev: &Event,
    derived: &mut Event,
    n: &Normalized,
    out: &mut Vec<Signal>,
) {
    let Some(signal) = out.iter().find(|s| {
        ["sensitive_request", "ambiguous_request", "backup_request"]
            .iter()
            .any(|k| s.key == format!("_sec.content.{k}"))
    }) else {
        return;
    };
    let path = signal.excerpt.matched.to_ascii_lowercase();
    let family =
        if path.contains("id_rsa") || path.contains("id_ecdsa") || path.contains("id_ed25519") {
            "private-key"
        } else if path.contains("history") {
            "shell-history"
        } else if path.contains(".sql") {
            "database-dump"
        } else if path.contains(".env") {
            "environment"
        } else if path.contains("htpass")
            || path.contains("pgpass")
            || path.contains("credentials")
            || path.contains("esmtprc")
        {
            "credentials"
        } else if path.contains("/etc/") || path.contains("/config/sam") {
            "system-accounts"
        } else {
            "configuration"
        };
    let file = norm::field_text(ev, "caminho");
    let scope = serde_json::to_string(&(
        ev.source.as_str(),
        file,
        n.get("namespace"),
        n.get("host"),
        n.get("service"),
    ))
    .unwrap();
    // Never join files from an unidentified source, nor merge different recorded clients.
    if ev.source.is_empty() {
        return;
    }
    let client = n.get("source_address").map(str::to_string).or_else(|| {
        crate::entities::value(ev, crate::entities::Role::SrcIp).map(|v| v.into_owned())
    });
    derived.fields.insert(
        "_sec.content.probe_scope".into(),
        Value::from(serde_json::to_string(&(scope, client)).unwrap()),
    );
    derived
        .fields
        .insert("_sec.content.probe_family".into(), Value::from(family));
    out.push(Signal {
        key: "_sec.content.probe_family".into(),
        excerpt: signal.excerpt.clone(),
    });
}
