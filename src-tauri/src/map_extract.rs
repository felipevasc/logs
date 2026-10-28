//! Extraction strategies of a map rule. Structured data uses structured parsers (JSON, XML,
//! delimiters, key=value, dissect); only free text goes through Grok or a regular
//! expression. Every strategy reports where in the subject each value came from (byte range
//! of the subject) whenever the parser knows it, so the interface can highlight real
//! positions instead of searching for the value again.
use crate::grok;
use crate::search_regex::{Matcher, Pattern};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Largest subject an extraction will read. Longer records are reported, never truncated silently.
pub(crate) const SUBJECT_LIMIT: usize = 256 << 10;
/// Occurrences kept when a regular expression extracts every match of a record.
pub(crate) const ALL_LIMIT: usize = 200;

fn message() -> String {
    "message".into()
}
fn search() -> String {
    "search".into()
}
fn equals() -> String {
    "=".into()
}
fn pcre() -> String {
    "pcre2".into()
}
fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Extraction {
    Grok {
        patterns: Vec<String>,
        #[serde(default = "search")]
        anchor: String,
        #[serde(default)]
        case_insensitive: bool,
        #[serde(default)]
        dot_all: bool,
        #[serde(default = "message")]
        source: String,
    },
    Dissect {
        pattern: String,
        #[serde(default = "message")]
        source: String,
    },
    Json {
        paths: BTreeMap<String, String>,
        #[serde(default = "message")]
        source: String,
    },
    Xml {
        paths: BTreeMap<String, String>,
        #[serde(default = "message")]
        source: String,
    },
    Delimited {
        delimiter: String,
        #[serde(default)]
        quote: Option<String>,
        columns: Vec<String>,
        #[serde(default = "message")]
        source: String,
    },
    KeyValue {
        #[serde(default)]
        pair_delimiter: String,
        #[serde(default = "equals")]
        separator: String,
        #[serde(default)]
        quote: Option<String>,
        keys: BTreeMap<String, String>,
        #[serde(default = "message")]
        source: String,
    },
    Regex {
        pattern: String,
        #[serde(default = "pcre")]
        engine: String,
        #[serde(default = "yes")]
        case_sensitive: bool,
        #[serde(default = "message")]
        source: String,
        /// Every occurrence in the subject, joined with ", " (default: the first one only).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
    },
}
impl Extraction {
    pub(crate) fn source(&self) -> &str {
        match self {
            Self::Grok { source, .. } | Self::Dissect { source, .. } | Self::Json { source, .. } | Self::Xml { source, .. } | Self::Delimited { source, .. } | Self::KeyValue { source, .. } | Self::Regex { source, .. } => source,
        }
    }
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Grok { .. } => "grok",
            Self::Dissect { .. } => "dissect",
            Self::Json { .. } => "json",
            Self::Xml { .. } => "xml",
            Self::Delimited { .. } => "delimited",
            Self::KeyValue { .. } => "key_value",
            Self::Regex { .. } => "regex",
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub name: String,
    pub value: String,
    /// Byte range inside the subject, when the parser knows it.
    pub range: Option<(usize, usize)>,
    pub conv: Option<grok::Conv>,
}
#[derive(Debug, Default)]
pub(crate) struct Extracted {
    /// The alternative that recognised the record (Grok), for provenance.
    pub alternative: Option<usize>,
    pub items: Vec<Item>,
}

enum Plan {
    Grok(grok::Compiled),
    Dissect(Vec<DissectPart>),
    Json(BTreeMap<String, String>),
    Xml(BTreeMap<String, String>),
    Delimited { delimiter: String, quote: Option<char>, columns: Vec<String> },
    KeyValue { pair: String, separator: String, quote: Option<char>, keys: BTreeMap<String, String> },
    Regex { matcher: Matcher, names: Vec<(usize, String)>, all: bool },
}
pub(crate) struct Compiled {
    plan: Plan,
    pub spec: Extraction,
}
#[derive(Debug, Clone)]
enum DissectPart {
    Literal(String),
    Field { name: Option<String>, pad: bool },
}

