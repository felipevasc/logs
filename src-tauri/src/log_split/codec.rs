//! The original file's bytes, unit by unit: line breaks and delimiters are
//! found in the file's own encoding, so outputs keep it byte for byte.
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// UTF-8, ASCII, or UTF-8 with stray legacy bytes (read as Windows-1252).
    Utf8,
    Legacy(&'static encoding_rs::Encoding),
    Cp850,
    Utf16Le,
    Utf16Be,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Codec {
    pub encoding: Encoding,
    /// Byte order mark at the start of the file, copied to every output.
    pub bom: usize,
    /// Line break unit: `\n`, or `\r` in files that only use carriage returns.
    pub newline: u8,
}

/// Compressed or archived files cannot be cut in place.
fn container(sample: &[u8]) -> Option<&'static str> {
    if sample.starts_with(&[0x1f, 0x8b]) {
        Some("gzip")
    } else if sample.starts_with(b"BZh") {
        Some("bzip2")
    } else if sample.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Some("xz")
    } else if sample.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some("zstd")
    } else if sample.starts_with(b"PK\x03\x04") {
        Some("zip")
    } else if sample.starts_with(b"ElfFile\0") {
        Some("evtx")
    } else {
        None
    }
}

impl Codec {
    pub(crate) fn sniff(sample: &[u8]) -> Result<Codec, String> {
        if let Some(kind) = container(sample) {
            return Err(format!("O arquivo está em formato {kind}. Descompacte-o antes de separar."));
        }
        use crate::workspace::TextEncoding;
        let (encoding, bom) = match crate::workspace::sniff_encoding(sample) {
            None => (Encoding::Utf8, if sample.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 }),
            Some(TextEncoding::Standard(e)) if e == encoding_rs::UTF_16LE => (Encoding::Utf16Le, if sample.starts_with(&[0xff, 0xfe]) { 2 } else { 0 }),
            Some(TextEncoding::Standard(e)) if e == encoding_rs::UTF_16BE => (Encoding::Utf16Be, if sample.starts_with(&[0xfe, 0xff]) { 2 } else { 0 }),
            Some(TextEncoding::Standard(e)) => (Encoding::Legacy(e), 0),
            Some(TextEncoding::Cp850) => (Encoding::Cp850, 0),
            Some(TextEncoding::Utf32Le | TextEncoding::Utf32Be) => {
                return Err("Arquivos em UTF-32 não podem ser separados. Converta-os para UTF-8 ou UTF-16.".into())
            }
        };
        let mut codec = Codec { encoding, bom, newline: b'\n' };
        // Classic Mac OS and some network devices end lines with CR alone.
        let body = codec.decode(&sample[bom..sample.len() - (sample.len() - bom) % codec.width()]);
        let lone_cr = body.as_bytes().windows(2).filter(|w| w[0] == b'\r' && w[1] != b'\n').count();
        if lone_cr >= 2 && lone_cr > body.bytes().filter(|&b| b == b'\n').count() * 10 {
            codec.newline = b'\r';
        }
        Ok(codec)
    }

    pub(crate) fn width(&self) -> usize {
        match self.encoding {
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
            _ => 1,
        }
    }

    pub(crate) fn name(&self) -> String {
        let name = match self.encoding {
            Encoding::Utf8 => "UTF-8",
            Encoding::Legacy(e) => e.name(),
            Encoding::Cp850 => "IBM850",
            Encoding::Utf16Le => "UTF-16LE",
            Encoding::Utf16Be => "UTF-16BE",
        };
        if self.bom > 0 { format!("{name} com BOM") } else { name.to_string() }
    }

    /// Start of the first line break unit at or after `from` (unit aligned).
    pub(crate) fn find_newline(&self, buf: &[u8], from: usize) -> Option<usize> {
        let nl = self.newline;
        match self.encoding {
            Encoding::Utf16Le => {
                let mut at = from;
                while let Some(p) = memchr::memchr(nl, buf.get(at..)?) {
                    let p = at + p;
                    if (p - from) % 2 == 0 && buf.get(p + 1) == Some(&0) {
                        return Some(p);
                    }
                    at = p + 1;
                }
                None
            }
            Encoding::Utf16Be => {
                let mut at = from + 1;
                while let Some(p) = memchr::memchr(nl, buf.get(at..)?) {
                    let p = at + p;
                    if (p - from) % 2 == 1 && buf[p - 1] == 0 {
                        return Some(p - 1);
                    }
                    at = p + 1;
                }
                None
            }
            _ => memchr::memchr(nl, buf.get(from..)?).map(|p| from + p),
        }
    }

    /// Line break units in `bytes` (which starts unit aligned).
    pub(crate) fn count_newlines(&self, bytes: &[u8]) -> u64 {
        if self.width() == 1 {
            return memchr::memchr_iter(self.newline, bytes).count() as u64;
        }
        let mut count = 0;
        let mut at = 0;
        while let Some(p) = self.find_newline(bytes, at) {
            count += 1;
            at = p + 2;
        }
        count
    }

    /// Text of some bytes of the file (starting unit aligned).
    pub(crate) fn decode<'a>(&self, bytes: &'a [u8]) -> Cow<'a, str> {
        match self.encoding {
            Encoding::Utf8 => crate::sources::decode_text(bytes),
            Encoding::Legacy(e) => e.decode_without_bom_handling(bytes).0,
            Encoding::Utf16Le => encoding_rs::UTF_16LE.decode_without_bom_handling(bytes).0,
            Encoding::Utf16Be => encoding_rs::UTF_16BE.decode_without_bom_handling(bytes).0,
            Encoding::Cp850 => {
                if bytes.is_ascii() {
                    return Cow::Borrowed(std::str::from_utf8(bytes).unwrap_or_default());
                }
                Cow::Owned(bytes.iter().map(|&b| if b < 0x80 { b as char } else { crate::workspace::CP850_HIGH[usize::from(b - 0x80)] }).collect())
            }
        }
    }

    /// Text written in the file's encoding (delimiters typed by the user).
    pub(crate) fn encode(&self, text: &str) -> Result<Vec<u8>, String> {
        let text = if self.newline == b'\r' { Cow::Owned(text.replace('\n', "\r")) } else { Cow::Borrowed(text) };
        Ok(match self.encoding {
            Encoding::Utf8 => text.as_bytes().to_vec(),
            Encoding::Legacy(e) => {
                let (bytes, _, unmappable) = e.encode(&text);
                if unmappable {
                    return Err(format!("O separador tem caracteres que não existem na codificação {}.", e.name()));
                }
                bytes.into_owned()
            }
            Encoding::Cp850 => text
                .chars()
                .map(|c| {
                    if c.is_ascii() {
                        Ok(c as u8)
                    } else {
                        crate::workspace::CP850_HIGH
                            .iter()
                            .position(|&h| h == c)
                            .map(|p| 0x80 + p as u8)
                            .ok_or_else(|| "O separador tem caracteres que não existem na codificação IBM850.".to_string())
                    }
                })
                .collect::<Result<_, _>>()?,
            Encoding::Utf16Le => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            Encoding::Utf16Be => text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        })
    }

    /// Rounds a length down to whole units.
    pub(crate) fn align(&self, n: usize) -> usize {
        n - n % self.width()
    }
}

