//! Encoded tokens become ordinary dotted subfields at ingestion, exactly like
//! nested JSON and URL query parameters, so they can be filtered, grouped and
//! queried. Nothing is verified: decoding a JWT never checks its signature,
//! expiry or issuer, and an encrypted token (JWE) is left untouched.
//!
//! - JWT (optionally `Bearer …`): `<field>.header.*` and `<field>.payload.*`
//! - Base64/Base64url text holding a JSON object: `<field>.*`
//! - HTTP `Basic …` credentials: `<field>.basic.user` and `<field>.basic.password`
//!
//! Explicit fields always win over generated ones.
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use serde_json::{Map, Value};

use crate::model::Event;

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_encode_padding(false)
    .with_decode_padding_mode(DecodePaddingMode::Indifferent);
const STANDARD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT);

/// Bounds per record, shared by every candidate value.
const MAX_INSPECTED: usize = 64;
const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_VALUE_BYTES: usize = 16 * 1024;
const MAX_OUTPUT: usize = 512;
const MAX_PER_TOKEN: usize = 128;

pub(crate) fn expand_token_fields(ev: &mut Event) {
    let mut candidates: Vec<(&String, &str)> = ev.fields.iter()
        .filter(|(key, _)| key.len() <= 512 && key.matches('.').count() < 8)
        .filter_map(|(key, value)| value.as_str().map(|text| (key, text.trim())))
        .filter(|(_, text)| text.len() >= 16 && text.len() <= MAX_VALUE_BYTES && looks_encoded(text))
        .collect();
    // Parents before descendants keeps the generated names deterministic.
    candidates.sort_by(|a, b| a.0.cmp(b.0));
    let (mut inspected, mut bytes) = (0usize, 0usize);
    let mut additions: Vec<(String, Value)> = Vec::new();
    for (key, text) in candidates {
        if inspected >= MAX_INSPECTED || bytes.saturating_add(text.len()) > MAX_INPUT_BYTES || additions.len() >= MAX_OUTPUT {
            break;
        }
        inspected += 1;
        bytes += text.len();
        for (path, value) in decode(text).into_iter().flatten().take(MAX_PER_TOKEN) {
            additions.push((format!("{key}.{path}"), value));
        }
    }
    for (name, value) in additions.into_iter().take(MAX_OUTPUT) {
        ev.fields.entry(name).or_insert(value);
    }
}

/// Cheap shape test before any decoding: a JSON object always encodes to a
/// value starting with `ey`/`ew` (`{"`, `{ `, `{\n`), and Basic credentials
/// carry their scheme. Ordinary hex identifiers and prose never qualify.
fn looks_encoded(text: &str) -> bool {
    let token = strip_scheme(text).map_or(text, |(_, token)| token);
    let basic = strip_scheme(text).is_some_and(|(scheme, _)| scheme == Scheme::Basic);
    (basic || token.starts_with("ey") || token.starts_with("ew"))
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_' | b'.'))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Bearer,
    Basic,
}

fn strip_scheme(text: &str) -> Option<(Scheme, &str)> {
    let (scheme, rest) = text.split_once(char::is_whitespace)?;
    let scheme = if scheme.eq_ignore_ascii_case("bearer") {
        Scheme::Bearer
    } else if scheme.eq_ignore_ascii_case("basic") {
        Scheme::Basic
    } else {
        return None;
    };
    let rest = rest.trim();
    (!rest.is_empty() && !rest.contains(char::is_whitespace)).then_some((scheme, rest))
}

fn decode(text: &str) -> Option<Vec<(String, Value)>> {
    let (scheme, token) = match strip_scheme(text) {
        Some((scheme, token)) => (Some(scheme), token),
        None if text.contains(char::is_whitespace) => return None,
        None => (None, text),
    };
    if scheme == Some(Scheme::Basic) {
        return basic(token);
    }
    jwt(token).or_else(|| base64_object(token).map(|object| flatten(object, "")))
}

fn base64_bytes(text: &str) -> Option<Vec<u8>> {
    let engine = if text.contains(['-', '_']) { &URL_SAFE } else { &STANDARD };
    engine.decode(text).ok().filter(|bytes| bytes.len() <= MAX_VALUE_BYTES)
}

fn base64_object(text: &str) -> Option<Map<String, Value>> {
    if text.contains('.') {
        return None;
    }
    let bytes = base64_bytes(text)?;
    match serde_json::from_slice::<Value>(&bytes).ok()? {
        Value::Object(object) if !object.is_empty() => Some(object),
        _ => None,
    }
}