fn parse_dissect(pattern: &str) -> Result<Vec<DissectPart>, String> {
    let mut parts = Vec::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(open) = rest.find("%{") {
            if open > 0 {
                parts.push(DissectPart::Literal(rest[..open].to_owned()));
            }
            let Some(close) = rest[open..].find('}') else {
                return Err("Falta o } de fechamento em um campo %{…}.".into());
            };
            let body = &rest[open + 2..open + close];
            let (body, pad) = body.strip_suffix("->").map_or((body, false), |b| (b, true));
            if body.starts_with(['+', '*', '&']) || body.contains('/') {
                return Err(format!("O modificador de %{{{body}}} (acumular, chave/valor dinâmico ou ordem) não é suportado. Use campos simples ou %{{?ignorar}}."));
            }
            let name = match body.strip_prefix('?') {
                Some(_) => None,
                None if body.is_empty() => None,
                None => Some(body.to_owned()),
            };
            if let Some(n) = &name {
                if n.chars().any(|c| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '@'))) {
                    return Err(format!("Nome de campo inválido: “{n}”."));
                }
            }
            parts.push(DissectPart::Field { name, pad });
            rest = &rest[open + close + 1..];
        } else {
            parts.push(DissectPart::Literal(rest.to_owned()));
            break;
        }
    }
    for pair in parts.windows(2) {
        if matches!((&pair[0], &pair[1]), (DissectPart::Field { .. }, DissectPart::Field { .. })) {
            return Err("Dois campos seguidos precisam de um delimitador entre eles.".into());
        }
    }
    if !parts.iter().any(|p| matches!(p, DissectPart::Field { name: Some(_), .. })) {
        return Err("Informe ao menos um campo %{nome}.".into());
    }
    Ok(parts)
}
fn single_char(text: &Option<String>, what: &str) -> Result<Option<char>, String> {
    match text.as_deref() {
        None | Some("") => Ok(None),
        Some(t) if t.chars().count() == 1 => Ok(t.chars().next()),
        Some(_) => Err(format!("{what} deve ser um único caractere.")),
    }
}

