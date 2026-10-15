//! Date and time recognition shared by every text parser: anywhere in a line
//! (leftmost wins) or as a whole field value.
//!
//! - ISO 8601 / RFC 3339 and variants: `2026-10-12T00:00:00.123Z`,
//!   `2026/10/12 00:00:00,123 -03:00`, `2026.10.12 00:00:00`, `20261012T000000Z`,
//!   `2026-10-12 00:00:00:123` (Windows), Go's `+0000 UTC`, Java's `[Region/City]`
//! - Common Log Format and named months day first: `12/Oct/2026:00:00:00 -0300`,
//!   `12-Oct-2026 00:00:00.123`, `12 Oct 2026 00:00:00,123`, `Mon, 12 Oct 2026 00:00:00 GMT`
//! - Month first: syslog `Oct 12 00:00:00`, Cisco `Oct 12 2026 00:00:00`, Java
//!   `Oct 12, 2026 12:00:00 AM`, ctime `Mon Oct 12 00:00:00.123456 2026`, `date`
//!   output `Mon Oct 12 00:00:00 UTC 2026`
//! - Numeric dates: `12/10/2026 00:00:00`, `10/12/2026 12:00:00 AM`,
//!   `12.10.2026 00:00:00`, DHCP `10/12/26,00:00:00`, Snort `10/12/2026-00:00:00.123456`
//!   and `10/12-00:00:00.123456`, Android `10-12 00:00:00.123`
//! - Epoch seconds or milliseconds at the start of a line
//! - Time only at the start of a line, dated by the source's reference date
//!
//! Month and weekday names in English, Portuguese, Spanish, French, German and
//! Italian. Zones: `Z`, numeric offsets, unambiguous abbreviations (UTC, BRT,
//! EST, CEST...) and common IANA names (with European/US daylight saving).
//! Without a zone the machine's local time applies, as everywhere else.
use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};

/// Bytes of a line searched for a timestamp.
const SCAN_LIMIT: usize = 400;
/// A date without a time only counts near the start of a line.
const DATE_ONLY_LIMIT: usize = 64;

/// What completes partial values: the year for year-less dates and the date for
/// time-only values. Fixed per source so results never depend on the clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub year: i32,
    /// Date the source refers to (from its name or modification time). A
    /// year-less date later than it belongs to the previous year.
    pub reference: Option<NaiveDate>,
    /// Order of ambiguous numeric dates seen in the source (true = day first).
    pub day_first: Option<bool>,
}