/// Compact JWS only: three parts, a JSON header with `alg`, a JSON payload.
fn jwt(token: &str) -> Option<Vec<(String, Value)>> {
    let mut parts = token.split('.');
    let (header, payload, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || header.is_empty() || payload.is_empty() || signature.contains(['+', '/', '=']) {
        return None;
    }
    let decode_part = |part: &str| -> Option<Value> {
        let bytes = URL_SAFE.decode(part).ok().filter(|bytes| bytes.len() <= MAX_VALUE_BYTES)?;
        serde_json::from_slice(&bytes).ok()
    };
    let Value::Object(header) = decode_part(header)? else { return None };
    if !header.get("alg").is_some_and(Value::is_string) {
        return None;
    }
    let payload = decode_part(payload)?;
    let mut fields = flatten(header, "header");
    match payload {
        Value::Object(object) => fields.extend(flatten(object, "payload")),
        other => fields.push(("payload".into(), other)),
    }
    Some(fields)
}

fn basic(token: &str) -> Option<Vec<(String, Value)>> {
    let text = String::from_utf8(base64_bytes(token)?).ok()?;
    let (user, password) = text.split_once(':')?;
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(vec![("basic.user".into(), Value::from(user)), ("basic.password".into(), Value::from(password))])
}

fn flatten(object: Map<String, Value>, prefix: &str) -> Vec<(String, Value)> {
    let mut flat = Map::new();
    super::flatten_json(object, prefix, 0, &mut flat);
    flat.into_iter().filter(|(key, _)| key.len() <= 512).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn b64url(value: &Value) -> String { URL_SAFE.encode(serde_json::to_vec(value).unwrap()) }
    fn event(fields: Value) -> Event {
        let mut ev = Event::empty();
        ev.fields = fields.as_object().unwrap().clone();
        expand_token_fields(&mut ev);
        ev
    }

    #[test]
    fn jwt_claims_become_header_and_payload_subfields_without_verification() {
        let token = format!("{}.{}.c2ln", b64url(&json!({"alg":"HS256","typ":"JWT"})),
            b64url(&json!({"sub":"alice","iat":1700000000,"ctx":{"role":"admin"}})));
        let ev = event(json!({ "token": token, "authorization": format!("Bearer {token}") }));
        assert_eq!(ev.fields["token.header.alg"], "HS256");
        assert_eq!(ev.fields["token.payload.sub"], "alice");
        assert_eq!(ev.fields["token.payload.iat"], 1700000000);
        assert_eq!(ev.fields["token.payload.ctx.role"], "admin");
        assert_eq!(ev.fields["authorization.payload.sub"], "alice");
        assert_eq!(ev.fields["token"], json!(token), "the original value is kept as is");
    }

    #[test]
    fn base64_json_and_basic_credentials_expand_but_explicit_fields_win() {
        let encoded = STANDARD.encode(br#"{"user":"bob","n":{"k":2}}"#);
        let ev = event(json!({ "data": encoded, "data.user": "explicit", "auth": format!("Basic {}", STANDARD.encode("carol:s3cr3t")) }));
        assert_eq!(ev.fields["data.user"], "explicit");
        assert_eq!(ev.fields["data.n.k"], 2);
        assert_eq!(ev.fields["auth.basic.user"], "carol");
        assert_eq!(ev.fields["auth.basic.password"], "s3cr3t");
    }

    #[test]
    fn ordinary_values_jwe_and_invalid_tokens_are_left_alone() {
        let jwe = format!("{}.a.b.c.d", b64url(&json!({"alg":"RSA-OAEP","enc":"A256GCM"})));
        let ev = event(json!({
            "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "message": "eyJ looks like a token but has spaces",
            "jwe": jwe,
            "broken": "eyJhbGciOiJIUzI1NiJ9.!!!.x",
            "noalg": format!("{}.{}.", b64url(&json!({"typ":"JWT"})), b64url(&json!({"a":1}))),
        }));
        assert_eq!(ev.fields.len(), 5, "{:?}", ev.fields.keys().collect::<Vec<_>>());
    }

    #[test]
    fn expansion_is_bounded_and_idempotent() {
        let token = format!("{}.{}.", b64url(&json!({"alg":"none"})), b64url(&json!({"a":1})));
        let mut fields = Map::new();
        for i in 0..200 { fields.insert(format!("t{i:03}"), Value::from(token.clone())); }
        let mut ev = Event::empty(); ev.fields = fields;
        expand_token_fields(&mut ev);
        let generated = ev.fields.len() - 200;
        assert!(generated <= MAX_INSPECTED * 2, "{generated}");
        let snapshot = ev.fields.clone();
        expand_token_fields(&mut ev);
        assert_eq!(ev.fields, snapshot);
    }
}
