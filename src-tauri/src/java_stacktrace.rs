//! Bounded structural interpretation of an already-framed Java trace.
//!
//! This module never changes record boundaries or original text. All spans are
//! UTF-8 byte offsets in the caller's unchanged block. `complete` means that the
//! observed trace was understood within the limits, not that a log producer
//! necessarily printed every frame or that the original file was not cut off.
use serde::Serialize;
use sha2::{Digest, Sha256};

const ENVELOPE_CREDIT: usize = 4096;
const NODE_CREDIT: usize = 384;
const FRAME_CREDIT: usize = 512;
const MAX_DIAGNOSTICS: usize = 16;

/// Enrichment only: never include this revision in record-framing identities.
pub(crate) fn enrichment_signature(format: &str) -> Option<&'static str> {
    matches!(format, "log4j" | "wildfly").then_some("java-trace-v1")
}

pub(crate) const COLUMNS: &[&str] = &[
    "java.trace.complete",
    "java.trace.fingerprint",
    "java.exception.class",
    "java.exception.message",
    "java.root_cause.class",
];

pub(crate) fn extend_columns(format: &str, columns: &mut Vec<String>) {
    if enrichment_signature(format).is_some() {
        for &column in COLUMNS {
            if !columns.iter().any(|value| value == column) {
                columns.push(column.to_owned());
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub input_bytes: usize,
    pub nodes: usize,
    pub frames: usize,
    pub depth: usize,
    pub string_bytes: usize,
    pub structured_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: 256 << 10,
            nodes: 64,
            frames: 1024,
            depth: 32,
            string_bytes: 4096,
            structured_bytes: 256 << 10,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Relation {
    Root,
    Cause,
    Suppressed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Node {
    pub relation: Relation,
    pub parent: Option<usize>,
    pub depth: usize,
    pub header: Span,
    pub class: String,
    pub message: Option<String>,
    pub frame_start: usize,
    pub frame_count: usize,
    pub elided_frames: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Location {
    Native,
    Unknown,
    File { name: String, line: Option<u32> },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrameParts {
    pub class: String,
    pub method: String,
    pub loader: Option<String>,
    pub module: Option<String>,
    pub module_version: Option<String>,
    pub location: Location,
}

#[derive(Debug, Serialize)]
pub(crate) struct Frame {
    pub node: Option<usize>,
    pub span: Span,
    /// None explicitly preserves an unparsed frame; never guess its parts.
    pub parts: Option<FrameParts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Code {
    InputLimit,
    NodeLimit,
    FrameLimit,
    DepthLimit,
    StringLimit,
    PayloadLimit,
    UnknownLine,
    UnknownFrame,
    OrphanFrame,
    InvalidIndent,
    InvalidElision,
    CircularReference,
    MultipleRoots,
    MissingRoot,
    Unrecognized,
}

#[derive(Debug, Serialize)]
pub(crate) struct Diagnostic {
    pub code: Code,
    pub span: Option<Span>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Trace {
    /// Keeps a diagnosed trace visible even when its first allocation hit a
    /// limit. This parser-admission fact is not part of the public wire schema.
    #[serde(skip)]
    observed: bool,
    pub schema_version: u8,
    pub span_basis: &'static str,
    pub complete: bool,
    pub root: Option<usize>,
    pub nodes: Vec<Node>,
    pub frames: Vec<Frame>,
    pub diagnostics: Vec<Diagnostic>,
    pub fingerprint_version: u8,
    /// Exception-shape grouping only, never an evidence or source identity.
    pub fingerprint: Option<String>,
}
impl Trace {
    fn empty() -> Self {
        Self {
            observed: false,
            schema_version: 1,
            span_basis: "utf8_bytes_in_raw_block",
            complete: true,
            root: None,
            nodes: vec![],
            frames: vec![],
            diagnostics: vec![],
            fingerprint_version: 1,
            fingerprint: None,
        }
    }
    fn issue(&mut self, code: Code, span: Option<Span>) {
        self.complete = false;
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(Diagnostic { code, span });
        }
    }
    pub(crate) fn exception_class(&self) -> Option<&str> {
        self.root
            .and_then(|root| self.nodes.get(root))
            .map(|node| node.class.as_str())
    }
    pub(crate) fn exception_message(&self) -> Option<&str> {
        self.root
            .and_then(|root| self.nodes.get(root))
            .and_then(|node| node.message.as_deref())
    }
    pub(crate) fn observed(&self) -> bool {
        self.observed
    }
    /// Small first-class fields for ordinary Events and indexes. The full
    /// structure belongs only to explicit detail metadata, never a hidden field.
    pub(crate) fn scalar_fields(&self) -> serde_json::Map<String, serde_json::Value> {
        use serde_json::Value;
        let mut fields = serde_json::Map::new();
        if !self.observed {
            return fields;
        }
        fields.insert("java.trace.complete".into(), Value::Bool(self.complete));
        for (column, value) in [
            ("java.exception.class", self.exception_class()),
            ("java.exception.message", self.exception_message()),
            ("java.root_cause.class", self.root_cause_class()),
            ("java.trace.fingerprint", self.fingerprint.as_deref()),
        ] {
            if let Some(value) = value {
                fields.insert(column.into(), Value::String(value.to_owned()));
            }
        }
        fields
    }
    /// Follow only the complete observed main cause chain, not suppressed peers.
    pub(crate) fn root_cause_class(&self) -> Option<&str> {
        if !self.complete {
            return None;
        }
        let mut current = self.root?;
        for _ in 0..self.nodes.len() {
            match self
                .nodes
                .iter()
                .position(|node| node.parent == Some(current) && node.relation == Relation::Cause)
            {
                Some(next) => current = next,
                None => return Some(self.nodes[current].class.as_str()),
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidStart,
    InvalidLimits,
    Cancelled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidStart => "O início do stacktrace não é um limite UTF-8 válido.",
            Self::InvalidLimits => "O orçamento estrutural do stacktrace é inválido.",
            Self::Cancelled => "Leitura do stacktrace cancelada.",
        })
    }
}

struct Budget {
    limits: Limits,
    credit: usize,
}
impl Budget {
    fn charge(&mut self, bytes: usize) -> Result<(), Code> {
        self.credit = self.credit.checked_add(bytes).ok_or(Code::PayloadLimit)?;
        if self.credit > self.limits.structured_bytes {
            return Err(Code::PayloadLimit);
        }
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<String, Code> {
        if value.len() > self.limits.string_bytes {
            return Err(Code::StringLimit);
        }
        // Six bytes per input byte bounds JSON escaping, with room for the
        // retained allocation. Fixed node/frame credits cover scalar members.
        self.charge(
            value
                .len()
                .checked_mul(6)
                .and_then(|n| n.checked_add(32))
                .ok_or(Code::PayloadLimit)?,
        )?;
        Ok(value.to_owned())
    }
    fn optional(&mut self, value: Option<&str>) -> Result<Option<String>, Code> {
        value.map(|value| self.string(value)).transpose()
    }
}

fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}
fn class_name(value: &str) -> bool {
    !value.is_empty() && value.split('.').all(identifier)
}
fn header(text: &str) -> Option<(&str, Option<&str>)> {
    let text = if let Some(tail) = text.strip_prefix("Exception in thread \"") {
        tail.split_once("\" ")?.1
    } else {
        text
    };
    let (class, message) = match text.split_once(':') {
        Some((class, value)) => (class, Some(value.strip_prefix(' ').unwrap_or(value))),
        None => (text, None),
    };
    class_name(class).then_some((class, message))
}
fn familiar_throwable(class: &str) -> bool {
    (class.contains('.') || class.contains('$'))
        && ["Exception", "Error", "Throwable"]
            .iter()
            .any(|suffix| class.ends_with(suffix))
}
fn indentation(line: &str) -> (usize, &str) {
    let mut columns = 0usize;
    let mut bytes = 0usize;
    for byte in line.bytes() {
        match byte {
            b' ' => columns = columns.saturating_add(1),
            b'\t' => columns = columns.saturating_add(8 - columns % 8),
            _ => break,
        }
        bytes += 1;
    }
    (columns, &line[bytes..])
}

fn frame_parts(text: &str, budget: &mut Budget) -> Result<Option<FrameParts>, Code> {
    let Some(text) = text.strip_prefix("at ") else {
        return Ok(None);
    };
    let text = text.trim_start_matches(' ');
    let Some(text) = text.strip_suffix(')') else {
        return Ok(None);
    };
    let Some((member, location)) = text.rsplit_once('(') else {
        return Ok(None);
    };
    if location.is_empty() {
        return Ok(None);
    }
    let pieces: Vec<_> = member.splitn(4, '/').collect();
    let (loader, module, qualified) = match pieces.as_slice() {
        [qualified] => (None, None, *qualified),
        [module, qualified] if !module.is_empty() => (None, Some(*module), *qualified),
        [loader, module, qualified] if !loader.is_empty() => (
            Some(*loader),
            (!module.is_empty()).then_some(*module),
            *qualified,
        ),
        _ => return Ok(None),
    };
    let Some((class, method)) = qualified.rsplit_once('.') else {
        return Ok(None);
    };
    if !class_name(class) || !(identifier(method) || matches!(method, "<init>" | "<clinit>")) {
        return Ok(None);
    }
    let (module, version) = match module {
        Some(value) => match value.split_once('@') {
            Some((name, version)) if !name.is_empty() && !version.is_empty() => {
                (Some(name), Some(version))
            }
            Some(_) => return Ok(None),
            None => (Some(value), None),
        },
        None => (None, None),
    };
    let location = match location {
        "Native Method" => Location::Native,
        "Unknown Source" => Location::Unknown,
        _ => {
            let (name, line) = match location.rsplit_once(':') {
                Some((name, number)) if !name.is_empty() => match number.parse::<u32>() {
                    Ok(line) => (name, Some(line)),
                    Err(_) => (location, None),
                },
                _ => (location, None),
            };
            Location::File {
                name: budget.string(name)?,
                line,
            }
        }
    };
    Ok(Some(FrameParts {
        class: budget.string(class)?,
        method: budget.string(method)?,
        loader: budget.optional(loader)?,
        module: budget.optional(module)?,
        module_version: budget.optional(version)?,
        location,
    }))
}

/// Parse a suffix of the unchanged raw block. A later source adapter can pass
/// the message/body start while keeping every returned span in raw coordinates.
pub(crate) fn parse(block: &str, start: usize, limits: Limits) -> Result<Trace, Error> {
    parse_with_cancel(block, start, limits, &|| false)
}

pub(crate) fn parse_with_cancel(
    block: &str,
    start: usize,
    limits: Limits,
    cancelled: &dyn Fn() -> bool,
) -> Result<Trace, Error> {
    if cancelled() {
        return Err(Error::Cancelled);
    }
    if start > block.len() || !block.is_char_boundary(start) {
        return Err(Error::InvalidStart);
    }
    if limits.structured_bytes < ENVELOPE_CREDIT {
        return Err(Error::InvalidLimits);
    }
    let mut trace = Trace::empty();
    let mut budget = Budget {
        limits,
        credit: ENVELOPE_CREDIT,
    };
    let mut end = start.saturating_add(limits.input_bytes).min(block.len());
    while !block.is_char_boundary(end) {
        end -= 1;
    }
    if end < block.len() {
        trace.issue(Code::InputLimit, Some(Span { start: end, end }));
        // Never interpret a byte-limited partial line as a complete header/frame.
        if !block[start..end].ends_with('\n') && block.as_bytes().get(end) != Some(&b'\n') {
            let partial = block[start..end].rsplit('\n').next().unwrap_or("");
            let (_, partial) = indentation(partial);
            // Presence may be established from a bounded, complete class name
            // before a cut message. Do not retain or parse the partial line.
            trace.observed = partial.starts_with("at ")
                || partial.starts_with("Caused by: ")
                || partial.starts_with("Suppressed: ")
                || partial.starts_with("Exception in thread \"")
                || header(partial).is_some_and(|(class, _)| familiar_throwable(class));
            end = block[start..end]
                .rfind('\n')
                .map_or(start, |at| start + at + 1);
        }
    }
    let mut active: Vec<(usize, usize)> = Vec::new();
    let mut offset = start;
    let mut structural_evidence = false;
    let mut lines = block[start..end].split_inclusive('\n');
    while let Some(raw_line) = lines.next() {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let span = Span {
            start: offset,
            end: offset + line.len(),
        };
        offset += raw_line.len();
        let (indent, text) = indentation(line);
        if text.is_empty() {
            continue;
        }
        let circular = text
            .strip_prefix("Caused by: ")
            .or_else(|| text.strip_prefix("Suppressed: "))
            .unwrap_or(text);
        if circular.starts_with("[CIRCULAR REFERENCE:") && circular.ends_with(']') {
            trace.observed = true;
            trace.issue(Code::CircularReference, Some(span));
            continue;
        }
        if text.starts_with("at ") {
            trace.observed = true;
            if active
                .last()
                .is_some_and(|(_, node)| trace.nodes[*node].elided_frames.is_some())
            {
                trace.issue(Code::InvalidElision, Some(span));
                break;
            }
            if trace.frames.len() >= limits.frames {
                trace.issue(Code::FrameLimit, Some(span));
                break;
            }
            if let Err(code) = budget.charge(FRAME_CREDIT) {
                trace.issue(code, Some(span));
                break;
            }
            let parts = match frame_parts(text, &mut budget) {
                Ok(parts) => parts,
                Err(code) => {
                    trace.issue(code, Some(span));
                    break;
                }
            };
            structural_evidence |= parts.is_some();
            if parts.is_none() {
                trace.issue(Code::UnknownFrame, Some(span));
            }
            let mut node = active.last().map(|(_, node)| *node);
            let mut invalid_indent = false;
            if let Some((level, node)) = active.last() {
                if indent <= *level {
                    trace.issue(Code::InvalidIndent, Some(span));
                    invalid_indent = true;
                } else {
                    trace.nodes[*node].frame_count += 1;
                }
            } else {
                trace.issue(Code::OrphanFrame, Some(span));
            }
            if invalid_indent {
                node = None;
            }
            trace.frames.push(Frame { node, span, parts });
            // Do not attach an outdented frame to the last suppressed/cause
            // node or manufacture a discontinuous frame range after it.
            if invalid_indent {
                break;
            }
            continue;
        }
        if text.starts_with("...") {
            let count = text
                .strip_prefix("... ")
                .and_then(|value| value.strip_suffix(" more"))
                .and_then(|value| value.parse::<u32>().ok());
            match (active.last(), count) {
                (Some((level, node)), Some(count))
                    if indent > *level && trace.nodes[*node].elided_frames.is_none() =>
                {
                    trace.nodes[*node].elided_frames = Some(count);
                    // Java's printed elision refers to the enclosing trace.
                    // Retain the reported count verbatim, but never call an
                    // impossible or root-level elision complete.
                    let enclosing = trace.nodes[*node].parent.map(|parent| {
                        trace.nodes[parent].frame_count as u64
                            + u64::from(trace.nodes[parent].elided_frames.unwrap_or(0))
                    });
                    if enclosing.is_none_or(|available| u64::from(count) > available) {
                        trace.issue(Code::InvalidElision, Some(span));
                    }
                }
                _ => trace.issue(Code::InvalidElision, Some(span)),
            }
            continue;
        }
        let (relation, value) = if let Some(value) = text.strip_prefix("Caused by: ") {
            (Relation::Cause, value)
        } else if let Some(value) = text.strip_prefix("Suppressed: ") {
            (Relation::Suppressed, value)
        } else {
            (Relation::Root, text)
        };
        trace.observed |= relation != Relation::Root;
        let Some((class, message)) = header(value) else {
            if trace.root.is_some() {
                trace.issue(Code::UnknownLine, Some(span));
            }
            if relation != Relation::Root {
                // An explicit child header establishes a new, unknown owner.
                // Later frames must never be attributed to the prior node.
                active.clear();
            }
            continue;
        };
        if relation == Relation::Root
            && trace.root.is_none()
            && !familiar_throwable(class)
            && !value.starts_with("Exception in thread \"")
        {
            // A single-word application message is not enough to name a
            // Throwable. A custom/default-package class needs adjacent trace
            // structure. Borrowed look-ahead is bounded by the inspected input
            // and cannot create an owned line list or a second parse tree.
            let next = lines.clone().find_map(|line| {
                let (_, value) = indentation(line.trim_end_matches(['\r', '\n']));
                (!value.is_empty()).then_some(value)
            });
            if !next.is_some_and(|value| {
                value.starts_with("at ")
                    || value.starts_with("Caused by: ")
                    || value.starts_with("Suppressed: ")
            }) {
                continue;
            }
        }
        trace.observed = true;
        if relation == Relation::Root && trace.root.is_some() {
            trace.issue(Code::MultipleRoots, Some(span));
            break;
        }
        let parent = match relation {
            Relation::Root => None,
            Relation::Cause => match active.iter().rposition(|(level, _)| *level == indent) {
                Some(at) => {
                    let parent = active[at].1;
                    active.truncate(at);
                    Some(parent)
                }
                None => {
                    trace.issue(Code::InvalidIndent, Some(span));
                    break;
                }
            },
            Relation::Suppressed => match active.iter().rposition(|(level, _)| *level < indent) {
                Some(at) => {
                    let parent = active[at].1;
                    active.truncate(at + 1);
                    Some(parent)
                }
                None => {
                    trace.issue(Code::InvalidIndent, Some(span));
                    break;
                }
            },
        };
        if trace.nodes.len() >= limits.nodes {
            trace.issue(Code::NodeLimit, Some(span));
            break;
        }
        let depth = parent.map_or(1, |parent| trace.nodes[parent].depth.saturating_add(1));
        if depth > limits.depth {
            trace.issue(Code::DepthLimit, Some(span));
            break;
        }
        if let Err(code) = budget.charge(NODE_CREDIT) {
            trace.issue(code, Some(span));
            break;
        }
        let retained = budget
            .string(class)
            .and_then(|class| budget.optional(message).map(|message| (class, message)));
        let (class, message) = match retained {
            Ok(values) => values,
            Err(code) => {
                trace.issue(code, Some(span));
                break;
            }
        };
        structural_evidence |= relation != Relation::Root
            || familiar_throwable(&class)
            || value.starts_with("Exception in thread \"");
        let index = trace.nodes.len();
        trace.nodes.push(Node {
            relation,
            parent,
            depth,
            header: span,
            class,
            message,
            frame_start: trace.frames.len(),
            frame_count: 0,
            elided_frames: None,
        });
        if relation == Relation::Root {
            trace.root = Some(index);
        }
        active.push((indent, index));
    }
    if trace.root.is_none() {
        trace.issue(Code::MissingRoot, None);
    }
    if !structural_evidence {
        trace.issue(Code::Unrecognized, None);
        trace.root = None;
    }
    if cancelled() {
        return Err(Error::Cancelled);
    }
    if trace.complete {
        trace.fingerprint = Some(fingerprint(&trace, cancelled)?);
    }
    Ok(trace)
}

fn fingerprint(trace: &Trace, cancelled: &dyn Fn() -> bool) -> Result<String, Error> {
    fn field(hash: &mut Sha256, value: &[u8]) {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    let mut hash = Sha256::new();
    field(&mut hash, b"java-stack-shape-v1");
    field(&mut hash, &(trace.nodes.len() as u64).to_be_bytes());
    for (index, node) in trace.nodes.iter().enumerate() {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        field(
            &mut hash,
            &[match node.relation {
                Relation::Root => 0,
                Relation::Cause => 1,
                Relation::Suppressed => 2,
            }],
        );
        field(
            &mut hash,
            &node.parent.map_or(u64::MAX, |n| n as u64).to_be_bytes(),
        );
        field(&mut hash, node.class.as_bytes());
        field(
            &mut hash,
            &node.elided_frames.map_or(u64::MAX, u64::from).to_be_bytes(),
        );
        field(&mut hash, &(node.frame_count as u64).to_be_bytes());
        for frame in trace
            .frames
            .iter()
            .filter(|frame| frame.node == Some(index))
        {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let Some(parts) = &frame.parts else { continue };
            for value in [&parts.class, &parts.method] {
                field(&mut hash, value.as_bytes());
            }
            for value in [parts.loader.as_deref(), parts.module.as_deref()] {
                field(&mut hash, &[u8::from(value.is_some())]);
                if let Some(value) = value {
                    field(&mut hash, value.as_bytes());
                }
            }
            field(
                &mut hash,
                &[u8::from(matches!(parts.location, Location::Native))],
            );
        }
    }
    // Prefix + Base32 is one guaranteed nonhex word, shorter than the current
    // text index's64-byte LONG threshold. No standalone hexadecimal word leaks.
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut output = String::from("jvone");
    let mut buffer = 0u32;
    let mut bits = 0usize;
    for byte in hash.finalize() {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAIN: &str = "Exception in thread \"main\" com.acme.TopException: café\r\n\
\tat loader/module@1/com.acme.Top.run(Top.java:10)\r\n\
\tSuppressed: com.acme.CloseFailure: closing\r\n\
\t\tat com.acme.Close.close(Native Method)\r\n\
\tCaused by: com.acme.SuppCause: suppressed cause\r\n\
\t\tat com.acme.Other.open(Unknown Source)\r\n\
\t\t... 1 more\r\n\
Caused by: com.acme.BottomProblem: actual main cause\r\n\
\tat com.acme.Bottom.run(Bottom.java)\r\n\
\t... 1 more";

    fn parsed(text: &str) -> Trace {
        parse(text, 0, Limits::default()).unwrap()
    }
    fn has(trace: &Trace, code: Code) -> bool {
        trace
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
    }
    fn bounded(text: &str, limits: Limits, code: Code) -> Trace {
        let trace = parse(text, 0, limits).unwrap();
        assert!(has(&trace, code), "{:?}", trace.diagnostics);
        assert!(!trace.complete);
        assert!(trace.fingerprint.is_none());
        assert!(trace.root_cause_class().is_none());
        assert!(trace.nodes.len() <= limits.nodes);
        assert!(trace.frames.len() <= limits.frames);
        assert!(serde_json::to_vec(&trace).unwrap().len() <= limits.structured_bytes);
        trace
    }

    #[test]
    fn cause_and_suppressed_arenas_preserve_relations_and_elision_without_expansion() {
        let trace = parsed(CHAIN);
        assert!(trace.complete, "{:?}", trace.diagnostics);
        assert_eq!(trace.nodes.len(), 4);
        assert_eq!(trace.frames.len(), 4);
        assert_eq!(trace.root, Some(0));
        assert_eq!(trace.exception_class(), Some("com.acme.TopException"));
        assert_eq!(trace.exception_message(), Some("café"));
        assert_eq!(trace.root_cause_class(), Some("com.acme.BottomProblem"));
        assert_eq!(
            trace
                .nodes
                .iter()
                .map(|node| (node.relation, node.parent, node.depth))
                .collect::<Vec<_>>(),
            [
                (Relation::Root, None, 1),
                (Relation::Suppressed, Some(0), 2),
                (Relation::Cause, Some(1), 3),
                (Relation::Cause, Some(0), 2)
            ]
        );
        assert_eq!(trace.nodes[2].elided_frames, Some(1));
        assert_eq!(trace.nodes[3].elided_frames, Some(1));
        for (index, node) in trace.nodes.iter().enumerate() {
            assert_eq!(node.frame_count, 1);
            assert_eq!(node.frame_start, index);
            assert_eq!(trace.frames[index].node, Some(index));
        }
        assert_eq!(
            &CHAIN[trace.frames[0].span.start..trace.frames[0].span.end],
            "\tat loader/module@1/com.acme.Top.run(Top.java:10)"
        );
    }

    #[test]
    fn modern_native_unknown_and_line_unavailable_frames_remain_distinct() {
        let trace = parsed(CHAIN);
        let first = trace.frames[0].parts.as_ref().unwrap();
        assert_eq!(first.class, "com.acme.Top");
        assert_eq!(first.method, "run");
        assert_eq!(first.loader.as_deref(), Some("loader"));
        assert_eq!(first.module.as_deref(), Some("module"));
        assert_eq!(first.module_version.as_deref(), Some("1"));
        assert_eq!(
            first.location,
            Location::File {
                name: "Top.java".into(),
                line: Some(10)
            }
        );
        assert_eq!(
            trace.frames[1].parts.as_ref().unwrap().location,
            Location::Native
        );
        assert_eq!(
            trace.frames[2].parts.as_ref().unwrap().location,
            Location::Unknown
        );
        assert_eq!(
            trace.frames[3].parts.as_ref().unwrap().location,
            Location::File {
                name: "Bottom.java".into(),
                line: None
            }
        );
        let other = parsed("a.CustomFailure\n\tat custom.loader//a.Clazz.<init>(Clazz.java:2)\n\tat java.base@25/java.lang.Object.<clinit>(Unknown Source)\n");
        assert!(other.complete);
        assert_eq!(
            other.frames[0].parts.as_ref().unwrap().loader.as_deref(),
            Some("custom.loader")
        );
        assert!(other.frames[0].parts.as_ref().unwrap().module.is_none());
        assert!(other.frames[1].parts.as_ref().unwrap().loader.is_none());
        assert_eq!(
            other.frames[1].parts.as_ref().unwrap().module.as_deref(),
            Some("java.base")
        );
    }

    #[test]
    fn custom_throwables_need_structure_and_generic_error_messages_are_not_roots() {
        let custom = parsed("DefaultPackageProblem: details\n\tat Service.run(Service.java:4)\n");
        assert!(custom.complete);
        assert_eq!(custom.exception_class(), Some("DefaultPackageProblem"));
        for message in [
            "Error",
            "Information: Exception processing completed",
            "Error while opening the file",
            "ordinary message",
        ] {
            let trace = parsed(message);
            assert!(!trace.complete);
            assert!(trace.root.is_none());
            assert!(trace.exception_class().is_none());
            assert!(trace.fingerprint.is_none());
        }
        assert!(
            parsed("java.lang.Exception").complete,
            "a Throwable may omit writable frames"
        );
    }

    #[test]
    fn unknown_frames_or_lines_do_not_invent_parts_or_a_complete_root_cause() {
        let text = "a.FailureException: failure\n\tat malformed frame\nforeign logger text\n";
        let trace = parsed(text);
        assert_eq!(trace.exception_class(), Some("a.FailureException"));
        assert!(has(&trace, Code::UnknownFrame));
        assert!(has(&trace, Code::UnknownLine));
        assert!(trace.frames[0].parts.is_none());
        assert_eq!(
            &text[trace.frames[0].span.start..trace.frames[0].span.end],
            "\tat malformed frame"
        );
        assert!(trace.root_cause_class().is_none());
        assert!(trace.fingerprint.is_none());
        let orphan = parsed("\tat a.Service.run(Service.java:1)\n");
        assert!(has(&orphan, Code::OrphanFrame));
        assert_eq!(orphan.frames[0].node, None);
        assert!(orphan.root.is_none());
    }

    #[test]
    fn malformed_child_headers_invalidate_previous_frame_ownership() {
        for header in ["Caused by: ???", "\tSuppressed: ???"] {
            let text = format!("a.RootException\n\tat a.Root.before(A.java:1)\n{header}\n\t\tat child.Service.after(B.java:2)\n");
            let trace = parsed(&text);
            assert_eq!(trace.nodes.len(), 1);
            assert_eq!(trace.nodes[0].frame_count, 1);
            assert_eq!(trace.frames.len(), 2);
            assert_eq!(trace.frames[0].node, Some(0));
            assert_eq!(trace.frames[1].node, None);
            assert_eq!(
                trace.frames[1].parts.as_ref().unwrap().class,
                "child.Service"
            );
            assert!(has(&trace, Code::UnknownLine));
            assert!(has(&trace, Code::OrphanFrame));
            assert!(!trace.complete);
            assert!(trace.root_cause_class().is_none());
            assert!(trace.fingerprint.is_none());
        }
    }

    #[test]
    fn ambiguous_new_roots_and_malformed_indentation_are_not_merged() {
        let trace = parsed("a.FirstException\n\tat a.Service.one(A.java:1)\nb.SecondException\n\tat b.Service.two(B.java:2)\n");
        assert!(has(&trace, Code::MultipleRoots));
        assert_eq!(trace.nodes.len(), 1);
        assert!(trace.fingerprint.is_none());
        let trace = parsed(
            "a.FirstException\nSuppressed: b.SecondException\n\tat b.Service.two(B.java:2)\n",
        );
        assert!(has(&trace, Code::InvalidIndent));
        assert_eq!(trace.nodes.len(), 1);
        assert!(trace.root_cause_class().is_none());
    }

    #[test]
    fn ordinary_preamble_cannot_steal_the_following_exception_header() {
        let trace = parsed("Failure\nInformation: handled request\n\na.RealException: detail\n\tat a.Service.run(A.java:1)\n");
        assert!(trace.complete, "{:?}", trace.diagnostics);
        assert_eq!(trace.exception_class(), Some("a.RealException"));
        assert_eq!(trace.nodes.len(), 1);
        let custom = parsed("CustomProblem: detail\n\n\tat a.Service.run(A.java:1)\n");
        assert!(custom.complete);
        assert_eq!(custom.exception_class(), Some("CustomProblem"));
    }

    #[test]
    fn outdented_frames_and_frames_after_elision_do_not_invent_node_ownership() {
        let text = "a.FirstException\n\tat a.Service.run(A.java:1)\n\tSuppressed: a.CloseException\n\t\tat a.Close.close(A.java:2)\n\tat a.Service.extra(A.java:3)\n";
        let trace = parsed(text);
        assert!(has(&trace, Code::InvalidIndent));
        assert_eq!(trace.frames.len(), 3);
        assert_eq!(trace.frames[2].node, None);
        assert_eq!(trace.nodes[0].frame_count, 1);
        assert_eq!(trace.nodes[1].frame_count, 1);
        assert!(trace.fingerprint.is_none());
        assert_eq!(
            &text[trace.frames[2].span.start..trace.frames[2].span.end],
            "\tat a.Service.extra(A.java:3)"
        );
        let trace = parsed("a.FirstException\n\tat a.Service.run(A.java:1)\nCaused by: a.LastException\n\t... 1 more\n\tat a.Service.extra(A.java:2)\n");
        assert!(has(&trace, Code::InvalidElision));
        assert_eq!(trace.frames.len(), 1);
        assert_eq!(trace.nodes[1].frame_count, 0);
        assert!(trace.root_cause_class().is_none());
    }

    #[test]
    fn span_coordinates_remain_original_utf8_bytes_after_a_header() {
        let prefix = "cabeçalho: 日 — ignorado\n";
        let original = format!("{prefix}{CHAIN}");
        let saved = original.clone();
        let trace = parse(&original, prefix.len(), Limits::default()).unwrap();
        assert_eq!(trace.span_basis, "utf8_bytes_in_raw_block");
        assert_eq!(trace.nodes[0].header.start, prefix.len());
        for span in trace
            .nodes
            .iter()
            .map(|node| node.header)
            .chain(trace.frames.iter().map(|frame| frame.span))
        {
            assert!(original.is_char_boundary(span.start));
            assert!(original.is_char_boundary(span.end));
            assert!(span.start >= prefix.len() && span.end <= original.len());
        }
        assert_eq!(original, saved);
        assert_eq!(
            parse("é", 1, Limits::default()).unwrap_err(),
            Error::InvalidStart
        );
    }

    #[test]
    fn individual_node_frame_and_graph_depth_limits_are_explicit() {
        bounded(
            CHAIN,
            Limits {
                nodes: 1,
                ..Limits::default()
            },
            Code::NodeLimit,
        );
        bounded(
            CHAIN,
            Limits {
                frames: 1,
                ..Limits::default()
            },
            Code::FrameLimit,
        );
        bounded(
            CHAIN,
            Limits {
                depth: 2,
                ..Limits::default()
            },
            Code::DepthLimit,
        );
        bounded(
            CHAIN,
            Limits {
                string_bytes: 4,
                ..Limits::default()
            },
            Code::StringLimit,
        );
        let causes =
            "a.FirstException\nCaused by: a.SecondException\nCaused by: a.ThirdException\n";
        let trace = bounded(
            causes,
            Limits {
                depth: 2,
                ..Limits::default()
            },
            Code::DepthLimit,
        );
        assert_eq!(
            trace.nodes.len(),
            2,
            "same-indent causes still consume graph depth"
        );
    }

    #[test]
    fn input_budget_never_parses_a_partial_utf8_line_as_a_complete_trace() {
        let text = "a.FailureException: root\n\tat a.Service.run(Fiché.java:4)\n";
        let cut = text.find('é').unwrap() + 1;
        let trace = bounded(
            text,
            Limits {
                input_bytes: cut,
                ..Limits::default()
            },
            Code::InputLimit,
        );
        assert!(trace.frames.is_empty());
        assert_eq!(trace.exception_class(), Some("a.FailureException"));
        for diagnostic in trace.diagnostics {
            if let Some(span) = diagnostic.span {
                assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
            }
        }
        bounded(
            text,
            Limits {
                input_bytes: 0,
                ..Limits::default()
            },
            Code::InputLimit,
        );
    }

    #[test]
    fn structured_credit_bounds_output_even_with_escaped_strings_and_many_frames() {
        let mut text = "a.FailureException: quotes\\\\ \" \t 日\n".to_string();
        for _ in 0..1000 {
            text.push_str("\tat a.Service.run(A.java:1)\n");
        }
        let limit = Limits {
            structured_bytes: 12 << 10,
            ..Limits::default()
        };
        let trace = bounded(&text, limit, Code::PayloadLimit);
        assert!(!trace.frames.is_empty());
        assert!(serde_json::to_vec(&trace).unwrap().len() <= limit.structured_bytes);
        assert_eq!(
            parse(
                "a.FailureException",
                0,
                Limits {
                    structured_bytes: 10,
                    ..Limits::default()
                }
            )
            .unwrap_err(),
            Error::InvalidLimits
        );
        let text = "a.FailureException\n".to_string() + &"unrecognized text\n".repeat(100);
        assert_eq!(parsed(&text).diagnostics.len(), MAX_DIAGNOSTICS);
    }

    #[test]
    fn huge_elision_is_retained_but_never_expands_or_claims_inferred_frames() {
        let trace = parsed("a.FirstException\n\tat a.Service.run(A.java:1)\nCaused by: b.SecondException\n\tat b.Service.run(B.java:2)\n\t... 4294967295 more\n");
        assert!(has(&trace, Code::InvalidElision));
        assert!(!trace.complete);
        assert_eq!(trace.nodes[1].elided_frames, Some(u32::MAX));
        assert_eq!(trace.frames.len(), 2);
        assert!(trace.fingerprint.is_none());
        for line in [
            "\t... -1 more",
            "\t... 4294967296 more",
            "\t... unknown more",
        ] {
            let trace = parsed(&format!("a.FirstException\n{line}\n"));
            assert!(has(&trace, Code::InvalidElision));
            assert!(trace.fingerprint.is_none());
        }
    }

    #[test]
    fn circular_markers_are_diagnostics_but_literal_message_text_is_preserved() {
        let message =
            "a.FirstException: text [CIRCULAR REFERENCE: literal]\n\tat a.Service.run(A.java:1)\n";
        let trace = parsed(message);
        assert!(trace.complete);
        assert_eq!(
            trace.exception_message(),
            Some("text [CIRCULAR REFERENCE: literal]")
        );
        let circular =
            parsed(&(message.to_owned() + "Caused by: [CIRCULAR REFERENCE: a.FirstException]\n"));
        assert!(has(&circular, Code::CircularReference));
        assert!(circular.root_cause_class().is_none());
        assert!(circular.fingerprint.is_none());
    }

    #[test]
    fn shape_fingerprint_ignores_messages_source_locations_and_module_versions() {
        let a =
            parsed("a.FailureException: secret=alpha\n\tat loader/mod@1/a.Service.run(A.java:1)\n");
        let b = parsed(
            "a.FailureException: secret=beta\n\tat loader/mod@2/a.Service.run(Other.java:999)\n",
        );
        assert_eq!(a.fingerprint, b.fingerprint);
        let changed = parsed(
            "a.FailureException: secret=alpha\n\tat loader/mod@1/a.Service.stop(A.java:1)\n",
        );
        assert_ne!(a.fingerprint, changed.fingerprint);
        let token = a.fingerprint.as_deref().unwrap();
        assert_eq!(token.len(), 57);
        assert!(token.starts_with("jvone"));
        assert!(token.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        assert!(!token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(token.len() <= 64, "must not become a LONG text-index word");
    }

    #[test]
    fn cancellation_is_an_error_and_never_an_ordinary_incomplete_result() {
        assert_eq!(
            parse_with_cancel(CHAIN, 0, Limits::default(), &|| true).unwrap_err(),
            Error::Cancelled
        );
        let calls = std::cell::Cell::new(0);
        let cancelled = || {
            let previous = calls.get();
            calls.set(previous + 1);
            previous >= 3
        };
        assert_eq!(
            parse_with_cancel(CHAIN, 0, Limits::default(), &cancelled).unwrap_err(),
            Error::Cancelled
        );
        let calls = std::cell::Cell::new(0);
        let fingerprint_cancel = || {
            let previous = calls.get();
            calls.set(previous + 1);
            previous >= CHAIN.lines().count() + 2
        };
        assert_eq!(
            parse_with_cancel(CHAIN, 0, Limits::default(), &fingerprint_cancel).unwrap_err(),
            Error::Cancelled,
            "cancellation during fingerprinting is still an operation error"
        );
        assert!(
            parsed(CHAIN).complete,
            "no hidden operation state survives cancellation"
        );
    }

    #[test]
    fn enrichment_fields_match_typed_structure_and_do_not_invent_missing_summaries() {
        let complete = parsed(CHAIN);
        let fields = complete.scalar_fields();
        assert_eq!(fields["java.trace.complete"], true);
        assert_eq!(fields["java.exception.class"], "com.acme.TopException");
        assert_eq!(fields["java.exception.message"], "café");
        assert_eq!(fields["java.root_cause.class"], "com.acme.BottomProblem");
        assert!(!fields.contains_key("java.trace"));
        let metadata = serde_json::to_value(&complete).unwrap();
        assert_eq!(metadata["complete"], fields["java.trace.complete"]);
        assert_eq!(fields["java.trace.fingerprint"], metadata["fingerprint"]);
        let incomplete = parsed("a.FailureException\n\tat unknown frame\n").scalar_fields();
        assert_eq!(incomplete["java.trace.complete"], false);
        assert!(!incomplete.contains_key("java.trace.fingerprint"));
        assert!(!incomplete.contains_key("java.root_cause.class"));
        assert!(!incomplete.contains_key("java.exception.message"));
        assert!(parsed("ordinary application message")
            .scalar_fields()
            .is_empty());
        assert!(
            parsed("").scalar_fields().is_empty(),
            "missing raw is unavailable"
        );
    }

    #[test]
    fn declared_columns_and_enrichment_revision_are_java_only() {
        for format in [
            "apache",
            "jsonl",
            "snapshot",
            "text",
            "custom",
            "syslog3164",
        ] {
            assert!(enrichment_signature(format).is_none());
            let mut columns = vec!["message".to_owned()];
            extend_columns(format, &mut columns);
            assert_eq!(columns, ["message"]);
        }
        for format in ["log4j", "wildfly"] {
            assert_eq!(enrichment_signature(format), Some("java-trace-v1"));
            let mut columns = vec!["message".to_owned()];
            extend_columns(format, &mut columns);
            extend_columns(format, &mut columns);
            assert_eq!(columns.len(), COLUMNS.len() + 1);
            for &column in COLUMNS {
                assert!(columns.iter().any(|value| value == column));
            }
        }
    }

    #[test]
    fn recognized_trace_limits_remain_visible_even_before_the_first_node() {
        let long = format!("a.FailureException: {}", "x".repeat(8192));
        let trace = parsed(&long);
        assert!(trace.nodes.is_empty());
        assert!(has(&trace, Code::StringLimit));
        let fields = trace.scalar_fields();
        assert_eq!(fields["java.trace.complete"], false);
        assert!(serde_json::to_value(&trace).unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "string_limit"));
        assert!(!fields.contains_key("java.exception.class"));
        let limits = Limits {
            input_bytes: 100,
            ..Limits::default()
        };
        let trace = parse(&long, 0, limits).unwrap();
        assert!(trace.nodes.is_empty());
        assert!(has(&trace, Code::InputLimit));
        assert_eq!(trace.scalar_fields()["java.trace.complete"], false);
        let ordinary = "an ordinary message ".repeat(100);
        assert!(parse(&ordinary, 0, limits)
            .unwrap()
            .scalar_fields()
            .is_empty());
        let no_nodes = parse(
            "a.FailureException",
            0,
            Limits {
                nodes: 0,
                ..Limits::default()
            },
        )
        .unwrap();
        assert_eq!(no_nodes.scalar_fields()["java.trace.complete"], false);
        assert!(has(&no_nodes, Code::NodeLimit));
    }
}