impl Compiled {
    pub(crate) fn new(spec: &Extraction, custom: &BTreeMap<String, String>) -> Result<Self, Vec<grok::Diagnostic>> {
        let plain = |message: String| vec![grok::Diagnostic { code: "invalid", message, start: 0, end: 0, alternative: 0 }];
        let plan = match spec {
            Extraction::Grok { patterns, anchor, case_insensitive, dot_all, .. } => {
                let anchor = match anchor.as_str() {
                    "search" => grok::Anchor::Search,
                    "full" => grok::Anchor::Full,
                    other => return Err(plain(format!("Política de correspondência desconhecida: {other} (use search ou full)."))),
                };
                Plan::Grok(grok::compile(&grok::Spec { patterns, custom, anchor, case_insensitive: *case_insensitive, dot_all: *dot_all })?)
            }
            Extraction::Dissect { pattern, .. } => Plan::Dissect(parse_dissect(pattern).map_err(plain)?),
            Extraction::Json { paths, .. } | Extraction::Xml { paths, .. } => {
                if paths.is_empty() || paths.len() > 64 || paths.iter().any(|(n, p)| n.is_empty() || n.starts_with('_') || p.is_empty() || p.len() > 512) {
                    return Err(plain("Informe de 1 a 64 propriedades, com nome de saída e caminho.".into()));
                }
                if matches!(spec, Extraction::Json { .. }) { Plan::Json(paths.clone()) } else { Plan::Xml(paths.clone()) }
            }
            Extraction::Delimited { delimiter, quote, columns, .. } => {
                if delimiter.is_empty() || delimiter.len() > 16 || columns.is_empty() || columns.len() > 256 {
                    return Err(plain("Informe o delimitador (até 16 caracteres) e as colunas.".into()));
                }
                Plan::Delimited { delimiter: delimiter.clone(), quote: single_char(quote, "A aspa").map_err(plain)?, columns: columns.clone() }
            }
            Extraction::KeyValue { pair_delimiter, separator, quote, keys, .. } => {
                if separator.is_empty() || separator.len() > 8 || keys.is_empty() || keys.len() > 64 || pair_delimiter.len() > 8 {
                    return Err(plain("Informe o separador chave/valor e de 1 a 64 chaves.".into()));
                }
                Plan::KeyValue { pair: pair_delimiter.clone(), separator: separator.clone(), quote: single_char(quote, "A aspa").map_err(plain)?, keys: keys.clone() }
            }
            Extraction::Regex { pattern, engine, case_sensitive, all, .. } => {
                if !["pcre2", "rust"].contains(&engine.as_str()) {
                    return Err(plain("Use o motor pcre2 ou rust.".into()));
                }
                let matcher = Matcher::new(&Pattern { engine: engine.clone(), text: pattern.clone(), case_sensitive: *case_sensitive, multiline: false, dot_all: false }).map_err(plain)?;
                let names: Vec<(usize, String)> = matcher.groups().into_iter().filter_map(|g| g.names.first().map(|n| (g.index, n.clone()))).collect();
                if names.is_empty() {
                    return Err(plain("Use grupos nomeados, como (?<usuario>\\w+), para dar nome aos campos.".into()));
                }
                Plan::Regex { matcher, names, all: *all }
            }
        };
        Ok(Self { plan, spec: spec.clone() })
    }
    /// Names this extraction can produce (for the output list and the diff).
    pub(crate) fn field_names(&self) -> Vec<String> {
        match &self.plan {
            Plan::Grok(g) => g.fields(),
            Plan::Dissect(parts) => parts.iter().filter_map(|p| if let DissectPart::Field { name: Some(n), .. } = p { Some(n.clone()) } else { None }).collect(),
            Plan::Json(paths) | Plan::Xml(paths) => paths.keys().cloned().collect(),
            Plan::Delimited { columns, .. } => columns.iter().filter(|c| !c.is_empty()).cloned().collect(),
            Plan::KeyValue { keys, .. } => keys.keys().cloned().collect(),
            Plan::Regex { names, .. } => names.iter().map(|n| n.1.clone()).collect(),
        }
    }
    /// `Ok(None)`: the subject is not recognised by this extraction.
    pub(crate) fn run(&self, subject: &str) -> Result<Option<Extracted>, String> {
        if subject.len() > SUBJECT_LIMIT {
            return Err(format!("O texto de origem excede {} KiB; a extração não foi aplicada.", SUBJECT_LIMIT >> 10));
        }
        crate::operations::check()?;
        match &self.plan {
            Plan::Grok(g) => Ok(g.find(subject)?.map(|m| Extracted {
                alternative: Some(m.alternative),
                items: m.fields.into_iter().map(|(name, a, b)| Item { conv: g.conv_of(m.alternative, &name), name, value: subject[a..b].to_owned(), range: Some((a, b)) }).collect(),
            })),
            Plan::Dissect(parts) => Ok(dissect(parts, subject)),
            Plan::Json(paths) => Ok(json(paths, subject)),
            Plan::Xml(paths) => xml(paths, subject),
            Plan::Delimited { delimiter, quote, columns } => Ok(delimited(delimiter, *quote, columns, subject)),
            Plan::KeyValue { pair, separator, quote, keys } => Ok(key_value(pair, separator, *quote, keys, subject)),
            Plan::Regex { matcher, names, all: true } => {
                let found = matcher.all_ranges(subject, ALL_LIMIT)?;
                if found.is_empty() {
                    return Ok(None);
                }
                let items = names
                    .iter()
                    .filter_map(|(group, name)| {
                        let ranges: Vec<(usize, usize)> = found.iter().filter_map(|r| r.get(*group).copied().flatten()).collect();
                        let first = *ranges.first()?;
                        let value = ranges.iter().map(|&(a, b)| &subject[a..b]).collect::<Vec<_>>().join(", ");
                        Some(Item { name: name.clone(), value, range: Some(first), conv: None })
                    })
                    .collect();
                Ok(Some(Extracted { alternative: None, items }))
            }
            Plan::Regex { matcher, names, .. } => Ok(matcher.first_ranges(subject)?.map(|ranges| Extracted {
                alternative: None,
                items: names.iter().filter_map(|(group, name)| ranges.get(*group).copied().flatten().map(|(a, b)| Item { name: name.clone(), value: subject[a..b].to_owned(), range: Some((a, b)), conv: None })).collect(),
            })),
        }
    }
}

