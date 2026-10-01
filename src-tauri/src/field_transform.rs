//! Local, bounded transformations for derived fields. No network, execution,
//! decryption or signature verification occurs here. Callers keep the original
//! field and handle an error per record instead of aborting an import.
use base64::{engine::general_purpose, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io::Write;

/// Include this in baked-store identity; bump when transform/limit semantics change.
pub const VERSION: &str = "field-transform-1";

/// Regex replacement syntax, with a byte limit checked before each append.
/// Capture references match regex::Captures::expand ($1, $name, ${name}, $$).
pub fn expand_capture(
    captures: &regex::Captures<'_>,
    template: &str,
    limit: usize,
) -> Result<String, Error> {
    fn append(output: &mut String, value: &str, limit: usize) -> Result<(), Error> {
        if value.len() > limit.saturating_sub(output.len()) {
            return Err(Error::OutputLimit);
        }
        output.push_str(value);
        Ok(())
    }
    let mut output = String::new();
    let mut rest = template;
    while let Some(dollar) = rest.find('$') {
        append(&mut output, &rest[..dollar], limit)?;
        rest = &rest[dollar + 1..];
        if let Some(next) = rest.strip_prefix('$') {
            append(&mut output, "$", limit)?;
            rest = next;
            continue;
        }
        let reference = if let Some(braced) = rest.strip_prefix('{') {
            braced.find('}').map(|end| (&braced[..end], end + 2))
        } else {
            let end = rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            (end > 0).then(|| (&rest[..end], end))
        };
        if let Some((name, consumed)) = reference {
            let value = name
                .parse::<usize>()
                .ok()
                .and_then(|index| captures.get(index))
                .or_else(|| {
                    if name.parse::<usize>().is_err() {
                        captures.name(name)
                    } else {
                        None
                    }
                });
            if let Some(value) = value {
                append(&mut output, value.as_str(), limit)?;
            }
            rest = &rest[consumed..];
        } else {
            append(&mut output, "$", limit)?;
        }
    }
    append(&mut output, rest, limit)?;
    Ok(output)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Base64Decode,
    Base64UrlDecode,
    Base64Encode,
    Base64UrlEncode,
    UrlDecode,
    UrlEncode,
    FormDecode,
    HexToText,
    TextToHex,
    ParseJson,
    ParseXml,
    ParseQuery,
    JwtPayload,
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub nodes: usize,
    pub depth: usize,
    pub steps: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: 256 << 10,
            output_bytes: 512 << 10,
            nodes: 4096,
            depth: 16,
            steps: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    InputLimit,
    OutputLimit,
    StructureLimit,
    PipelineLimit,
    InvalidBase64,
    InvalidUtf8,
    InvalidHex,
    InvalidUrl,
    InvalidJson,
    InvalidXml,
    InvalidJwt,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InputLimit => "O campo excede o limite de entrada da transformação.",
            Self::OutputLimit => "A transformação excede o limite de saída.",
            Self::StructureLimit => "A estrutura excede o limite de profundidade ou campos.",
            Self::PipelineLimit => "A sequência de transformações excede o limite.",
            Self::InvalidBase64 => "O valor não é um Base64 válido para o formato selecionado.",
            Self::InvalidUtf8 => "Os bytes decodificados não são texto UTF-8 válido.",
            Self::InvalidHex => "O valor hexadecimal é inválido ou tem comprimento ímpar.",
            Self::InvalidUrl => "O valor contém uma sequência percentual inválida.",
            Self::InvalidJson => "O valor não é um JSON válido.",
            Self::InvalidXml => "O XML é inválido, excede os limites ou contém DTD não permitido.",
            Self::InvalidJwt => "O valor não é um JWT de três partes com payload JSON legível.",
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub value: Value,
    /// The UI must display this independently from decoded claims.
    pub notices: Vec<&'static str>,
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("output limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn check_structure(value: &Value, limits: Limits) -> Result<(), Error> {
    fn visit(
        value: &Value,
        depth: usize,
        remaining: &mut usize,
        max_depth: usize,
    ) -> Result<(), Error> {
        if depth > max_depth || *remaining == 0 {
            return Err(Error::StructureLimit);
        }
        *remaining -= 1;
        match value {
            Value::Array(values) => {
                for child in values {
                    visit(child, depth + 1, remaining, max_depth)?;
                }
            }
            Value::Object(values) => {
                for child in values.values() {
                    visit(child, depth + 1, remaining, max_depth)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut remaining = limits.nodes;
    visit(value, 0, &mut remaining, limits.depth)
}

fn text(value: &Value, max: usize, limits: Limits) -> Result<std::borrow::Cow<'_, str>, Error> {
    check_structure(value, limits)?;
    if let Value::String(value) = value {
        if value.len() > max {
            return Err(Error::OutputLimit);
        }
        return Ok(std::borrow::Cow::Borrowed(value));
    }
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: max,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::OutputLimit)?;
    Ok(std::borrow::Cow::Owned(
        String::from_utf8(writer.bytes).map_err(|_| Error::InvalidUtf8)?,
    ))
}

pub fn payload_bytes(value: &Value, limit: usize) -> Result<usize, Error> {
    Ok(text(value, limit, Limits::default())?.len())
}

fn checked_text(bytes: Vec<u8>, max: usize) -> Result<Value, Error> {
    if bytes.len() > max {
        return Err(Error::OutputLimit);
    }
    String::from_utf8(bytes)
        .map(Value::String)
        .map_err(|_| Error::InvalidUtf8)
}

fn decode_base64(value: &str, url: bool, max: usize) -> Result<Vec<u8>, Error> {
    // The caller has already bounded the input; never preallocate from an
    // untrusted decoded-length declaration or silently accept invalid bytes.
    let estimate = (value.len() / 4).checked_mul(3).ok_or(Error::OutputLimit)?;
    if estimate > max.saturating_add(3) {
        return Err(Error::OutputLimit);
    }
    let engine = match (url, value.ends_with('=')) {
        (false, true) => &general_purpose::STANDARD,
        (false, false) => &general_purpose::STANDARD_NO_PAD,
        (true, true) => &general_purpose::URL_SAFE,
        (true, false) => &general_purpose::URL_SAFE_NO_PAD,
    };
    let bytes = engine.decode(value).map_err(|_| Error::InvalidBase64)?;
    if bytes.len() > max {
        return Err(Error::OutputLimit);
    }
    Ok(bytes)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(value: &str, plus: bool, max: usize) -> Result<String, Error> {
    let mut output = Vec::with_capacity(value.len().min(max));
    let bytes = value.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if output.len() >= max {
            return Err(Error::OutputLimit);
        }
        match bytes[at] {
            b'%' => {
                let a = bytes
                    .get(at + 1)
                    .and_then(|b| hex(*b))
                    .ok_or(Error::InvalidUrl)?;
                let b = bytes
                    .get(at + 2)
                    .and_then(|b| hex(*b))
                    .ok_or(Error::InvalidUrl)?;
                output.push(a * 16 + b);
                at += 3;
            }
            b'+' if plus => {
                output.push(b' ');
                at += 1;
            }
            byte => {
                output.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| Error::InvalidUtf8)
}

fn insert_repeated(object: &mut Map<String, Value>, key: String, value: Value) {
    match object.entry(key) {
        serde_json::map::Entry::Vacant(entry) => {
            entry.insert(value);
        }
        serde_json::map::Entry::Occupied(mut entry) => match entry.get_mut() {
            Value::Array(values) => values.push(value),
            old => {
                let first = std::mem::replace(old, Value::Null);
                *old = Value::Array(vec![first, value]);
            }
        },
    }
}

fn query(value: &str, limits: Limits) -> Result<Value, Error> {
    let value = value.strip_prefix('?').unwrap_or(value);
    let mut object = Map::new();
    if value.is_empty() {
        return Ok(Value::Object(object));
    }
    for (index, pair) in value.split('&').enumerate() {
        if index >= limits.nodes {
            return Err(Error::StructureLimit);
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        insert_repeated(
            &mut object,
            percent_decode(key, true, limits.output_bytes)?,
            Value::String(percent_decode(value, true, limits.output_bytes)?),
        );
    }
    Ok(Value::Object(object))
}

fn xml(value: &str, limits: Limits) -> Result<Value, Error> {
    let doc = roxmltree::Document::parse_with_options(
        value,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: limits.nodes.min(u32::MAX as usize) as u32,
        },
    )
    .map_err(|_| Error::InvalidXml)?;
    fn take(budget: &mut usize, size: usize) -> Result<(), Error> {
        if size > *budget {
            return Err(Error::OutputLimit);
        }
        *budget -= size;
        Ok(())
    }
    fn name(namespace: Option<&str>, local: &str, budget: &mut usize) -> Result<String, Error> {
        // A long namespace reused by many short tags must not amplify into
        // unbounded owned keys before final JSON serialization checks it.
        let size = local
            .len()
            .checked_add(namespace.map_or(0, |ns| ns.len().saturating_add(2)))
            .ok_or(Error::OutputLimit)?;
        take(budget, size)?;
        Ok(namespace
            .map(|ns| format!("{{{ns}}}{local}"))
            .unwrap_or_else(|| local.to_owned()))
    }
    fn project(
        node: roxmltree::Node<'_, '_>,
        depth: usize,
        limits: Limits,
        budget: &mut usize,
    ) -> Result<Value, Error> {
        if depth > limits.depth {
            return Err(Error::StructureLimit);
        }
        let mut object = Map::new();
        for attribute in node.attributes() {
            take(budget, 1 + attribute.value().len())?;
            object.insert(
                format!(
                    "@{}",
                    name(attribute.namespace(), attribute.name(), budget)?
                ),
                Value::String(attribute.value().into()),
            );
        }
        let mut text = String::new();
        for child in node.children() {
            if child.is_element() {
                let key = name(
                    child.tag_name().namespace(),
                    child.tag_name().name(),
                    budget,
                )?;
                insert_repeated(&mut object, key, project(child, depth + 1, limits, budget)?);
            } else if child.is_text() {
                let part = child.text().unwrap_or("");
                take(budget, part.len())?;
                if part.len() > limits.output_bytes.saturating_sub(text.len()) {
                    return Err(Error::OutputLimit);
                }
                text.push_str(part);
            }
        }
        if object.is_empty() {
            return Ok(Value::String(text));
        }
        if !text.is_empty() {
            object.insert("#text".into(), Value::String(text));
        }
        Ok(Value::Object(object))
    }
    let root = doc.root_element();
    let mut object = Map::new();
    let mut budget = limits.output_bytes;
    let key = name(
        root.tag_name().namespace(),
        root.tag_name().name(),
        &mut budget,
    )?;
    object.insert(key, project(root, 0, limits, &mut budget)?);
    Ok(Value::Object(object))
}

fn apply(value: &Value, step: Step, limits: Limits) -> Result<Value, Error> {
    let input = text(value, limits.output_bytes, limits)?;
    let max = limits.output_bytes;
    match step {
        Step::Base64Decode | Step::Base64UrlDecode => checked_text(
            decode_base64(&input, step == Step::Base64UrlDecode, max)?,
            max,
        ),
        Step::Base64Encode | Step::Base64UrlEncode => {
            let size = input
                .len()
                .checked_add(2)
                .and_then(|n| n.checked_div(3))
                .and_then(|n| n.checked_mul(4))
                .ok_or(Error::OutputLimit)?;
            if size > max {
                return Err(Error::OutputLimit);
            }
            Ok(Value::String(if step == Step::Base64UrlEncode {
                general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes())
            } else {
                general_purpose::STANDARD.encode(input.as_bytes())
            }))
        }
        Step::UrlDecode | Step::FormDecode => Ok(Value::String(percent_decode(
            &input,
            step == Step::FormDecode,
            max,
        )?)),
        Step::UrlEncode => {
            let mut output = String::new();
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            for byte in input.bytes() {
                let plain = byte.is_ascii_alphanumeric() || b"-._~".contains(&byte);
                let size = if plain { 1 } else { 3 };
                if size > max.saturating_sub(output.len()) {
                    return Err(Error::OutputLimit);
                }
                if plain {
                    output.push(byte as char);
                } else {
                    output.push('%');
                    output.push(HEX[(byte >> 4) as usize] as char);
                    output.push(HEX[(byte & 15) as usize] as char);
                }
            }
            Ok(Value::String(output))
        }
        Step::HexToText => {
            if input.len() % 2 != 0 {
                return Err(Error::InvalidHex);
            }
            if input.len() / 2 > max {
                return Err(Error::OutputLimit);
            }
            let bytes = input
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    Ok(hex(pair[0]).ok_or(Error::InvalidHex)? * 16
                        + hex(pair[1]).ok_or(Error::InvalidHex)?)
                })
                .collect::<Result<Vec<_>, Error>>()?;
            checked_text(bytes, max)
        }
        Step::TextToHex => {
            if input.len() > max / 2 {
                return Err(Error::OutputLimit);
            }
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut output = String::with_capacity(input.len() * 2);
            for byte in input.bytes() {
                output.push(HEX[(byte >> 4) as usize] as char);
                output.push(HEX[(byte & 15) as usize] as char);
            }
            Ok(Value::String(output))
        }
        Step::ParseJson => serde_json::from_str(&input).map_err(|_| Error::InvalidJson),
        Step::ParseXml => xml(&input, limits),
        Step::ParseQuery => query(&input, limits),
        Step::JwtPayload => {
            let mut pieces = input.split('.');
            let header = pieces.next().ok_or(Error::InvalidJwt)?;
            let payload = pieces.next().ok_or(Error::InvalidJwt)?;
            let signature = pieces.next().ok_or(Error::InvalidJwt)?;
            if header.is_empty()
                || payload.is_empty()
                || pieces.next().is_some()
                || !signature
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-=".contains(&b))
            {
                return Err(Error::InvalidJwt);
            }
            let header = decode_base64(header, true, max).map_err(|_| Error::InvalidJwt)?;
            let header: Value = serde_json::from_slice(&header).map_err(|_| Error::InvalidJwt)?;
            if !header.is_object() {
                return Err(Error::InvalidJwt);
            }
            check_structure(&header, limits)?;
            let payload = decode_base64(payload, true, max).map_err(|_| Error::InvalidJwt)?;
            let payload: Value = serde_json::from_slice(&payload).map_err(|_| Error::InvalidJwt)?;
            if !payload.is_object() {
                return Err(Error::InvalidJwt);
            }
            Ok(payload)
        }
    }
}

pub fn transform(value: &Value, steps: &[Step], limits: Limits) -> Result<Output, Error> {
    if steps.len() > limits.steps {
        return Err(Error::PipelineLimit);
    }
    // Validate before cloning, and serialize structured values through a capped
    // writer rather than allocating an arbitrarily large intermediate string.
    text(value, limits.input_bytes, limits).map_err(|e| {
        if e == Error::OutputLimit {
            Error::InputLimit
        } else {
            e
        }
    })?;
    let mut value = value.clone();
    let mut notices = Vec::new();
    for step in steps {
        value = apply(&value, *step, limits)?;
        text(&value, limits.output_bytes, limits)?;
        if *step == Step::JwtPayload && notices.is_empty() {
            notices.push("jwt_signature_not_verified");
        }
    }
    Ok(Output { value, notices })
}

/// Keep the complete typed parent and expose typed descendants through the
/// application's existing dotted-column convention. Explicit dotted keys win
/// over a nested alias, as in the original JSON importer. No partial map is
/// returned if field count or combined payload would exceed the budget.
pub fn expanded_fields(
    target: &str,
    value: &Value,
    limits: Limits,
    field_limit: usize,
) -> Result<Map<String, Value>, Error> {
    if target.is_empty() || target.len() > 512 {
        return Err(Error::StructureLimit);
    }
    check_structure(value, limits)?;
    fn put(
        out: &mut Map<String, Value>,
        key: String,
        value: &Value,
        left: &mut usize,
        limits: Limits,
        fields: usize,
    ) -> Result<(), Error> {
        if !out.contains_key(&key) && out.len() >= fields {
            return Err(Error::StructureLimit);
        }
        let size = key
            .len()
            .checked_add(text(value, *left, limits)?.len())
            .and_then(|n| n.checked_add(16))
            .ok_or(Error::OutputLimit)?;
        if size > *left {
            return Err(Error::OutputLimit);
        }
        *left -= size;
        out.insert(key, value.clone());
        Ok(())
    }
    fn descend(
        out: &mut Map<String, Value>,
        prefix: &str,
        value: &Value,
        left: &mut usize,
        limits: Limits,
        fields: usize,
    ) -> Result<(), Error> {
        match value {
            Value::Object(object) => {
                // Descend before inserting scalar/literal keys so direct keys
                // have the same deterministic precedence as flatten_json.
                for (key, child) in object {
                    if matches!(child, Value::Object(_) | Value::Array(_)) {
                        let path = format!("{prefix}.{key}");
                        if path.len() > 4096 {
                            return Err(Error::StructureLimit);
                        }
                        descend(out, &path, child, left, limits, fields)?;
                    }
                }
                for (key, child) in object {
                    if !matches!(child, Value::Object(_) | Value::Array(_)) {
                        let path = format!("{prefix}.{key}");
                        if path.len() > 4096 {
                            return Err(Error::StructureLimit);
                        }
                        put(out, path, child, left, limits, fields)?;
                    }
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    let path = format!("{prefix}.{index}");
                    if path.len() > 4096 {
                        return Err(Error::StructureLimit);
                    }
                    if matches!(child, Value::Object(_) | Value::Array(_)) {
                        descend(out, &path, child, left, limits, fields)?;
                    } else {
                        put(out, path, child, left, limits, fields)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut out = Map::new();
    let mut left = limits.output_bytes;
    put(
        &mut out,
        target.to_owned(),
        value,
        &mut left,
        limits,
        field_limit,
    )?;
    descend(&mut out, target, value, &mut left, limits, field_limit)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn run(value: Value, steps: &[Step]) -> Result<Output, Error> {
        transform(&value, steps, Limits::default())
    }
    #[test]
    fn bounded_capture_expansion_matches_regex_syntax_without_pessimistic_rejection() {
        let regex = regex::Regex::new(r"(?P<word>foo)(/é)?").unwrap();
        for input in ["foo/é", "foo"] {
            let captures = regex.captures(input).unwrap();
            for template in [
                "literal",
                "$0",
                "$1-$2",
                "$word",
                "${word}",
                "$$",
                "$$1",
                "$$$1",
                "$42a",
                "${1}a",
                "$missing",
                "${}",
                "${word",
                "$é",
                "${not a name}",
                "$99999999999999999999999999999",
                "a$!b",
            ] {
                let mut expected = String::new();
                captures.expand(template, &mut expected);
                assert_eq!(
                    expand_capture(&captures, template, 1024).unwrap(),
                    expected,
                    "{template}"
                );
                if !expected.is_empty() {
                    assert_eq!(
                        expand_capture(&captures, template, expected.len() - 1),
                        Err(Error::OutputLimit)
                    );
                }
            }
        }
        let large = "x".repeat(256 << 10);
        let captures = regex::Regex::new("(.*)").unwrap();
        let captures = captures.captures(&large).unwrap();
        assert_eq!(expand_capture(&captures, "ok", 2).unwrap(), "ok");
        assert_eq!(
            expand_capture(&captures, "$1$1", 300 << 10),
            Err(Error::OutputLimit)
        );
    }
    #[test]
    fn encodings_round_trip_unicode_without_touching_original() {
        let original = json!("á 😀 + / ? & =\n");
        for (encode, decode) in [
            (Step::Base64Encode, Step::Base64Decode),
            (Step::Base64UrlEncode, Step::Base64UrlDecode),
            (Step::UrlEncode, Step::UrlDecode),
            (Step::TextToHex, Step::HexToText),
        ] {
            assert_eq!(
                run(original.clone(), &[encode, decode]).unwrap().value,
                original
            );
        }
        assert_eq!(
            run(json!("a+b%2Bc"), &[Step::UrlDecode]).unwrap().value,
            "a+b+c"
        );
        assert_eq!(
            run(json!("a+b%2Bc"), &[Step::FormDecode]).unwrap().value,
            "a b+c"
        );
    }
    #[test]
    fn typed_json_and_duplicate_parameters_are_preserved() {
        let value = json!({"flag":false,"empty":"","nil":null,"n":42,"list":[1,2]});
        assert_eq!(
            run(
                value.clone(),
                &[Step::Base64Encode, Step::Base64Decode, Step::ParseJson]
            )
            .unwrap()
            .value,
            value
        );
        assert_eq!(
            run(
                json!("?a=1&a=2&empty=&flag&utf=%C3%A1&plus=a+b"),
                &[Step::ParseQuery]
            )
            .unwrap()
            .value,
            json!({"a":["1","2"],"empty":"","flag":"","utf":"á","plus":"a b"})
        );
        assert_eq!(
            run(json!(""), &[Step::ParseQuery]).unwrap().value,
            json!({})
        );
    }
    #[test]
    fn xml_preserves_attributes_duplicates_namespaces_and_rejects_dtd() {
        assert_eq!(
            run(
                json!("<root id='7'><x>1</x><x>2</x></root>"),
                &[Step::ParseXml]
            )
            .unwrap()
            .value,
            json!({"root":{"@id":"7","x":["1","2"]}})
        );
        assert_eq!(
            run(
                json!("<r xmlns:a='urn:a' xmlns:b='urn:b'><a:x>1</a:x><b:x>2</b:x></r>"),
                &[Step::ParseXml]
            )
            .unwrap()
            .value,
            json!({"r":{"{urn:a}x":"1","{urn:b}x":"2"}})
        );
        for input in [
            "<!DOCTYPE x SYSTEM 'https://example.invalid/secret'><x/>",
            "<!DOCTYPE x [<!ENTITY a 'bomb'>]><x>&a;</x>",
            "<broken>",
        ] {
            assert!(matches!(
                run(json!(input), &[Step::ParseXml]),
                Err(Error::InvalidXml)
            ));
        }
    }
    #[test]
    fn jwt_decode_is_explicitly_unverified_and_does_not_accept_jwe() {
        let head = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let body = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"example","admin":false}"#);
        let result = run(json!(format!("{head}.{body}.")), &[Step::JwtPayload]).unwrap();
        assert_eq!(result.value, json!({"sub":"example","admin":false}));
        assert_eq!(result.notices, ["jwt_signature_not_verified"]);
        assert!(matches!(
            run(json!("a.b.c.d.e"), &[Step::JwtPayload]),
            Err(Error::InvalidJwt)
        ));
    }
    #[test]
    fn malformed_bytes_fail_without_leaking_content_or_lossy_decoding() {
        for (input, step, error) in [
            ("$secret", Step::Base64Decode, Error::InvalidBase64),
            ("/w==", Step::Base64Decode, Error::InvalidUtf8),
            ("ff", Step::HexToText, Error::InvalidUtf8),
            ("abc", Step::HexToText, Error::InvalidHex),
            ("%xx-secret", Step::UrlDecode, Error::InvalidUrl),
            ("%FF", Step::UrlDecode, Error::InvalidUtf8),
            ("{secret", Step::ParseJson, Error::InvalidJson),
        ] {
            let actual = run(json!(input), &[step]).unwrap_err();
            assert_eq!(actual, error);
            assert!(!actual.to_string().contains(input));
        }
    }
    #[test]
    fn every_allocation_path_has_byte_structure_and_step_limits() {
        let limits = Limits {
            input_bytes: 16,
            output_bytes: 20,
            nodes: 4,
            depth: 2,
            steps: 2,
        };
        assert!(matches!(
            transform(&json!("x".repeat(17)), &[], limits),
            Err(Error::InputLimit)
        ));
        assert!(matches!(
            transform(&json!("x".repeat(16)), &[Step::TextToHex], limits),
            Err(Error::OutputLimit)
        ));
        assert!(matches!(
            transform(&json!("x"), &[Step::ParseJson; 3], limits),
            Err(Error::PipelineLimit)
        ));
        assert!(matches!(
            transform(&json!([[[[1]]]]), &[], limits),
            Err(Error::StructureLimit)
        ));
        assert!(matches!(
            transform(&json!("[1,2,3,4,5]"), &[Step::ParseJson], limits),
            Err(Error::StructureLimit)
        ));
        assert!(matches!(
            transform(&json!("a=1&b=2&c=3&d=4"), &[Step::ParseQuery], limits),
            Err(Error::StructureLimit)
        ));
        let xml = format!(
            "<r xmlns:p='{}'>{}</r>",
            "x".repeat(200),
            "<p:x/>".repeat(100)
        );
        let limits = Limits {
            input_bytes: 4096,
            output_bytes: 4096,
            ..Limits::default()
        };
        assert!(matches!(
            transform(&json!(xml), &[Step::ParseXml], limits),
            Err(Error::OutputLimit)
        ));
    }
    #[test]
    fn children_remain_typed_and_share_importer_literal_key_precedence() {
        let original = json!({"n":42,"flag":false,"nil":null,"empty":"","list":[{"id":7}],"a":{"b":1},"a.b":2});
        let fields = expanded_fields("decoded", &original, Limits::default(), 100).unwrap();
        assert_eq!(fields["decoded"], original);
        assert_eq!(fields["decoded.n"], 42);
        assert_eq!(fields["decoded.flag"], false);
        assert_eq!(fields["decoded.nil"], Value::Null);
        assert_eq!(fields["decoded.empty"], "");
        assert_eq!(fields["decoded.list.0.id"], 7);
        assert_eq!(fields["decoded.a.b"], 2);
        assert!(matches!(
            expanded_fields("decoded", &original, Limits::default(), 2),
            Err(Error::StructureLimit)
        ));
    }
    #[test]
    fn child_expansion_is_atomic_and_payload_bounded() {
        let limits = Limits {
            output_bytes: 128,
            ..Limits::default()
        };
        let value = json!({"large":"x".repeat(60)});
        assert!(matches!(
            expanded_fields("decoded", &value, limits, 100),
            Err(Error::OutputLimit)
        ));
        assert!(matches!(
            expanded_fields("", &json!({}), Limits::default(), 100),
            Err(Error::StructureLimit)
        ));
    }
}
