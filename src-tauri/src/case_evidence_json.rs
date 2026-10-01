//! Borrowed JSON spans, after serde's non-materializing syntax validation.
//! This does not enable serde_json's global raw_value feature or redefine JSON
//! grammar. The scanner only locates child boundaries in already checked text.
use serde::de::IgnoredAny;

const INVALID: &str = "CASE_EVIDENCE_JSON: Envelope JSON inválido.";
const LIMIT: &str = "CASE_EVIDENCE_JSON_LIMIT: O envelope excede o limite de estrutura.";
const MAX_DEPTH: usize = 128;
fn whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}
fn skip(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && whitespace(bytes[at]) {
        at += 1;
    }
    at
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RawJson<'a> {
    text: &'a str,
}
impl<'a> RawJson<'a> {
    pub(crate) fn checked(text: &'a str) -> Result<Self, String> {
        crate::operations::check()?;
        if text.len() > super::ENVELOPE_BYTES {
            return Err(LIMIT.into());
        }
        Self::checked_with_limit(text, super::ENVELOPE_BYTES)
    }
    pub(super) fn checked_with_limit(text: &'a str, limit: usize) -> Result<Self, String> {
        if text.len() > limit {
            return Err(LIMIT.into());
        }
        serde_json::from_str::<IgnoredAny>(text).map_err(|e| format!("{INVALID} {e}"))?;
        let bytes = text.as_bytes();
        let start = skip(bytes, 0);
        let mut end = bytes.len();
        while end > start && whitespace(bytes[end - 1]) {
            end -= 1;
        }
        if start == end {
            return Err(INVALID.into());
        }
        Ok(Self {
            text: &text[start..end],
        })
    }
    pub(crate) fn get(&self) -> &'a str {
        self.text
    }
    pub(crate) fn kind(&self) -> u8 {
        self.text.as_bytes()[0]
    }
    pub(crate) fn elements(&self) -> Result<Elements<'a>, String> {
        if self.kind() != b'[' {
            return Err(INVALID.into());
        }
        Ok(Elements {
            raw: *self,
            at: 1,
            done: false,
        })
    }
    pub(crate) fn members(&self) -> Result<Members<'a>, String> {
        if self.kind() != b'{' {
            return Err(INVALID.into());
        }
        Ok(Members {
            raw: *self,
            at: 1,
            done: false,
        })
    }
}
pub(crate) struct RawMember<'a> {
    pub(crate) key: &'a str,
    pub(crate) value: RawJson<'a>,
}
impl RawMember<'_> {
    pub(crate) fn key(&self) -> Result<String, String> {
        serde_json::from_str(self.key).map_err(|e| e.to_string())
    }
}
pub(crate) struct Elements<'a> {
    raw: RawJson<'a>,
    at: usize,
    done: bool,
}
impl<'a> Elements<'a> {
    pub(crate) fn next(&mut self) -> Result<Option<RawJson<'a>>, String> {
        if self.done {
            return Ok(None);
        }
        let bytes = self.raw.text.as_bytes();
        self.at = skip(bytes, self.at);
        if bytes.get(self.at) == Some(&b']') {
            self.done = true;
            return Ok(None);
        }
        let start = self.at;
        let end = span_end(bytes, start)?;
        let value = RawJson {
            text: &self.raw.text[start..end],
        };
        self.at = skip(bytes, end);
        match bytes.get(self.at) {
            Some(b',') => self.at += 1,
            Some(b']') => self.done = true,
            _ => return Err(INVALID.into()),
        };
        Ok(Some(value))
    }
}
pub(crate) struct Members<'a> {
    raw: RawJson<'a>,
    at: usize,
    done: bool,
}
impl<'a> Members<'a> {
    pub(crate) fn next(&mut self) -> Result<Option<RawMember<'a>>, String> {
        if self.done {
            return Ok(None);
        }
        let bytes = self.raw.text.as_bytes();
        self.at = skip(bytes, self.at);
        if bytes.get(self.at) == Some(&b'}') {
            self.done = true;
            return Ok(None);
        }
        if bytes.get(self.at) != Some(&b'"') {
            return Err(INVALID.into());
        }
        let start = self.at;
        let end = string_end(bytes, start)?;
        let key = &self.raw.text[start..end];
        self.at = skip(bytes, end);
        if bytes.get(self.at) != Some(&b':') {
            return Err(INVALID.into());
        }
        self.at = skip(bytes, self.at + 1);
        let start = self.at;
        let end = span_end(bytes, start)?;
        let value = RawJson {
            text: &self.raw.text[start..end],
        };
        self.at = skip(bytes, end);
        match bytes.get(self.at) {
            Some(b',') => self.at += 1,
            Some(b'}') => self.done = true,
            _ => return Err(INVALID.into()),
        };
        Ok(Some(RawMember { key, value }))
    }
}
fn string_end(bytes: &[u8], at: usize) -> Result<usize, String> {
    let mut i = at + 1;
    let mut next_check = i;
    while let Some(&byte) = bytes.get(i) {
        if i >= next_check {
            crate::operations::check()?;
            next_check = i.saturating_add(65536);
        }
        match byte {
            b'"' => return Ok(i + 1),
            b'\\' => i = i.checked_add(2).ok_or(LIMIT)?,
            _ => i += 1,
        }
    }
    Err(INVALID.into())
}
fn span_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    match bytes.get(start) {
        Some(b'"') => string_end(bytes, start),
        Some(b'[' | b'{') => {
            let mut depth = 1usize;
            let mut i = start + 1;
            let mut next_check = i;
            while let Some(&byte) = bytes.get(i) {
                if i >= next_check {
                    crate::operations::check()?;
                    next_check = i.saturating_add(65536);
                }
                match byte {
                    b'"' => {
                        i = string_end(bytes, i)?;
                        continue;
                    }
                    b'[' | b'{' => {
                        depth += 1;
                        if depth > MAX_DEPTH {
                            return Err(LIMIT.into());
                        }
                    }
                    b']' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(i + 1);
                        }
                    }
                    _ => (),
                };
                i += 1;
            }
            Err(INVALID.into())
        }
        Some(_) => {
            let mut end = start;
            let mut next_check = end;
            while let Some(&byte) = bytes.get(end) {
                if end >= next_check {
                    crate::operations::check()?;
                    next_check = end.saturating_add(65536);
                }
                if whitespace(byte) || matches!(byte, b',' | b']' | b'}') {
                    break;
                }
                end += 1;
            }
            if end == start {
                Err(INVALID.into())
            } else {
                Ok(end)
            }
        }
        None => Err(INVALID.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_spans_keep_escaped_delimiters_unicode_and_number_lexemes() {
        let raw = RawJson::checked(
            r#" { "x": ["\\\"},[日", {"n":1.000,"z":-0.0}], "\u0079":18446744073709551615 } "#,
        )
        .unwrap();
        let mut members = raw.members().unwrap();
        let x = members.next().unwrap().unwrap();
        assert_eq!(x.key().unwrap(), "x");
        let mut rows = x.value.elements().unwrap();
        assert_eq!(
            serde_json::from_str::<String>(rows.next().unwrap().unwrap().get()).unwrap(),
            "\\\"},[日"
        );
        assert_eq!(
            rows.next().unwrap().unwrap().get(),
            r#"{"n":1.000,"z":-0.0}"#
        );
        assert!(rows.next().unwrap().is_none());
        let y = members.next().unwrap().unwrap();
        assert_eq!(y.key().unwrap(), "y");
        assert_eq!(y.value.get(), "18446744073709551615");
        assert!(members.next().unwrap().is_none());
    }
    #[test]
    fn syntax_is_checked_before_a_span_is_exposed() {
        for raw in [
            "{\"x\":1,}",
            "[1,]",
            "{}{}",
            "{\"x\":01}",
            "{\"x\":\"\\uZZZZ\"}",
        ] {
            assert!(RawJson::checked(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn private_token_is_a_literal_key_in_borrowed_native_spans() {
        let text = r#"{"$serde_json::private::RawValue":"1","extra":2}"#;
        let raw = RawJson::checked(text).unwrap();
        let mut fields = raw.members().unwrap();
        assert_eq!(
            fields.next().unwrap().unwrap().key().unwrap(),
            "$serde_json::private::RawValue"
        );
        let extra = fields.next().unwrap().unwrap();
        assert_eq!(extra.key().unwrap(), "extra");
        assert_eq!(extra.value.get(), "2");
        assert!(fields.next().unwrap().is_none());
    }
}
