//! Syslog records. BSD (RFC 3164) headers accept any timestamp layout: the
//! classic `Oct 12 00:00:00`, with a year (Cisco), ISO 8601 (rsyslog high
//! precision, `journalctl -o short-iso`), Cisco sequence numbers and clock
//! markers (`123: *Oct 12 ...`). RFC 5424 structured data becomes `sd.<id>.<param>`
//! fields. A priority (`<PRI>`) gives the level and facility.
use super::generic::{self, severity_level};
use super::stamp::{self, Context};
use crate::model::Event;
use serde_json::Value;

fn priority(rest: &str) -> Result<(Option<u32>, &str), ()> {
    let Some(after) = rest.strip_prefix('<') else { return Ok((None, rest)) };
    let (number, tail) = after.split_once('>').ok_or(())?;
    if number.is_empty() || number.len() > 3 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    let pri: u32 = number.parse().map_err(|_| ())?;
    if pri > 191 {
        return Err(());
    }
    Ok((Some(pri), tail))
}

/// A syslog tag: `name:` or `name[pid]:`.
fn tag(word: &str) -> Option<(&str, Option<&str>, bool)> {
    generic::process_token(word).filter(|(_, _, terminal)| *terminal)
}

fn first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches([' ', '\t']);
    match s.find([' ', '\t']) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

fn apply_priority(ev: &mut Event, pri: Option<u32>) -> bool {
    let Some(pri) = pri else { return false };
    ev.fields.insert("syslog.facility".into(), Value::from((pri / 8).to_string()));
    ev.fields.insert("syslog.severity".into(), Value::from((pri % 8).to_string()));
    ev.level = severity_level(pri % 8).into();
    true
}

/// BSD syslog with any timestamp. A tag (`name:`, `name[pid]:`) or a vendor
/// mnemonic (`%ASA-6-302013:`) must follow the optional host, so ordinary
/// timestamped application lines are not mistaken for syslog.
pub(super) fn bsd(line: &str, ctx: &Context) -> Option<Event> {
    let (pri, rest) = priority(line.trim_start()).ok()?;
    if pri.is_some() && rest.starts_with("1 ") {
        return None;
    }
    let mut rest = rest;
    let mut sequence = None;
    if let Some((number, tail)) = rest.split_once(": ") {
        if !number.is_empty() && number.len() <= 10 && number.bytes().all(|b| b.is_ascii_digit()) {
            sequence = Some(number);
            rest = tail;
        }
    }
    let rest = rest.trim_start_matches(['*', '.']);
    let found = stamp::at(rest, 0, ctx)?;
    if !found.has_time {
        return None;
    }
    let after = rest[found.end..].trim_start_matches(':');
    let (first, after_first) = first_word(after);
    let (host, tag_word, message) = if first.starts_with('%') {
        (None, None, after.trim_start())
    } else if let Some(found) = tag(first) {
        (None, Some(found), after_first)
    } else {
        let (second, after_second) = first_word(after_first);
        if second.starts_with('%') {
            (Some(first), None, after_first.trim_start())
        } else {
            (Some(first), Some(tag(second)?), after_second)
        }
    };
    let host_like = |h: &str| !h.is_empty() && h.len() <= 255 && h.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%' | '@'));
    if host.is_some_and(|h| !host_like(h) || h.ends_with(':')) {
        return None;
    }
    // "ERROR:", "LOG:" after a timestamp are levels of an application log, not tags.
    if tag_word.is_some_and(|(name, pid, _)| pid.is_none() && name.bytes().all(|b| b.is_ascii_uppercase()) && generic::level_word(name).is_some()) {
        return None;
    }
    let message = message.strip_prefix(' ').unwrap_or(message);
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = Some(found.ms);
    if let Some(host) = host {
        ev.source = host.to_string();
    }
    if let Some((name, pid, _)) = tag_word {
        ev.fields.insert("process".into(), Value::from(name));
        if let Some(pid) = pid {
            ev.fields.insert("pid".into(), Value::from(pid));
        }
        if ev.source.is_empty() {
            ev.source = name.to_string();
        }
    }
    if let Some(sequence) = sequence {
        ev.fields.insert("sequence".into(), Value::from(sequence));
    }
    let explicit = apply_priority(&mut ev, pri);
    if !explicit {
        ev.level = crate::model::class_label(super::text_level_class(message.as_bytes())).to_string();
    }
    ev.message = message.to_string();
    embedded_formats(&mut ev, message);
    generic::enrich(&mut ev, message, explicit);
    Some(ev)
}

