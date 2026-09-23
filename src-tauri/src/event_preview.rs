//! Bounded event previews for analytical result pages; original records stay intact.
use crate::model::Event;
use std::io::Write;

pub(crate) const PREVIEW_BYTES: usize = 64 * 1024;

fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
struct BoundedWriter {
    bytes: Vec<u8>,
    max: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("JSON excede o orçamento"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn preview(event: &Event) -> (Event, bool) {
    let mut json = BoundedWriter {
        bytes: vec![],
        max: PREVIEW_BYTES,
    };
    if serde_json::to_writer(&mut json, event).is_ok() {
        if let Ok(event) = serde_json::from_slice(&json.bytes) {
            return (event, false);
        }
    }
    let mut result = Event::empty();
    result.id = event.id;
    result.event_ref = prefix(&event.event_ref, 512).into();
    result.timestamp = event.timestamp;
    result.source = prefix(&event.source, 1000).into();
    result.level = prefix(&event.level, 100).into();
    result.code = prefix(&event.code, 1000).into();
    result.name = prefix(&event.name, 1000).into();
    result.parse_status = prefix(&event.parse_status, 100).into();
    result.description = prefix(&event.description, 2000).into();
    result.message = prefix(&event.message, 12000).into();
    result.raw = prefix(&event.raw, 24000).into();
    for (key, value) in &event.fields {
        if result.fields.len() >= 20 {
            break;
        }
        let mut json = BoundedWriter {
            bytes: vec![],
            max: 512,
        };
        if serde_json::to_writer(&mut json, value).is_ok() {
            result.fields.insert(prefix(key, 160).into(), value.clone());
        }
    }
    // Text byte limits alone do not bound JSON: control characters can expand
    // to six escaped bytes each. Check the actual serialized response budget.
    loop {
        let mut json = BoundedWriter {
            bytes: vec![],
            max: PREVIEW_BYTES,
        };
        if serde_json::to_writer(&mut json, &result).is_ok() {
            break;
        }
        let fields = [
            &mut result.raw,
            &mut result.message,
            &mut result.description,
            &mut result.name,
            &mut result.source,
            &mut result.code,
            &mut result.event_ref,
            &mut result.level,
            &mut result.parse_status,
        ];
        if let Some(longest) = fields.into_iter().max_by_key(|value| value.len()) {
            let length = prefix(longest, longest.len() / 2).len();
            longest.truncate(length);
        }
    }
    (result, true)
}
