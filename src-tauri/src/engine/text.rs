//! Inverted index of the free-text column, like the one search engines use:
//! for each word, the rows that contain it. A search finds in milliseconds
//! the rows whose text can contain the needle, and the exact test runs only
//! on them, so results stay identical to scanning every row.
//!
//! Words are the maximal runs of alphanumeric characters of the lowercase
//! text. Every alphanumeric piece of a needle lies inside one word of any
//! text containing the needle, so the rows holding a word that contains each
//! piece are a superset of the matches. Words too long to index become a
//! marker that every search includes.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tantivy::query::{EnableScoring, Query, TermQuery};
#[cfg(test)]
use tantivy::query::{BooleanQuery, Occur, RegexQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST,
};
use tantivy::tokenizer::{PreTokenizedString, Token};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term, TERMINATED};

/// Word standing for every word longer than [`MAX_WORD`] bytes.
const LONG: &str = "\u{1}";
const MAX_WORD: usize = 64;
/// Word standing for every hexadecimal word of [`HEX_WORD`] or more
/// characters (ids, hashes): the general substring dictionary keeps a marker
/// so regex expansion does not walk every unique ID. A separate exact field
/// handles full words when the recorded maximum length proves it sufficient.
const HEX: &str = "\u{2}";
const HEX_WORD: usize = 8;
/// Shorter pieces narrow almost nothing, so neither they nor words this short
/// are indexed (a piece of this length only lies inside words at least as long).
const MIN_PIECE: usize = 3;

#[derive(Clone, Copy)]
struct ProbeLimits {
    terms: usize,
    postings: usize,
    bitmap_bytes: usize,
}
impl ProbeLimits {
    fn for_candidates(limit: usize) -> Self {
        Self {
            // Two traversals fit the ~100k-term checkpoint dictionaries in
            // the retained 50M fixture; larger dictionaries still decline.
            terms: 262_144,
            postings: limit.saturating_add(1).saturating_mul(8).clamp(16_384, 2_000_008),
            // Ordinary immutable stores contain at most 1M records (~125 KB).
            // A malformed/unexpected larger segment must not allocate freely.
            bitmap_bytes: 1 << 20,
        }
    }
}

#[derive(Default, Debug)]
struct ProbeWork {
    terms: usize,
    postings: usize,
    attempts: usize,
    peak_bitmap_bytes: usize,
}

#[derive(Debug)]
enum ProbeStop { Candidates, Work, Invalid, Cancelled }

/// At most two required pieces, without allocating for every word of a long
/// user query. Either piece alone is a safe superset for canonical verification.
fn required_pieces(needle: &str) -> Vec<&str> {
    let mut pieces = Vec::with_capacity(3);
    for piece in needle.split(|c: char| !c.is_alphanumeric())
        .filter(|piece| piece.chars().nth(MIN_PIECE - 1).is_some()) {
        if pieces.contains(&piece) { continue; }
        let at = pieces.iter().position(|old: &&str| old.len() < piece.len()).unwrap_or(pieces.len());
        if at < 2 { pieces.insert(at, piece); pieces.truncate(2); }
    }
    pieces
}

fn visit_postings(
    inverted: &tantivy::InvertedIndexReader,
    info: &tantivy::postings::TermInfo,
    limits: ProbeLimits,
    work: &mut ProbeWork,
    keep_going: &mut impl FnMut() -> bool,
    visit: &mut impl FnMut(u32) -> Result<(), ProbeStop>,
) -> Result<(), ProbeStop> {
    let mut postings = inverted.read_block_postings_from_terminfo(info, IndexRecordOption::Basic)
        .map_err(|_| ProbeStop::Invalid)?;
    loop {
        if !keep_going() { return Err(ProbeStop::Cancelled); }
        let docs = postings.docs();
        if docs.is_empty() { return Ok(()); }
        for &doc in docs {
            if work.postings == limits.postings { return Err(ProbeStop::Work); }
            work.postings += 1;
            visit(doc)?;
        }
        postings.advance();
    }
}

/// Index folder of a store (`<key>.text` beside `<key>.duckdb`).
pub(crate) fn dir_of(store: &Path) -> PathBuf {
    store.with_extension("text")
}

fn schema() -> (Schema, Field, Field, Field) {
    let mut builder = Schema::builder();
    let lid = builder.add_u64_field("lid", FAST);
    let indexing = TextFieldIndexing::default()
        .set_tokenizer("raw")
        .set_index_option(IndexRecordOption::Basic);
    let text = builder.add_text_field("t", TextOptions::default().set_indexing_options(indexing.clone()));
    let hex = builder.add_text_field("hex", TextOptions::default().set_indexing_options(indexing));
    (builder.build(), lid, text, hex)
}

/// Distinct words of a lowercase text. The writer separates hexadecimal words
/// from the general substring dictionary.
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().nth(MIN_PIECE - 1).is_some())
        .map(|w| if w.len() > MAX_WORD { LONG } else { w })
        .filter(|w| seen.insert(*w))
        .map(str::to_string)
        .collect()
}

pub(crate) struct Writer {
    writer: IndexWriter,
    lid: Field,
    text: Field,
    hex: Field,
    max_hex_word: AtomicUsize,
    dir: PathBuf,
}