/// CEF/LEEF payloads sent over syslog.
fn embedded_formats(ev: &mut Event, message: &str) {
    let inner = if message.starts_with("CEF:") {
        super::parse_cef(message)
    } else if message.starts_with("LEEF:") {
        super::parse_leef(message)
    } else {
        None
    };
    if let Some(inner) = inner {
        for (key, value) in inner.fields {
            ev.fields.entry(key).or_insert(value);
        }
        if ev.code.is_empty() {
            ev.code = inner.code;
        }
        ev.level = inner.level;
        ev.message = inner.message;
        if let Some(ts) = inner.timestamp {
            ev.timestamp = Some(ts);
        }
        ev.fields.entry("product").or_insert_with(|| Value::from(inner.source));
    }
}

/// `[id name="value" ...][id2 ...]` blocks; returns the pairs and the message.
fn structured_data(s: &str) -> Option<(Vec<(String, String)>, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while b.get(i) == Some(&b'[') {
        i += 1;
        let id_start = i;
        while i < b.len() && !matches!(b[i], b' ' | b']') {
            i += 1;
        }
        let id = &s[id_start..i];
        if id.is_empty() {
            return None;
        }
        loop {
            while b.get(i) == Some(&b' ') {
                i += 1;
            }
            if *b.get(i)? == b']' {
                i += 1;
                break;
            }
            let name_start = i;
            while i < b.len() && !matches!(b[i], b'=' | b' ' | b']') {
                i += 1;
            }
            let name = &s[name_start..i];
            if name.is_empty() || b.get(i) != Some(&b'=') || b.get(i + 1) != Some(&b'"') {
                return None;
            }
            i += 2;
            let mut value = String::new();
            loop {
                match *b.get(i)? {
                    b'\\' if matches!(b.get(i + 1), Some(b'"' | b'\\' | b']')) => {
                        value.push(b[i + 1] as char);
                        i += 2;
                    }
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => {
                        let ch = s[i..].chars().next()?;
                        value.push(ch);
                        i += ch.len_utf8();
                    }
                }
            }
            if out.len() < 256 {
                out.push((format!("sd.{id}.{name}"), value));
            }
        }
    }
    Some((out, &s[i..]))
}

