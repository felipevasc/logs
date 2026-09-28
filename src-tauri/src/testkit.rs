//! Entry points for the integration tests in `tests/`, which exercise the same
//! loading and query paths as the app. Not part of the application's behavior.

pub use crate::model::Event;

/// Loads a file as the app does (packages, spreadsheets, encodings, detected
/// format) and returns the detected format and every event.
pub fn load(path: &str) -> Result<(String, Vec<Event>), String> {
    let idx = crate::index_source_file(path, "auto", None)?;
    let empty = crate::model::CodesConfig::default();
    let events = (0..idx.lines.len())
        .map(|i| crate::sources::event_at(&idx, i, &empty, &empty, &[]))
        .collect();
    Ok((idx.parts[0].format.clone(), events))
}

pub fn detect_format(sample: &[u8]) -> &'static str {
    crate::sources::detect_format(sample)
}

pub fn encoding_of(sample: &[u8]) -> Option<&'static str> {
    crate::workspace::sniff_encoding(sample).map(|encoding| encoding.name())
}
