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
use tantivy::query::{BooleanQuery, EnableScoring, Occur, Query, RegexQuery, TermQuery};
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
        self.collect_candidates(&self.candidate_query(needle)?, limit)
    }

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
                for legacy in if iteration % 2 == 0 { [true, false] } else { [false, true] } {
                    let started = Instant::now();
                    let mut count = 0usize;
                    let mut signature = 0u64;
                    let mut too_broad = false;
                    for text in &texts {
                        let query = text.candidate_query(needle).unwrap();
                        let limit = 250_000 - count;
                        let found = if legacy {
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
                        } else { text.collect_candidates(&query, limit) };
                        match found {
                            Some(lids) => { count += lids.len(); for lid in lids { signature = signature.wrapping_mul(1_099_511_628_211).wrapping_add(u64::from(lid) + 1); } }
                            None => { too_broad = true; break; }
                        }
                        signature = signature.wrapping_mul(1_099_511_628_211);
                    }
                    answers.push((count, too_broad, signature));
                    println!("TEXT_BENCH {}", serde_json::json!({"needle":needle,"iteration":iteration,
                        "collector":if legacy { "v0.9-count-then-docset" } else { "v0.10-bounded-one-pass" },
                        "elapsedMs":started.elapsed().as_secs_f64()*1000.0,"count":count,"tooBroad":too_broad,"signature":signature,"stores":texts.len()}));
                }
                assert_eq!(answers[0], answers[1], "candidate result parity");
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
