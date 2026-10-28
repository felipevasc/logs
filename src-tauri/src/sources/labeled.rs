//! Records made of `Label: value` pairs separated by a tab or by two or more spaces:
//! `Host: 192.168.2.4 (srv)<TAB>Ports: 80/open/tcp//http///` (nmap `-oG`), `Time: …<TAB>User: ana`.
//!
//! This is purely additive. The format is claimed only when nothing else recognizes the file and
//! nearly every line has that shape; each record is read exactly as the generic reader reads it
//! today and the pairs are added as `label.<name>` fields, so no existing field changes.
use serde_json::Value;

const MAX_LABEL: usize = 32;
const MAX_WORDS: usize = 3;

/// `Label: value` -> (`Label`, `value`). The colon must be followed by a space (or end the
/// segment), which leaves `http://x`, `12:30` and `a:b` alone.
fn pair(segment: &str) -> Option<(&str, &str)> {
    let at = segment.find(':')?;
    let (label, rest) = (segment[..at].trim(), &segment[at + 1..]);
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let valid = !label.is_empty()
        && label.len() <= MAX_LABEL
        && label.split(' ').count() <= MAX_WORDS
        && label.chars().next().is_some_and(char::is_alphabetic)
        && label.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
        && !label.contains("  ");
    valid.then_some((label, rest.trim()))
}

/// Every segment of the line is a pair, and there are at least two. None otherwise.
pub(super) fn pairs(line: &str) -> Option<Vec<(&str, &str)>> {
    let line = line.trim();
    let mut segments: Vec<&str> = line.split('\t').map(str::trim).filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        segments = split_wide_spaces(line);
    }
    if segments.len() < 2 {
        return None;
    }
    segments.into_iter().map(pair).collect()
}

/// Segments separated by two or more spaces.
fn split_wide_spaces(line: &str) -> Vec<&str> {
    let (mut out, mut start, bytes) = (Vec::new(), 0usize, line.as_bytes());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' && bytes.get(i + 1) == Some(&b' ') {
            let end = i;
            while bytes.get(i) == Some(&b' ') {
                i += 1;
            }
            out.push(line[start..end].trim());
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(line[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// The file is `Label: value` records when almost every line is a run of pairs that open with
/// the same label. Comment lines are skipped; too little text proves nothing.
pub(super) fn detect(lines: &[std::borrow::Cow<'_, str>]) -> bool {
    let lines: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).take(40).collect();
    if lines.len() < 2 {
        return false;
    }
    let mut openings = std::collections::HashMap::<String, usize>::new();
    let mut matching = 0usize;
    for line in &lines {
        if let Some(found) = pairs(line) {
            matching += 1;
            *openings.entry(found[0].0.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let commonest = openings.values().copied().max().unwrap_or(0);
    matching * 10 >= lines.len() * 8 && commonest * 10 >= lines.len() * 7
}

/// `Ignored State` -> `ignored_state`.
fn key(label: &str) -> String {
    label.to_lowercase().replace([' ', '-', '.'], "_")
}

/// Adds the pairs of a single-line record as `label.<name>` fields (never replacing a field).
pub(super) fn add_fields(ev: &mut crate::model::Event, text: &str) {
    if text.contains('\n') {
        return;
    }
    if let Some(found) = pairs(text) {
        for (label, value) in found {
            if !value.is_empty() {
                ev.fields.entry(format!("label.{}", key(label))).or_insert_with(|| Value::from(value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    fn lines<'a>(text: &'a str) -> Vec<Cow<'a, str>> {
        text.lines().map(Cow::Borrowed).collect()
    }

    #[test]
    fn pairs_need_two_labeled_segments_and_a_space_after_the_colon() {
        let found = pairs("Host: 10.0.0.1 (srv)\tPorts: 80/open/tcp//http///\tIgnored State: filtered (997)").unwrap();
        assert_eq!(found.iter().map(|p| p.0).collect::<Vec<_>>(), ["Host", "Ports", "Ignored State"]);
        assert_eq!(found[0].1, "10.0.0.1 (srv)");
        assert_eq!(pairs("level: info  msg: hello world").unwrap().len(), 2);
        for not in ["Host: example.test", "see http://x.test:80/a  and b: c", "12:30\tdone", "a:b\tc:d", "Host: x\tnot a pair", ""] {
            assert!(pairs(not).is_none(), "{not}");
        }
    }

    #[test]
    fn only_files_where_nearly_every_line_has_the_same_shape_qualify() {
        let nmap = "Host: 10.0.0.1 ()\tPorts: 80/open/tcp//http///\nHost: 10.0.0.2 ()\tPorts: 443/closed/tcp//https///\nHost: 10.0.0.3 ()\tStatus: Up\n";
        assert!(detect(&lines(nmap)));
        assert!(detect(&lines("# comment\nTime: 1\tUser: ana\nTime: 2\tUser: bruno\n")));
        // single pairs per line, HTTP headers, prose and a stray pair line are not claimed
        for text in [
            "Host: example.test\nUser-Agent: curl/8\nAccept: */*\n",
            "Name: web01\nRole: frontend\nOwner: ops\n",
            "The quick brown fox.\nSecond line.\nThird: one\tFourth: two\n",
            "Host: a\tPorts: b\n",
        ] {
            assert!(!detect(&lines(text)), "{text}");
        }
    }

    #[test]
    fn fields_are_added_without_replacing_any() {
        let mut ev = crate::model::Event::empty();
        ev.fields.insert("label.host".into(), Value::from("kept"));
        add_fields(&mut ev, "Host: 10.0.0.1 ()\tIgnored State: filtered (997)");
        assert_eq!(ev.fields["label.host"], "kept");
        assert_eq!(ev.fields["label.ignored_state"], "filtered (997)");
    }
}