impl Writer {
    pub(crate) fn create(dir: &Path, threads: usize, memory: usize) -> Result<Writer, String> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let (schema, lid, text, hex) = schema();
        let index = Index::create_in_dir(dir, schema).map_err(|e| e.to_string())?;
        let writer = index
            .writer_with_num_threads(threads.max(1), memory.max(threads.max(1) * (16 << 20)))
            .map_err(|e| e.to_string())?;
        Ok(Writer { writer, lid, text, hex, max_hex_word: AtomicUsize::new(0), dir: dir.to_path_buf() })
    }

    pub(crate) fn add(&self, lid: u32, words: Vec<String>) -> Result<(), String> {
        let mut doc = TantivyDocument::default();
        let mut seen = HashSet::new();
        let tokens = words
            .into_iter()
            .map(|word| {
                if is_hex_word(&word) {
                    self.max_hex_word.fetch_max(word.len(), Ordering::Relaxed);
                    doc.add_text(self.hex, &word);
                    HEX.to_string()
                } else { word }
            })
            .filter(|word| seen.insert(word.clone()))
            .enumerate()
            .map(|(position, text)| Token {
                offset_from: 0,
                offset_to: 0,
                position,
                text,
                position_length: 1,
            })
            .collect();
        doc.add_u64(self.lid, u64::from(lid));
        doc.add_pre_tokenized_text(self.text, PreTokenizedString { text: String::new(), tokens });
        self.writer.add_document(doc).map(|_| ()).map_err(|e| e.to_string())
    }

    pub(crate) fn finish(mut self) -> Result<(), String> {
        use std::io::Write;
        self.writer.commit().map_err(|e| e.to_string())?;
        self.writer.wait_merging_threads().map_err(|e| e.to_string())?;
        let mut metadata = std::fs::File::create(self.dir.join("hex-length")).map_err(|e| e.to_string())?;
        write!(metadata, "{}", self.max_hex_word.load(Ordering::Relaxed)).map_err(|e| e.to_string())?;
        metadata.sync_all().map_err(|e| e.to_string())
    }
}

pub(crate) struct Text {
    reader: IndexReader,
    lid: Field,
    text: Field,
    hex: Field,
    max_hex_word: usize,
}

impl Text {
    pub(crate) fn open(dir: &Path) -> Option<Text> {
        if !dir.is_dir() {
            return None;
        }
        let index = Index::open_in_dir(dir).ok()?;
        let (_, lid, text, hex) = schema();
        let reader = index.reader().ok()?;
        let max_hex_word = std::fs::read_to_string(dir.join("hex-length")).ok()?.parse().ok()?;
        Some(Text { reader, lid, text, hex, max_hex_word })
    }

    /// Rows (sorted) whose text may contain `needle`; `None` when the index
    /// cannot narrow the search or more than `limit` rows qualify.
    pub(crate) fn candidates(&self, needle: &str, limit: usize) -> Option<Vec<u32>> {
        let token = crate::operations::current_token();
        self.probe_while(needle, limit, ProbeLimits::for_candidates(limit),
            &mut ProbeWork::default(), || !token.cancelled())
    }