impl Context {
    pub(crate) fn year(year: i32) -> Self {
        Self { year, reference: None, day_first: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Inferred {
    Nothing,
    Year,
    Date,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Found {
    pub ms: i64,
    pub start: usize,
    pub end: usize,
    pub inferred: Inferred,
    pub has_time: bool,
    /// The text carried its zone (or is an epoch): `ms` is absolute. Without
    /// one, `ms` reads the wall-clock time in the machine's local zone.
    pub zoned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Fixed(i32),
    /// European Union daylight saving from a standard offset.
    Eu(i32),
    /// United States daylight saving from a standard offset.
    Us(i32),
    /// A recognized name without a known offset: local time.
    Local,
}

#[derive(Clone, Copy, Debug)]
struct Raw {
    /// (year, month, day); no year for syslog-style values; None = time only.
    date: Option<(Option<i32>, u32, u32)>,
    time: Option<(u32, u32, u32, u32)>,
    zone: Option<Zone>,
    end: usize,
    /// Numeric date whose first two parts could be swapped: (first, second).
    ambiguous: Option<(u32, u32)>,
}

struct Cur<'a> {
    s: &'a [u8],
    i: usize,
}

fn digit(b: Option<u8>) -> bool {
    b.is_some_and(|b| b.is_ascii_digit())
}

fn alnum(b: Option<u8>) -> bool {
    b.is_some_and(|b| b.is_ascii_alphanumeric())
}

impl<'a> Cur<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn at(&self, k: usize) -> Option<u8> {
        self.s.get(self.i + k).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// `min..=max` digits not followed by another digit; returns (value, count).
    fn num(&mut self, min: usize, max: usize) -> Option<(u32, usize)> {
        let start = self.i;
        let mut value = 0u32;
        while self.i - start < max && digit(self.peek()) {
            value = value.saturating_mul(10).saturating_add(u32::from(self.s[self.i] - b'0'));
            self.i += 1;
        }
        let count = self.i - start;
        if count < min || digit(self.peek()) {
            self.i = start;
            return None;
        }
        Some((value, count))
    }
    /// Exactly `n` digits, whatever follows (compact forms).
    fn fixed(&mut self, n: usize) -> Option<u32> {
        let start = self.i;
        let mut value = 0u32;
        for _ in 0..n {
            if !digit(self.peek()) {
                self.i = start;
                return None;
            }
            value = value * 10 + u32::from(self.s[self.i] - b'0');
            self.i += 1;
        }
        Some(value)
    }
    fn spaces(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
        self.i - start
    }
    /// Letters, including accented ones (whole UTF-8 sequences).
    fn word(&mut self) -> &'a [u8] {
        let start = self.i;
        while self.peek().is_some_and(|b| b.is_ascii_alphabetic() || b >= 0x80) && self.i - start < 24 {
            self.i += 1;
        }
        // Never stop inside a UTF-8 sequence.
        while self.peek().is_some_and(|b| (0x80..0xC0).contains(&b)) {
            self.i += 1;
        }
        &self.s[start..self.i]
    }
}

/// Lowercase form of a short word without allocating for ASCII words.
fn with_lower<R>(word: &[u8], f: impl FnOnce(&str) -> R) -> R {
    let mut buffer = [0u8; 24];
    if word.is_ascii() && word.len() <= buffer.len() {
        for (slot, b) in buffer.iter_mut().zip(word) {
            *slot = b.to_ascii_lowercase();
        }
        f(std::str::from_utf8(&buffer[..word.len()]).unwrap_or(""))
    } else {
        f(&String::from_utf8_lossy(word).to_lowercase())
    }
}

fn month_of(word: &[u8]) -> Option<u32> {
    if word.len() < 3 || word.len() > 20 {
        return None;
    }
    with_lower(word, |name| Some(match name {
        "jan" | "january" | "janeiro" | "ene" | "enero" | "janv" | "janvier" | "januar" | "gen" | "gennaio" => 1,
        "feb" | "february" | "fev" | "fevereiro" | "febrero" | "févr" | "fevr" | "février" | "fevrier" | "februar"
        | "febbraio" => 2,
        "mar" | "march" | "março" | "marco" | "marzo" | "mars" | "märz" | "marz" | "mrz" | "mär" => 3,
        "apr" | "april" | "abr" | "abril" | "avr" | "avril" | "aprile" => 4,
        "may" | "mai" | "maio" | "mayo" | "mag" | "maggio" => 5,
        "jun" | "june" | "junho" | "junio" | "juin" | "juni" | "giu" | "giugno" => 6,
        "jul" | "july" | "julho" | "julio" | "juil" | "juillet" | "juli" | "lug" | "luglio" => 7,
        "aug" | "august" | "ago" | "agosto" | "août" | "aout" => 8,
        "sep" | "sept" | "september" | "set" | "setembro" | "septiembre" | "setiembre" | "septembre" | "settembre" => 9,
        "oct" | "october" | "out" | "outubro" | "octubre" | "octobre" | "okt" | "oktober" | "ott" | "ottobre" => 10,
        "nov" | "november" | "novembro" | "noviembre" | "novembre" => 11,
        "dec" | "december" | "dez" | "dezembro" | "dic" | "diciembre" | "déc" | "décembre" | "decembre" | "dezember"
        | "dicembre" => 12,
        _ => return None,
    }))
}

fn weekday(word: &[u8]) -> bool {
    if word.len() < 3 || word.len() > 20 {
        return false;
    }
    with_lower(word, |name| matches!(
        name,
        "mon" | "monday" | "tue" | "tues" | "tuesday" | "wed" | "wednesday" | "thu" | "thur" | "thurs" | "thursday"
            | "fri" | "friday" | "sat" | "saturday" | "sun" | "sunday" | "seg" | "segunda" | "ter" | "terça" | "terca"
            | "qua" | "quarta" | "qui" | "quinta" | "sex" | "sexta" | "sáb" | "sab" | "sábado" | "sabado" | "dom"
            | "domingo" | "lun" | "lunes" | "martes" | "mié" | "mie" | "miércoles" | "miercoles" | "jue" | "jueves"
            | "vie" | "viernes" | "lundi" | "mardi" | "mer" | "mercredi" | "jeu" | "jeudi" | "ven" | "vendredi" | "sam"
            | "samedi" | "dim" | "dimanche" | "montag" | "dienstag" | "mittwoch" | "donnerstag" | "freitag" | "samstag"
            | "sonntag"
    ))
}

/// Digits after a decimal separator as nanoseconds; extra digits are ignored.
fn fraction(c: &mut Cur) -> u32 {
    let mut nanos = 0u32;
    let mut count = 0;
    while digit(c.peek()) {
        if count < 9 {
            nanos = nanos * 10 + u32::from(c.s[c.i] - b'0');
            count += 1;
        }
        c.i += 1;
    }
    nanos * 10u32.pow(9 - count)
}

/// `AM`/`PM` (or `a.m.`/`p.m.`): Some(true) for PM.
fn meridiem(c: &mut Cur) -> Option<bool> {
    let start = c.i;
    let first = c.peek()?.to_ascii_lowercase();
    if first != b'a' && first != b'p' {
        return None;
    }
    c.i += 1;
    let dotted = c.eat(b'.');
    if !c.peek().is_some_and(|b| b.eq_ignore_ascii_case(&b'm')) {
        c.i = start;
        return None;
    }
    c.i += 1;
    if dotted {
        c.eat(b'.');
    }
    if c.peek().is_some_and(|b| b.is_ascii_alphanumeric()) {
        c.i = start;
        return None;
    }
    Some(first == b'p')
}

/// `H:MM[:SS[.frac]]` with an optional AM/PM. `seconds` requires them.
fn time(c: &mut Cur, seconds: bool) -> Option<(u32, u32, u32, u32)> {
    let start = c.i;
    let fail = |c: &mut Cur| {
        c.i = start;
        None
    };
    let Some((mut hour, _)) = c.num(1, 2) else { return fail(c) };
    if !c.eat(b':') {
        return fail(c);
    }
    let Some(minute) = c.fixed(2) else { return fail(c) };
    if digit(c.peek()) {
        return fail(c);
    }
    let (mut second, mut nanos, mut has_seconds) = (0, 0, false);
    if c.peek() == Some(b':') && digit(c.at(1)) && digit(c.at(2)) && !digit(c.at(3)) {
        c.i += 1;
        second = c.fixed(2)?;
        has_seconds = true;
        match c.peek() {
            Some(b'.' | b',') if digit(c.at(1)) => {
                c.i += 1;
                nanos = fraction(c);
            }
            // Windows Update and a few others: `HH:MM:SS:mmm`.
            Some(b':') if digit(c.at(1)) && digit(c.at(2)) && digit(c.at(3)) && !digit(c.at(4)) => {
                c.i += 1;
                nanos = c.fixed(3)? * 1_000_000;
            }
            _ => {}
        }
    }
    if (seconds && !has_seconds) || hour > 23 || minute > 59 || second > 60 {
        return fail(c);
    }
    let before = c.i;
    if c.spaces() <= 1 {
        match meridiem(c) {
            Some(pm) if (1..=12).contains(&hour) => {
                hour = match (pm, hour) {
                    (false, 12) => 0,
                    (true, 12) => 12,
                    (true, h) => h + 12,
                    (false, h) => h,
                };
            }
            _ => c.i = before,
        }
    } else {
        c.i = before;
    }
    Some((hour, minute, second.min(59), nanos))
}

const ABBREVIATIONS: &[(&str, i32)] = &[
    ("UTC", 0), ("GMT", 0), ("UT", 0), ("Z", 0), ("WET", 0), ("WEST", 60), ("BST", 60), ("CET", 60), ("MET", 60),
    ("CEST", 120), ("MEST", 120), ("EET", 120), ("EEST", 180), ("MSK", 180), ("SAST", 120), ("WAT", 60), ("CAT", 120),
    ("EAT", 180), ("BRT", -180), ("BRST", -120), ("ART", -180), ("UYT", -180), ("CLT", -240), ("CLST", -180),
    ("COT", -300), ("PET", -300), ("VET", -240), ("EST", -300), ("EDT", -240), ("CDT", -300), ("MST", -420),
    ("MDT", -360), ("PST", -480), ("PDT", -420), ("AKST", -540), ("AKDT", -480), ("HST", -600), ("JST", 540),
    ("KST", 540), ("HKT", 480), ("SGT", 480), ("PHT", 480), ("WIB", 420), ("ICT", 420), ("AWST", 480),
    ("ACST", 570), ("ACDT", 630), ("AEST", 600), ("AEDT", 660), ("NZST", 720), ("NZDT", 780),
];

/// Common IANA names. Zones without daylight saving use a fixed offset; the
/// European and US ones follow their current rules. Others read as local time.
fn iana(name: &str) -> Option<Zone> {
    const FIXED: &[(&str, i32)] = &[
        ("America/Sao_Paulo", -180), ("America/Bahia", -180), ("America/Fortaleza", -180), ("America/Recife", -180),
        ("America/Belem", -180), ("America/Maceio", -180), ("America/Araguaina", -180), ("America/Santarem", -180),
        ("America/Manaus", -240), ("America/Cuiaba", -240), ("America/Campo_Grande", -240),
        ("America/Porto_Velho", -240), ("America/Boa_Vista", -240), ("America/Rio_Branco", -300),
        ("America/Noronha", -120), ("Brazil/East", -180), ("America/Argentina/Buenos_Aires", -180),
        ("America/Buenos_Aires", -180), ("America/Montevideo", -180), ("America/Bogota", -300),
        ("America/Lima", -300), ("America/Caracas", -240), ("America/La_Paz", -240), ("America/Mexico_City", -360),
        ("America/Phoenix", -420), ("Asia/Tokyo", 540), ("Asia/Seoul", 540), ("Asia/Shanghai", 480),
        ("Asia/Hong_Kong", 480), ("Asia/Singapore", 480), ("Asia/Taipei", 480), ("Asia/Manila", 480),
        ("Asia/Kolkata", 330), ("Asia/Calcutta", 330), ("Asia/Dubai", 240), ("Asia/Jakarta", 420),
        ("Asia/Bangkok", 420), ("Asia/Ho_Chi_Minh", 420), ("Asia/Karachi", 300), ("Asia/Riyadh", 180),
        ("Africa/Johannesburg", 120), ("Africa/Lagos", 60), ("Africa/Nairobi", 180), ("Africa/Cairo", 120),
        ("Europe/Moscow", 180), ("Europe/Istanbul", 180), ("Europe/Minsk", 180), ("Australia/Brisbane", 600),
        ("Australia/Perth", 480), ("UTC", 0), ("Etc/UTC", 0), ("Etc/GMT", 0), ("Etc/Universal", 0), ("GMT", 0),
        ("Universal", 0), ("Zulu", 0),
    ];
    const EU: &[(&str, i32)] = &[
        ("Europe/London", 0), ("Europe/Dublin", 0), ("Europe/Lisbon", 0), ("Atlantic/Canary", 0),
        ("Atlantic/Madeira", 0), ("Atlantic/Azores", -60), ("Europe/Paris", 60), ("Europe/Berlin", 60),
        ("Europe/Madrid", 60), ("Europe/Rome", 60), ("Europe/Amsterdam", 60), ("Europe/Brussels", 60),
        ("Europe/Vienna", 60), ("Europe/Zurich", 60), ("Europe/Stockholm", 60), ("Europe/Oslo", 60),
        ("Europe/Copenhagen", 60), ("Europe/Prague", 60), ("Europe/Warsaw", 60), ("Europe/Budapest", 60),
        ("Europe/Luxembourg", 60), ("Europe/Belgrade", 60), ("Europe/Zagreb", 60), ("Europe/Athens", 120),
        ("Europe/Helsinki", 120), ("Europe/Kiev", 120), ("Europe/Kyiv", 120), ("Europe/Bucharest", 120),
        ("Europe/Sofia", 120), ("Europe/Riga", 120), ("Europe/Vilnius", 120), ("Europe/Tallinn", 120),
    ];
    const US: &[(&str, i32)] = &[
        ("America/New_York", -300), ("America/Toronto", -300), ("America/Detroit", -300), ("US/Eastern", -300),
        ("America/Chicago", -360), ("America/Winnipeg", -360), ("US/Central", -360), ("America/Denver", -420),
        ("America/Edmonton", -420), ("US/Mountain", -420), ("America/Los_Angeles", -480),
        ("America/Vancouver", -480), ("US/Pacific", -480), ("America/Anchorage", -540),
    ];
    let find = |table: &[(&str, i32)]| table.iter().find(|(zone, _)| zone.eq_ignore_ascii_case(name)).map(|(_, m)| *m * 60);
    find(FIXED)
        .map(Zone::Fixed)
        .or_else(|| find(EU).map(Zone::Eu))
        .or_else(|| find(US).map(Zone::Us))
        .or_else(|| {
            let area = name.split('/').next().unwrap_or("");
            const AREAS: &[&str] = &[
                "Africa", "America", "Antarctica", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific",
                "Etc", "US", "Brazil", "Canada", "Mexico", "Chile",
            ];
            (name.contains('/') && AREAS.iter().any(|a| a.eq_ignore_ascii_case(area))).then_some(Zone::Local)
        })
}

/// `+HH`, `+HHMM`, `+HH:MM` (also `-`), not followed by another digit.
fn numeric_offset(c: &mut Cur) -> Option<i32> {
    let start = c.i;
    let sign = match c.peek()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    c.i += 1;
    let Some(hours) = c.fixed(2) else {
        c.i = start;
        return None;
    };
    let mut minutes = 0;
    if c.peek() == Some(b':') && digit(c.at(1)) {
        c.i += 1;
        minutes = c.fixed(2).unwrap_or(99);
    } else if digit(c.peek()) {
        minutes = c.fixed(2).unwrap_or(99);
    }
    if digit(c.peek()) || hours > 14 || minutes > 59 {
        c.i = start;
        return None;
    }
    Some(sign * (hours as i32 * 3600 + minutes as i32 * 60))
}

/// A zone right after a time: at most one space before it.
fn zone(c: &mut Cur) -> Option<Zone> {
    let start = c.i;
    if c.spaces() > 1 {
        c.i = start;
        return None;
    }
    let zone = match c.peek() {
        Some(b'Z' | b'z') if !alnum(c.at(1)) => {
            c.i += 1;
            Some(Zone::Fixed(0))
        }
        Some(b'+' | b'-') => numeric_offset(c).map(Zone::Fixed),
        Some(b) if b.is_ascii_alphabetic() => {
            let word_start = c.i;
            while c.peek().is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+')) {
                c.i += 1;
            }
            let word = std::str::from_utf8(&c.s[word_start..c.i]).unwrap_or("");
            // "UTC+3", "GMT-03:00": the offset may follow the name directly.
            let base = word.split(['+', '-']).next().unwrap_or(word);
            if word.contains('/') {
                let found = iana(word.trim_end_matches(['-', '+']));
                if found.is_none() {
                    c.i = start;
                }
                found
            } else if ["UTC", "GMT", "UT"].contains(&base) {
                c.i = word_start + base.len();
                Some(Zone::Fixed(numeric_offset(c).unwrap_or(0)))
            } else {
                c.i = word_start + base.len();
                let fixed = (base.bytes().all(|b| b.is_ascii_uppercase()) && !alnum(c.peek()))
                    .then(|| ABBREVIATIONS.iter().find(|(name, _)| *name == base).map(|(_, m)| Zone::Fixed(m * 60)))
                    .flatten();
                if fixed.is_none() {
                    c.i = start;
                }
                fixed
            }
        }
        _ => None,
    };
    let Some(zone) = zone else {
        c.i = start;
        return None;
    };
    // Go's "+0000 UTC" and Java's "+01:00[Europe/Paris]" carry a second name.
    let after = c.i;
    if c.eat(b'[') {
        let name_start = c.i;
        while c.peek().is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+')) {
            c.i += 1;
        }
        let name = std::str::from_utf8(&c.s[name_start..c.i]).unwrap_or("");
        if !c.eat(b']') || name.is_empty() {
            c.i = after;
        }
    } else if c.spaces() == 1 && c.peek().is_some_and(|b| b.is_ascii_uppercase()) {
        let name_start = c.i;
        while c.peek().is_some_and(|b| b.is_ascii_uppercase()) {
            c.i += 1;
        }
        let name = std::str::from_utf8(&c.s[name_start..c.i]).unwrap_or("");
        if alnum(c.peek()) || !(["UTC", "GMT"].contains(&name) || ABBREVIATIONS.iter().any(|(n, _)| *n == name)) {
            c.i = after;
        }
    } else {
        c.i = after;
    }
    Some(zone)
}