fn dissect(parts: &[DissectPart], text: &str) -> Option<Extracted> {
    let mut pos = 0usize;
    let mut items = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        match &parts[i] {
            DissectPart::Literal(lit) => {
                if !text[pos..].starts_with(lit.as_str()) {
                    return None;
                }
                pos += lit.len();
                i += 1;
            }
            DissectPart::Field { name, pad } => {
                let end = match parts.get(i + 1) {
                    Some(DissectPart::Literal(next)) => pos + text[pos..].find(next.as_str())?,
                    _ => text.len(),
                };
                if let Some(name) = name {
                    items.push(Item { name: name.clone(), value: text[pos..end].to_owned(), range: Some((pos, end)), conv: None });
                }
                pos = end;
                if *pad {
                    if let Some(DissectPart::Literal(next)) = parts.get(i + 1) {
                        while text[pos..].starts_with(next.as_str()) && !next.is_empty() {
                            pos += next.len();
                        }
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    Some(Extracted { alternative: None, items })
}

fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}
/// `a.b[0].c`; a literal dotted key wins over nesting.
fn json_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if let Some(v) = root.as_object().and_then(|o| o.get(path)) {
        return Some(v);
    }
    let mut current = root;
    for segment in path.split('.') {
        let (key, indexes) = match segment.find('[') {
            Some(at) => (&segment[..at], &segment[at..]),
            None => (segment, ""),
        };
        if !key.is_empty() {
            current = current.get(key)?;
        }
        for index in indexes.split('[').filter(|s| !s.is_empty()) {
            current = current.get(index.trim_end_matches(']').parse::<usize>().ok()?)?;
        }
    }
    Some(current)
}
fn json(paths: &BTreeMap<String, String>, text: &str) -> Option<Extracted> {
    // Log lines often carry a prefix before the JSON document; the first value is read.
    let start = text.find(['{', '['])?;
    let value = serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>().next()?.ok()?;
    let items = paths
        .iter()
        .filter_map(|(name, path)| json_path(&value, path).and_then(scalar).map(|v| Item { name: name.clone(), value: v, range: None, conv: None }))
        .collect();
    Some(Extracted { alternative: None, items })
}

/// `a/b/c`, `a/b@attr`, `Data[@Name='X']` (Windows event data).
fn xml(paths: &BTreeMap<String, String>, text: &str) -> Result<Option<Extracted>, String> {
    let Some(start) = text.find('<') else { return Ok(None) };
    let Ok(doc) = roxmltree::Document::parse(&text[start..]) else { return Ok(None) };
    let mut items = Vec::new();
    for (name, path) in paths {
        let (element_path, attribute) = match path.rsplit_once('@') {
            Some((p, a)) if !a.contains(['\'', '=', ']']) => (p.trim_end_matches('/'), Some(a)),
            _ => (path.as_str(), None),
        };
        let mut nodes = vec![doc.root_element()];
        let mut segments = element_path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).peekable();
        // The first segment may name the root element itself.
        if let Some(first) = segments.peek() {
            if first.split('[').next() == Some(doc.root_element().tag_name().name()) {
                segments.next();
            }
        }
        for segment in segments {
            let (tag, predicate) = match segment.split_once('[') {
                Some((t, p)) => (t, Some(p.trim_end_matches(']'))),
                None => (segment, None),
            };
            let wanted = predicate.and_then(|p| p.strip_prefix("@Name=")).map(|v| v.trim_matches(['\'', '"']).to_owned());
            nodes = nodes
                .iter()
                .flat_map(|n| n.children().filter(|c| c.is_element() && c.tag_name().name() == tag))
                .filter(|c| wanted.as_deref().is_none_or(|w| c.attribute("Name") == Some(w)))
                .collect();
        }
        let Some(node) = nodes.first() else { continue };
        let (value, range) = match attribute {
            Some(a) => match node.attributes().find(|x| x.name() == a) {
                Some(attr) => {
                    let r = attr.range_value();
                    (attr.value().to_owned(), Some((r.start + start, r.end + start)))
                }
                None => continue,
            },
            None => {
                let text_node = node.children().find(|c| c.is_text());
                (node.text().unwrap_or_default().to_owned(), text_node.map(|t| (t.range().start + start, t.range().end + start)))
            }
        };
        items.push(Item { name: name.clone(), value, range, conv: None });
    }
    Ok(Some(Extracted { alternative: None, items }))
}

fn split_fields(text: &str, delimiter: &str, quote: Option<char>) -> Vec<(usize, usize)> {
    let mut fields = Vec::new();
    let mut i = 0usize;
    loop {
        if let Some(q) = quote.filter(|q| text[i..].starts_with(*q)) {
            // Quoted field: a doubled or backslash-escaped quote does not close it.
            let mut j = i + q.len_utf8();
            let mut closed = None;
            while j < text.len() {
                let c = text[j..].chars().next().unwrap();
                if c == '\\' && j + 1 < text.len() {
                    j += 1 + text[j + 1..].chars().next().map_or(0, char::len_utf8);
                    continue;
                }
                if c == q {
                    if text[j + 1..].starts_with(q) {
                        j += 2 * q.len_utf8();
                        continue;
                    }
                    closed = Some(j);
                    break;
                }
                j += c.len_utf8();
            }
            if let Some(end) = closed {
                let after = end + q.len_utf8();
                if after == text.len() || text[after..].starts_with(delimiter) {
                    fields.push((i + q.len_utf8(), end));
                    if after == text.len() {
                        return fields;
                    }
                    i = after + delimiter.len();
                    continue;
                }
            }
        }
        match text[i..].find(delimiter) {
            Some(at) => {
                fields.push((i, i + at));
                i += at + delimiter.len();
            }
            None => {
                fields.push((i, text.len()));
                return fields;
            }
        }
        if fields.len() > 4096 {
            return fields;
        }
    }
}
fn delimited(delimiter: &str, quote: Option<char>, columns: &[String], text: &str) -> Option<Extracted> {
    let fields = split_fields(text, delimiter, quote);
    if fields.len() < columns.len() {
        return None;
    }
    let items = columns
        .iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
        .map(|(i, name)| Item { name: name.clone(), value: text[fields[i].0..fields[i].1].to_owned(), range: Some(fields[i]), conv: None })
        .collect();
    Some(Extracted { alternative: None, items })
}

fn key_value(pair: &str, separator: &str, quote: Option<char>, keys: &BTreeMap<String, String>, text: &str) -> Option<Extracted> {
    let mut found: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let wanted: Vec<&str> = keys.values().map(String::as_str).collect();
    let mut i = 0usize;
    while i < text.len() {
        if pair.is_empty() {
            while text[i..].starts_with(char::is_whitespace) {
                i += text[i..].chars().next().unwrap().len_utf8();
            }
        }
        if i >= text.len() {
            break;
        }
        let rest = &text[i..];
        // The token is read up to the pair delimiter, but a quoted value may contain it.
        let key_end = rest.find(separator);
        let stop = if pair.is_empty() { rest.find(char::is_whitespace) } else { rest.find(pair) };
        match key_end {
            Some(k) if stop.is_none_or(|s| k < s) => {
                let key = &rest[..k];
                let value_start = i + k + separator.len();
                let (vs, ve, next) = if let Some(q) = quote.filter(|q| text[value_start..].starts_with(*q)) {
                    let inner = value_start + q.len_utf8();
                    match text[inner..].find(q) {
                        Some(close) => (inner, inner + close, inner + close + q.len_utf8()),
                        None => (value_start, text.len(), text.len()),
                    }
                } else {
                    let end = if pair.is_empty() { text[value_start..].find(char::is_whitespace) } else { text[value_start..].find(pair) }.map_or(text.len(), |e| value_start + e);
                    (value_start, end, end)
                };
                if wanted.contains(&key) {
                    found.entry(&text[i..i + k]).or_insert((vs, ve));
                }
                i = next;
            }
            _ => i += stop.map_or(rest.len(), |s| s.max(1)),
        }
        if !pair.is_empty() && text[i..].starts_with(pair) {
            i += pair.len();
        }
    }
    let items: Vec<Item> = keys.iter().filter_map(|(name, key)| found.get(key.as_str()).map(|&(a, b)| Item { name: name.clone(), value: text[a..b].to_owned(), range: Some((a, b)), conv: None })).collect();
    (!items.is_empty()).then_some(Extracted { alternative: None, items })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compile(spec: Extraction) -> Compiled {
        Compiled::new(&spec, &BTreeMap::new()).unwrap_or_else(|d| panic!("{d:?}"))
    }
    fn pairs(x: Option<Extracted>) -> Vec<(String, String)> {
        x.map(|e| e.items.into_iter().map(|i| (i.name, i.value)).collect()).unwrap_or_default()
    }
    #[test]
    fn dissect_splits_on_delimiters_pads_and_skips() {
        let c = compile(Extraction::Dissect { pattern: "%{ts} [%{?thread}] %{level->} %{msg}".into(), source: "message".into() });
        let got = pairs(c.run("10:00 [main] WARN   disk full").unwrap());
        assert_eq!(got, vec![("ts".into(), "10:00".into()), ("level".into(), "WARN".into()), ("msg".into(), "disk full".into())]);
        assert!(c.run("no delimiters").unwrap().is_none());
        assert!(Compiled::new(&Extraction::Dissect { pattern: "%{+a} %{b}".into(), source: "message".into() }, &BTreeMap::new()).is_err());
        assert!(Compiled::new(&Extraction::Dissect { pattern: "%{a}%{b}".into(), source: "message".into() }, &BTreeMap::new()).is_err());
    }
    #[test]
    fn json_reads_properties_after_a_prefix_and_keeps_numbers_exact() {
        let paths: BTreeMap<_, _> = [("usuario", "user.name"), ("primeiro", "tags[0]"), ("porta", "net.port"), ("a.b", "a.b")].map(|(a, b)| (a.to_owned(), b.to_owned())).into();
        let c = compile(Extraction::Json { paths, source: "message".into() });
        let got = pairs(c.run("2026-10-23 INFO {\"user\":{\"name\":\"ana\"},\"tags\":[\"x\",\"y\"],\"net\":{\"port\":9007199254740993},\"a.b\":true}").unwrap());
        assert_eq!(got, vec![("a.b".into(), "true".into()), ("porta".into(), "9007199254740993".into()), ("primeiro".into(), "x".into()), ("usuario".into(), "ana".into())]);
        assert!(c.run("plain text").unwrap().is_none());
    }
    #[test]
    fn xml_reads_elements_attributes_and_windows_data_with_real_ranges() {
        let text = "<Event><System><EventID>4688</EventID><Provider Name='P'/></System><EventData><Data Name='NewProcessId'>0x10</Data></EventData></Event>";
        let paths: BTreeMap<_, _> = [("id", "Event/System/EventID"), ("provider", "System/Provider@Name"), ("pid", "EventData/Data[@Name='NewProcessId']")].map(|(a, b)| (a.to_owned(), b.to_owned())).into();
        let c = compile(Extraction::Xml { paths, source: "raw".into() });
        let got = c.run(text).unwrap().unwrap();
        for item in &got.items {
            let (a, b) = item.range.unwrap();
            assert_eq!(&text[a..b], item.value);
        }
        assert_eq!(got.items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), vec!["4688", "0x10", "P"]);
    }
    #[test]
    fn delimited_respects_quotes_escapes_and_reports_positions() {
        let c = compile(Extraction::Delimited { delimiter: ",".into(), quote: Some("\"".into()), columns: vec!["a".into(), "".into(), "c".into()], source: "message".into() });
        let text = "x,\"y,\"\"z\",ação";
        let got = c.run(text).unwrap().unwrap();
        assert_eq!(got.items[0].value, "x");
        assert_eq!(got.items[1].value, "ação");
        let (a, b) = got.items[1].range.unwrap();
        assert_eq!(&text[a..b], "ação");
        assert!(c.run("só,uma").unwrap().is_none());
    }
    #[test]
    fn key_value_handles_quotes_and_selected_keys() {
        let keys: BTreeMap<_, _> = [("usuario", "user"), ("acao", "action")].map(|(a, b)| (a.to_owned(), b.to_owned())).into();
        let c = compile(Extraction::KeyValue { pair_delimiter: String::new(), separator: "=".into(), quote: Some("\"".into()), keys, source: "message".into() });
        let text = "t=1 user=\"ana maria\" action=login other=x";
        let got = c.run(text).unwrap().unwrap();
        assert_eq!(got.items.iter().map(|i| (i.name.as_str(), i.value.as_str())).collect::<Vec<_>>(), vec![("acao", "login"), ("usuario", "ana maria")]);
        for item in &got.items {
            let (a, b) = item.range.unwrap();
            assert_eq!(&text[a..b], item.value);
        }
    }
    #[test]
    fn regex_extraction_needs_named_groups_and_reports_ranges() {
        assert!(Compiled::new(&Extraction::Regex { pattern: "(\\d+)".into(), engine: "pcre2".into(), case_sensitive: true, source: "message".into(), all: false }, &BTreeMap::new()).is_err());
        let c = compile(Extraction::Regex { pattern: "(?<=user=)(?<usuario>\\w+)".into(), engine: "pcre2".into(), case_sensitive: true, source: "message".into(), all: false });
        let got = c.run("😀 user=ana").unwrap().unwrap();
        assert_eq!(got.items[0].value, "ana");
    }
    #[test]
    fn a_regex_can_take_every_occurrence_of_a_record() {
        let all = |pattern: &str, all: bool| compile(Extraction::Regex { pattern: pattern.into(), engine: "pcre2".into(), case_sensitive: true, source: "message".into(), all });
        let text = "Ports: 80/open/tcp, 443/closed/tcp, 8080/open/tcp ação 9090/open/tcp";
        let every = all(r"(?<porta>\d+)/open/", true).run(text).unwrap().unwrap();
        assert_eq!(every.items[0].value, "80, 8080, 9090");
        // the range is the first occurrence, so the page still highlights where it starts
        let (a, b) = every.items[0].range.unwrap();
        assert_eq!(&text[a..b], "80");
        let first = all(r"(?<porta>\d+)/open/", false).run(text).unwrap().unwrap();
        assert_eq!(first.items[0].value, "80");
        assert!(all(r"(?<porta>\d+)/udp/", true).run(text).unwrap().is_none());
        // look-behind sees what came before the previous match, and an empty match cannot loop
        assert_eq!(all(r"(?<=, )(?<porta>\d+)", true).run(text).unwrap().unwrap().items[0].value, "443, 8080");
        assert!(all(r"(?<vazio>x*)", true).run("abc").unwrap().is_some());
        // an old definition (no `all`) reads and writes without the new key
        let old: Extraction = serde_json::from_str(r#"{"kind":"regex","pattern":"(?<a>x)"}"#).unwrap();
        assert!(!serde_json::to_string(&old).unwrap().contains("all"));
    }
    #[test]
    fn grok_reports_the_recognising_alternative_and_oversized_subjects_fail_loudly() {
        let c = compile(Extraction::Grok { patterns: vec!["a=%{INT:a}".into(), "b=%{INT:b}".into()], anchor: "search".into(), case_insensitive: false, dot_all: false, source: "message".into() });
        assert_eq!(c.run("x b=5").unwrap().unwrap().alternative, Some(1));
        assert!(c.run(&"x".repeat(SUBJECT_LIMIT + 1)).is_err());
    }
}
