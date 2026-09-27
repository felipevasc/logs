//! Independent original scalar segments. No field concatenation or generated enrichment.
use crate::model::Event;
use serde_json::Value;

pub(super) struct Segment<'a> {
    pub field: String,
    pub value: &'a str,
}
pub(super) fn segments(ev: &Event) -> (Vec<Segment<'_>>, bool) {
    struct Walk<'a> {
        out: Vec<Segment<'a>>,
        bytes: usize,
        nodes: usize,
        clipped: bool,
    }
    impl<'a> Walk<'a> {
        fn text(&mut self, field: String, value: &'a str) {
            if value.is_empty() {
                return;
            }
            if self.out.len() >= 128 || self.bytes >= 256 * 1024 || field.len() > 1024 {
                self.clipped = true;
                return;
            }
            let mut end = value.len().min(65536).min(256 * 1024 - self.bytes);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            self.clipped |= end < value.len();
            self.bytes += end;
            self.out.push(Segment {
                field,
                value: &value[..end],
            });
        }
        fn visit(&mut self, field: String, value: &'a Value, depth: usize) {
            if depth > 12 || self.nodes >= 1024 || field.len() > 1024 {
                self.clipped = true;
                return;
            }
            self.nodes += 1;
            match value {
                Value::String(s) => self.text(field, s),
                Value::Object(o) => {
                    for (k, v) in o {
                        if self.nodes >= 1024 {
                            self.clipped = true;
                            break;
                        }
                        self.visit(format!("{field}.{k}"), v, depth + 1);
                    }
                }
                Value::Array(a) => {
                    for (i, v) in a.iter().enumerate() {
                        if self.nodes >= 1024 {
                            self.clipped = true;
                            break;
                        }
                        self.visit(format!("{field}.{i}"), v, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }
    let mut walk = Walk {
        out: vec![],
        bytes: 0,
        nodes: 0,
        clipped: false,
    };
    walk.text("message".into(), &ev.message);
    for (field, value) in &ev.fields {
        if field.starts_with("_sec.") || field == "_sec" {
            continue;
        }
        if walk.nodes >= 1024 {
            walk.clipped = true;
            break;
        }
        walk.visit(field.clone(), value, 0);
    }
    if ev.raw != ev.message {
        walk.text("raw".into(), &ev.raw);
    }
    (walk.out, walk.clipped)
}
