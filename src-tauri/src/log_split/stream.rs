//! One pass over the file: items are framed in order, their text conditions
//! evaluated in parallel batches, and each item handed to a sink. Regions
//! whose result is already known (before a date found by binary search, after
//! a range is passed) go to the sink as byte ranges, without being framed.
use super::codec::{trim_break, Codec};
use super::framing::{self, blank, starts_with_date, Framing, Reader, HEAD};
use super::rule::{self, DateReader, Facts, Rule, Running};
use super::spec::{FramingSpec, SplitSpec};
use crate::sources::stamp;
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SAMPLE: usize = 256 * 1024;
/// Bytes read around a binary-search probe.
const PROBE: u64 = 1 << 20;

pub(crate) struct Prepared {
    pub path: PathBuf,
    pub file: File,
    pub len: u64,
    pub codec: Codec,
    pub framing: Framing,
    pub framing_note: String,
    pub rule: Rule,
    pub ctx: stamp::Context,
}

impl Prepared {
    pub(crate) fn start(&self) -> u64 {
        (self.codec.bom as u64).min(self.len)
    }
    pub(crate) fn handle(&self) -> Result<File, String> {
        self.file.try_clone().map_err(|e| format!("Não foi possível ler o arquivo: {e}"))
    }
}

pub(crate) fn read_at(file: &mut File, at: u64, buf: &mut [u8]) -> Result<usize, String> {
    file.seek(SeekFrom::Start(at)).map_err(|e| format!("Não foi possível ler o arquivo: {e}"))?;
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("Não foi possível ler o arquivo: {e}")),
        }
    }
    Ok(got)
}

pub(crate) fn sample(file: &mut File, len: u64) -> Result<Vec<u8>, String> {
    let mut buf = vec![0; (len as usize).min(SAMPLE)];
    let got = read_at(file, 0, &mut buf)?;
    buf.truncate(got);
    Ok(buf)
}

/// Year and date that complete partial dates: a date in the file name, else
/// the day it was last written. Numeric date order learned from the sample.
pub(crate) fn calendar(path: &Path, file: &File, text: &str) -> stamp::Context {
    let reference = stamp::date_in_name(&path.to_string_lossy()).or_else(|| {
        let modified = file.metadata().ok()?.modified().ok()?;
        Some(chrono::DateTime::<chrono::Local>::from(modified).date_naive())
    });
    let year = reference.map(|d| chrono::Datelike::year(&d)).unwrap_or_else(|| chrono::Datelike::year(&chrono::Local::now()));
    stamp::Context { year, reference, day_first: stamp::learn_day_first(text.lines().take(2_000)) }
}

fn unescape(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                let code = u8::from_str_radix(&hex, 16).map_err(|_| format!("Escape \\x{hex} inválido no separador."))?;
                out.push(char::from(code));
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    Ok(out)
}

