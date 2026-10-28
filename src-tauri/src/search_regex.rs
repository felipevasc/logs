//! One native matcher for searching, previews, highlights and derived captures.
//! PCRE2 deliberately runs without JIT: interpreter limits and automatic callouts
//! bound backtracking, including a cancelled match in the middle of one subject.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ptr::NonNull,
    time::{Duration, Instant},
};

pub(crate) const SUBJECT_LIMIT: usize = 8 << 20;
const MATCHES_LIMIT: usize = 512;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Pattern {
    #[serde(default = "default_engine")]
    pub engine: String,
    pub text: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub multiline: bool,
    #[serde(default)]
    pub dot_all: bool,
}
fn default_engine() -> String {
    "literal".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Span {
    /// UTF-16 offsets for DOM slicing. Byte offsets remain available for source navigation.
    pub start: usize,
    pub end: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub captures: BTreeMap<String, String>,
    /// Per-group ranges reported by the engine. Only filled by detailed matching.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupRange>,
}
/// One capture group of one occurrence, exactly as the matcher reported it.
/// A group that did not take part carries no range and no value.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupRange {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    pub participated: bool,
    pub start: usize,
    pub end: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub value_truncated: bool,
}
/// Static description of a pattern's groups (number and every name bound to it).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupInfo {
    pub index: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Matches {
    pub spans: Vec<Span>,
    pub complete: bool,
    /// Which budget stopped the work when `complete` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<&'static str>,
}
impl Matches {
    fn done(spans: Vec<Span>, complete: bool) -> Self {
        Self {
            spans,
            complete,
            limit: (!complete).then_some("occurrences"),
        }
    }
}
/// Compilation failure with its position. PCRE2 reports bytes; the editor needs UTF-16.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostic {
    pub message: String,
    pub byte_offset: Option<usize>,
    pub offset: Option<usize>,
}
impl Diagnostic {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            byte_offset: None,
            offset: None,
        }
    }
    fn at(pattern: &str, byte: usize, message: String) -> Self {
        let byte = byte.min(pattern.len());
        let offset = pattern
            .is_char_boundary(byte)
            .then(|| pattern[..byte].encode_utf16().count());
        Self {
            message,
            byte_offset: Some(byte),
            offset,
        }
    }
}
/// Ranges reported per response, across every occurrence and group.
const GROUP_RANGES_LIMIT: usize = 8192;
const GROUP_VALUE_LIMIT: usize = 4096;
/// Version of the PCRE2 library linked into this build.
pub(crate) fn pcre2_version() -> String {
    let mut buffer = [0u8; 64];
    let written = unsafe {
        pcre2_sys::pcre2_config_8(pcre2_sys::PCRE2_CONFIG_VERSION, buffer.as_mut_ptr().cast())
    };
    if written < 0 {
        return "desconhecida".into();
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}
/// UTF-16 position of any byte offset without rescanning the subject each time.
struct Utf16Index<'a> {
    text: &'a str,
    ascii: bool,
    marks: Vec<(usize, usize)>,
}
impl<'a> Utf16Index<'a> {
    fn new(text: &'a str) -> Self {
        let ascii = text.is_ascii();
        let mut marks = Vec::new();
        if !ascii {
            let (mut units, mut next) = (0usize, 0usize);
            for (byte, ch) in text.char_indices() {
                if byte >= next {
                    marks.push((byte, units));
                    next = byte + 1024;
                }
                units += ch.len_utf16();
            }
        }
        Self { text, ascii, marks }
    }
    fn at(&self, byte: usize) -> Option<usize> {
        if self.ascii {
            return (byte <= self.text.len()).then_some(byte);
        }
        if byte > self.text.len() || !self.text.is_char_boundary(byte) {
            return None;
        }
        let at = self.marks.partition_point(|(b, _)| *b <= byte);
        let (base, units) = at.checked_sub(1).map_or((0, 0), |i| self.marks[i]);
        Some(units + self.text[base..byte].encode_utf16().count())
    }
}
enum Kind {
    Fast(regex::Regex),
    Pcre(Pcre),
}
pub(crate) struct Matcher {
    kind: Kind,
}
struct Pcre {
    code: NonNull<pcre2_sys::pcre2_code_8>,
    names: Vec<(usize, String)>,
    captures: usize,
}
// PCRE2 compiled code is immutable. Every invocation owns match data and context.
unsafe impl Send for Pcre {}
unsafe impl Sync for Pcre {}
impl Drop for Pcre {
    fn drop(&mut self) {
        unsafe {
            pcre2_sys::pcre2_code_free_8(self.code.as_ptr());
        }
    }
}
fn message(code: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { pcre2_sys::pcre2_get_error_message_8(code, buf.as_mut_ptr(), buf.len()) };
    if n > 0 {
        String::from_utf8_lossy(&buf[..n as usize]).into_owned()
    } else {
        format!("PCRE2 {code}")
    }
}
struct Deadline {
    until: Instant,
    ticks: u32,
}
unsafe extern "C" {
    fn pcre2_set_callout_8(
        context: *mut pcre2_sys::pcre2_match_context_8,
        callback: Option<unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32>,
        data: *mut std::ffi::c_void,
    ) -> i32;
}
unsafe extern "C" fn callout(_: *mut std::ffi::c_void, data: *mut std::ffi::c_void) -> i32 {
    // The pointer is installed only for the synchronous pcre2_match_8 below.
    let state = unsafe { &mut *(data as *mut Deadline) };
    state.ticks = state.ticks.wrapping_add(1);
    if state.ticks & 255 == 0 && (Instant::now() >= state.until || crate::operations::cancelled()) {
        -37
    } else {
        0
    }
}
impl Matcher {
    pub(crate) fn new(pattern: &Pattern) -> Result<Self, String> {
        Self::with_limits(
            pattern,
            crate::analysis_context::RegexLimits {
                size_limit: 2 << 20,
                dfa_size_limit: 512 << 10,
            },
        )
    }
    pub(crate) fn with_limits(
        pattern: &Pattern,
        limits: crate::analysis_context::RegexLimits,
    ) -> Result<Self, String> {
        Self::build(pattern, limits, 16_384).map_err(|d| d.message)
    }
    /// Machine-generated patterns (an expanded Grok pattern) may exceed the 16 KiB a
    /// person can type; the compiled-size budget still applies.
    pub(crate) fn expanded(pattern: &Pattern, max_len: usize) -> Result<Self, String> {
        Self::build(
            pattern,
            crate::analysis_context::RegexLimits { size_limit: 2 << 20, dfa_size_limit: 512 << 10 },
            max_len,
        )
        .map_err(|d| d.message)
    }
    /// Compile with a positioned diagnostic. Messages keep the historical wording.
    pub(crate) fn diagnose(pattern: &Pattern) -> Result<Self, Diagnostic> {
        Self::build(
            pattern,
            crate::analysis_context::RegexLimits {
                size_limit: 2 << 20,
                dfa_size_limit: 512 << 10,
            },
            16_384,
        )
    }
    fn build(
        pattern: &Pattern,
        limits: crate::analysis_context::RegexLimits,
        max_len: usize,
    ) -> Result<Self, Diagnostic> {
        if pattern.text.is_empty() || pattern.text.len() > max_len {
            return Err(Diagnostic::plain(format!("Informe um padrão de até {} KiB.", max_len >> 10)));
        }
        crate::operations::check().map_err(Diagnostic::plain)?;
        match pattern.engine.as_str() {
            "literal" | "rust" => {
                let text = if pattern.engine == "literal" {
                    regex::escape(&pattern.text)
                } else {
                    pattern.text.clone()
                };
                let re = regex::RegexBuilder::new(&text)
                    .case_insensitive(!pattern.case_sensitive)
                    .multi_line(pattern.multiline)
                    .dot_matches_new_line(pattern.dot_all)
                    .size_limit(limits.size_limit)
                    .dfa_size_limit(limits.dfa_size_limit)
                    .build()
                    .map_err(|e| Diagnostic::plain(e.to_string()))?;
                if re.captures_len() > 129 {
                    return Err(Diagnostic::plain("Use até 128 grupos de captura."));
                }
                Ok(Self {
                    kind: Kind::Fast(re),
                })
            }
            "pcre2" => unsafe {
                use pcre2_sys::*;
                let mut error = 0;
                let mut offset = 0;
                let mut options =
                    PCRE2_UTF | PCRE2_UCP | PCRE2_AUTO_CALLOUT | PCRE2_NEVER_BACKSLASH_C;
                if !pattern.case_sensitive {
                    options |= PCRE2_CASELESS;
                }
                if pattern.multiline {
                    options |= PCRE2_MULTILINE;
                }
                if pattern.dot_all {
                    options |= PCRE2_DOTALL;
                }
                let code = NonNull::new(pcre2_compile_8(
                    pattern.text.as_ptr(),
                    pattern.text.len(),
                    options,
                    &mut error,
                    &mut offset,
                    std::ptr::null_mut(),
                ))
                .ok_or_else(|| {
                    Diagnostic::at(
                        &pattern.text,
                        offset,
                        format!("PCRE2, byte {offset}: {}", message(error)),
                    )
                })?;
                let mut pcre = Pcre {
                    code,
                    names: Vec::new(),
                    captures: 0,
                };
                let mut compiled_bytes = 0usize;
                pcre2_pattern_info_8(
                    code.as_ptr(),
                    PCRE2_INFO_SIZE,
                    (&mut compiled_bytes as *mut usize).cast(),
                );
                if compiled_bytes > limits.size_limit {
                    return Err(Diagnostic::plain(
                        "O padrão PCRE2 excede o orçamento compartilhado de compilação do Caso.",
                    ));
                }
                let mut captures = 0u32;
                pcre2_pattern_info_8(
                    code.as_ptr(),
                    PCRE2_INFO_CAPTURECOUNT,
                    (&mut captures as *mut u32).cast(),
                );
                if captures > 128 {
                    return Err(Diagnostic::plain("Use até 128 grupos de captura."));
                }
                pcre.captures = captures as usize;
                let mut count = 0u32;
                let mut width = 0u32;
                let mut table: *const u8 = std::ptr::null();
                pcre2_pattern_info_8(
                    code.as_ptr(),
                    PCRE2_INFO_NAMECOUNT,
                    (&mut count as *mut u32).cast(),
                );
                pcre2_pattern_info_8(
                    code.as_ptr(),
                    PCRE2_INFO_NAMEENTRYSIZE,
                    (&mut width as *mut u32).cast(),
                );
                pcre2_pattern_info_8(
                    code.as_ptr(),
                    PCRE2_INFO_NAMETABLE,
                    (&mut table as *mut *const u8).cast(),
                );
                for i in 0..count as usize {
                    let entry =
                        std::slice::from_raw_parts(table.add(i * width as usize), width as usize);
                    let number = ((entry[0] as usize) << 8) | entry[1] as usize;
                    let end = entry[2..]
                        .iter()
                        .position(|b| *b == 0)
                        .unwrap_or(entry.len() - 2)
                        + 2;
                    pcre.names
                        .push((number, String::from_utf8_lossy(&entry[2..end]).into_owned()));
                }
                Ok(Self {
                    kind: Kind::Pcre(pcre),
                })
            },
            _ => Err(Diagnostic::plain(
                "Motor desconhecido: use literal, rust ou pcre2.",
            )),
        }
    }
    pub(crate) fn find(&self, text: &str) -> Result<Matches, String> {
        self.find_impl(text, true, false, None, false)
    }
    /// Occurrences with the engine-reported range of every group.
    pub(crate) fn find_detailed(&self, text: &str) -> Result<Matches, String> {
        self.find_impl(text, true, false, None, true)
    }
    /// Groups of the compiled pattern with every name bound to each number.
    pub(crate) fn groups(&self) -> Vec<GroupInfo> {
        match &self.kind {
            Kind::Fast(re) => re
                .capture_names()
                .enumerate()
                .map(|(index, name)| GroupInfo {
                    index,
                    names: name.map(str::to_owned).into_iter().collect(),
                })
                .collect(),
            Kind::Pcre(p) => (0..=p.captures)
                .map(|index| GroupInfo {
                    index,
                    names: p
                        .names
                        .iter()
                        .filter(|(number, _)| *number == index)
                        .map(|(_, name)| name.clone())
                        .collect(),
                })
                .collect(),
        }
    }
    /// Whether any occurrence exists. `lines` tests each physical line on its own.
    /// Failures (limits, deadline, cancellation) are errors, never "no match".
    pub(crate) fn is_match(&self, text: &str, lines: bool) -> Result<bool, String> {
        if let Kind::Fast(re) = &self.kind {
            if !lines {
                return Ok(re.is_match(text));
            }
            for part in text.split_inclusive('\n') {
                if re.is_match(part.trim_end_matches(['\r', '\n'])) {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if !lines {
            return Ok(!self.find_impl(text, false, true, None, false)?.spans.is_empty());
        }
        if text.len() > SUBJECT_LIMIT {
            return Err("O registro excede 8 MiB. Busca incompleta; use um recorte menor.".into());
        }
        for part in text.split_inclusive('\n') {
            let part = part.trim_end_matches(['\r', '\n']);
            if !self.find_impl(part, false, true, None, false)?.spans.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// Byte ranges of the first occurrence: index 0 is the whole match, then every group
    /// (`None` = did not take part). Lean path for bulk extraction: no values, no UTF-16.
    pub(crate) fn first_ranges(&self, text: &str) -> Result<Option<Vec<Option<(usize, usize)>>>, String> {
        self.ranges_from(text, 0)
    }
    /// `first_ranges` for the first occurrence at or after byte `start` of `text` (look-behind
    /// still sees what precedes `start`).
    fn ranges_from(&self, text: &str, start: usize) -> Result<Option<Vec<Option<(usize, usize)>>>, String> {
        if text.len() > SUBJECT_LIMIT {
            return Err("O registro excede 8 MiB. Busca incompleta; use um recorte menor.".into());
        }
        crate::operations::check()?;
        match &self.kind {
            Kind::Fast(re) => Ok(re.captures_at(text, start).map(|cap| {
                (0..cap.len()).map(|i| cap.get(i).map(|m| (m.start(), m.end()))).collect()
            })),
            Kind::Pcre(pcre) => unsafe {
                use pcre2_sys::*;
                struct Scratch(*mut pcre2_match_context_8, *mut pcre2_match_data_8);
                impl Drop for Scratch {
                    fn drop(&mut self) {
                        unsafe {
                            pcre2_match_context_free_8(self.0);
                            pcre2_match_data_free_8(self.1);
                        }
                    }
                }
                let scratch = Scratch(
                    pcre2_match_context_create_8(std::ptr::null_mut()),
                    pcre2_match_data_create_from_pattern_8(pcre.code.as_ptr(), std::ptr::null_mut()),
                );
                if scratch.0.is_null() || scratch.1.is_null() {
                    return Err("PCRE2 sem memória para esta busca.".into());
                }
                pcre2_set_match_limit_8(scratch.0, 1_000_000);
                pcre2_set_depth_limit_8(scratch.0, 1000);
                pcre2_set_heap_limit_8(scratch.0, 8 * 1024);
                let mut deadline = Deadline { until: Instant::now() + Duration::from_millis(100), ticks: 0 };
                pcre2_set_callout_8(scratch.0, Some(callout), (&mut deadline as *mut Deadline).cast());
                let rc = pcre2_match_8(pcre.code.as_ptr(), text.as_ptr(), text.len(), start, 0, scratch.1, scratch.0);
                if rc == PCRE2_ERROR_NOMATCH {
                    return Ok(None);
                }
                if rc < 0 {
                    crate::operations::check()?;
                    return Err(format!("Busca PCRE2 incompleta: {}. Reduza o padrão ou o contexto.", message(rc)));
                }
                let pairs = std::slice::from_raw_parts(
                    pcre2_get_ovector_pointer_8(scratch.1),
                    pcre2_get_ovector_count_8(scratch.1) as usize * 2,
                );
                let ranges = pairs
                    .chunks_exact(2)
                    .map(|p| (p[0] != usize::MAX && p[0] <= p[1] && p[1] <= text.len()).then_some((p[0], p[1])))
                    .collect();
                Ok(Some(ranges))
            },
        }
    }
    /// Every non-overlapping occurrence (at most `limit`), each as in `first_ranges`. An empty
    /// match moves on by one character, so the loop always ends.
    pub(crate) fn all_ranges(&self, text: &str, limit: usize) -> Result<Vec<Vec<Option<(usize, usize)>>>, String> {
        let mut found = Vec::new();
        let mut at = 0usize;
        while at <= text.len() && found.len() < limit {
            let Some(ranges) = self.ranges_from(text, at)? else { break };
            let Some((a, b)) = ranges.first().copied().flatten() else { break };
            at = if b > a { b } else { text[b..].chars().next().map_or(text.len() + 1, |c| b + c.len_utf8()) };
            found.push(ranges);
        }
        Ok(found)
    }
    pub(crate) fn is_pcre2(&self) -> bool {
        matches!(self.kind, Kind::Pcre(_))
    }
    pub(crate) fn capture_exists(&self, name: &str) -> bool {
        match &self.kind {
            Kind::Fast(re) => name
                .parse::<usize>()
                .map(|n| n < re.captures_len())
                .unwrap_or_else(|_| re.capture_names().flatten().any(|s| s == name)),
            Kind::Pcre(p) => name
                .parse::<usize>()
                .map(|n| n <= p.captures)
                .unwrap_or_else(|_| p.names.iter().any(|(_, s)| s == name)),
        }
    }
    pub(crate) fn find_spans(&self, text: &str) -> Result<Matches, String> {
        self.find_impl(text, false, false, None, false)
    }
    pub(crate) fn find_lines(&self, text: &str) -> Result<Matches, String> {
        self.find_lines_with(text, false)
    }
    /// Per physical line, optionally with group ranges shifted to whole-text positions.
    pub(crate) fn find_lines_with(&self, text: &str, detailed: bool) -> Result<Matches, String> {
        if text.len() > SUBJECT_LIMIT {
            return Err("O evento excede 8 MiB na busca por linhas.".into());
        }
        let started = Instant::now();
        let mut spans = Vec::new();
        let (mut bytes, mut units) = (0, 0);
        let mut ranges = 0usize;
        let mut complete = true;
        for part in text.split_inclusive('\n') {
            crate::operations::check()?;
            if started.elapsed() > Duration::from_millis(200) {
                return Err(
                    "A busca por linhas excedeu 200 ms neste evento; resultado incompleto.".into(),
                );
            }
            let line = part.trim_end_matches(['\r', '\n']);
            let found = if detailed {
                self.find_impl(line, true, false, None, true)?
            } else {
                self.find_spans(line)?
            };
            complete &= found.complete;
            if matches!(found.limit, Some("groups") | Some("values")) {
                return Ok(Matches {
                    spans,
                    complete: false,
                    limit: found.limit,
                });
            }
            for mut span in found.spans {
                if spans.len() == MATCHES_LIMIT {
                    return Ok(Matches::done(spans, false));
                }
                ranges += span.groups.len();
                if ranges > GROUP_RANGES_LIMIT {
                    return Ok(Matches {
                        spans,
                        complete: false,
                        limit: Some("groups"),
                    });
                }
                span.start += units;
                span.end += units;
                span.byte_start += bytes;
                span.byte_end += bytes;
                for group in &mut span.groups {
                    if group.participated {
                        group.start += units;
                        group.end += units;
                        group.byte_start += bytes;
                        group.byte_end += bytes;
                    }
                }
                spans.push(span);
            }
            bytes += part.len();
            units += part.encode_utf16().count();
        }
        Ok(Matches {
            spans,
            complete,
            limit: (!complete).then_some("occurrences"),
        })
    }
    pub(crate) fn extract(&self, text: &str, name: &str) -> Result<Matches, String> {
        self.find_impl(text, true, true, Some(name), false)
    }
    fn find_impl(
        &self,
        text: &str,
        capture_values: bool,
        first_only: bool,
        requested: Option<&str>,
        detailed: bool,
    ) -> Result<Matches, String> {
        if text.len() > SUBJECT_LIMIT {
            return Err("O registro excede 8 MiB. Busca incompleta; use um recorte menor.".into());
        }
        crate::operations::check()?;
        let mut spans = Vec::new();
        let (mut previous_byte, mut previous_units) = (0, 0);
        let mut span = |a: usize, b: usize, captures| {
            let start = previous_units + text[previous_byte..a].encode_utf16().count();
            let end = start + text[a..b].encode_utf16().count();
            previous_byte = b;
            previous_units = end;
            Span {
                start,
                end,
                byte_start: a,
                byte_end: b,
                captures,
                groups: Vec::new(),
            }
        };
        let mut remaining = 2usize << 20;
        let infos = if detailed { self.groups() } else { Vec::new() };
        let index = detailed.then(|| Utf16Index::new(text));
        let mut ranges_used = 0usize;
        // Ranges of groups 1.. as `(byte_start, byte_end)`; `None` did not participate.
        let mut collect = |ranges: &[Option<(usize, usize)>],
                           remaining: &mut usize|
         -> Result<Vec<GroupRange>, &'static str> {
            let Some(index) = &index else {
                return Ok(Vec::new());
            };
            let mut groups = Vec::with_capacity(ranges.len());
            for (offset, range) in ranges.iter().enumerate() {
                let number = offset + 1;
                ranges_used += 1;
                if ranges_used > GROUP_RANGES_LIMIT {
                    return Err("groups");
                }
                let names = infos.get(number).map(|i| i.names.clone()).unwrap_or_default();
                let found = range.and_then(|(a, b)| {
                    if a <= b
                        && b <= text.len()
                        && text.is_char_boundary(a)
                        && text.is_char_boundary(b)
                    {
                        Some((a, b, index.at(a)?, index.at(b)?))
                    } else {
                        None
                    }
                });
                let Some((a, b, start, end)) = found else {
                    groups.push(GroupRange {
                        index: number,
                        names,
                        participated: false,
                        start: 0,
                        end: 0,
                        byte_start: 0,
                        byte_end: 0,
                        value: None,
                        value_truncated: false,
                    });
                    continue;
                };
                let mut cut = b.min(a + GROUP_VALUE_LIMIT);
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                if cut > *remaining {
                    return Err("values");
                }
                *remaining -= cut - a;
                groups.push(GroupRange {
                    index: number,
                    names,
                    participated: true,
                    start,
                    end,
                    byte_start: a,
                    byte_end: b,
                    value: Some(text[a..cut].to_owned()),
                    value_truncated: cut < b,
                });
            }
            Ok(groups)
        };
        let mut stopped: Option<&'static str> = None;
        fn insert(
            values: &mut BTreeMap<String, String>,
            key: String,
            value: &str,
            left: &mut usize,
        ) -> Result<(), String> {
            let bytes = key.len() + value.len();
            if bytes > *left {
                return Err("Capturas excedem 2 MiB; reduza a amostra ou os grupos.".into());
            }
            *left -= bytes;
            values.insert(key, value.to_owned());
            Ok(())
        }
        match &self.kind {
            Kind::Fast(re) => {
                for cap in re.captures_iter(text).take(MATCHES_LIMIT + 1) {
                    crate::operations::check()?;
                    if spans.len() == MATCHES_LIMIT {
                        return Ok(Matches::done(spans, false));
                    }
                    let m = cap.get(0).unwrap();
                    let mut values = BTreeMap::new();
                    if capture_values {
                        for (i, value) in cap
                            .iter()
                            .enumerate()
                            .filter_map(|(i, m)| m.map(|m| (i, m.as_str())))
                        {
                            if requested.is_some_and(|name| name != i.to_string()) {
                                continue;
                            }
                            insert(&mut values, i.to_string(), value, &mut remaining)?;
                        }
                        for name in re.capture_names().flatten() {
                            if requested.is_some_and(|wanted| wanted != name) {
                                continue;
                            }
                            if let Some(m) = cap.name(name) {
                                insert(&mut values, name.into(), m.as_str(), &mut remaining)?;
                            }
                        }
                    }
                    let mut made = span(m.start(), m.end(), values);
                    if detailed {
                        let ranges: Vec<_> = (1..cap.len())
                            .map(|i| cap.get(i).map(|g| (g.start(), g.end())))
                            .collect();
                        match collect(&ranges, &mut remaining) {
                            Ok(groups) => made.groups = groups,
                            Err(limit) => {
                                stopped = Some(limit);
                                break;
                            }
                        }
                    }
                    spans.push(made);
                    if first_only {
                        break;
                    }
                }
            }
            Kind::Pcre(pcre) => unsafe {
                use pcre2_sys::*;
                struct Scratch(*mut pcre2_match_context_8, *mut pcre2_match_data_8);
                impl Drop for Scratch {
                    fn drop(&mut self) {
                        unsafe {
                            pcre2_match_context_free_8(self.0);
                            pcre2_match_data_free_8(self.1);
                        }
                    }
                }
                let scratch = Scratch(
                    pcre2_match_context_create_8(std::ptr::null_mut()),
                    pcre2_match_data_create_from_pattern_8(
                        pcre.code.as_ptr(),
                        std::ptr::null_mut(),
                    ),
                );
                if scratch.0.is_null() || scratch.1.is_null() {
                    return Err("PCRE2 sem memória para esta busca.".into());
                }
                pcre2_set_match_limit_8(scratch.0, 1_000_000);
                pcre2_set_depth_limit_8(scratch.0, 1000);
                pcre2_set_heap_limit_8(scratch.0, 8 * 1024); // KiB
                let mut deadline = Deadline {
                    until: Instant::now() + Duration::from_millis(100),
                    ticks: 0,
                };
                pcre2_set_callout_8(
                    scratch.0,
                    Some(callout),
                    (&mut deadline as *mut Deadline).cast(),
                );
                let mut at = 0;
                while at <= text.len() {
                    crate::operations::check()?;
                    let rc = pcre2_match_8(
                        pcre.code.as_ptr(),
                        text.as_ptr(),
                        text.len(),
                        at,
                        0,
                        scratch.1,
                        scratch.0,
                    );
                    if rc == PCRE2_ERROR_NOMATCH {
                        break;
                    }
                    if rc < 0 {
                        crate::operations::check()?;
                        return Err(format!(
                            "Busca PCRE2 incompleta: {}. Reduza o padrão ou o contexto.",
                            message(rc)
                        ));
                    }
                    if spans.len() == MATCHES_LIMIT {
                        return Ok(Matches::done(spans, false));
                    }
                    let pairs = std::slice::from_raw_parts(
                        pcre2_get_ovector_pointer_8(scratch.1),
                        pcre2_get_ovector_count_8(scratch.1) as usize * 2,
                    );
                    let a = pairs[0];
                    let b = pairs[1];
                    if a > b || b > text.len() {
                        return Err("PCRE2 informou uma correspondência inválida.".into());
                    }
                    let mut values = BTreeMap::new();
                    if capture_values {
                        for (i, pair) in pairs.chunks_exact(2).enumerate() {
                            if requested.is_some_and(|wanted| {
                                wanted != i.to_string()
                                    && !pcre
                                        .names
                                        .iter()
                                        .any(|(number, name)| *number == i && name == wanted)
                            }) {
                                continue;
                            }
                            if pair[0] != usize::MAX && pair[1] <= text.len() {
                                insert(
                                    &mut values,
                                    i.to_string(),
                                    &text[pair[0]..pair[1]],
                                    &mut remaining,
                                )?;
                            }
                        }
                        for (number, name) in &pcre.names {
                            if requested.is_some_and(|wanted| wanted != name) {
                                continue;
                            }
                            if let Some(v) = values.get(&number.to_string()).cloned() {
                                insert(&mut values, name.clone(), &v, &mut remaining)?;
                            }
                        }
                    }
                    let mut made = span(a, b, values);
                    if detailed {
                        let ranges: Vec<_> = pairs
                            .chunks_exact(2)
                            .skip(1)
                            .map(|pair| (pair[0] != usize::MAX).then_some((pair[0], pair[1])))
                            .collect();
                        match collect(&ranges, &mut remaining) {
                            Ok(groups) => made.groups = groups,
                            Err(limit) => {
                                stopped = Some(limit);
                                break;
                            }
                        }
                    }
                    spans.push(made);
                    if first_only {
                        break;
                    }
                    at = if b > a {
                        b
                    } else if b == text.len() {
                        break;
                    } else {
                        b + text[b..].chars().next().unwrap().len_utf8()
                    };
                }
            },
        }
        Ok(Matches {
            spans,
            complete: stopped.is_none(),
            limit: stopped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(text: &str) -> Pattern {
        Pattern {
            engine: "pcre2".into(),
            text: text.into(),
            case_sensitive: true,
            multiline: false,
            dot_all: false,
        }
    }
    #[test]
    fn lookbehind_backreference_named_and_unicode_offsets() {
        let matches = Matcher::new(&p(r"(?<=id=)(?<v>\w+)-\k<v>"))
            .unwrap()
            .find("😀 id=ação-ação tail")
            .unwrap();
        assert_eq!(matches.spans.len(), 1);
        let s = &matches.spans[0];
        assert_eq!(s.start, 6);
        assert_eq!(s.captures["v"], "ação");
        assert_eq!(
            &"😀 id=ação-ação tail"[s.byte_start..s.byte_end],
            "ação-ação"
        );
    }
    #[test]
    fn pathological_pattern_refuses_instead_of_false_zero() {
        let matcher = Matcher::new(&p(r"(*NO_START_OPT)(a+)+$")).unwrap();
        assert!(matcher.find(&format!("{}!", "a".repeat(20000))).is_err());
    }
    #[test]
    fn empty_matches_advance_and_limit_is_explicit() {
        let matches = Matcher::new(&p(r"(?=.)"))
            .unwrap()
            .find(&"á".repeat(600))
            .unwrap();
        assert_eq!(matches.spans.len(), 512);
        assert!(!matches.complete);
    }
    #[test]
    fn extraction_only_copies_the_requested_group() {
        let matcher = Matcher::new(&p(r"(?s).*user=(?<user>\w+)")).unwrap();
        let text = format!("{}user=alice", "x".repeat(3 << 20));
        let found = matcher.extract(&text, "user").unwrap();
        assert_eq!(found.spans[0].captures["user"], "alice");
        assert!(!found.spans[0].captures.contains_key("0"));
    }
    #[test]
    fn physical_lines_trim_crlf_keep_anchors_and_shift_unicode_offsets() {
        for engine in ["rust", "pcre2"] {
            let matcher = Matcher::new(&Pattern {
                engine: engine.into(),
                ..p("^ação$")
            })
            .unwrap();
            let text = "😀 header\r\nação\r\nação\nend";
            assert!(matcher.find_spans(text).unwrap().spans.is_empty());
            let found = matcher.find_lines(text).unwrap();
            assert!(found.complete);
            assert_eq!(found.spans.len(), 2);
            assert_eq!(found.spans[0].start, 11);
            for span in found.spans {
                assert_eq!(&text[span.byte_start..span.byte_end], "ação");
            }
        }
    }
}