/// Between a date and its time: `T`, spaces, `_`, `,`, `-` or `:`.
fn date_time_separator(c: &mut Cur, allow_space: bool, allow_colon: bool) -> bool {
    let start = c.i;
    let ok = match c.peek() {
        Some(b'T' | b't' | b'_') => {
            c.i += 1;
            true
        }
        Some(b' ' | b'\t') if allow_space => c.spaces() <= 3,
        Some(b',') => {
            c.i += 1;
            c.spaces();
            true
        }
        Some(b'-') => {
            c.i += 1;
            true
        }
        Some(b':') if allow_colon => {
            c.i += 1;
            true
        }
        _ => false,
    };
    if !ok || !digit(c.peek()) {
        c.i = start;
        return false;
    }
    true
}

fn plausible_year(year: u32) -> bool {
    (1970..=2100).contains(&year)
}

/// `YYYY-MM-DD`, `YYYY/MM/DD`, `YYYY.MM.DD` or compact `YYYYMMDD[T]HHMMSS`.
fn iso(c: &mut Cur) -> Option<Raw> {
    let start = c.i;
    let year = c.fixed(4)?;
    if !plausible_year(year) {
        c.i = start;
        return None;
    }
    let (month, day, compact) = match c.peek() {
        Some(sep @ (b'-' | b'/' | b'.')) => {
            c.i += 1;
            let month = c.num(1, 2).map(|(v, _)| v);
            let day = month.filter(|_| c.eat(sep)).and_then(|_| c.num(1, 2)).map(|(v, _)| v);
            match (month, day) {
                (Some(m), Some(d)) => (m, d, false),
                _ => {
                    c.i = start;
                    return None;
                }
            }
        }
        Some(b) if b.is_ascii_digit() => {
            let (Some(m), Some(d)) = (c.fixed(2), c.fixed(2)) else {
                c.i = start;
                return None;
            };
            (m, d, true)
        }
        _ => {
            c.i = start;
            return None;
        }
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        c.i = start;
        return None;
    }
    let date = Some((Some(year as i32), month, day));
    if compact {
        // Eight digits alone are any number: a compact date needs its time.
        if !(c.eat(b'T') || c.eat(b't') || (c.spaces() == 1)) {
            c.i = start;
            return None;
        }
        let time = if digit(c.peek()) && digit(c.at(1)) && digit(c.at(2)) && digit(c.at(3)) && digit(c.at(4)) && digit(c.at(5)) {
            let (h, m, s) = (c.fixed(2)?, c.fixed(2)?, c.fixed(2)?);
            let nanos = if matches!(c.peek(), Some(b'.' | b',')) && digit(c.at(1)) {
                c.i += 1;
                fraction(c)
            } else {
                0
            };
            (h < 24 && m < 60 && s <= 60 && !digit(c.peek())).then_some((h, m, s.min(59), nanos))
        } else {
            time(c, true)
        };
        let Some(time) = time else {
            c.i = start;
            return None;
        };
        let zone = zone(c);
        return Some(Raw { date, time: Some(time), zone, end: c.i, ambiguous: None });
    }
    let date_end = c.i;
    if alnum(c.peek()) && !matches!(c.peek(), Some(b'T' | b't')) {
        c.i = start;
        return None;
    }
    if date_time_separator(c, true, true) {
        if let Some(time) = time(c, false) {
            let zone = zone(c);
            return Some(Raw { date, time: Some(time), zone, end: c.i, ambiguous: None });
        }
    }
    c.i = date_end;
    Some(Raw { date, time: None, zone: None, end: date_end, ambiguous: None })
}