/// RFC 5424: `<PRI>1 TIMESTAMP HOST APP PROCID MSGID SD MSG`.
pub(super) fn rfc5424(line: &str) -> Option<Event> {
    let (pri, rest) = priority(line).ok()?;
    let rest = rest.strip_prefix('1')?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let (timestamp, rest) = first_word(rest);
    let (host, rest) = first_word(rest);
    let (app, rest) = first_word(rest);
    let (procid, rest) = first_word(rest);
    let (msgid, rest) = first_word(rest);
    if msgid.is_empty() {
        return None;
    }
    let remaining = rest.trim_start_matches([' ', '\t']);
    if remaining.len() == rest.len() && !rest.is_empty() {
        return None;
    }
    let (sd, message) = if let Some(after) = remaining.strip_prefix('-') {
        (Vec::new(), after)
    } else if remaining.starts_with('[') {
        match structured_data(remaining) {
            Some(found) => found,
            None => (Vec::new(), first_word(remaining).1),
        }
    } else {
        // Lenient legacy reading: the structured-data token is skipped.
        let (token, after) = first_word(remaining);
        if token.is_empty() || after.is_empty() {
            return None;
        }
        (Vec::new(), after)
    };
    let message = message.strip_prefix([' ', '\t']).unwrap_or(message).trim_start_matches('\u{feff}');
    let mut ev = Event::empty();
    ev.raw = line.to_string();
    ev.timestamp = super::parse_timestamp(timestamp);
    ev.source = host.to_string();
    ev.fields.insert("app".into(), Value::from(app));
    ev.fields.insert("pid".into(), Value::from(procid));
    ev.fields.insert("msgid".into(), Value::from(msgid));
    for (key, value) in sd {
        ev.fields.entry(key).or_insert(Value::from(value));
    }
    let explicit = apply_priority(&mut ev, pri);
    if !explicit {
        ev.level = crate::model::class_label(super::text_level_class(message.as_bytes())).to_string();
    }
    ev.message = message.to_string();
    embedded_formats(&mut ev, message);
    generic::enrich(&mut ev, message, explicit);
    Some(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn ctx() -> Context {
        Context { year: 2026, reference: NaiveDate::from_ymd_opt(2026, 10, 14), day_first: None }
    }

    #[test]
    fn bsd_headers_with_any_timestamp() {
        let ev = bsd("Oct 12 00:00:00 web01 sshd[1234]: Failed password for root from 192.0.2.7 port 22 ssh2", &ctx()).unwrap();
        assert_eq!((ev.source.as_str(), ev.fields["process"].as_str(), ev.fields["pid"].as_str()), ("web01", Some("sshd"), Some("1234")));
        assert!(ev.message.starts_with("Failed password"));
        let ev = bsd("2026-10-12T00:00:00.123456-03:00 web01 sudo[99]: alice : TTY=pts/0 ; PWD=/home/alice ; USER=root ; COMMAND=/bin/ls", &ctx()).unwrap();
        assert_eq!(ev.timestamp, chrono::DateTime::parse_from_rfc3339("2026-10-12T03:00:00.123Z").ok().map(|t| t.timestamp_millis()));
        assert_eq!((ev.fields["USER"].as_str(), ev.fields["COMMAND"].as_str()), (Some("root"), Some("/bin/ls")));
        let ev = bsd("<189>123: *Oct 12 00:00:00.123: %SYS-5-CONFIG_I: Configured from console by vty0", &ctx()).unwrap();
        assert_eq!((ev.code.as_str(), ev.level.as_str(), ev.fields["sequence"].as_str()), ("SYS-5-CONFIG_I", "Informação", Some("123")));
        let ev = bsd("<86>Oct 12 00:00:00 host kernel: [UFW BLOCK] IN=eth0 OUT= SRC=203.0.113.9 DST=10.0.0.2 PROTO=TCP SPT=4444 DPT=22", &ctx()).unwrap();
        assert_eq!((ev.level.as_str(), ev.fields["SRC"].as_str(), ev.fields["syslog.facility"].as_str()), ("Informação", Some("203.0.113.9"), Some("10")));
        let ev = bsd("Oct 12 00:00:00 fw CEF:0|Vendor|Product|1.0|100|Blocked|8|src=192.0.2.1 dst=10.0.0.1", &ctx());
        assert!(ev.is_none(), "CEF after a host has no tag: left to the CEF parser");
        assert!(bsd("2026-10-12 10:00:00 INFO something happened", &ctx()).is_none());
    }

    #[test]
    fn rfc5424_structured_data_and_priority() {
        let ev = rfc5424(r#"<165>1 2026-10-12T22:14:15.003Z mymachine.example.com evntslog - ID47 [exampleSDID@32473 iut="3" eventSource="Application" eventID="1011"][meta escaped="a\"b\]c"] An application event"#).unwrap();
        assert_eq!(ev.fields["sd.exampleSDID@32473.eventID"], "1011");
        assert_eq!(ev.fields["sd.meta.escaped"], "a\"b]c");
        assert_eq!((ev.message.as_str(), ev.level.as_str(), ev.source.as_str()), ("An application event", "Informação", "mymachine.example.com"));
        let ev = rfc5424("<34>1 2026-10-12T22:14:15.003Z host su - ID47 - 'su root' failed for lonvick on /dev/pts/8").unwrap();
        assert_eq!(ev.level, "Crítico");
        assert_eq!(rfc5424("1 - this is a simple saying hello").unwrap().fields["app"], "is");
    }
}
