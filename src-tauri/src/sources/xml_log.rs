//! XML logs: Windows events saved as XML (Event Viewer, `wevtutil /f:xml`,
//! PowerShell), log4j `XMLLayout`, java.util.logging `XMLFormatter` and any
//! file of repeated XML records. The record element is learned from the file
//! (the most frequent element at the shallowest depth); each record keeps its
//! exact text as `raw`, and its attributes and child elements become fields.
use super::stamp::Context;
use crate::model::Event;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag<'a> {
    Start { name: &'a [u8], closed: bool },
    End { name: &'a [u8] },
    Other,
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(&bytes[from.min(bytes.len())..], needle).map(|i| from + i)
}

/// The markup item starting at `<` in position `i`: (end after it, tag).
fn tag_at(bytes: &[u8], i: usize) -> Option<(usize, Tag<'_>)> {
    let rest = &bytes[i..];
    let skip_to = |needle: &[u8]| find(bytes, i, needle).map(|end| (end + needle.len(), Tag::Other));
    if rest.starts_with(b"<!--") {
        return skip_to(b"-->");
    }
    if rest.starts_with(b"<![CDATA[") {
        return skip_to(b"]]>");
    }
    if rest.starts_with(b"<?") {
        return skip_to(b"?>");
    }
    if rest.starts_with(b"<!") {
        return skip_to(b">");
    }
    let closing = rest.get(1) == Some(&b'/');
    let name_start = i + 1 + usize::from(closing);
    let mut j = name_start;
    while j < bytes.len() && !matches!(bytes[j], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
        j += 1;
    }
    let name = &bytes[name_start..j];
    if name.is_empty() || !(name[0].is_ascii_alphabetic() || name[0] == b'_') {
        return None;
    }
    // Attribute values may contain '>' inside quotes.
    let mut quote = 0u8;
    while j < bytes.len() {
        let b = bytes[j];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            let tag = if closing { Tag::End { name } } else { Tag::Start { name, closed: bytes[j - 1] == b'/' } };
            return Some((j + 1, tag));
        }
        j += 1;
    }
    None
}

/// Element repeated as the record of a file: the most frequent name at the
/// shallowest depth where some element repeats (or a lone known record).
pub(super) fn record_element(sample: &[u8]) -> Option<(String, usize)> {
    let start = sample.iter().position(|b| !b.is_ascii_whitespace() && *b != 0xef && *b != 0xbb && *b != 0xbf)?;
    if sample[start] != b'<' {
        return None;
    }
    let mut counts: std::collections::HashMap<(usize, &[u8]), usize> = std::collections::HashMap::new();
    let mut depth = 0usize;
    let mut i = start;
    let mut tags = 0;
    while let Some(lt) = memchr::memchr(b'<', &sample[i..]).map(|n| i + n) {
        let Some((end, tag)) = tag_at(sample, lt) else { break };
        match tag {
            Tag::Start { name, closed } => {
                *counts.entry((depth, name)).or_default() += 1;
                if !closed {
                    depth += 1;
                }
            }
            Tag::End { .. } => depth = depth.saturating_sub(1),
            Tag::Other => {}
        }
        tags += 1;
        if tags > 200_000 {
            break;
        }
        i = end;
    }
    let known = |name: &[u8]| matches!(name, b"Event" | b"record" | b"log4j:event" | b"entry" | b"logEntry" | b"LogEntry");
    let mut repeated: Vec<((usize, &[u8]), usize)> = counts.iter().filter(|(_, &n)| n >= 2).map(|(k, n)| (*k, *n)).collect();
    repeated.sort_by(|a, b| a.0 .0.cmp(&b.0 .0).then(b.1.cmp(&a.1)));
    let chosen = repeated.first().map(|((depth, name), _)| (*depth, *name)).or_else(|| {
        counts.keys().filter(|key| known(key.1)).min_by_key(|key| key.0).copied()
    })?;
    // A repeated element only counts when it carries content, not a flat list
    // of the root's settings: records hold attributes or children.
    let name = String::from_utf8(chosen.1.to_vec()).ok()?;
    Some((name, chosen.0))
}

