//! What one log item is, and the streaming reader that cuts a file into items.
//!
//! Line framings decide where an item starts from each line alone (a line
//! matching the start pattern, a line that is not a continuation, the line
//! after an end marker or after a blank line), so a reader may begin at any
//! such line of the file and still agree with a reader that started at the top.
use super::codec::{trim_break, Codec};
use crate::sources::stamp;
use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Bytes of a line examined to decide whether it starts an item.
pub(crate) const HEAD: usize = 64 * 1024;
/// An item larger than this is decided by its beginning and streamed in pieces.
pub(crate) const MAX_ITEM: usize = 16 << 20;
const BLOCK: usize = 4 << 20;
const BATCH: usize = 8 << 20;

pub(crate) enum Framing {
    Line,
    /// A line matching the pattern starts an item.
    StartRegex(regex::Regex),
    /// A line beginning with a date (optionally after `[`) starts an item.
    StartDate(stamp::Context),
    /// Indented lines, stack traces and blank lines continue the item above.
    Continuation,
    /// A line matching the pattern is the last line of its item.
    EndRegex(regex::Regex),
    /// Blank lines close an item.
    Blank,
    /// Bytes (in the file's encoding) that end an item.
    Delimiter(Vec<u8>),
}

impl Framing {
    pub(crate) fn line_based(&self) -> bool {
        !matches!(self, Framing::Delimiter(_))
    }
}

pub(crate) fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

pub(crate) fn starts_with_date(text: &str, ctx: &stamp::Context) -> bool {
    let at = usize::from(text.starts_with('['));
    stamp::at(text, at, ctx).is_some()
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    /// Range in the batch's data.
    pub start: usize,
    pub end: usize,
    /// Item number (1-based) and number of its first line (1-based).
    pub item: u64,
    pub line: u64,
    /// The item begins in this span; false for later pieces of a huge item.
    pub first: bool,
    /// The item ends with this span.
    pub last: bool,
}

pub(crate) struct Batch<'a> {
    pub data: &'a [u8],
    /// Offset of `data[0]` in the file.
    pub base: u64,
    pub spans: Vec<Span>,
}

#[derive(Default)]
struct State {
    /// Lines of the current item already read.
    lines: u64,
    prev_blank: bool,
    end_pending: bool,
    /// The bytes at `scan` continue a line cut by a huge-item piece.
    mid_line: bool,
    /// The line at `scan` was classified as part of the current item.
    member: bool,
    /// No line break before this position for the line at `scan`.
    search_from: usize,
}

pub(crate) struct Reader<'a> {
    file: File,
    framing: &'a Framing,
    codec: Codec,
    limit: u64,
    buf: Vec<u8>,
    base: u64,
    next_read: u64,
    consumed: usize,
    item_start: usize,
    scan: usize,
    eof: bool,
    st: State,
    next_item: u64,
    next_line: u64,
    item_line: u64,
    partial: bool,
}

impl<'a> Reader<'a> {
    /// Items of `file` from `start` (an item boundary) to `limit`.
    pub(crate) fn new(mut file: File, start: u64, limit: u64, framing: &'a Framing, codec: Codec, first_item: u64, first_line: u64) -> Result<Self, String> {
        file.seek(SeekFrom::Start(start)).map_err(|e| format!("Não foi possível ler o arquivo: {e}"))?;
        Ok(Reader {
            file, framing, codec, limit,
            buf: Vec::with_capacity(BLOCK * 2), base: start, next_read: start, consumed: 0,
            item_start: 0, scan: 0, eof: start >= limit, st: State::default(),
            next_item: first_item, next_line: first_line, item_line: first_line, partial: false,
        })
    }

    fn fill(&mut self) -> Result<(), String> {
        let want = (self.limit - self.next_read).min(BLOCK as u64) as usize;
        if want == 0 {
            self.eof = true;
            return Ok(());
        }
        let old = self.buf.len();
        self.buf.resize(old + want, 0);
        // Handles cloned from one file share its position: always read at ours.
        self.file.seek(SeekFrom::Start(self.next_read)).map_err(|e| format!("Não foi possível ler o arquivo: {e}"))?;
        let mut got = 0;
        while got < want {
            match self.file.read(&mut self.buf[old + got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("Não foi possível ler o arquivo: {e}")),
            }
        }
        self.buf.truncate(old + got);
        self.next_read += got as u64;
        if got < want {
            self.eof = true;
        }
        Ok(())
    }

    fn discard(&mut self) {
        if self.consumed == 0 {
            return;
        }
        let n = self.consumed;
        self.buf.drain(..n);
        self.base += n as u64;
        self.item_start -= n;
        self.scan -= n;
        self.st.search_from = self.st.search_from.saturating_sub(n);
        self.consumed = 0;
    }