/// Numeric day/month in either order with an optional 2- or 4-digit year.
fn numeric(c: &mut Cur, ctx: &Context) -> Option<Raw> {
    let start = c.i;
    let fail = |c: &mut Cur| {
        c.i = start;
        None
    };
    let Some((a, _)) = c.num(1, 2) else { return fail(c) };
    let Some(sep) = c.peek().filter(|b| matches!(b, b'/' | b'-' | b'.')) else { return fail(c) };
    c.i += 1;
    let Some((b, _)) = c.num(1, 2) else { return fail(c) };
    let mut year = None;
    let mut short_year = false;
    if c.peek() == Some(sep) && digit(c.at(1)) {
        c.i += 1;
        match c.num(2, 4) {
            Some((y, 4)) if plausible_year(y) => year = Some(y as i32),
            Some((y, 2)) => {
                year = Some(if y < 70 { 2000 + y as i32 } else { 1900 + y as i32 });
                short_year = true;
            }
            _ => return fail(c),
        }
    }
    if a == 0 || b == 0 || (a > 12 && b > 12) || a > 31 || b > 31 {
        return fail(c);
    }
    let date_end = c.i;
    let mut comma = false;
    let mut time_value = None;
    if c.peek() != Some(sep) || year.is_some() {
        comma = c.peek() == Some(b',');
        // A date with no year needs a time ("10/12-00:00:00", "10-12 00:00:00").
        if date_time_separator(c, true, year.is_some()) {
            time_value = time(c, year.is_none());
        }
    }
    if time_value.is_none() {
        comma = false;
        c.i = date_end;
        if year.is_none() || alnum(c.peek()) {
            return fail(c);
        }
    }
    let pm_marker = time_value.is_some() && {
        let text = &c.s[date_end..c.i];
        text.windows(2).any(|w| w.eq_ignore_ascii_case(b"am") || w.eq_ignore_ascii_case(b"pm"))
    };
    // Snort/Suricata fast.log: "10/12/2026-00:00:00.123456" is month first.
    let snort = time_value.is_some() && sep == b'/' && c.s.get(date_end) == Some(&b'-');
    // Explicit evidence first; then what the source showed; then the format's
    // own convention; finally day first, as in Brazil and most of the world.
    let day_first = if a > 12 {
        true
    } else if b > 12 {
        false
    } else if year.is_none() {
        false
    } else if let Some(order) = ctx.day_first {
        order
    } else if pm_marker || snort || (short_year && comma && sep == b'/') {
        false
    } else {
        true
    };
    let (day, month) = if day_first { (a, b) } else { (b, a) };
    if month > 12 {
        return fail(c);
    }
    let zone = if time_value.is_some() { zone(c) } else { None };
    Some(Raw {
        date: Some((year, month, day)),
        time: time_value,
        zone,
        end: c.i,
        ambiguous: (a <= 31 && b <= 31).then_some((a, b)),
    })
}