fn clip(text: &str, chars: usize) -> String {
    let text = trim_break(text);
    match text.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

pub(crate) fn compile_framing(spec: &FramingSpec, codec: &Codec, text: &str, ctx: stamp::Context) -> Result<(Framing, String), String> {
    Ok(match spec {
        FramingSpec::Auto => match crate::sources::detect_record_start(text.as_bytes(), &ctx) {
            Some(pattern) => {
                let re = rule::regex(&pattern, false, "automática")?;
                let example = text.lines().map(trim_break).find(|line| re.is_match(line)).map(|line| clip(line, 60)).unwrap_or_default();
                (Framing::StartRegex(re), format!("Cada item começa em uma linha com data, como “{example}”; as linhas seguintes sem data (stack traces, continuações) ficam no mesmo item."))
            }
            None => (Framing::Continuation, "Cada linha é um item; linhas recuadas, stack traces e linhas em branco ficam com o item de cima.".into()),
        },
        FramingSpec::Line => (Framing::Line, "Cada linha é um item.".into()),
        FramingSpec::StartDate => (Framing::StartDate(ctx), "Cada linha que começa com data e hora inicia um item; as demais continuam o item de cima.".into()),
        FramingSpec::Continuation => (Framing::Continuation, "Linhas recuadas, stack traces e linhas em branco continuam o item de cima.".into()),
        FramingSpec::StartRegex { pattern, ignore_case } => (Framing::StartRegex(rule::regex(pattern, *ignore_case, "do início do item")?), "Cada linha que corresponde ao padrão inicia um item.".into()),
        FramingSpec::EndRegex { pattern, ignore_case } => (Framing::EndRegex(rule::regex(pattern, *ignore_case, "do fim do item")?), "Cada linha que corresponde ao padrão encerra o item.".into()),
        FramingSpec::BlankLine => (Framing::Blank, "Itens separados por linhas em branco.".into()),
        FramingSpec::Delimiter { text } => {
            let delimiter = unescape(text)?;
            if delimiter.is_empty() {
                return Err("Informe o separador que encerra cada item.".into());
            }
            (Framing::Delimiter(codec.encode(&delimiter)?), format!("Cada item termina em “{}”.", text))
        }
    })
}

pub(crate) fn prepare(path: &Path, file: File, spec: &SplitSpec) -> Result<Prepared, String> {
    let mut file = file;
    let len = file.metadata().map_err(|e| format!("Não foi possível ler o arquivo: {e}"))?.len();
    let bytes = sample(&mut file, len)?;
    let codec = Codec::sniff(&bytes)?;
    let body = &bytes[codec.bom.min(bytes.len())..];
    let text = codec.decode(&body[..codec.align(body.len())]).into_owned();
    let ctx = calendar(path, &file, &text);
    let (framing, framing_note) = compile_framing(&spec.framing, &codec, &text, ctx)?;
    let rule = Rule::compile(&spec.rule, ctx)?;
    Ok(Prepared { path: path.to_path_buf(), file, len, codec, framing, framing_note, rule, ctx })
}

/// Where a sink receives the file: items, or whole regions of the original.
pub(crate) trait Sink {
    fn write(&mut self, bytes: &[u8], matched: bool) -> Result<(), String>;
    fn region(&mut self, from: u64, to: u64, matched: bool, meter: &mut Meter) -> Result<(), String>;
    /// False when this side is not kept (only matches are written).
    fn keeps(&self, matched: bool) -> bool;
}

/// Progress in megabytes of the original.
pub(crate) struct Meter {
    phase_id: &'static str,
    phase: &'static str,
    origin: u64,
    total: u64,
    last: Option<Instant>,
    pub items: u64,
}

impl Meter {
    pub(crate) fn new(phase_id: &'static str, phase: &'static str, origin: u64, end: u64) -> Self {
        Meter { phase_id, phase, origin, total: end.saturating_sub(origin), last: None, items: 0 }
    }
    pub(crate) fn at(&mut self, position: u64) {
        if self.last.is_some_and(|t| t.elapsed() < Duration::from_millis(200)) {
            return;
        }
        self.last = Some(Instant::now());
        let done = position.saturating_sub(self.origin).min(self.total);
        crate::operations::report_progress("separação", self.phase_id, self.phase, (done >> 20) as usize, (self.total >> 20).max(1) as usize, "MB", self.items as usize);
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stats {
    /// Items evaluated one by one.
    pub items: u64,
    pub matched_items: u64,
    pub undated: u64,
    /// Items too large to hold, decided by their first 16 MiB.
    pub oversized: u64,
    /// Bytes decided as whole regions (binary search or a settled range).
    pub region_bytes: u64,
}

/// Start of the first line at or after `at` (`at` itself at the data start).
fn line_start_after(prep: &Prepared, file: &mut File, at: u64, buf: &mut Vec<u8>) -> Result<Option<(u64, usize)>, String> {
    let start = prep.start();
    let at = start + prep.codec.align((at - start) as usize) as u64;
    let want = (prep.len - at).min(PROBE + HEAD as u64) as usize;
    buf.resize(want, 0);
    let got = read_at(file, at, buf)?;
    buf.truncate(got);
    if at == start {
        return Ok(Some((at, 0)));
    }
    // `at` may sit inside a line: the line starts after the break before it.
    let back = prep.codec.width() as u64;
    let mut probe = vec![0; back as usize];
    read_at(file, at - back, &mut probe)?;
    if prep.codec.find_newline(&probe, 0) == Some(0) {
        return Ok(Some((at, 0)));
    }
    Ok(prep.codec.find_newline(buf, 0).map(|p| (at + (p + prep.codec.width()) as u64, p + prep.codec.width())))
}

/// First line in `buf[from..]` that certainly starts an item, whatever came before it.
fn resync(framing: &Framing, codec: &Codec, buf: &[u8], from: usize) -> Option<usize> {
    let w = codec.width();
    let (mut pos, mut first, mut prev_end, mut prev_blank) = (from, true, false, false);
    loop {
        let newline = codec.find_newline(buf, pos)?;
        let end = newline.min(pos + HEAD);
        let text = codec.decode(&buf[pos..pos + codec.align(end - pos)]);
        let head = trim_break(&text);
        let starts = match framing {
            Framing::Line => true,
            Framing::StartRegex(re) => re.is_match(head),
            Framing::StartDate(ctx) => starts_with_date(head, ctx),
            Framing::Continuation => !blank(head) && !crate::sources::continuation_line(head),
            Framing::EndRegex(_) => !first && prev_end,
            Framing::Blank => !first && prev_blank && !blank(head),
            Framing::Delimiter(_) => return None,
        };
        if starts {
            return Some(pos);
        }
        prev_end = matches!(framing, Framing::EndRegex(re) if re.is_match(head));
        prev_blank = blank(head);
        first = false;
        pos = newline + w;
    }
}

/// The first item boundary at or after `at` and the first date read from the
/// items there.
fn probe(prep: &Prepared, reader: &DateReader, at: u64) -> Result<Option<(u64, i64)>, String> {
    let mut file = prep.handle()?;
    let mut buf = Vec::new();
    let Some((line, offset)) = line_start_after(prep, &mut file, at, &mut buf)? else { return Ok(None) };
    let Some(found) = resync(&prep.framing, &prep.codec, &buf, offset) else { return Ok(None) };
    let boundary = line + (found - offset) as u64;
    let mut items = Reader::new(prep.handle()?, boundary, prep.len.min(boundary + PROBE), &prep.framing, prep.codec, 1, 1)?;
    let mut seen = 0;
    while let Some(batch) = items.next_batch()? {
        for span in batch.spans.iter().filter(|s| s.first) {
            if let Some(date) = reader.read(&prep.codec.decode(&batch.data[span.start..span.end])) {
                return Ok(Some((boundary, date)));
            }
            seen += 1;
            if seen >= 256 {
                return Ok(None);
            }
        }
    }
    Ok(None)
}

/// Offset of the first item dated at or after `bound` in a chronological
/// file, searching from the item boundary `lo` (undated items inherit the
/// date above them). The file length when no item reaches it.
pub(crate) fn seek_first(prep: &Prepared, slot: usize, bound: i64, lo: u64) -> Result<u64, String> {
    let reader = prep.rule.date_reader(slot);
    let (mut lo, mut hi) = (lo, prep.len);
    while hi - lo > PROBE {
        crate::operations::check()?;
        let mid = lo + (hi - lo) / 2;
        match probe(prep, reader, mid)? {
            Some((boundary, date)) if date < bound && boundary > lo && boundary < hi => lo = boundary,
            _ => hi = mid,
        }
    }
    let mut items = Reader::new(prep.handle()?, lo, prep.len, &prep.framing, prep.codec, 1, 1)?;
    let mut last = None;
    while let Some(batch) = items.next_batch()? {
        crate::operations::check()?;
        for span in batch.spans.iter().filter(|s| s.first) {
            let date = reader.read(&prep.codec.decode(&batch.data[span.start..span.end])).or(last);
            if date.is_some_and(|d| d >= bound) {
                return Ok(batch.base + span.start as u64);
            }
            last = date;
        }
    }
    Ok(prep.len)
}

fn facts_of(prep: &Prepared, data: &[u8], spans: &[framing::Span]) -> Vec<Option<Facts>> {
    let workers = crate::resources::workers().max(1);
    let size = (spans.len() / (workers * 4)).max(64);
    let groups: Vec<&[framing::Span]> = spans.chunks(size).collect();
    crate::global_scheduler::map(groups, |group| {
        group
            .iter()
            .map(|span| span.first.then(|| prep.rule.facts(&prep.codec.decode(&data[span.start..span.end]))))
            .collect::<Vec<_>>()
    })
    .into_iter()
    .flatten()
    .collect()
}

fn region(sink: &mut dyn Sink, meter: &mut Meter, stats: &mut Stats, from: u64, to: u64, matched: bool) -> Result<(), String> {
    if to > from {
        stats.region_bytes += to - from;
        if sink.keeps(matched) {
            sink.region(from, to, matched, meter)?;
        }
    }
    Ok(())
}

/// Every item of the file to the sink, in order.
pub(crate) fn run(prep: &Prepared, sink: &mut dyn Sink, meter: &mut Meter) -> Result<Stats, String> {
    let mut stats = Stats::default();
    let start = prep.start();
    let line_based = prep.framing.line_based();
    if let Some((slot, from, to)) = prep.rule.pure_range().filter(|_| line_based) {
        let first = match from {
            Some(f) => seek_first(prep, slot, f, start)?,
            None => start,
        };
        let end = match to {
            Some(t) => seek_first(prep, slot, t, first)?,
            None => prep.len,
        };
        region(sink, meter, &mut stats, start, first, false)?;
        region(sink, meter, &mut stats, first, end, true)?;
        region(sink, meter, &mut stats, end, prep.len, false)?;
        meter.at(prep.len);
        return Ok(stats);
    }
    let begin = match prep.rule.seek_bound().filter(|_| line_based) {
        Some((slot, from)) => seek_first(prep, slot, from, start)?,
        None => start,
    };
    region(sink, meter, &mut stats, start, begin, false)?;
    let settles = prep.rule.may_settle();
    let mut items = Reader::new(prep.handle()?, begin, prep.len, &prep.framing, prep.codec, 1, 1)?;
    let mut running = Running::default();
    let mut carry = false;
    while let Some(batch) = items.next_batch()? {
        crate::operations::check()?;
        let facts = facts_of(prep, batch.data, &batch.spans);
        for (span, facts) in batch.spans.iter().zip(facts) {
            let matched = match facts {
                Some(facts) => {
                    if settles {
                        if let Some(fate) = prep.rule.fate(&facts, span.item, span.line, &running) {
                            region(sink, meter, &mut stats, batch.base + span.start as u64, prep.len, fate)?;
                            stats.undated = running.undated;
                            meter.at(prep.len);
                            return Ok(stats);
                        }
                    }
                    let matched = prep.rule.decide(&facts, span.item, span.line, &mut running);
                    stats.items += 1;
                    stats.matched_items += u64::from(matched);
                    stats.oversized += u64::from(!span.last);
                    carry = matched;
                    matched
                }
                None => carry,
            };
            sink.write(&batch.data[span.start..span.end], matched)?;
        }
        meter.items = stats.matched_items;
        meter.at(batch.base + batch.data.len() as u64);
    }
    stats.undated = running.undated;
    Ok(stats)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewItem {
    pub number: u64,
    pub line: u64,
    pub lines: u64,
    pub text: String,
    pub matched: bool,
    pub date: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub framing: String,
    pub items: Vec<PreviewItem>,
    pub evaluated: u64,
    pub matched: u64,
    pub undated: u64,
    /// Bytes of the file the sample covered.
    pub sampled_bytes: u64,
    pub whole_file: bool,
}

const PREVIEW_BYTES: u64 = 4 << 20;
const PREVIEW_ITEMS: u64 = 5_000;
const PREVIEW_SHOWN: usize = 80;

/// The first items of the file, as the rule sees them.
pub(crate) fn preview(prep: &Prepared) -> Result<Preview, String> {
    let start = prep.start();
    let limit = prep.len.min(start + PREVIEW_BYTES);
    let shown_date = if prep.rule.has_dates() { None } else { Some(DateReader::auto(prep.ctx)) };
    let mut items = Reader::new(prep.handle()?, start, limit, &prep.framing, prep.codec, 1, 1)?;
    let mut running = Running::default();
    let mut out = Preview { framing: String::new(), items: Vec::new(), evaluated: 0, matched: 0, undated: 0, sampled_bytes: limit - start, whole_file: limit == prep.len };
    'read: while let Some(batch) = items.next_batch()? {
        crate::operations::check()?;
        for span in batch.spans.iter().filter(|s| s.first) {
            if out.evaluated >= PREVIEW_ITEMS {
                out.whole_file = false;
                out.sampled_bytes = batch.base + span.start as u64 - start;
                break 'read;
            }
            let text = prep.codec.decode(&batch.data[span.start..span.end]);
            let facts = prep.rule.facts(&text);
            let matched = prep.rule.decide(&facts, span.item, span.line, &mut running);
            out.evaluated += 1;
            out.matched += u64::from(matched);
            if out.items.len() < PREVIEW_SHOWN {
                let date = match &shown_date {
                    Some(reader) => reader.read(&text),
                    None => facts.dates[0],
                };
                let lines = text.lines().count() as u64;
                let mut shown: String = text.lines().take(24).collect::<Vec<_>>().join("\n");
                if lines > 24 {
                    shown.push_str(&format!("\n… mais {} linhas", lines - 24));
                }
                out.items.push(PreviewItem { number: span.item, line: span.line, lines, text: clip(&shown, 4_000), matched, date });
            }
        }
    }
    out.undated = running.undated;
    out.framing = prep.framing_note.clone();
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    #[derive(Default)]
    pub(crate) struct Collect {
        pub matched: Vec<u8>,
        pub rest: Vec<u8>,
        pub source: Vec<u8>,
        pub regions: usize,
        pub only_matched: bool,
    }
    impl Sink for Collect {
        fn write(&mut self, bytes: &[u8], matched: bool) -> Result<(), String> {
            if matched { self.matched.extend_from_slice(bytes) } else if !self.only_matched { self.rest.extend_from_slice(bytes) }
            Ok(())
        }
        fn region(&mut self, from: u64, to: u64, matched: bool, _: &mut Meter) -> Result<(), String> {
            self.regions += 1;
            let bytes = self.source[from as usize..to as usize].to_vec();
            self.write(&bytes, matched)
        }
        fn keeps(&self, matched: bool) -> bool {
            matched || !self.only_matched
        }
    }

    pub(crate) fn prepared(content: &[u8], spec: serde_json::Value) -> (tempfile::TempDir, Prepared) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::File::create(&path).unwrap().write_all(content).unwrap();
        let spec: SplitSpec = serde_json::from_value(spec).unwrap();
        let prep = prepare(&path, File::open(&path).unwrap(), &spec).unwrap();
        (dir, prep)
    }

    pub(crate) fn split(content: &[u8], spec: serde_json::Value, only_matched: bool) -> (String, String, Stats, usize) {
        let (_dir, prep) = prepared(content, spec);
        let mut sink = Collect { source: content.to_vec(), only_matched, ..Collect::default() };
        let mut meter = Meter::new("t", "t", 0, prep.len);
        let stats = run(&prep, &mut sink, &mut meter).unwrap();
        (String::from_utf8(sink.matched).unwrap(), String::from_utf8(sink.rest).unwrap(), stats, sink.regions)
    }

    fn day_log(hours: u32, per_hour: u32) -> String {
        let mut text = String::new();
        for h in 0..hours {
            for m in 0..per_hour {
                let secs = m * 3600 / per_hour;
                text.push_str(&format!("2026-10-12 {h:02}:{:02}:{:02} INFO request {h}-{m} {}\n", secs / 60, secs % 60, "x".repeat(80)));
                if m % 7 == 0 {
                    text.push_str("java.lang.RuntimeException: boom\n\tat a.B.c(B.java:1)\n");
                }
            }
        }
        text
    }

    fn local(h: u32) -> i64 {
        crate::sources::naive_to_ms(chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap().and_hms_opt(h, 0, 0).unwrap())
    }

    #[test]
    fn auto_framing_keeps_stack_traces_with_their_item() {
        let log = "2026-10-12 10:00:00 INFO a\n2026-10-12 10:00:01 ERROR b\njava.lang.IllegalStateException: x\n\tat a.B.c(B.java:1)\n2026-10-12 10:00:02 INFO c\n";
        let spec = serde_json::json!({"framing":{"kind":"auto"},"mode":"both","rule":{"conditions":[{"kind":"text","values":["ERROR"],"scope":"firstLine"}]}});
        let (matched, rest, stats, _) = split(log.as_bytes(), spec, false);
        assert_eq!(matched, "2026-10-12 10:00:01 ERROR b\njava.lang.IllegalStateException: x\n\tat a.B.c(B.java:1)\n");
        assert_eq!(rest, "2026-10-12 10:00:00 INFO a\n2026-10-12 10:00:02 INFO c\n");
        assert_eq!((stats.items, stats.matched_items), (3, 1));
    }

    #[test]
    fn sorted_date_ranges_use_binary_search_and_match_the_full_scan() {
        let log = day_log(24, 1500);
        assert!(log.len() as u64 > 4 * PROBE);
        for (from, to) in [(Some(local(9)), Some(local(17))), (None, Some(local(12))), (Some(local(23)), None), (Some(local(0)), Some(local(0)))] {
            let rule = |sorted: bool| serde_json::json!({"framing":{"kind":"auto"},"mode":"both","rule":{"sorted":sorted,"conditions":[{"kind":"date","from":from,"to":to}]}});
            let (fast_m, fast_r, fast, regions) = split(log.as_bytes(), rule(true), false);
            let (slow_m, slow_r, slow, _) = split(log.as_bytes(), rule(false), false);
            assert_eq!(fast_m, slow_m, "{from:?}..{to:?}");
            assert_eq!(fast_r, slow_r);
            assert!(regions >= 1 && fast.items == 0 && slow.items > 0);
            assert_eq!(fast_m.len() + fast_r.len(), log.len());
        }
        // A date range with another condition: seek to the start, stop at the end.
        let spec = serde_json::json!({"framing":{"kind":"auto"},"mode":"matched","rule":{"sorted":true,"conditions":[{"kind":"date","from":local(10),"to":local(11)},{"kind":"text","values":["Exception"]}]}});
        let (fast_m, _, fast, _) = split(log.as_bytes(), spec.clone(), true);
        let mut slow_spec = spec;
        slow_spec["rule"]["sorted"] = serde_json::json!(false);
        let (slow_m, _, slow, _) = split(log.as_bytes(), slow_spec, true);
        assert_eq!(fast_m, slow_m);
        assert!(fast.items < slow.items / 10);
        assert!(fast_m.lines().all(|l| !l.starts_with("2026") || l.starts_with("2026-10-12 10:")));
    }

    #[test]
    fn positions_count_items_or_lines_and_stop_early() {
        let log = "a\n  a2\nb\nc\n  c2\nd\n";
        let spec = |unit: &str| serde_json::json!({"framing":{"kind":"continuation"},"mode":"both","rule":{"conditions":[{"kind":"ordinal","unit":unit,"from":2,"to":3}]}});
        let (m, r, stats, regions) = split(log.as_bytes(), spec("item"), false);
        assert_eq!((m.as_str(), r.as_str()), ("b\nc\n  c2\n", "a\n  a2\nd\n"));
        assert_eq!((stats.items, regions), (3, 1));
        let (m, _, _, _) = split(log.as_bytes(), spec("line"), false);
        assert_eq!(m, "b\n");
    }

    #[test]
    fn preview_shows_items_dates_and_framing() {
        let log = day_log(1, 30);
        let (_dir, prep) = prepared(log.as_bytes(), serde_json::json!({"framing":{"kind":"auto"},"mode":"matched","rule":{"conditions":[{"kind":"regex","pattern":"request 0-1\\b"}]}}));
        let preview = preview(&prep).unwrap();
        assert!(preview.whole_file && preview.evaluated == 30 && preview.matched == 1);
        assert!(preview.framing.contains("data"));
        assert_eq!(preview.items[0].lines, 3);
        assert_eq!(preview.items[0].date, Some(local(0)));
    }
}