    /// The next spans of complete items (or one piece of a huge item); None at the end.
    pub(crate) fn next_batch(&mut self) -> Result<Option<Batch<'_>>, String> {
        self.discard();
        loop {
            let mut spans = Vec::new();
            let mut bytes = 0;
            while bytes < BATCH {
                let Some(end) = self.find_end() else { break };
                if end == self.item_start && !self.partial {
                    break;
                }
                bytes += end - self.item_start;
                spans.push(self.close(end));
            }
            if !spans.is_empty() {
                self.consumed = self.item_start;
                return Ok(Some(Batch { data: &self.buf[..self.consumed], base: self.base, spans }));
            }
            if self.eof {
                return Ok(None);
            }
            if self.buf.len() - self.item_start >= MAX_ITEM {
                let cut = self.piece_end();
                let span = self.piece(cut);
                self.consumed = self.item_start;
                return Ok(Some(Batch { data: &self.buf[..self.consumed], base: self.base, spans: vec![span] }));
            }
            self.fill()?;
        }
    }

    fn close(&mut self, end: usize) -> Span {
        let span = Span { start: self.item_start, end, item: self.next_item, line: self.item_line, first: !self.partial, last: true };
        self.next_line += self.codec.count_newlines(&self.buf[self.item_start..end]);
        self.next_item += 1;
        self.item_line = self.next_line;
        self.item_start = end;
        self.scan = end;
        self.st = State::default();
        self.partial = false;
        span
    }

    fn piece_end(&self) -> usize {
        match self.framing {
            Framing::Delimiter(_) => self.scan,
            _ if self.st.mid_line || self.st.member || self.st.lines == 0 => self.codec.align(self.buf.len()),
            _ => self.scan,
        }
    }

    fn piece(&mut self, cut: usize) -> Span {
        let span = Span { start: self.item_start, end: cut, item: self.next_item, line: self.item_line, first: !self.partial, last: false };
        self.next_line += self.codec.count_newlines(&self.buf[self.item_start..cut]);
        self.partial = true;
        self.item_start = cut;
        if self.scan < cut {
            if self.framing.line_based() {
                self.st.mid_line = true;
                self.st.member = false;
                self.st.search_from = 0;
            }
            self.scan = cut;
        }
        span
    }

    fn find_end(&mut self) -> Option<usize> {
        match self.framing {
            Framing::Delimiter(d) => self.find_delimiter(d),
            _ => self.find_line_end(),
        }
    }

    fn pending_end(&self) -> Option<usize> {
        let len = self.buf.len();
        (self.eof && (len > self.item_start || self.partial)).then_some(len)
    }

    fn find_delimiter(&mut self, d: &[u8]) -> Option<usize> {
        let w = self.codec.width();
        let mut from = self.scan.max(self.item_start);
        while let Some(p) = memchr::memmem::find(&self.buf[from..], d) {
            let p = from + p;
            if (p - self.item_start) % w == 0 {
                return Some(p + d.len());
            }
            from = p + 1;
        }
        if self.eof {
            return self.pending_end();
        }
        self.scan = self.item_start.max(self.codec.align(self.buf.len().saturating_sub(d.len() - 1)));
        None
    }

    fn head(&self, start: usize, newline: Option<usize>) -> Cow<'_, str> {
        let end = newline.unwrap_or(self.buf.len()).min(start + HEAD);
        let end = start + self.codec.align(end - start);
        match self.codec.decode(&self.buf[start..end]) {
            Cow::Borrowed(text) => Cow::Borrowed(trim_break(text)),
            Cow::Owned(text) => Cow::Owned(trim_break(&text).to_string()),
        }
    }

    fn find_line_end(&mut self) -> Option<usize> {
        let w = self.codec.width();
        loop {
            let start = self.scan;
            let len = self.buf.len();
            if start >= len {
                return self.pending_end();
            }
            let newline = self.codec.find_newline(&self.buf, start.max(self.st.search_from));
            let complete = newline.is_some() || self.eof;
            let mut head_blank: Option<bool> = None;
            if !self.st.mid_line && self.st.lines > 0 && !self.st.member {
                let starts = match self.framing {
                    Framing::Line => true,
                    Framing::EndRegex(_) => self.st.end_pending,
                    _ => {
                        if !complete && len - start < HEAD {
                            self.st.search_from = self.codec.align(len).max(start);
                            return None;
                        }
                        let text = self.head(start, newline);
                        match self.framing {
                            Framing::Blank => {
                                head_blank = Some(blank(&text));
                                self.st.prev_blank && !blank(&text)
                            }
                            Framing::StartRegex(re) => re.is_match(&text),
                            Framing::StartDate(ctx) => starts_with_date(&text, ctx),
                            Framing::Continuation => !blank(&text) && !crate::sources::continuation_line(&text),
                            Framing::Line | Framing::EndRegex(_) | Framing::Delimiter(_) => unreachable!(),
                        }
                    }
                };
                if starts {
                    return Some(start);
                }
                self.st.member = true;
            }
            if newline.is_none() && !self.eof {
                self.st.search_from = self.codec.align(len).max(start);
                return None;
            }
            if self.st.mid_line {
                self.st.end_pending = false;
                self.st.prev_blank = false;
            } else {
                match self.framing {
                    Framing::EndRegex(re) => self.st.end_pending = re.is_match(&self.head(start, newline)),
                    Framing::Blank => self.st.prev_blank = head_blank.unwrap_or_else(|| blank(&self.head(start, newline))),
                    _ => {}
                }
            }
            self.st.lines += 1;
            self.st.mid_line = false;
            self.st.member = false;
            self.st.search_from = 0;
            self.scan = newline.map_or(len, |p| p + w);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::log_split::codec::Encoding;
    use std::io::Write;

    pub(crate) fn utf8() -> Codec {
        Codec { encoding: Encoding::Utf8, bom: 0, newline: b'\n' }
    }

    pub(crate) fn items(bytes: &[u8], framing: &Framing, codec: Codec) -> Vec<(String, u64, u64, bool, bool)> {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        let mut reader = Reader::new(file, codec.bom as u64, bytes.len() as u64, framing, codec, 1, 1).unwrap();
        let mut out = Vec::new();
        while let Some(batch) = reader.next_batch().unwrap() {
            for span in &batch.spans {
                out.push((String::from_utf8_lossy(&batch.data[span.start..span.end]).into_owned(), span.item, span.line, span.first, span.last));
            }
        }
        out
    }

    fn texts(bytes: &[u8], framing: &Framing) -> Vec<String> {
        items(bytes, framing, utf8()).into_iter().map(|i| i.0).collect()
    }

    const JAVA: &str = "2026-10-12 10:00:00 INFO start\n2026-10-12 10:00:01 ERROR boom\njava.lang.IllegalStateException: x\n\tat a.B.c(B.java:1)\nCaused by: java.io.IOException\n\t... 3 more\n2026-10-12 10:00:02 INFO end";

    #[test]
    fn line_and_multiline_framings_group_stack_traces() {
        assert_eq!(texts(b"a\nb\r\nc", &Framing::Line), ["a\n", "b\r\n", "c"]);
        let ctx = stamp::Context::year(2026);
        let by_date = texts(JAVA.as_bytes(), &Framing::StartDate(ctx));
        assert_eq!(by_date.len(), 3);
        assert!(by_date[1].starts_with("2026-10-12 10:00:01 ERROR") && by_date[1].ends_with("3 more\n"));
        assert_eq!(texts(JAVA.as_bytes(), &Framing::Continuation), by_date);
        let re = regex::Regex::new(r"^\d{4}-").unwrap();
        assert_eq!(texts(JAVA.as_bytes(), &Framing::StartRegex(re)), by_date);
        let lines = items(JAVA.as_bytes(), &Framing::StartDate(ctx), utf8());
        assert_eq!(lines.iter().map(|i| (i.1, i.2)).collect::<Vec<_>>(), [(1, 1), (2, 2), (3, 7)]);
    }

    #[test]
    fn end_blank_and_delimiter_framings() {
        let end = regex::Regex::new(r"^</event>$").unwrap();
        assert_eq!(texts(b"<event>\na\n</event>\n<event>\nb\n</event>\n", &Framing::EndRegex(end)), ["<event>\na\n</event>\n", "<event>\nb\n</event>\n"]);
        assert_eq!(texts(b"a\nb\n\n\nc\n\nd", &Framing::Blank), ["a\nb\n\n\n", "c\n\n", "d"]);
        assert_eq!(texts(b"one||two||||three", &Framing::Delimiter(b"||".to_vec())), ["one||", "two||", "||", "three"]);
        assert_eq!(texts(b"", &Framing::Line), Vec::<String>::new());
    }

    #[test]
    fn huge_items_stream_in_pieces_without_moving_boundaries() {
        let mut text = String::from("2026-10-12 10:00:00 first\n");
        let filler = format!("\tat {}\n", "x".repeat(1018)).repeat(MAX_ITEM / 1024 * 2);
        text.push_str(&filler);
        text.push_str("2026-10-12 10:00:01 second\n");
        let got = items(text.as_bytes(), &Framing::StartDate(stamp::Context::year(2026)), utf8());
        assert!(got.len() >= 3);
        assert!(got[0].3 && !got[0].4);
        let joined: String = got[..got.len() - 1].iter().map(|i| i.0.as_str()).collect();
        assert_eq!(joined.len(), text.len() - "2026-10-12 10:00:01 second\n".len());
        assert!(got.iter().all(|i| i.1 == 1 || i.0.starts_with("2026-10-12 10:00:01")));
        let last = got.last().unwrap();
        assert_eq!((last.1, last.2, last.3, last.4), (2, 2 + filler.matches('\n').count() as u64, true, true));
        // One giant line without line breaks is still one item.
        let giant = "x".repeat(MAX_ITEM * 2 + 5);
        let pieces = items(format!("{giant}\nnext\n").as_bytes(), &Framing::Line, utf8());
        assert_eq!(pieces.last().unwrap().0, "next\n");
        assert_eq!(pieces.iter().filter(|p| p.1 == 1).map(|p| p.0.len()).sum::<usize>(), giant.len() + 1);
    }

    #[test]
    fn utf16_items_keep_whole_units() {
        let text = "linha um\nlinha dois\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let codec = Codec::sniff(&bytes).unwrap();
        let got = items(&bytes, &Framing::Line, codec);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].2, 2);
    }
}