/// `DD/Mon/YYYY[:HH:MM:SS]`, `DD-Mon-YYYY HH:MM:SS`, `DD Mon YYYY HH:MM:SS`.
fn day_month_name(c: &mut Cur) -> Option<Raw> {
    let start = c.i;
    let fail = |c: &mut Cur| {
        c.i = start;
        None
    };
    let Some((day, _)) = c.num(1, 2) else { return fail(c) };
    let sep = match c.peek() {
        Some(b'/' | b'-' | b'.') => {
            let sep = c.peek();
            c.i += 1;
            sep
        }
        Some(b' ') => {
            c.spaces();
            None
        }
        _ => return fail(c),
    };
    let Some(month) = month_of(c.word()) else { return fail(c) };
    c.eat(b'.');
    let separated = match sep {
        Some(sep) => c.eat(sep),
        None => c.spaces() >= 1,
    };
    if !separated || !(1..=31).contains(&day) {
        return fail(c);
    }
    let year = match c.num(2, 4) {
        Some((y, 4)) if plausible_year(y) => y as i32,
        Some((y, 2)) if sep == Some(b'-') => 2000 + y as i32,
        _ => return fail(c),
    };
    let date = Some((Some(year), month, day));
    let date_end = c.i;
    if date_time_separator(c, true, true) {
        if let Some(time) = time(c, false) {
            let zone = zone(c);
            return Some(Raw { date, time: Some(time), zone, end: c.i, ambiguous: None });
        }
    }
    c.i = date_end;
    if alnum(c.peek()) {
        return fail(c);
    }
    Some(Raw { date, time: None, zone: None, end: date_end, ambiguous: None })
}

/// `Mon DD [YYYY] HH:MM:SS`, `Mon DD, YYYY HH:MM:SS AM`, ctime `Mon DD HH:MM:SS [zone] YYYY`.
fn month_name_first(c: &mut Cur, month: u32) -> Option<Raw> {
    let start = c.i;
    let fail = |c: &mut Cur| {
        c.i = start;
        None
    };
    c.eat(b'.');
    if c.spaces() == 0 {
        return fail(c);
    }
    let Some((day, _)) = c.num(1, 2) else { return fail(c) };
    if !(1..=31).contains(&day) {
        return fail(c);
    }
    for suffix in [&b"st"[..], b"nd", b"rd", b"th"] {
        if c.s[c.i..].len() >= 2 && c.s[c.i..c.i + 2].eq_ignore_ascii_case(suffix) && !alnum(c.at(2)) {
            c.i += 2;
        }
    }
    c.eat(b',');
    let after_day = c.i;
    if c.spaces() == 0 {
        return fail(c);
    }
    let mut year = None;
    let mut after_year = c.i;
    if digit(c.peek()) && digit(c.at(1)) && digit(c.at(2)) && digit(c.at(3)) && !digit(c.at(4)) && c.at(4) != Some(b':') {
        let y = c.fixed(4)?;
        if !plausible_year(y) {
            return fail(c);
        }
        year = Some(y as i32);
        after_year = c.i;
        c.eat(b',');
        if c.spaces() == 0 && c.peek() != Some(b':') {
            c.i = after_year;
        }
        c.eat(b':');
    }
    let Some(time) = time(c, year.is_none()) else {
        return match year {
            Some(year) if !alnum(c.s.get(after_year).copied()) => {
                c.i = after_year;
                Some(Raw { date: Some((Some(year), month, day)), time: None, zone: None, end: after_year, ambiguous: None })
            }
            _ => {
                c.i = after_day;
                fail(c)
            }
        };
    };
    let zone = zone(c);
    if year.is_none() {
        let before = c.i;
        if c.spaces() >= 1 && digit(c.peek()) {
            match c.num(4, 4) {
                Some((y, _)) if plausible_year(y) && !alnum(c.peek()) => year = Some(y as i32),
                _ => c.i = before,
            }
        } else {
            c.i = before;
        }
    }
    Some(Raw { date: Some((year, month, day)), time: Some(time), zone, end: c.i, ambiguous: None })
}

/// Words at `c`: a month first, or a weekday followed by either order.
fn named(c: &mut Cur) -> Option<Raw> {
    let start = c.i;
    let word = c.word();
    if let Some(month) = month_of(word) {
        if let Some(raw) = month_name_first(c, month) {
            return Some(raw);
        }
    }
    if !weekday(word) {
        c.i = start;
        return None;
    }
    if c.s[c.i..].starts_with(b"-feira") {
        c.i += 6;
    }
    c.eat(b'.');
    c.eat(b',');
    if c.spaces() == 0 {
        c.i = start;
        return None;
    }
    let found = if digit(c.peek()) {
        day_month_name(c)
    } else {
        let word = c.word();
        month_of(word).and_then(|month| month_name_first(c, month))
    };
    if found.is_none() {
        c.i = start;
    }
    found
}

/// Epoch seconds (10 digits, optional fraction) or milliseconds (13 digits).
fn epoch(c: &mut Cur) -> Option<i64> {
    let start = c.i;
    let Some((value, count)) = c.num(9, 13) else { return None };
    let _ = value;
    let digits = std::str::from_utf8(&c.s[start..c.i]).ok()?;
    let whole: i64 = digits.parse().ok()?;
    let ms = match count {
        9 | 10 => {
            let mut ms = whole * 1000;
            if c.peek() == Some(b'.') && digit(c.at(1)) {
                c.i += 1;
                ms += i64::from(fraction(c) / 1_000_000);
            }
            ms
        }
        13 => whole,
        _ => {
            c.i = start;
            return None;
        }
    };
    // 2000-01-01 .. 2100-01-01, and never part of a longer token.
    if !(946_684_800_000..4_102_444_800_000).contains(&ms) || c.peek().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'.') {
        c.i = start;
        return None;
    }
    Some(ms)
}

/// Recognizers starting exactly at `i`. Epoch and time-only values only count
/// at the start of a line (`leading`), where they cannot be ordinary numbers.
fn scan_at(s: &[u8], i: usize, ctx: &Context, leading: bool) -> Option<(Raw, Option<i64>)> {
    let mut c = Cur { s, i };
    let first = *s.get(i)?;
    if first.is_ascii_digit() {
        if let Some(raw) = iso(&mut c) {
            return Some((raw, None));
        }
        if let Some(raw) = day_month_name(&mut c) {
            return Some((raw, None));
        }
        if let Some(raw) = numeric(&mut c, ctx) {
            return Some((raw, None));
        }
        if leading {
            if let Some(ms) = epoch(&mut c) {
                return Some((Raw { date: None, time: None, zone: None, end: c.i, ambiguous: None }, Some(ms)));
            }
            if let Some(time) = time(&mut c, true) {
                if !alnum(c.peek()) && !matches!(c.peek(), Some(b'/' | b'-')) {
                    return Some((Raw { date: None, time: Some(time), zone: None, end: c.i, ambiguous: None }, None));
                }
            }
        }
        return None;
    }
    if first.is_ascii_alphabetic() {
        return named(&mut c).map(|raw| (raw, None));
    }
    None
}

