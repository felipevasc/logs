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
use tantivy::collector::{Count, DocSetCollector};
use tantivy::query::{BooleanQuery, Occur, Query, RegexQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST,
};
use tantivy::tokenizer::{PreTokenizedString, Token};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term};

/// Word standing for every word longer than [`MAX_WORD`] bytes.
const LONG: &str = "\u{1}";
const MAX_WORD: usize = 64;
/// Word standing for every hexadecimal word of [`HEX_WORD`] or more
/// characters (ids, hashes): unique per record, they would fill the word list
/// that each search scans. Only pieces made of hexadecimal digits can lie
/// inside them, so only those searches include this word.
const HEX: &str = "\u{2}";
const HEX_WORD: usize = 8;
/// Shorter pieces narrow almost nothing, so neither they nor words this short
/// are indexed (a piece of this length only lies inside words at least as long).
const MIN_PIECE: usize = 3;

/// Index folder of a store (`<key>.text` beside `<key>.duckdb`).
pub(crate) fn dir_of(store: &Path) -> PathBuf {
    store.with_extension("text")
}

fn schema() -> (Schema, Field, Field) {
    let mut builder = Schema::builder();
    let lid = builder.add_u64_field("lid", FAST);
    let indexing = TextFieldIndexing::default()
        .set_tokenizer("raw")
        .set_index_option(IndexRecordOption::Basic);
    let text = builder.add_text_field("t", TextOptions::default().set_indexing_options(indexing));
    (builder.build(), lid, text)
}

/// Distinct words of a lowercase text.
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().nth(MIN_PIECE - 1).is_some())
        .map(|w| if w.len() > MAX_WORD { LONG } else if is_hex_word(w) { HEX } else { w })
        .filter(|w| seen.insert(*w))
        .map(str::to_string)
        .collect()
}

pub(crate) struct Writer {
    writer: IndexWriter,
    lid: Field,
    text: Field,
}

impl Writer {
    pub(crate) fn create(dir: &Path, threads: usize, memory: usize) -> Result<Writer, String> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let (schema, lid, text) = schema();
        let index = Index::create_in_dir(dir, schema).map_err(|e| e.to_string())?;
        let writer = index
            .writer_with_num_threads(threads.max(1), memory.max(threads.max(1) * (16 << 20)))
            .map_err(|e| e.to_string())?;
        Ok(Writer { writer, lid, text })
    }

    pub(crate) fn add(&self, lid: u32, words: Vec<String>) -> Result<(), String> {
        let tokens = words
            .into_iter()
            .enumerate()
            .map(|(position, text)| Token {
                offset_from: 0,
                offset_to: 0,
                position,
                text,
                position_length: 1,
            })
            .collect();
        let mut doc = TantivyDocument::default();
        doc.add_u64(self.lid, u64::from(lid));
        doc.add_pre_tokenized_text(self.text, PreTokenizedString { text: String::new(), tokens });
        self.writer.add_document(doc).map(|_| ()).map_err(|e| e.to_string())
    }

    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.writer.commit().map_err(|e| e.to_string())?;
        self.writer.wait_merging_threads().map_err(|e| e.to_string())
    }
}

pub(crate) struct Text {
    reader: IndexReader,
    lid: Field,
    text: Field,
}

impl Text {
    pub(crate) fn open(dir: &Path) -> Option<Text> {
        if !dir.is_dir() {
            return None;
        }
        let index = Index::open_in_dir(dir).ok()?;
        let (_, lid, text) = schema();
        let reader = index.reader().ok()?;
        Some(Text { reader, lid, text })
    }

    /// Rows (sorted) whose text may contain `needle`; `None` when the index
    /// cannot narrow the search or more than `limit` rows qualify.
    pub(crate) fn candidates(&self, needle: &str, limit: usize) -> Option<Vec<u32>> {
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
                any.push((
                    Occur::Should,
                    Box::new(TermQuery::new(Term::from_field_text(self.text, HEX), IndexRecordOption::Basic)),
                ));
            }
            if piece.len() <= MAX_WORD {
                // Alphanumeric characters are literals in the pattern.
                let pattern = format!(".*{}.*", regex_syntax_escape(piece));
                any.push((Occur::Should, Box::new(RegexQuery::from_pattern(&pattern, self.text).ok()?)));
            }
            must.push((Occur::Must, Box::new(BooleanQuery::new(any))));
        }
        let query = BooleanQuery::new(must);
        let searcher = self.reader.searcher();
        if searcher.search(&query, &Count).ok()? > limit {
            return None;
        }
        let docs = searcher.search(&query, &DocSetCollector).ok()?;
        let mut lids = Vec::with_capacity(docs.len());
        let mut columns = std::collections::HashMap::new();
        for doc in docs {
            let column = match columns.entry(doc.segment_ord) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(
                    searcher.segment_reader(doc.segment_ord).fast_fields().u64("lid").ok()?,
                ),
            };
            lids.push(column.first(doc.doc_id)? as u32);
        }
        lids.sort_unstable();
        Some(lids)
    }
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