/// Byte ranges of the records named `name` at `depth`, from `cursor`.
pub(super) struct Scanner<'a> {
    bytes: &'a [u8],
    name: Vec<u8>,
    record_depth: usize,
    pub cursor: usize,
    depth: usize,
}

impl<'a> Scanner<'a> {
    /// `cursor` must be a record boundary: the file start or a record end,
    /// where the depth is the record depth.
    pub(super) fn new(bytes: &'a [u8], name: &str, record_depth: usize, cursor: usize) -> Self {
        let depth = if cursor == 0 { 0 } else { record_depth };
        Self { bytes, name: name.as_bytes().to_vec(), record_depth, cursor, depth }
    }

    pub(super) fn next(&mut self) -> Result<Option<(usize, usize)>, String> {
        let mut begin = None;
        let mut i = self.cursor;
        while let Some(lt) = memchr::memchr(b'<', &self.bytes[i..]).map(|n| i + n) {
            if lt % 65_536 < 64 {
                crate::operations::check()?;
            }
            let Some((end, tag)) = tag_at(self.bytes, lt) else {
                if memchr::memchr(b'>', &self.bytes[lt..]).is_some() {
                    // A stray '<' in text: not markup.
                    i = lt + 1;
                    continue;
                }
                // Cut in the middle of a tag: the partial record is still kept.
                self.cursor = self.bytes.len();
                return Ok(Some((begin.unwrap_or(lt), self.bytes.len())));
            };
            match tag {
                Tag::Start { name, closed } => {
                    if begin.is_none() && self.depth == self.record_depth && name == self.name.as_slice() {
                        if closed {
                            self.cursor = end;
                            return Ok(Some((lt, end)));
                        }
                        begin = Some(lt);
                    }
                    if !closed {
                        self.depth += 1;
                    }
                }
                Tag::End { name } => {
                    self.depth = self.depth.saturating_sub(1);
                    if let Some(start) = begin {
                        if self.depth == self.record_depth && name == self.name.as_slice() {
                            self.cursor = end;
                            return Ok(Some((start, end)));
                        }
                    }
                }
                Tag::Other => {}
            }
            i = end;
        }
        self.cursor = self.bytes.len();
        Ok(begin.map(|b| (b, self.bytes.len())))
    }
}

/// Namespace prefixes a record uses without declaring them (declared on the
/// file's root element, outside the record).
fn wrapped(text: &str) -> String {
    let mut prefixes: Vec<&str> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(lt) = memchr::memchr(b'<', &bytes[i..]).map(|n| i + n) {
        let start = lt + 1 + usize::from(bytes.get(lt + 1) == Some(&b'/'));
        let end = bytes[start..].iter().position(|b| matches!(b, b' ' | b'>' | b'/' | b'\t' | b'\r' | b'\n')).map_or(bytes.len(), |n| start + n);
        if let Some((prefix, _)) = text[start..end].split_once(':') {
            if !prefix.is_empty() && prefix != "xml" && prefix.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') && !prefixes.contains(&prefix) {
                prefixes.push(prefix);
            }
        }
        i = end.max(lt + 1);
    }
    for word in text.split(|c: char| c.is_whitespace() || c == '<') {
        if let Some((prefix, _)) = word.split_once(':').filter(|(_, rest)| rest.contains('=')) {
            if !prefix.is_empty() && prefix != "xmlns" && prefix != "xml" && prefix.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') && !prefixes.contains(&prefix) {
                prefixes.push(prefix);
            }
        }
    }
    let declarations: String = prefixes
        .iter()
        .filter(|p| !text.contains(&format!("xmlns:{p}=")))
        .map(|p| format!(" xmlns:{p}=\"urn:loginsight:{p}\""))
        .collect();
    format!("<loginsight-record{declarations}>{text}</loginsight-record>")
}