    /// The previous eager query is retained only for comparison tests. Never
    /// retry it after a bounded probe declines a broad or expensive needle.
    #[cfg(test)]
    fn candidate_query(&self, needle: &str) -> Option<BooleanQuery> {
        // The longest pieces narrow the most; one or two are enough.
        let mut pieces: Vec<&str> = needle
            .split(|c: char| !c.is_alphanumeric())
            .filter(|p| p.chars().nth(MIN_PIECE - 1).is_some())
            .collect();
        pieces.sort_by_key(|p| std::cmp::Reverse(p.len()));
        pieces.truncate(2);
        if pieces.is_empty() {
            return None;
        }
        let long: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.text, LONG),
            IndexRecordOption::Basic,
        ));
        let mut must: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for piece in pieces {
            let mut any: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Should, long.box_clone())];
            if piece.bytes().all(|b| b.is_ascii_hexdigit()) {
                // A needle at least as long as every indexed hex word can
                // only equal one. Shorter substrings retain the conservative
                // HEX marker; LONG remains included for >64-byte words.
                let (field, value) = if piece.len() >= self.max_hex_word {
                    (self.hex, piece)
                } else { (self.text, HEX) };
                any.push((
                    Occur::Should,
                    Box::new(TermQuery::new(Term::from_field_text(field, value), IndexRecordOption::Basic)),
                ));
            }
            if piece.len() <= MAX_WORD {
                // Alphanumeric characters are literals in the pattern.
                let pattern = format!(".*{}.*", regex_syntax_escape(piece));
                any.push((Occur::Should, Box::new(RegexQuery::from_pattern(&pattern, self.text).ok()?)));
            }
            must.push((Occur::Must, Box::new(BooleanQuery::new(any))));
        }
        Some(BooleanQuery::new(must))
    }

    fn probe_while(&self, needle: &str, limit: usize, limits: ProbeLimits, work: &mut ProbeWork,
        mut keep_going: impl FnMut() -> bool) -> Option<Vec<u32>> {
        let mut found: Option<Vec<u32>> = None;
        for piece in required_pieces(needle) {
            if !keep_going() { return None; }
            work.attempts += 1;
            match self.probe_piece(piece, limit, limits, work, &mut keep_going) {
                Ok(next) => {
                    if let Some(previous) = &mut found {
                        let mut at = 0;
                        previous.retain(|id| {
                            while at < next.len() && next[at] < *id { at += 1; }
                            next.get(at) == Some(id)
                        });
                    } else { found = Some(next); }
                    // The canonical reader cheaply verifies this bounded
                    // superset. Larger sets also try the second piece so a
                    // common-but-under-limit word does not hide a rare one.
                    if found.as_ref().is_some_and(|ids| ids.len() <= 4_096) { return found; }
                }
                // A shorter second piece can be rare even when the longest
                // is common. Both individually broad => exact SQL fallback.
                Err(ProbeStop::Candidates) => {}
                // Refinement is optional after a complete bounded superset.
                // Never expose the incomplete `next` set: the caller still
                // confirms the entire needle against the previous result.
                Err(ProbeStop::Work) => return if keep_going() { found } else { None },
                Err(_) => return None,
            }
        }
        found
    }

    fn probe_piece(&self, piece: &str, limit: usize, limits: ProbeLimits, work: &mut ProbeWork,
        keep_going: &mut impl FnMut() -> bool) -> Result<Vec<u32>, ProbeStop> {
        let searcher = self.reader.searcher();
        let mut lids = Vec::new();
        for reader in searcher.segment_readers() {
            if !keep_going() { return Err(ProbeStop::Cancelled); }
            let words = (reader.max_doc() as usize).div_ceil(64);
            let bytes = words.checked_mul(8).ok_or(ProbeStop::Invalid)?;
            if bytes > limits.bitmap_bytes { return Err(ProbeStop::Work); }
            let mut seen = Vec::new();
            seen.try_reserve_exact(words).map_err(|_| ProbeStop::Work)?;
            seen.resize(words, 0u64);
            work.peak_bitmap_bytes = work.peak_bitmap_bytes.max(bytes);
            let column = reader.fast_fields().u64("lid").map_err(|_| ProbeStop::Invalid)?;
            let alive = reader.alive_bitset();
            let mut visit = |doc: u32| -> Result<(), ProbeStop> {
                if doc >= reader.max_doc() { return Err(ProbeStop::Invalid); }
                if alive.is_some_and(|bits| !bits.is_alive(doc)) { return Ok(()); }
                let word = &mut seen[doc as usize / 64];
                let bit = 1u64 << (doc % 64);
                if *word & bit != 0 { return Ok(()); }
                *word |= bit;
                if lids.len() == limit { return Err(ProbeStop::Candidates); }
                let lid = u32::try_from(column.first(doc).ok_or(ProbeStop::Invalid)?)
                    .map_err(|_| ProbeStop::Invalid)?;
                if lids.len() == lids.capacity() {
                    let capacity = lids.capacity().max(128).saturating_mul(2).min(limit);
                    lids.try_reserve_exact(capacity - lids.len()).map_err(|_| ProbeStop::Work)?;
                }
                lids.push(lid);
                Ok(())
            };
            let inverted = reader.inverted_index(self.text).map_err(|_| ProbeStop::Invalid)?;
            // Markers can prove broadness immediately, before dictionary work.
            if let Some(info) = inverted.get_term_info(&Term::from_field_text(self.text, LONG)).map_err(|_| ProbeStop::Invalid)? {
                visit_postings(&inverted, &info, limits, work, keep_going, &mut visit)?;
            }
            if piece.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                let (field, value) = if piece.len() >= self.max_hex_word { (self.hex, piece) } else { (self.text, HEX) };
                let hex = reader.inverted_index(field).map_err(|_| ProbeStop::Invalid)?;
                if let Some(info) = hex.get_term_info(&Term::from_field_text(field, value)).map_err(|_| ProbeStop::Invalid)? {
                    visit_postings(&hex, &info, limits, work, keep_going, &mut visit)?;
                }
            }
            if piece.len() <= MAX_WORD {
                let mut terms = inverted.terms().stream().map_err(|_| ProbeStop::Invalid)?;
                while terms.advance() {
                    if !keep_going() { return Err(ProbeStop::Cancelled); }
                    if work.terms == limits.terms { return Err(ProbeStop::Work); }
                    work.terms += 1;
                    let word = std::str::from_utf8(terms.key()).map_err(|_| ProbeStop::Invalid)?;
                    if word.contains(piece) {
                        visit_postings(&inverted, terms.value(), limits, work, keep_going, &mut visit)?;
                    }
                }
            }
        }
        if !keep_going() { return Err(ProbeStop::Cancelled); }
        lids.sort_unstable();
        Ok(lids)
    }

    /// Exact scalar equality has stronger semantics than free substring
    /// search: longer hex words cannot satisfy it. No max-width restriction
    /// or regex dictionary expansion is required for this candidate source.
    pub(crate) fn exact_hex_candidates(&self, value: &str, limit: usize) -> Option<Vec<u32>> {
        if !exact_hex_literal(value) { return None; }
        let query = TermQuery::new(Term::from_field_text(self.hex, &value.to_ascii_lowercase()), IndexRecordOption::Basic);
        self.collect_candidates(&query, limit)
    }

    fn collect_candidates(&self, query: &dyn Query, limit: usize) -> Option<Vec<u32>> {
        let token = crate::operations::current_token();
        self.collect_candidates_while(query, limit, || !token.cancelled())
    }

    /// One pass, with no hash set or preliminary full count. Stop as soon as
    /// one live match proves the candidate budget insufficient. A missing lid
    /// or cancellation rejects the entire probe, never a partial result.
    ///
    /// Creating a regex scorer can still expand its dictionary/postings before
    /// yielding its first document. The bound is on candidate collection, not
    /// on all work performed internally by a Tantivy scorer.
    fn collect_candidates_while(
        &self,
        query: &dyn Query,
        limit: usize,
        mut keep_going: impl FnMut() -> bool,
    ) -> Option<Vec<u32>> {
        if !keep_going() { return None; }
        let searcher = self.reader.searcher();
        let weight = query.weight(EnableScoring::disabled_from_searcher(&searcher)).ok()?;
        let mut lids = Vec::new();
        for reader in searcher.segment_readers() {
            if !keep_going() { return None; }
            let mut scorer = weight.scorer(reader, 1.0).ok()?;
            let column = reader.fast_fields().u64("lid").ok()?;
            let alive = reader.alive_bitset();
            let mut doc = scorer.doc();
            let mut examined = 0usize;
            while doc != TERMINATED {
                if examined % 256 == 0 && !keep_going() { return None; }
                examined += 1;
                if alive.is_none_or(|bits| bits.is_alive(doc)) {
                    if lids.len() == limit { return None; }
                    lids.push(u32::try_from(column.first(doc)?).ok()?);
                }
                doc = scorer.advance();
            }
        }
        if !keep_going() { return None; }
        lids.sort_unstable();
        Some(lids)
    }

}