/// Text of a line without its line break.
pub(crate) fn trim_break(text: &str) -> &str {
    text.trim_end_matches(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_line_breaks_are_unit_aligned() {
        let text = "a\u{0a0a}b\nc\n";
        let le: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        let codec = Codec::sniff(&le).unwrap();
        assert_eq!(codec.encoding, Encoding::Utf16Le);
        assert_eq!(codec.bom, 2);
        // U+0A0A has 0x0a bytes that are not a line break.
        assert_eq!(codec.find_newline(&le, 2), Some(2 + 6));
        assert_eq!(codec.count_newlines(&le[2..]), 2);
        assert_eq!(codec.decode(&le[2..8]), "a\u{0a0a}b");
        let be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let codec = Codec { encoding: Encoding::Utf16Be, bom: 0, newline: b'\n' };
        assert_eq!(codec.find_newline(&be, 0), Some(6));
        assert_eq!(codec.encode("\n").unwrap(), vec![0, 0x0a]);
    }

    #[test]
    fn compressed_and_cr_only_files_are_recognized() {
        assert!(Codec::sniff(&[0x1f, 0x8b, 8, 0]).unwrap_err().contains("gzip"));
        let codec = Codec::sniff(b"one\rtwo\rthree\rfour\r").unwrap();
        assert_eq!(codec.newline, b'\r');
        assert_eq!(codec.encode("\n\n").unwrap(), b"\r\r");
        let latin = Codec::sniff(b"a\xe7\xe3o fun\xe7\xe3o\nlinha\n").unwrap();
        assert!(matches!(latin.encoding, Encoding::Legacy(_)));
        assert_eq!(latin.decode(b"fun\xe7\xe3o"), "função");
    }
}