fn insert(fields: &mut Map<String, Value>, key: String, value: &str) {
    if key.len() > 512 || fields.len() >= 1024 {
        return;
    }
    match fields.get_mut(&key) {
        None => {
            fields.insert(key, Value::from(value));
        }
        Some(Value::Array(values)) => values.push(Value::from(value)),
        Some(existing) => *existing = Value::Array(vec![existing.clone(), Value::from(value)]),
    }
}

fn element_fields(node: roxmltree::Node, prefix: &str, fields: &mut Map<String, Value>, depth: usize) {
    let join = |name: &str| if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
    for attribute in node.attributes() {
        insert(fields, join(attribute.name()), attribute.value());
    }
    if depth > 12 {
        return;
    }
    let mut text = String::new();
    for child in node.children() {
        if child.is_text() {
            text.push_str(child.text().unwrap_or(""));
            continue;
        }
        if !child.is_element() {
            continue;
        }
        let name = child.tag_name().name();
        // <data name="key" value="value"/> and <Data Name="key">value</Data>.
        let label = child.attribute("name").or_else(|| child.attribute("Name"));
        let elements = child.children().any(|c| c.is_element());
        match (label, child.attribute("value")) {
            (Some(label), Some(value)) if !elements => insert(fields, join(label), value),
            (Some(label), None) if !elements && child.attributes().count() == 1 => {
                insert(fields, join(label), child.text().unwrap_or("").trim())
            }
            _ if !elements && child.attributes().count() == 0 => insert(fields, join(name), child.text().unwrap_or("").trim()),
            _ => element_fields(child, &join(name), fields, depth + 1),
        }
    }
    let text = text.trim();
    if !text.is_empty() && !prefix.is_empty() {
        insert(fields, join("text"), text);
    } else if !text.is_empty() {
        insert(fields, "text".into(), text);
    }
}