pub(crate) fn exact_hex_literal(value: &str) -> bool {
    (HEX_WORD..=MAX_WORD).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// For exact field equality, a complete word of an ASCII literal must also be
/// a complete word in the indexed value. Delimiters match `words` exactly;
/// never extract a hex substring from a larger alphanumeric word.
pub(crate) fn exact_field_hex_word(value: &str) -> Option<&str> {
    if value.len() > 512 || !value.is_ascii() { return None; }
    value.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| exact_hex_literal(word))
        .max_by_key(|word| word.len())
}

fn is_hex_word(word: &str) -> bool {
    word.len() >= HEX_WORD && word.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Escapes regex metacharacters (pieces are alphanumeric; kept for safety).
#[cfg(test)]
fn regex_syntax_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if !c.is_alphanumeric() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rows: &[String]) -> (tempfile::TempDir, Text) {
        let directory = tempfile::tempdir().unwrap();
        let writer = Writer::create(directory.path(), 1, 32 << 20).unwrap();
        for (id, row) in rows.iter().enumerate() {
            writer.add(id as u32, words(&row.to_lowercase())).unwrap();
        }
        writer.finish().unwrap();
        let text = Text::open(directory.path()).unwrap();
        (directory, text)
    }

    #[test]
    fn java_shape_fingerprint_is_a_selective_word_without_long_or_hex_inflation() {
        let trace = crate::java_stacktrace::parse(
            "a.FirstException: private message\n\tat a.Service.run(Service.java:12)\n",
            0,
            Default::default(),
        ).unwrap();
        assert!(trace.complete);
        let fingerprint = trace.fingerprint.unwrap();
        assert_eq!(words(&fingerprint), [fingerprint.clone()]);
        assert!(fingerprint.len() <= MAX_WORD);
        assert!(!is_hex_word(&fingerprint));
        let hex = "0000000000000000000000006585cfa1";
        let (_directory, text) = fixture(&[fingerprint.clone(), "ordinary record".into(), hex.into()]);
        assert_eq!(text.max_hex_word, hex.len(), "a Java fingerprint must not widen the hex shortcut domain");
        assert_eq!(text.candidates(&fingerprint, 1), Some(vec![0]));
        assert_eq!(text.candidates(hex, 1), Some(vec![2]));
    }

    #[test]
    fn direct_probe_preserves_unicode_markers_and_mixed_piece_completeness() {
        let hex = "0000000000000000000000006585cfa1";
        let rows: Vec<String> = ["ação concluída", "事件记录已处理", "café com açúcar", "açaí pedido",
            "longcommonword xyz", "longcommonword ordinary", "pedido /api/login", "unrelated"]
            .into_iter().map(str::to_owned).chain([
                format!("prefix{hex}suffix"), hex.into(), "f".repeat(64),
                format!("a{hex}{}", "b".repeat(65)),
            ]).collect();
        let (_directory, text) = fixture(&rows);
        for needle in ["ação", "记录已", "café", "açaí", "longcommonword xyz", "pedido api", "api login",
            "6585", hex, "prefix", "bbbb", "absentword", "--", "ab"] {
            let found = text.candidates(needle, 100);
            let old = text.candidate_query(needle).and_then(|query| text.collect_candidates(&query, 100));
            match (&found, &old) {
                (Some(found), Some(old)) => {
                    assert!(old.iter().all(|id| found.contains(id)), "old candidates lost for {needle}");
                    for (id, row) in rows.iter().enumerate() {
                        if row.contains(needle) { assert!(found.contains(&(id as u32)), "{needle}: {id}"); }
                    }
                }
                (None, None) => {}
                other => panic!("unexpected probe capability for {needle}: {other:?}"),
            }
        }
    }

    #[test]
    fn direct_probe_tries_a_rare_short_piece_after_a_common_long_piece() {
        let mut rows = vec!["longcommonword ordinary".to_string(); 128];
        rows[83] = "longcommonword xyz".into();
        let (_directory, text) = fixture(&rows);
        let mut work = ProbeWork::default();
        assert_eq!(text.probe_while("longcommonword xyz", 1, ProbeLimits::for_candidates(1), &mut work, || true), Some(vec![83]));
        assert_eq!(work.attempts, 2);
        assert_eq!(work.postings, 3, "two broad candidates plus the complete rare posting");

        let mut rows = vec!["longcommonword ordinary".to_string(); 4_500];
        rows[83] = "longcommonword xyz".into();
        let (_directory, text) = fixture(&rows);
        let mut work = ProbeWork::default();
        assert_eq!(text.probe_while("longcommonword xyz", 5_000, ProbeLimits::for_candidates(5_000), &mut work, || true), Some(vec![83]));
        assert_eq!(work.attempts, 2, "refine a common set even when it is below the caller's limit");

        // Two broad pieces with a rare intersection deliberately decline;
        // the caller must run exact SQL rather than publish a partial set.
        let rows: Vec<_> = (0..65).map(|id| match id {
            0 => "longcommonword short".into(),
            1..=32 => "longcommonword ordinary".into(),
            _ => "short ordinary".into(),
        }).collect();
        let (_directory, text) = fixture(&rows);
        assert_eq!(text.collect_candidates(&text.candidate_query("longcommonword short").unwrap(), 1), Some(vec![0]));
        assert_eq!(text.candidates("longcommonword short", 1), None);
        assert_eq!(required_pieces("repeat repeat abc def"), vec!["repeat", "abc"]);
    }

    #[test]
    fn direct_probe_rejects_partial_work_on_each_explicit_budget() {
        let rows: Vec<_> = (0..128).map(|id| format!("common word{id:04}")).collect();
        let (_directory, text) = fixture(&rows);
        let mut work = ProbeWork::default();
        assert_eq!(text.probe_while("common", 10, ProbeLimits::for_candidates(10), &mut work, || true), None);
        assert_eq!(work.postings, 11, "broad posting stops before full eager scorer setup");
        assert!(work.peak_bitmap_bytes <= 16);
        let mut work = ProbeWork::default();
        let limits = ProbeLimits { terms: 3, ..ProbeLimits::for_candidates(128) };
        assert_eq!(text.probe_while("word", 128, limits, &mut work, || true), None);
        assert_eq!(work.terms, 3);
        assert!(work.postings > 0, "partial candidates existed but were discarded");
        let mut work = ProbeWork::default();
        let limits = ProbeLimits { postings: 3, ..ProbeLimits::for_candidates(128) };
        assert_eq!(text.probe_while("common", 128, limits, &mut work, || true), None);
        assert_eq!(work.postings, 3);
        let mut work = ProbeWork::default();
        let limits = ProbeLimits { bitmap_bytes: 0, ..ProbeLimits::for_candidates(128) };
        assert_eq!(text.probe_while("common", 128, limits, &mut work, || true), None);
        assert_eq!(work.peak_bitmap_bytes, 0);
        assert_eq!(text.candidates("absent", 0), Some(Vec::new()));
        assert_eq!(text.candidates("common", 0), None);
    }

    #[test]
    fn direct_probe_retains_only_a_complete_superset_when_refinement_exhausts_work() {
        let rows: Vec<String> = (0..4_504).map(|id| match id {
            0..3_000 => "longcommonword short",
            3_000..4_500 => "longcommonword ordinary",
            _ => "short",
        }.into()).collect();
        let (_directory, text) = fixture(&rows);
        let limits = ProbeLimits::for_candidates(5_000);
        let mut first = ProbeWork::default();
        let complete = text.probe_while("longcommonword", 5_000, limits, &mut first, || true).unwrap();
        assert_eq!(complete, (0..4_500).collect::<Vec<u32>>());
        let exact: Vec<_> = rows.iter().enumerate().filter_map(|(id, row)|
            row.contains("longcommonword short").then_some(id as u32)).collect();
        assert_eq!(exact.len(), 3_000);
        for budget in [
            ProbeLimits { terms: first.terms + 1, ..limits },
            ProbeLimits { postings: first.postings + 7, ..limits },
        ] {
            let mut work = ProbeWork::default();
            let found = text.probe_while("longcommonword short", 5_000, budget, &mut work, || true).unwrap();
            assert_eq!(work.attempts, 2);
            assert_eq!(found, complete, "only the earlier complete set may be returned");
            let confirmed: Vec<_> = found.iter().copied().filter(|&id|
                rows[id as usize].contains("longcommonword short")).collect();
            assert_eq!(confirmed, exact);
            assert!(work.terms <= budget.terms && work.postings <= budget.postings);
            if budget.postings < limits.postings {
                assert_eq!(work.postings, first.postings + 7, "partial second-piece IDs existed");
            }
            eprintln!("TEXT_REFINEMENT {}", serde_json::json!({"rows":rows.len(),
                "candidates":found.len(),"exactMatches":confirmed.len(),"attempts":work.attempts,
                "terms":work.terms,"postings":work.postings,"peakBitmapBytes":work.peak_bitmap_bytes}));
        }
        let mut work = ProbeWork::default();
        let incomplete = ProbeLimits { postings: first.postings - 1, ..limits };
        assert_eq!(text.probe_while("longcommonword short", 5_000, incomplete, &mut work, || true), None,
            "an incomplete first piece never establishes a reusable superset");
        assert_eq!(work.attempts, 1);
        assert_eq!(text.candidates("longcommonword short", 1_000), None,
            "two individually over-cap pieces still require exact fallback");
    }

    #[test]
    fn direct_probe_never_reuses_a_superset_after_refinement_cancellation() {
        let rows = vec!["longcommonword short".to_string(); 4_500];
        let (_directory, text) = fixture(&rows);
        let limits = ProbeLimits::for_candidates(5_000);
        let mut first_polls = 0;
        let mut first = ProbeWork::default();
        assert_eq!(text.probe_while("longcommonword", 5_000, limits, &mut first, || {
            first_polls += 1; true
        }).unwrap().len(), 4_500);
        for (extra_polls, budget) in [
            (2, limits),
            // The second dictionary exhausts its allowance before visiting
            // any postings. Cancellation arrives at the reuse boundary.
            (4, ProbeLimits { terms: first.terms, ..limits }),
        ] {
            let id = format!("text-refinement-{}", uuid::Uuid::new_v4());
            let token = crate::operations::token(Some(id.clone())).unwrap();
            let mut polls = 0;
            let mut work = ProbeWork::default();
            let result = crate::operations::run_with_token(token, || {
                let token = crate::operations::current_token();
                assert_eq!(text.probe_while("longcommonword short", 5_000, budget, &mut work, || {
                    polls += 1;
                    if polls == first_polls + extra_polls { assert!(crate::operations::cancel_id(&id)); }
                    !token.cancelled()
                }), None);
                assert!(token.cancelled());
            });
            assert!(result.is_err());
            assert_eq!(work.attempts, 2);
            assert_eq!(polls, first_polls + extra_polls);
        }
    }

    #[test]
    fn direct_probe_never_reuses_a_superset_after_invalid_refinement_data() {
        let directory = tempfile::tempdir().unwrap();
        let writer = Writer::create(directory.path(), 1, 32 << 20).unwrap();
        for id in 0..4_500 { writer.add(id, words("longcommonword short")).unwrap(); }
        // Only the second piece reaches this malformed document. Its missing
        // source-row locator must not be ignored because the first set worked.
        let mut invalid = TantivyDocument::default();
        invalid.add_text(writer.text, "short");
        writer.writer.add_document(invalid).unwrap();
        writer.finish().unwrap();
        let text = Text::open(directory.path()).unwrap();
        let limits = ProbeLimits::for_candidates(5_000);
        assert_eq!(text.probe_while("longcommonword", 5_000, limits, &mut ProbeWork::default(), || true).unwrap().len(), 4_500);
        let mut work = ProbeWork::default();
        assert_eq!(text.probe_while("longcommonword short", 5_000, limits, &mut work, || true), None);
        assert_eq!(work.attempts, 2);
        assert!(work.postings > 4_500);
    }

    #[test]
    fn direct_probe_observes_named_cancellation_inside_dictionary_and_postings_work() {
        let rows: Vec<_> = (0..1024).map(|id| format!("common word{id:04}")).collect();
        let (_directory, text) = fixture(&rows);
        for (needle, cancel_after, expected_postings) in [("absent", 6, false), ("common", 5, true)] {
            let id = format!("text-probe-{needle}-{}", uuid::Uuid::new_v4());
            let token = crate::operations::token(Some(id.clone())).unwrap();
            let mut work = ProbeWork::default();
            let mut polls = 0;
            let result = crate::operations::run_with_token(token, || {
                let token = crate::operations::current_token();
                assert_eq!(text.probe_while(needle, 1024, ProbeLimits::for_candidates(1024), &mut work, || {
                    polls += 1;
                    if polls == cancel_after { assert!(crate::operations::cancel_id(&id)); }
                    !token.cancelled()
                }), None);
                assert!(token.cancelled());
                assert_eq!(text.candidates(needle, 1024), None);
            });
            assert!(result.is_err());
            assert_eq!(polls, cancel_after);
            assert_eq!(work.postings > 0, expected_postings);
            assert!(work.postings < 1024);
        }
    }

    #[test]
    #[ignore = "small explicit comparison of eager scorer versus bounded postings"]
    fn benchmark_direct_probe_on_small_dictionary() {
        let rows: Vec<_> = (0..10_000).map(|id| {
            let rare = if id == 5_123 { " xyz rareword 0000000000000000000000006585cfa1" } else { "" };
            format!("commonlongword request word{id:05}{rare}")
        }).collect();
        let (_directory, text) = fixture(&rows);
        for needle in ["commonlongword", "rareword", "absentword", "commonlongword xyz", "0000000000000000000000006585cfa1"] {
            for iteration in 0..21 {
                let mut answers = Vec::new();
                for direct in if iteration % 2 == 0 { [true, false] } else { [false, true] } {
                    let mut work = ProbeWork::default();
                    let started = std::time::Instant::now();
                    let found = if direct {
                        text.probe_while(needle, 32, ProbeLimits::for_candidates(32), &mut work, || true)
                    } else {
                        text.collect_candidates(&text.candidate_query(needle).unwrap(), 32)
                    };
                    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                    println!("DIRECT_PROBE_BENCH {}", serde_json::json!({
                        "needle":needle,"iteration":iteration,"direct":direct,"elapsedMs":elapsed,
                        "rows":rows.len(),"limit":32,"found":found.as_ref().map(Vec::len),
                        "termsExamined":direct.then_some(work.terms),"postingsExamined":direct.then_some(work.postings),
                        "peakBitmapBytes":direct.then_some(work.peak_bitmap_bytes),"attempts":direct.then_some(work.attempts)
                    }));
                    answers.push(found);
                }
                assert_eq!(answers[0], answers[1], "fixture candidate parity: {needle}");
            }
        }
    }

    /// Compare collectors against already-prepared immutable text stores. No
    /// ingestion, source hashing or cache deletion belongs in this microbench.
    #[test]
    #[ignore = "requires explicit LOGINSIGHT_TEXT_BENCH_DIR; read-only existing indexes"]
    fn benchmark_bounded_candidate_collection() {
        use std::time::Instant;
        use tantivy::collector::{Count, DocSetCollector};
        let dir = std::env::var("LOGINSIGHT_TEXT_BENCH_DIR").expect("set text-store parent directory");
        let repeats: usize = std::env::var("LOGINSIGHT_TEXT_BENCH_REPEATS").unwrap_or_else(|_| "10".into()).parse().unwrap();
        assert!((1..=100).contains(&repeats));
        let mut dirs: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "text")).collect();
        dirs.sort();
        assert!(!dirs.is_empty());
        let texts: Vec<_> = dirs.iter().map(|p| Text::open(p).expect("valid text store")).collect();
        for needle in ["request", "rareneedle", "0000000000000000000000000a1aa3c1", "absentwordneverpresent"] {
            for iteration in 0..repeats {
                let mut answers = Vec::new();
                // Alternate order to avoid systematically favoring the second
                // probe's page-cache state. These are warm-index measurements.
                for method in match iteration % 3 { 0 => [0, 1, 2], 1 => [1, 2, 0], _ => [2, 0, 1] } {
                    let started = Instant::now();
                    let mut count = 0usize;
                    let mut signature = 0u64;
                    let mut too_broad = false;
                    for text in &texts {
                        let limit = 250_000 - count;
                        let found = if method == 0 {
                            let query = text.candidate_query(needle).unwrap();
                            let searcher = text.reader.searcher();
                            if searcher.search(&query, &Count).unwrap() > limit { None } else {
                                let docs = searcher.search(&query, &DocSetCollector).unwrap();
                                let mut lids = Vec::with_capacity(docs.len());
                                let mut columns = std::collections::HashMap::new();
                                for doc in docs {
                                    let column = columns.entry(doc.segment_ord).or_insert_with(||
                                        searcher.segment_reader(doc.segment_ord).fast_fields().u64("lid").unwrap());
                                    lids.push(column.first(doc.doc_id).unwrap() as u32);
                                }
                                lids.sort_unstable(); Some(lids)
                            }
                        } else if method == 1 {
                            text.collect_candidates(&text.candidate_query(needle).unwrap(), limit)
                        }
                        else { text.candidates(needle, limit) };
                        match found {
                            Some(lids) => { count += lids.len(); for lid in lids { signature = signature.wrapping_mul(1_099_511_628_211).wrapping_add(u64::from(lid) + 1); } }
                            None => { too_broad = true; break; }
                        }
                        signature = signature.wrapping_mul(1_099_511_628_211);
                    }
                    answers.push((count, too_broad, signature));
                    println!("TEXT_BENCH {}", serde_json::json!({"needle":needle,"iteration":iteration,
                        "collector":match method { 0 => "v0.9-count-then-docset", 1 => "v0.10-eager-one-pass", _ => "v0.10-direct-bounded-probe" },
                        "elapsedMs":started.elapsed().as_secs_f64()*1000.0,"count":count,"tooBroad":too_broad,"signature":signature,"stores":texts.len()}));
                }
                assert_eq!(answers[0], answers[1], "candidate result parity");
                assert_eq!(answers[0], answers[2], "candidate result parity or new fallback regression");
            }
        }
    }

    #[test]
    fn bounded_collection_matches_legacy_with_live_docs_and_segment_local_lids() {
        use tantivy::collector::{Count, DocSetCollector};
        let dir = tempfile::tempdir().unwrap();
        let (schema, lid, text_field, _) = schema();
        let index = Index::create_in_dir(dir.path(), schema).unwrap();
        let mut writer: IndexWriter = index.writer_with_num_threads(1, 32 << 20).unwrap();
        writer.set_merge_policy(Box::new(tantivy::merge_policy::NoMergePolicy));
        for group in 0..3u32 {
            for row in 0..300u32 {
                let mut doc = TantivyDocument::default();
                doc.add_u64(lid, u64::from(group * 300 + row));
                doc.add_text(text_field, "common");
                if row == 0 { doc.add_text(text_field, "deleted"); }
                writer.add_document(doc).unwrap();
            }
            writer.commit().unwrap();
        }
        writer.delete_term(Term::from_field_text(text_field, "deleted"));
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
        std::fs::write(dir.path().join("hex-length"), "0").unwrap();
        let text = Text::open(dir.path()).unwrap();
        let query = TermQuery::new(Term::from_field_text(text_field, "common"), IndexRecordOption::Basic);
        let searcher = text.reader.searcher();
        assert_eq!(searcher.search(&query, &Count).unwrap(), 897);
        let mut expected: Vec<u32> = searcher.search(&query, &DocSetCollector).unwrap().into_iter()
            .map(|doc| searcher.segment_reader(doc.segment_ord).fast_fields().u64("lid").unwrap().first(doc.doc_id).unwrap() as u32)
            .collect();
        expected.sort_unstable();
        assert_eq!(text.collect_candidates(&query, expected.len()).unwrap(), expected);
        assert_eq!(text.candidates("common", expected.len()).unwrap(), expected);
        assert_eq!(text.candidates("common", expected.len() - 1), None);
        assert_eq!(text.collect_candidates(&query, expected.len() - 1), None);
        assert_eq!(text.collect_candidates(&query, 0), None);
        let absent = TermQuery::new(Term::from_field_text(text_field, "absent"), IndexRecordOption::Basic);
        assert_eq!(text.collect_candidates(&absent, 0), Some(Vec::new()));

        // The collection visits at most the first budget+1 live matches. This
        // callback is also the cooperative-cancellation boundary used in production.
        let mut polls = 0;
        assert_eq!(text.collect_candidates_while(&query, 1, || { polls += 1; true }), None);
        assert_eq!(polls, 3, "no second segment or full preliminary count");
        let mut polls = 0;
        assert_eq!(text.collect_candidates_while(&query, 900, || { polls += 1; polls < 4 }), None,
            "cancellation never returns partially collected candidates");
        assert_eq!(polls, 4);
    }

    #[test]
    fn exact_field_words_require_complete_ascii_tokens() {
        let uuid = "3f2a1b4c-1111-2222-3333-444455556666";
        assert_eq!(exact_field_hex_word(uuid), Some("444455556666"));
        assert_eq!(exact_field_hex_word("request/DEADBEEF?x=1"), Some("DEADBEEF"));
        assert_eq!(exact_field_hex_word("prefixdeadbeef"), None);
        assert_eq!(exact_field_hex_word("deadbeefsuffix"), None);
        assert_eq!(exact_field_hex_word("ação-deadbeef"), None);
        assert_eq!(exact_field_hex_word(&format!("{}-deadbeef", "x".repeat(512))), None);
    }

    #[test]
    fn full_hex_words_are_selective_but_substrings_and_long_words_stay_safe() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Writer::create(dir.path(), 1, 32 << 20).unwrap();
        let first = "0000000000000000000000006585cfa1";
        let second = "000000000000000000000000e1677d9c";
        writer.add(0, words(first)).unwrap();
        writer.add(1, words(second)).unwrap();
        writer.finish().unwrap();
        let text = Text::open(dir.path()).unwrap();
        assert_eq!(text.candidates(first, 1).unwrap(), vec![0]);
        assert_eq!(text.candidates(second, 1).unwrap(), vec![1]);
        assert_eq!(text.candidates("6585", 1), None, "partial hex keeps the broad safe marker");

        let dir = tempfile::tempdir().unwrap();
        let writer = Writer::create(dir.path(), 1, 32 << 20).unwrap();
        writer.add(0, words(&format!("aa{first}bb"))).unwrap();
        writer.add(1, words(&format!("{}{first}", "f".repeat(80)))).unwrap();
        writer.add(2, words(&format!("prefix{first}suffix"))).unwrap();
        writer.finish().unwrap();
        let text = Text::open(dir.path()).unwrap();
        assert_eq!(text.candidates(first, 10).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn mixed_32_and_64_character_hex_values_keep_conservative_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Writer::create(dir.path(), 1, 32 << 20).unwrap();
        let trace = "0000000000000000000000006585cfa1";
        writer.add(0, words(trace)).unwrap();
        writer.add(1, words(&"f".repeat(64))).unwrap();
        writer.add(2, words(&format!("{}{}{}", "a".repeat(16), trace, "b".repeat(16)))).unwrap();
        writer.finish().unwrap();
        let text = Text::open(dir.path()).unwrap();
        // An exact-token-only shortcut would miss the trace inside row 2.
        // The broad candidate set also includes row 1 and exceeds this budget.
        assert_eq!(text.candidates(trace, 1), None);
        assert_eq!(text.candidates(trace, 10).unwrap(), vec![0, 1, 2]);
        assert_eq!(text.exact_hex_candidates(trace, 1).unwrap(), vec![0]);
        assert_eq!(text.exact_hex_candidates(&trace.to_ascii_uppercase(), 1).unwrap(), vec![0]);
    }

    #[test]
    fn candidates_cover_every_row_containing_the_needle() {
        let dir = tempfile::tempdir().unwrap();
        let texts = [
            "failed password for root",
            "timeout for request 42",
            "ação concluída",
            &format!("token {} fim", "x".repeat(100)),
            "nada aqui",
        ];
        let writer = Writer::create(dir.path(), 1, 32 << 20).unwrap();
        for (i, t) in texts.iter().enumerate() {
            writer.add(i as u32, words(t)).unwrap();
        }
        writer.finish().unwrap();
        let text = Text::open(dir.path()).unwrap();
        for needle in ["out for re", "ação", "root", "xxxx", "ssword f", "nada"] {
            let found = text.candidates(needle, 100).unwrap();
            for (i, t) in texts.iter().enumerate() {
                if t.contains(needle) {
                    assert!(found.contains(&(i as u32)), "{needle} in {t}");
                }
            }
        }
        assert_eq!(text.candidates("root", 100).unwrap(), vec![0, 3]);
        assert_eq!(text.candidates("--", 100), None);
        // Ids become one word; searching part of one still finds its row.
        let dir = tempfile::tempdir().unwrap();
        let writer = Writer::create(dir.path(), 1, 32 << 20).unwrap();
        writer.add(0, words("trace 3f2a1b4c99 ok")).unwrap();
        writer.add(1, words("nothing")).unwrap();
        writer.finish().unwrap();
        let text = Text::open(dir.path()).unwrap();
        assert_eq!(text.candidates("a1b4", 100).unwrap(), vec![0]);
        assert_eq!(text.candidates("trace", 100).unwrap(), vec![0]);
    }
}