/// Days of a month in a year, for daylight saving rules.
fn nth_sunday(year: i32, month: u32, n: u8) -> Option<NaiveDate> {
    NaiveDate::from_weekday_of_month_opt(year, month, Weekday::Sun, n)
}

fn last_sunday(year: i32, month: u32) -> Option<NaiveDate> {
    nth_sunday(year, month, 5).or_else(|| nth_sunday(year, month, 4))
}

fn offset_seconds(zone: Zone, local: NaiveDateTime) -> Option<i32> {
    match zone {
        Zone::Fixed(offset) => Some(offset),
        Zone::Local => None,
        Zone::Eu(standard) => {
            // Last Sunday of March to last Sunday of October, 01:00 UTC.
            let year = local.year();
            let utc = local - chrono::Duration::seconds(i64::from(standard));
            let start = last_sunday(year, 3)?.and_hms_opt(1, 0, 0)?;
            let end = last_sunday(year, 10)?.and_hms_opt(1, 0, 0)?;
            Some(standard + if utc >= start && utc < end { 3600 } else { 0 })
        }
        Zone::Us(standard) => {
            // Second Sunday of March to first Sunday of November, 02:00 local.
            let year = local.year();
            let start = nth_sunday(year, 3, 2)?.and_hms_opt(2, 0, 0)?;
            let end = nth_sunday(year, 11, 1)?.and_hms_opt(2, 0, 0)?;
            Some(standard + if local >= start && local < end { 3600 } else { 0 })
        }
    }
}

fn infer_year(month: u32, day: u32, ctx: &Context) -> Option<NaiveDate> {
    match ctx.reference {
        Some(reference) => {
            let year = reference.year();
            let candidate = NaiveDate::from_ymd_opt(year, month, day);
            match candidate {
                Some(date) if date <= reference + chrono::Duration::days(1) => Some(date),
                _ => NaiveDate::from_ymd_opt(year - 1, month, day),
            }
        }
        None => NaiveDate::from_ymd_opt(ctx.year, month, day),
    }
}

fn complete(raw: Raw, epoch_ms: Option<i64>, start: usize, ctx: &Context) -> Option<Found> {
    if let Some(ms) = epoch_ms {
        return Some(Found { ms, start, end: raw.end, inferred: Inferred::Nothing, has_time: true, zoned: true });
    }
    let (date, inferred) = match raw.date {
        Some((Some(year), month, day)) => (NaiveDate::from_ymd_opt(year, month, day)?, Inferred::Nothing),
        Some((None, month, day)) => (infer_year(month, day, ctx)?, Inferred::Year),
        None => (ctx.reference?, Inferred::Date),
    };
    let (hour, minute, second, nanos) = raw.time.unwrap_or((0, 0, 0, 0));
    let local = date.and_hms_nano_opt(hour, minute, second, nanos)?;
    let offset = raw.zone.and_then(|zone| offset_seconds(zone, local));
    let ms = match offset {
        Some(offset) => local.and_utc().timestamp_millis() - i64::from(offset) * 1000,
        None => super::naive_to_ms(local),
    };
    Some(Found { ms, start, end: raw.end, inferred, has_time: raw.time.is_some(), zoned: offset.is_some() })
}

/// The leftmost date/time in a line. A complete date and time wins over an
/// earlier date without a time.
pub(crate) fn find(text: &str, ctx: &Context) -> Option<Found> {
    let s = text.as_bytes();
    let limit = s.len().min(SCAN_LIMIT);
    let lead = s.iter().take(2).take_while(|b| matches!(b, b'[' | b'(' | b'*' | b'.' | b'<' | b'"')).count();
    let mut date_only: Option<Found> = None;
    let mut i = 0;
    while i < limit {
        let b = s[i];
        if (b.is_ascii_alphanumeric()) && (i == 0 || !s[i - 1].is_ascii_alphanumeric()) {
            if let Some((raw, epoch_ms)) = scan_at(s, i, ctx, i == lead) {
                if let Some(found) = complete(raw, epoch_ms, i, ctx) {
                    if found.has_time {
                        return Some(found);
                    }
                    if i < DATE_ONLY_LIMIT && date_only.is_none() {
                        date_only = Some(found);
                    }
                }
                // Skip the rest of a word so its digits are not rescanned.
            }
        }
        i += 1;
    }
    date_only
}

/// A timestamp starting exactly at byte `at` of `text`.
pub(crate) fn at(text: &str, at: usize, ctx: &Context) -> Option<Found> {
    let s = text.as_bytes();
    let (raw, epoch_ms) = scan_at(s, at, ctx, true)?;
    complete(raw, epoch_ms, at, ctx)
}

/// A whole value that is a complete date (with year): field and column values.
/// Year-less and time-only values are rejected since a single value has no
/// source context. Surrounding brackets and quotes are ignored.
pub(crate) fn exact(text: &str) -> Option<i64> {
    exact_ordered(text, None)
}

/// `exact`, reading ambiguous numeric dates in the order the source showed.
pub(crate) fn exact_ordered(text: &str, day_first: Option<bool>) -> Option<i64> {
    let text = text.trim().trim_start_matches(['[', '(', '"', '\'']).trim_end_matches([']', ')', '"', '\'']).trim();
    if text.len() < 6 {
        return None;
    }
    let s = text.as_bytes();
    let ctx = Context { day_first, ..Context::default() };
    let (raw, epoch_ms) = scan_at(s, 0, &ctx, false)?;
    if raw.end != s.len() || epoch_ms.is_some() || !matches!(raw.date, Some((Some(_), _, _))) {
        return None;
    }
    complete(raw, None, 0, &ctx).map(|found| found.ms)
}