/// One XML record as an event: Windows events through the event reader
/// (with the rendered message when the export has one), any other record
/// through the JSON reader over its attributes and elements.
pub(super) fn parse_record(text: &str, ctx: &Context) -> Option<Event> {
    let record = text.trim();
    if !record.starts_with('<') {
        return None;
    }
    let document = wrapped(record);
    let doc = roxmltree::Document::parse(&document).ok()?;
    let element = doc.root_element().children().find(|n| n.is_element())?;
    if element.tag_name().name() == "Event" && element.children().any(|n| n.has_tag_name("System")) {
        let mut ev = super::parse_event_xml(record)?;
        if let Some(rendering) = element.children().find(|n| n.has_tag_name("RenderingInfo")) {
            if let Some(message) = rendering.children().find(|n| n.has_tag_name("Message")).and_then(|n| n.text()) {
                if !message.trim().is_empty() {
                    ev.message = message.trim().to_string();
                }
            }
            for (tag, key) in [("Task", "task"), ("Opcode", "opcode"), ("Keywords", "keywords"), ("Provider", "provider_label")] {
                if let Some(value) = rendering.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()) {
                    ev.fields.entry(key).or_insert_with(|| Value::from(value.trim()));
                }
            }
        }
        ev.raw = text.to_string();
        ev.fields.insert("parser.format".into(), Value::from("xml.windows-event"));
        return Some(ev);
    }
    let mut fields = Map::new();
    element_fields(element, "", &mut fields, 0);
    if fields.is_empty() {
        return None;
    }
    let mut ev = super::event_from_json_at(fields, text, ctx);
    ev.fields.entry("parser.format").or_insert_with(|| Value::from(format!("xml.{}", element.tag_name().name())));
    Some(ev)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Events>\n<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-Security-Auditing' Guid='{54849625-5478-4994-a5ba-3e3b0328c30d}'/><EventID>4625</EventID><Level>0</Level><TimeCreated SystemTime='2026-10-12T03:00:00.1234567Z'/><Channel>Security</Channel><Computer>DC-01.example.test</Computer></System><EventData><Data Name='TargetUserName'>alice</Data><Data Name='IpAddress'>192.0.2.8</Data></EventData><RenderingInfo Culture='pt-BR'><Message>Falha no logon de uma conta.</Message><Level>Informações</Level><Task>Logon</Task></RenderingInfo></Event><Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Service Control Manager'/><EventID Qualifiers='16384'>7036</EventID><Level>4</Level><TimeCreated SystemTime='2026-10-12T03:00:01.000Z'/><Computer>DC-01.example.test</Computer></System><EventData><Data Name='param1'>Spooler</Data></EventData></Event>\n</Events>\n";

    fn records(text: &str) -> Vec<&str> {
        let (name, depth) = record_element(text.as_bytes()).unwrap();
        let mut scanner = Scanner::new(text.as_bytes(), &name, depth, 0);
        let mut out = Vec::new();
        while let Some((begin, end)) = scanner.next().unwrap() {
            out.push(&text[begin..end]);
        }
        out
    }

    #[test]
    fn windows_events_saved_as_xml() {
        assert_eq!(record_element(WINDOWS.as_bytes()), Some(("Event".into(), 1)));
        let found = records(WINDOWS);
        assert_eq!(found.len(), 2);
        let ctx = Context::year(2026);
        let ev = parse_record(found[0], &ctx).unwrap();
        assert_eq!((ev.code.as_str(), ev.source.as_str(), ev.message.as_str()), ("4625", "Microsoft-Windows-Security-Auditing", "Falha no logon de uma conta."));
        assert_eq!(ev.fields["TargetUserName"], "alice");
        assert_eq!(ev.fields["task"], "Logon");
        assert_eq!(ev.timestamp, chrono::DateTime::parse_from_rfc3339("2026-10-12T03:00:00.123Z").ok().map(|t| t.timestamp_millis()));
        let ev = parse_record(found[1], &ctx).unwrap();
        assert_eq!((ev.code.as_str(), ev.fields["param1"].as_str()), ("7036", Some("Spooler")));
    }

    #[test]
    fn log4j_and_java_logging_xml() {
        let log4j = "<log4j:event logger=\"com.example.Service\" timestamp=\"1791774000000\" level=\"ERROR\" thread=\"main\">\n<log4j:message><![CDATA[Falha > limite]]></log4j:message>\n<log4j:properties><log4j:data name=\"user\" value=\"bob\"/></log4j:properties>\n</log4j:event>\n<log4j:event logger=\"com.example.Service\" timestamp=\"1791774001000\" level=\"INFO\" thread=\"main\">\n<log4j:message>ok</log4j:message>\n</log4j:event>\n";
        let found = records(log4j);
        assert_eq!(found.len(), 2);
        let ev = parse_record(found[0], &Context::year(2026)).unwrap();
        assert_eq!((ev.level.as_str(), ev.message.as_str(), ev.source.as_str()), ("Erro", "Falha > limite", "com.example.Service"));
        assert_eq!(ev.timestamp, Some(1791774000000));
        assert_eq!(ev.fields["properties.user"], "bob");

        let jul = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<!DOCTYPE log SYSTEM \"logger.dtd\">\n<log>\n<record>\n  <date>2026-10-12T03:00:00Z</date>\n  <millis>1791774000000</millis>\n  <logger>app</logger>\n  <level>WARNING</level>\n  <message>Disco quase cheio</message>\n</record>\n<record>\n  <date>2026-10-12T03:00:01Z</date>\n  <level>INFO</level>\n  <message>ok</message>\n</record>\n</log>\n";
        let found = records(jul);
        assert_eq!(found.len(), 2);
        let ev = parse_record(found[0], &Context::year(2026)).unwrap();
        assert_eq!((ev.level.as_str(), ev.message.as_str()), ("Aviso", "Disco quase cheio"));
        assert!(ev.timestamp.is_some());
    }

    #[test]
    fn records_without_a_root_or_cut_at_the_end() {
        let text = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><EventID>1</EventID><TimeCreated SystemTime='2026-10-12T03:00:00Z'/></System></Event><Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><EventID>2</EventID><TimeCreated SystemTime='2026-10-12T03:00:01Z'/></System></Event><Event xmlns='http://schemas";
        let found = records(text);
        assert_eq!(found.len(), 3);
        assert!(parse_record(found[2], &Context::year(2026)).is_none(), "the cut record stays raw text");
    }
}