/// Order of ambiguous numeric dates shown by a sample: dates whose first part
/// exceeds 12 are day-first, whose second does are month-first. None when the
/// sample has no evidence or contradicts itself.
pub(crate) fn learn_day_first<'a>(lines: impl Iterator<Item = &'a str>) -> Option<bool> {
    let (mut day_first, mut month_first) = (0usize, 0usize);
    let ctx = Context::year(2000);
    for line in lines {
        let s = line.as_bytes();
        let limit = s.len().min(SCAN_LIMIT);
        let mut i = 0;
        while i < limit {
            if s[i].is_ascii_digit() && (i == 0 || !s[i - 1].is_ascii_alphanumeric()) {
                let mut c = Cur { s, i };
                if iso(&mut c).is_none() {
                    let mut c = Cur { s, i };
                    if let Some(raw) = numeric(&mut c, &ctx) {
                        if let Some((a, b)) = raw.ambiguous {
                            if a > 12 && b <= 12 {
                                day_first += 1;
                            } else if b > 12 && a <= 12 {
                                month_first += 1;
                            }
                        }
                        break;
                    }
                } else {
                    break;
                }
            }
            i += 1;
        }
    }
    match (day_first > 0, month_first > 0) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Date written in a file name: `server.log.2026-06-24`, `app_2026_10_12.log`,
/// `access-20261012.gz`, `u_ex261012.log`.
pub(crate) fn date_in_name(name: &str) -> Option<NaiveDate> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s = file.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if s[i].is_ascii_digit() && (i == 0 || !s[i - 1].is_ascii_digit()) {
            let run = s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
            let date = if run == 4 && matches!(s.get(i + 4), Some(b'-' | b'_' | b'.')) {
                let sep = s[i + 4];
                let text = std::str::from_utf8(&s[i..]).ok()?;
                let parts: Vec<&str> = text.splitn(4, sep as char).take(3).collect();
                let day: String = parts.get(2).map(|d| d.chars().take_while(char::is_ascii_digit).collect()).unwrap_or_default();
                match (parts[0].parse::<i32>(), parts.get(1).and_then(|m| m.parse::<u32>().ok()), day.parse::<u32>()) {
                    (Ok(y), Some(m), Ok(d)) if plausible_year(y as u32) && parts[1].len() <= 2 && (1..=2).contains(&day.len()) => {
                        NaiveDate::from_ymd_opt(y, m, d)
                    }
                    _ => None,
                }
            } else if run == 8 {
                let text = std::str::from_utf8(&s[i..i + 8]).ok()?;
                let (y, m, d) = (text[..4].parse::<i32>().ok()?, text[4..6].parse().ok()?, text[6..].parse().ok()?);
                plausible_year(y as u32).then(|| NaiveDate::from_ymd_opt(y, m, d)).flatten()
            } else if run == 6 && i >= 2 && s[i - 2..i].eq_ignore_ascii_case(b"ex") {
                // IIS: u_exYYMMDD.log
                let text = std::str::from_utf8(&s[i..i + 6]).ok()?;
                NaiveDate::from_ymd_opt(2000 + text[..2].parse::<i32>().ok()?, text[2..4].parse().ok()?, text[4..].parse().ok()?)
            } else {
                None
            };
            if date.is_some() {
                return date;
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text).unwrap().timestamp_millis()
    }
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> i64 {
        super::super::naive_to_ms(NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_milli_opt(h, mi, s, ms).unwrap())
    }
    fn ctx() -> Context {
        Context { year: 2026, reference: NaiveDate::from_ymd_opt(2026, 10, 14), day_first: None }
    }
    fn found(text: &str) -> Option<(i64, &str)> {
        find(text, &ctx()).map(|f| (f.ms, &text[f.start..f.end]))
    }

    #[test]
    fn iso_variants_with_zones_and_fractions() {
        for (text, expected, span) in [
            ("2026-10-12T03:00:00Z boot", utc("2026-10-12T03:00:00Z"), "2026-10-12T03:00:00Z"),
            ("2026-10-12T00:00:00.123-03:00 x", utc("2026-10-12T03:00:00.123Z"), "2026-10-12T00:00:00.123-03:00"),
            ("2026-10-12 00:00:00,123 -0300 INFO", utc("2026-10-12T03:00:00.123Z"), "2026-10-12 00:00:00,123 -0300"),
            ("[2026-10-12T00:00:00,123][INFO ]", local(2026, 10, 12, 0, 0, 0, 123), "2026-10-12T00:00:00,123"),
            ("2026/10/12 00:00:00 [error] 1#1", local(2026, 10, 12, 0, 0, 0, 0), "2026/10/12 00:00:00"),
            ("2026.10.12 08:01:02 x", local(2026, 10, 12, 8, 1, 2, 0), "2026.10.12 08:01:02"),
            ("20261012T030000Z run", utc("2026-10-12T03:00:00Z"), "20261012T030000Z"),
            ("2026-10-12 00:00:00:123  1234 Agent", local(2026, 10, 12, 0, 0, 0, 123), "2026-10-12 00:00:00:123"),
            ("2026-10-12 03:00:00.5 +0000 UTC m=+0.1", utc("2026-10-12T03:00:00.5Z"), "2026-10-12 03:00:00.5 +0000 UTC"),
            ("2026-10-12T00:00:00.123456789-03:00[America/Sao_Paulo] ok", utc("2026-10-12T03:00:00.123Z"), "2026-10-12T00:00:00.123456789-03:00[America/Sao_Paulo]"),
            ("2026-10-12 00:00:00.123 -03 [42] LOG:", utc("2026-10-12T03:00:00.123Z"), "2026-10-12 00:00:00.123 -03"),
            ("2026-10-12 00:00:00 BRT msg", utc("2026-10-12T03:00:00Z"), "2026-10-12 00:00:00 BRT"),
            ("2026-10-12 10:00 relatorio", local(2026, 10, 12, 10, 0, 0, 0), "2026-10-12 10:00"),
        ] {
            assert_eq!(found(text), Some((expected, span)), "{text}");
        }
    }

    #[test]
    fn named_months_in_both_orders_and_languages() {
        for (text, expected) in [
            ("192.168.21.40 - - [12/Oct/2026:00:00:00 -0300] GET /", utc("2026-10-12T03:00:00Z")),
            ("[12/Oct/2026:00:00:00.250 +0000] fe", utc("2026-10-12T00:00:00.25Z")),
            ("12-Oct-2026 00:00:00.123 INFO [main]", local(2026, 10, 12, 0, 0, 0, 123)),
            ("[12-Oct-2026 00:00:00 UTC] PHP Warning", utc("2026-10-12T00:00:00Z")),
            ("[12-Oct-2026 00:00:00 America/Sao_Paulo] PHP", utc("2026-10-12T03:00:00Z")),
            ("1234:M 12 Oct 2026 00:00:00.123 * Ready", local(2026, 10, 12, 0, 0, 0, 123)),
            ("Mon, 12 Oct 2026 03:00:00 GMT", utc("2026-10-12T03:00:00Z")),
            ("[Mon Oct 12 00:00:00.123456 2026] [core:error]", local(2026, 10, 12, 0, 0, 0, 123)),
            ("Mon Oct 12 03:00:00 UTC 2026", utc("2026-10-12T03:00:00Z")),
            ("seg 12 out 2026 00:00:00 -03", utc("2026-10-12T03:00:00Z")),
            ("Oct 12 2026 00:00:00: %ASA-6-302013", local(2026, 10, 12, 0, 0, 0, 0)),
            ("Oct 12, 2026 12:30:00 AM org.apache.Catalina start", local(2026, 10, 12, 0, 30, 0, 0)),
            ("Oct 12, 2026 1:05:09 PM x", local(2026, 10, 12, 13, 5, 9, 0)),
            ("12 de outubro", 0),
            ("12 out. 2026 10:00:00", local(2026, 10, 12, 10, 0, 0, 0)),
            ("12 Dez 2026 10:00:00", local(2026, 12, 12, 10, 0, 0, 0)),
            ("12-OCT-26 10:00:00", local(2026, 10, 12, 10, 0, 0, 0)),
        ] {
            let got = found(text).map(|(ms, _)| ms).unwrap_or(0);
            assert_eq!(got, expected, "{text}");
        }
    }

    #[test]
    fn year_less_values_use_the_reference_date() {
        let c = ctx();
        let f = find("Oct 12 00:00:00 host sshd[1]: ok", &c).unwrap();
        assert_eq!((f.ms, f.inferred), (local(2026, 10, 12, 0, 0, 0, 0), Inferred::Year));
        // Later than the reference: written the year before.
        assert_eq!(find("Dec 30 23:59:59 host x", &c).unwrap().ms, local(2025, 12, 30, 23, 59, 59, 0));
        assert_eq!(find("Oct  2 01:02:03 host x", &c).unwrap().ms, local(2026, 10, 2, 1, 2, 3, 0));
        assert_eq!(find("*Oct 12 00:00:00.123: %LINK-3-UPDOWN", &c).unwrap().ms, local(2026, 10, 12, 0, 0, 0, 123));
        // Snort and Android: month first, no year.
        assert_eq!(find("10/12-00:00:00.123456  [**] [1:1:1]", &c).unwrap().ms, local(2026, 10, 12, 0, 0, 0, 123));
        assert_eq!(find("10-12 00:00:00.123  1234  5678 I Tag: x", &c).unwrap().ms, local(2026, 10, 12, 0, 0, 0, 123));
        // Without a reference, the source's year.
        assert_eq!(find("Sep 30 12:30:00 host app: hello", &Context::year(2032)).unwrap().ms, local(2032, 9, 30, 12, 30, 0, 0));
    }

    #[test]
    fn numeric_dates_resolve_order_from_values_context_and_markers() {
        let c = ctx();
        assert_eq!(find("13/10/2026 10:00:00 x", &c).unwrap().ms, local(2026, 10, 13, 10, 0, 0, 0));
        assert_eq!(find("10/13/2026 10:00:00 x", &c).unwrap().ms, local(2026, 10, 13, 10, 0, 0, 0));
        // Ambiguous: day first by default, month first with AM/PM.
        assert_eq!(find("02/10/2026 10:00:00 x", &c).unwrap().ms, local(2026, 10, 2, 10, 0, 0, 0));
        assert_eq!(find("10/12/2026 12:00:00 AM 0A3C PACKET", &c).unwrap().ms, local(2026, 10, 12, 0, 0, 0, 0));
        let us = Context { day_first: Some(false), ..c };
        assert_eq!(find("02/10/2026 10:00:00 x", &us).unwrap().ms, local(2026, 2, 10, 10, 0, 0, 0));
        assert_eq!(find("12.10.2026 10:00:00 x", &c).unwrap().ms, local(2026, 10, 12, 10, 0, 0, 0));
        assert_eq!(find("10/12/26,00:00:00,Started", &c).unwrap().ms, local(2026, 10, 12, 0, 0, 0, 0));
        assert_eq!(find("10/12/2026-00:00:00.123456  [**]", &c).unwrap().ms, local(2026, 10, 12, 0, 0, 0, 123));
        assert_eq!(learn_day_first(["01/02/2026 x", "25/02/2026 y"].into_iter()), Some(true));
        assert_eq!(learn_day_first(["02/25/2026 10:00:00 x"].into_iter()), Some(false));
        assert_eq!(learn_day_first(["2026-10-12 x"].into_iter()), None);
    }

    #[test]
    fn epoch_and_time_only_only_at_line_start() {
        let c = ctx();
        assert_eq!(find("1286536309.586    921 192.168.0.68 TCP_MISS/200", &c).unwrap().ms, 1286536309586);
        assert_eq!(find("[1700000000] SERVICE ALERT", &c).unwrap().ms, 1_700_000_000_000);
        assert_eq!(find("1700000000123 msg", &c).unwrap().ms, 1_700_000_000_123);
        assert!(find("pid 1286536309 exited", &c).is_none());
        let f = find("00:00:00,001 WARN [x] (y) msg", &c).unwrap();
        assert_eq!((f.ms, f.inferred), (local(2026, 10, 14, 0, 0, 0, 1), Inferred::Date));
        assert!(find("retry at 10:00:00 later", &c).is_none());
        assert!(find("00:00:00,001 WARN", &Context::year(2026)).is_none(), "no reference date: time only is undated");
    }

    #[test]
    fn ordinary_numbers_versions_and_addresses_are_not_dates() {
        let c = ctx();
        for text in [
            "192.168.21.40 GET /x 200", "version 1.2.3 build 4567", "took 12/34 ms", "10.0.0.1:8443 connected",
            "id 20261012 created", "May I help", "out of memory 12 times", "ratio 3/4 done", "HTTP/1.1 200 2789 0 0.000",
            "port 8443-86 open",
        ] {
            assert!(find(text, &c).is_none(), "{text}: {:?}", find(text, &c).map(|f| &text[f.start..f.end]));
        }
    }

    #[test]
    fn dst_rules_for_european_and_us_zones() {
        let summer = exact("2026-07-01 12:00:00 Europe/Paris").unwrap();
        assert_eq!(summer, utc("2026-07-01T10:00:00Z"));
        let winter = exact("2026-01-15 12:00:00 Europe/Paris").unwrap();
        assert_eq!(winter, utc("2026-01-15T11:00:00Z"));
        assert_eq!(exact("2026-07-01 12:00:00 America/New_York").unwrap(), utc("2026-07-01T16:00:00Z"));
        assert_eq!(exact("2026-12-01 12:00:00 America/New_York").unwrap(), utc("2026-12-01T17:00:00Z"));
        assert_eq!(exact("2026-12-01 12:00:00 Mars/Base"), None);
    }

    #[test]
    fn exact_values_require_a_complete_date() {
        assert_eq!(exact("12/Oct/2026:00:00:00 -0300"), Some(utc("2026-10-12T03:00:00Z")));
        assert_eq!(exact("\"Oct 12, 2026 12:00:00 PM\""), Some(local(2026, 10, 12, 12, 0, 0, 0)));
        assert_eq!(exact("2026-10-12"), Some(local(2026, 10, 12, 0, 0, 0, 0)));
        assert_eq!(exact("Oct 12 00:00:00"), None);
        assert_eq!(exact("00:00:00"), None);
        assert_eq!(exact("2026-10-12 trailing words"), None);
        assert_eq!(exact("1700000000"), None, "plain numbers are handled by epoch rules");
        let _ = chrono::Utc.timestamp_millis_opt(0);
    }

    #[test]
    fn dates_in_file_names() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d);
        assert_eq!(date_in_name("access_log.2026-10-12"), d(2026, 10, 12));
        assert_eq!(date_in_name(r"C:\logs\server.log.2026-06-24"), d(2026, 6, 24));
        assert_eq!(date_in_name("app_2026_10_12.log"), d(2026, 10, 12));
        assert_eq!(date_in_name("access-20261012.gz"), d(2026, 10, 12));
        assert_eq!(date_in_name("u_ex261012.log"), d(2026, 10, 12));
        assert_eq!(date_in_name("syslog.1"), None);
        assert_eq!(date_in_name("app-123456789.log"), None);
    }
}
