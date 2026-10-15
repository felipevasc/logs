//! Conditions compiled once and evaluated per item: text and patterns in
//! parallel, then dates, positions and the combination in file order.
use super::spec::{Combine, Condition, DateOrder, Locate, Missing, RuleSpec, Scope, Test, TextOp, Unit, Zone};
use crate::sources::stamp;
use aho_corasick::AhoCorasick;
use chrono::TimeZone;

pub(crate) const MAX_CONDITIONS: usize = 32;
pub(crate) const MAX_DATES: usize = 4;
/// Bytes of an item searched line by line for its date.
const DATE_SCAN: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fold {
    Exact,
    Ascii,
    Unicode,
}

struct Matcher {
    op: TextOp,
    values: Vec<String>,
    fold: Fold,
    any: Option<AhoCorasick>,
}

impl Matcher {
    fn new(op: TextOp, values: &[String], ignore_case: bool) -> Result<Self, String> {
        let values: Vec<String> = values.iter().filter(|v| !v.is_empty()).cloned().collect();
        if values.is_empty() {
            return Err("Informe ao menos um texto em cada critério de texto.".into());
        }
        let fold = match (ignore_case, values.iter().all(|v| v.is_ascii())) {
            (false, _) => Fold::Exact,
            (true, true) => Fold::Ascii,
            (true, false) => Fold::Unicode,
        };
        let values: Vec<String> = if fold == Fold::Unicode { values.iter().map(|v| v.to_lowercase()).collect() } else { values };
        let any = if op == TextOp::Contains {
            Some(
                AhoCorasick::builder()
                    .ascii_case_insensitive(fold == Fold::Ascii)
                    .build(&values)
                    .map_err(|e| format!("Textos do critério inválidos: {e}"))?,
            )
        } else {
            None
        };
        Ok(Matcher { op, values, fold, any })
    }

    fn test(&self, hay: &str) -> bool {
        let lowered;
        let hay = if self.fold == Fold::Unicode {
            lowered = hay.to_lowercase();
            lowered.as_str()
        } else {
            hay
        };
        let same = |a: &str, b: &str| if self.fold == Fold::Ascii { a.eq_ignore_ascii_case(b) } else { a == b };
        match self.op {
            TextOp::Contains => self.any.as_ref().is_some_and(|ac| ac.is_match(hay)),
            TextOp::StartsWith => self.values.iter().any(|v| hay.get(..v.len()).is_some_and(|h| same(h, v))),
            TextOp::EndsWith => {
                let hay = hay.trim_end();
                self.values.iter().any(|v| hay.len() >= v.len() && hay.get(hay.len() - v.len()..).is_some_and(|h| same(h, v)))
            }
            TextOp::Equals => {
                let hay = hay.trim();
                self.values.iter().any(|v| same(hay, v.trim()))
            }
        }
    }
}

pub(crate) fn regex(pattern: &str, ignore_case: bool, what: &str) -> Result<regex::Regex, String> {
    if pattern.is_empty() {
        return Err(format!("Informe a expressão regular {what}."));
    }
    regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .multi_line(true)
        .size_limit(64 << 20)
        .dfa_size_limit(64 << 20)
        .build()
        .map_err(|e| format!("Expressão regular {what} inválida: {e}"))
}

#[derive(Clone, Debug)]
enum Format {
    Chrono(String),
    Epoch,
    EpochSeconds,
    EpochMillis,
}

/// `dd/MM/yyyy HH:mm:ss.SSS` and similar Java/.NET patterns as strftime.
fn translate(format: &str) -> String {
    if format.contains('%') {
        return format.to_string();
    }
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        let run = chars[i..].iter().take_while(|&&x| x == c).count();
        let token = match (c, run) {
            ('y', 2) => "%y",
            ('y', _) => "%Y",
            ('M', 3) => "%b",
            ('M', n) if n >= 4 => "%B",
            ('M', _) => "%m",
            ('d', _) => "%d",
            ('H', _) => "%H",
            ('h', _) => "%I",
            ('m', _) => "%M",
            ('s', _) => "%S",
            ('S', 3) => "%3f",
            ('S', 6) => "%6f",
            ('S', 9) => "%9f",
            ('S', _) => "%f",
            ('a', _) => "%p",
            ('E', n) if n >= 4 => "%A",
            ('E', _) => "%a",
            ('X', n) if n >= 3 => "%:z",
            ('X' | 'Z', _) => "%z",
            _ => {
                for _ in 0..run {
                    out.push(c);
                }
                i += run;
                continue;
            }
        };
        out.push_str(token);
        i += run;
    }
    out
}

pub(crate) struct DateReader {
    locate: Locate,
    pattern: Option<(regex::Regex, usize)>,
    field: Vec<String>,
    format: Option<Format>,
    ctx: stamp::Context,
    utc: bool,
}

fn local_wall_to_utc(ms: i64) -> i64 {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.naive_local().and_utc().timestamp_millis())
        .unwrap_or(ms)
}

fn epoch(text: &str, unit: &Format) -> Option<i64> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-') {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    let ms = match unit {
        Format::EpochSeconds => value * 1000.0,
        Format::EpochMillis => value,
        _ => match value.abs() {
            v if v >= 1e17 => value / 1e6,
            v if v >= 1e14 => value / 1e3,
            v if v >= 1e11 => value,
            _ => value * 1000.0,
        },
    };
    ms.is_finite().then_some(ms as i64)
}

impl DateReader {
    fn new(locate: Locate, pattern: Option<&str>, format: Option<&str>, order: DateOrder, zone: Zone, ctx: stamp::Context) -> Result<Self, String> {
        let format = match format.map(str::trim).filter(|f| !f.is_empty()) {
            None => None,
            Some(f) if f.eq_ignore_ascii_case("epoch") => Some(Format::Epoch),
            Some(f) if f.eq_ignore_ascii_case("epoch_s") => Some(Format::EpochSeconds),
            Some(f) if f.eq_ignore_ascii_case("epoch_ms") => Some(Format::EpochMillis),
            Some(f) => Some(Format::Chrono(translate(f))),
        };
        let pattern_text = pattern.map(str::trim).unwrap_or_default();
        let (pattern, field) = match locate {
            Locate::Regex => {
                let re = regex(pattern_text, false, "da data")?;
                let group = ["data", "date"]
                    .iter()
                    .find_map(|name| re.capture_names().position(|n| n == Some(*name)))
                    .unwrap_or(usize::from(re.captures_len() > 1));
                (Some((re, group)), Vec::new())
            }
            Locate::Field => {
                if pattern_text.is_empty() {
                    return Err("Informe o campo JSON que tem a data (por exemplo, @timestamp).".into());
                }
                (None, pattern_text.split('.').map(str::to_string).collect())
            }
            _ => (None, Vec::new()),
        };
        let ctx = stamp::Context {
            day_first: match order {
                DateOrder::Auto => ctx.day_first,
                DateOrder::DayFirst => Some(true),
                DateOrder::MonthFirst => Some(false),
            },
            ..ctx
        };
        Ok(DateReader { locate, pattern, field, format, ctx, utc: zone == Zone::Utc })
    }

    /// The first date of an item, in any layout (shown in previews).
    pub(crate) fn auto(ctx: stamp::Context) -> Self {
        DateReader { locate: Locate::Auto, pattern: None, field: Vec::new(), format: None, ctx, utc: false }
    }

    fn found(&self, found: stamp::Found) -> i64 {
        if !found.zoned && self.utc { local_wall_to_utc(found.ms) } else { found.ms }
    }

    /// A date at the start of `text` written as the explicit format.
    fn chrono_prefix(&self, text: &str, format: &str) -> Option<i64> {
        if let Ok((date, _)) = chrono::DateTime::parse_and_remainder(text, format) {
            return Some(date.timestamp_millis());
        }
        let naive = chrono::NaiveDateTime::parse_and_remainder(text, format)
            .map(|(n, _)| n)
            .or_else(|_| chrono::NaiveDate::parse_and_remainder(text, format).map(|(d, _)| d.and_hms_opt(0, 0, 0).unwrap_or_default()))
            .ok()?;
        Some(if self.utc { naive.and_utc().timestamp_millis() } else { crate::sources::naive_to_ms(naive) })
    }

    /// A whole value (regex capture, JSON field).
    fn value(&self, text: &str) -> Option<i64> {
        let text = text.trim();
        match &self.format {
            Some(Format::Chrono(format)) => self.chrono_prefix(text, format),
            Some(unit) => epoch(text, unit),
            None => epoch(text, &Format::Epoch)
                .filter(|_| text.len() >= 9)
                .or_else(|| stamp::find(text, &self.ctx).map(|f| self.found(f))),
        }
    }

    fn in_line(&self, line: &str) -> Option<i64> {
        match &self.format {
            Some(Format::Chrono(format)) => {
                let bytes = line.as_bytes();
                let limit = line.len().min(400);
                (0..limit)
                    .filter(|&i| line.is_char_boundary(i) && bytes[i].is_ascii_alphanumeric() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()))
                    .find_map(|i| self.chrono_prefix(&line[i..], format))
            }
            Some(unit) => epoch(line.split_whitespace().next().unwrap_or_default(), unit),
            None => stamp::find(line, &self.ctx).map(|f| self.found(f)),
        }
    }

    pub(crate) fn read(&self, text: &str) -> Option<i64> {
        let first = super::codec::trim_break(text.split('\n').next().unwrap_or_default());
        match self.locate {
            Locate::Auto => {
                let mut taken = 0;
                for line in text.split('\n') {
                    if taken > DATE_SCAN {
                        break;
                    }
                    taken += line.len() + 1;
                    if let Some(ms) = self.in_line(super::codec::trim_break(line)) {
                        return Some(ms);
                    }
                }
                None
            }
            Locate::LineStart => {
                let at = usize::from(first.starts_with('['));
                match &self.format {
                    Some(Format::Chrono(format)) => self.chrono_prefix(&first[at..], format),
                    Some(unit) => epoch(first[at..].split_whitespace().next().unwrap_or_default(), unit),
                    None => stamp::at(first, at, &self.ctx).map(|f| self.found(f)),
                }
            }
            Locate::Regex => {
                let (re, group) = self.pattern.as_ref()?;
                let captures = re.captures(text)?;
                let found = captures.get(*group).or_else(|| captures.get(0))?;
                self.value(found.as_str())
            }
            Locate::Field => {
                let value: serde_json::Value = serde_json::from_str(first.trim()).ok()?;
                let mut node = &value;
                for key in &self.field {
                    node = node.get(key)?;
                }
                match node {
                    serde_json::Value::String(s) => self.value(s),
                    serde_json::Value::Number(n) => epoch(&n.to_string(), self.format.as_ref().unwrap_or(&Format::Epoch)),
                    _ => None,
                }
            }
        }
    }
}

enum Kind {
    Text { matcher: Matcher, first_line: bool },
    Regex { re: regex::Regex, first_line: bool },
    Date { slot: usize, from: Option<i64>, to: Option<i64>, missing: Missing },
    Ordinal { lines: bool, from: Option<u64>, to: Option<u64> },
}

struct Cond {
    negate: bool,
    kind: Kind,
}

pub(crate) struct Rule {
    all: bool,
    sorted: bool,
    conds: Vec<Cond>,
    dates: Vec<DateReader>,
}

/// What can be computed for an item without looking at the others.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Facts {
    bits: u64,
    pub dates: [Option<i64>; MAX_DATES],
}

/// Order-dependent state: last date seen per date condition.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Running {
    last: [Option<i64>; MAX_DATES],
    pub undated: u64,
}

impl Rule {
    pub(crate) fn compile(spec: &RuleSpec, ctx: stamp::Context) -> Result<Rule, String> {
        if spec.conditions.is_empty() {
            return Err("Adicione ao menos um critério.".into());
        }
        if spec.conditions.len() > MAX_CONDITIONS {
            return Err(format!("Use no máximo {MAX_CONDITIONS} critérios."));
        }
        let mut dates = Vec::new();
        let mut conds = Vec::new();
        for Condition { negate, test } in &spec.conditions {
            let kind = match test {
                Test::Text { op, values, ignore_case, scope } => Kind::Text { matcher: Matcher::new(*op, values, *ignore_case)?, first_line: *scope == Scope::FirstLine },
                Test::Regex { pattern, ignore_case, scope } => Kind::Regex { re: regex(pattern, *ignore_case, "do critério")?, first_line: *scope == Scope::FirstLine },
                Test::Date { from, to, locate, pattern, format, order, zone, missing } => {
                    if from.is_none() && to.is_none() {
                        return Err("Informe o início, o fim ou ambos em cada critério de data.".into());
                    }
                    if dates.len() == MAX_DATES {
                        return Err(format!("Use no máximo {MAX_DATES} critérios de data."));
                    }
                    dates.push(DateReader::new(*locate, pattern.as_deref(), format.as_deref(), *order, *zone, ctx)?);
                    Kind::Date { slot: dates.len() - 1, from: *from, to: *to, missing: *missing }
                }
                Test::Ordinal { unit, from, to } => {
                    if from.is_none() && to.is_none() {
                        return Err("Informe o início, o fim ou ambos em cada critério de posição.".into());
                    }
                    Kind::Ordinal { lines: *unit == Unit::Line, from: *from, to: *to }
                }
            };
            conds.push(Cond { negate: *negate, kind });
        }
        Ok(Rule { all: spec.combine == Combine::All || conds.len() == 1, sorted: spec.sorted, conds, dates })
    }

    pub(crate) fn date_reader(&self, slot: usize) -> &DateReader {
        &self.dates[slot]
    }

    pub(crate) fn has_dates(&self) -> bool {
        !self.dates.is_empty()
    }

    fn has_ordinals(&self) -> bool {
        self.conds.iter().any(|c| matches!(c.kind, Kind::Ordinal { .. }))
    }

    pub(crate) fn facts(&self, text: &str) -> Facts {
        let mut facts = Facts::default();
        let first = || super::codec::trim_break(text.split('\n').next().unwrap_or_default());
        for (i, cond) in self.conds.iter().enumerate() {
            let hit = match &cond.kind {
                Kind::Text { matcher, first_line } => matcher.test(if *first_line { first() } else { text }),
                Kind::Regex { re, first_line } => re.is_match(if *first_line { first() } else { text }),
                Kind::Date { slot, .. } => {
                    facts.dates[*slot] = self.dates[*slot].read(text);
                    false
                }
                Kind::Ordinal { .. } => false,
            };
            facts.bits |= u64::from(hit) << i;
        }
        facts
    }

    /// Whether the item matches, in file order.
    pub(crate) fn decide(&self, facts: &Facts, item: u64, line: u64, running: &mut Running) -> bool {
        let mut undated = false;
        let mut result = self.all;
        for (i, cond) in self.conds.iter().enumerate() {
            let hit = match &cond.kind {
                Kind::Text { .. } | Kind::Regex { .. } => facts.bits >> i & 1 == 1,
                Kind::Date { slot, from, to, missing } => {
                    let value = match facts.dates[*slot] {
                        Some(date) => {
                            running.last[*slot] = Some(date);
                            Some(Some(date))
                        }
                        None => {
                            undated = true;
                            match missing {
                                Missing::Previous => Some(running.last[*slot]),
                                Missing::Match => None,
                                Missing::NoMatch => Some(None),
                            }
                        }
                    };
                    match value {
                        None => true,
                        Some(date) => date.is_some_and(|d| from.is_none_or(|f| d >= f) && to.is_none_or(|t| d < t)),
                    }
                }
                Kind::Ordinal { lines, from, to } => {
                    let at = if *lines { line } else { item };
                    from.is_none_or(|f| at >= f) && to.is_none_or(|t| at <= t)
                }
            };
            let hit = hit != cond.negate;
            result = if self.all { result && hit } else { result || hit };
        }
        running.undated += u64::from(undated);
        result
    }

    /// When the result for this item and every item after it is already
    /// known: positions past their range, or dates past theirs in a
    /// chronological file.
    pub(crate) fn fate(&self, facts: &Facts, item: u64, line: u64, running: &Running) -> Option<bool> {
        let mut fates = self.conds.iter().map(|cond| {
            let fate = match &cond.kind {
                Kind::Ordinal { lines, from, to } => {
                    let at = if *lines { line } else { item };
                    if to.is_some_and(|t| at > t) {
                        Some(false)
                    } else if to.is_none() && from.is_none_or(|f| at >= f) {
                        Some(true)
                    } else {
                        None
                    }
                }
                Kind::Date { slot, from, to, missing: Missing::Previous } if self.sorted => {
                    let date = facts.dates[*slot].or(running.last[*slot]);
                    match date {
                        Some(d) if to.is_some_and(|t| d >= t) => Some(false),
                        Some(d) if to.is_none() && from.is_none_or(|f| d >= f) => Some(true),
                        _ => None,
                    }
                }
                _ => None,
            };
            fate.map(|f| f != cond.negate)
        });
        if self.all {
            let mut all_true = true;
            for fate in &mut fates {
                match fate {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => all_true = false,
                }
            }
            all_true.then_some(true)
        } else {
            let mut all_false = true;
            for fate in &mut fates {
                match fate {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => all_false = false,
                }
            }
            all_false.then_some(false)
        }
    }

    pub(crate) fn may_settle(&self) -> bool {
        self.conds.iter().any(|c| match c.kind {
            Kind::Ordinal { .. } => true,
            Kind::Date { missing: Missing::Previous, .. } => self.sorted,
            _ => false,
        })
    }

    /// Lower date bound every match must reach, for a binary search in a
    /// chronological file: (date slot, bound).
    pub(crate) fn seek_bound(&self) -> Option<(usize, i64)> {
        // Items read after the jump start without an inherited date: exact
        // only when a single date condition depends on it.
        if !self.sorted || !self.all || self.has_ordinals() || self.dates.len() != 1 {
            return None;
        }
        self.conds.iter().find_map(|c| match c.kind {
            Kind::Date { slot, from: Some(from), missing: Missing::Previous, .. } if !c.negate => Some((slot, from)),
            _ => None,
        })
    }

    /// A rule that is only a date range over a chronological file: its
    /// result is decided by two binary searches.
    pub(crate) fn pure_range(&self) -> Option<(usize, Option<i64>, Option<i64>)> {
        match self.conds.as_slice() {
            [Cond { negate: false, kind: Kind::Date { slot, from, to, missing: Missing::Previous } }] if self.sorted => Some((*slot, *from, *to)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_split::spec::{Condition, RuleSpec};

    fn rule(json: serde_json::Value) -> Rule {
        let spec: RuleSpec = serde_json::from_value(json).unwrap();
        Rule::compile(&spec, stamp::Context::year(2026)).unwrap()
    }

    fn matches(rule: &Rule, items: &[&str]) -> Vec<bool> {
        let mut running = Running::default();
        items.iter().enumerate().map(|(i, text)| rule.decide(&rule.facts(text), i as u64 + 1, i as u64 + 1, &mut running)).collect()
    }

    #[test]
    fn text_conditions_cover_contains_prefix_suffix_and_case() {
        let r = rule(serde_json::json!({"conditions":[{"kind":"text","op":"contains","values":["error","FALHA"],"ignoreCase":true}]}));
        assert_eq!(matches(&r, &["an ERROR here", "falha geral", "ok"]), [true, true, false]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"text","op":"startsWith","values":["["]},{"kind":"text","op":"endsWith","values":["fim"],"negate":true}],"combine":"all"}));
        assert_eq!(matches(&r, &["[x] fim\n", "[x] meio\n", "x meio"]), [false, true, false]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"text","op":"contains","values":["ATENÇÃO"],"ignoreCase":true,"scope":"firstLine"}]}));
        assert_eq!(matches(&r, &["atenção: disco\nmais", "linha\natenção"]), [true, false]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"text","op":"equals","values":["OK"]},{"kind":"regex","pattern":"^Caused by"}],"combine":"any"}));
        assert_eq!(matches(&r, &[" OK \n", "x\nCaused by: y", "OK!"]), [true, true, false]);
    }

    #[test]
    fn dates_inherit_and_respect_bounds_zones_and_formats() {
        let from = chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap().and_hms_opt(10, 0, 0).unwrap();
        let from = crate::sources::naive_to_ms(from);
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","from":from}]}));
        assert_eq!(matches(&r, &["2026-10-12 09:59:59 a", "  at stack", "12/Oct/2026:10:00:00 b", "continuation"]), [false, false, true, true]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","to":from,"missing":"noMatch"}]}));
        assert_eq!(matches(&r, &["2026-10-12 09:59:59 a", "  at stack"]), [true, false]);
        let utc = chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap().and_hms_opt(10, 0, 0).unwrap().and_utc().timestamp_millis();
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","from":utc,"zone":"utc"}]}));
        assert_eq!(matches(&r, &["2026-10-12 09:59:59 a", "2026-10-12T07:00:00-03:00 b"]), [false, true]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","from":from,"locate":"regex","pattern":"ts=(?P<data>\\S+)","format":"dd/MM/yyyy-HH:mm:ss"}]}));
        assert_eq!(matches(&r, &["ts=12/10/2026-09:00:00", "ts=12/10/2026-10:30:00"]), [false, true]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","from":from,"locate":"field","pattern":"event.time"}]}));
        assert_eq!(matches(&r, &[r#"{"event":{"time":"2026-10-12T09:00:00"}, "x":"2027-01-01"}"#, &format!(r#"{{"event":{{"time":{}}}}}"#, from)]), [false, true]);
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","from":from,"locate":"lineStart","order":"dayFirst"}]}));
        assert_eq!(matches(&r, &["12/10/2026 10:00:00 dia primeiro", "x 2027-01-01 00:00:00"]), [true, true]);
    }

    #[test]
    fn positions_and_fates_settle_the_rest_of_the_file() {
        let r = rule(serde_json::json!({"conditions":[{"kind":"ordinal","from":2,"to":3}]}));
        assert_eq!(matches(&r, &["a", "b", "c", "d"]), [false, true, true, false]);
        let facts = Facts::default();
        assert_eq!(r.fate(&facts, 4, 4, &Running::default()), Some(false));
        assert_eq!(r.fate(&facts, 2, 2, &Running::default()), None);
        let r = rule(serde_json::json!({"conditions":[{"kind":"ordinal","from":2}],"combine":"all"}));
        assert_eq!(r.fate(&facts, 2, 2, &Running::default()), Some(true));
        let r = rule(serde_json::json!({"conditions":[{"kind":"ordinal","from":2,"negate":true}]}));
        assert_eq!(r.fate(&facts, 5, 5, &Running::default()), Some(false));
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","to":1000},{"kind":"text","values":["x"]}],"sorted":true}));
        let mut late = Facts::default();
        late.dates[0] = Some(1000);
        assert_eq!(r.fate(&late, 9, 9, &Running::default()), Some(false));
        assert!(r.pure_range().is_none());
        assert!(r.seek_bound().is_none());
        let r = rule(serde_json::json!({"conditions":[{"kind":"date","from":5,"to":9}],"sorted":true}));
        assert_eq!(r.pure_range(), Some((0, Some(5), Some(9))));
        assert_eq!(r.seek_bound(), Some((0, 5)));
    }

    #[test]
    fn invalid_rules_explain_themselves() {
        let bad = |json: serde_json::Value| {
            let spec: RuleSpec = serde_json::from_value(json).unwrap();
            Rule::compile(&spec, stamp::Context::year(2026)).err().unwrap()
        };
        assert!(bad(serde_json::json!({"conditions":[]})).contains("critério"));
        assert!(bad(serde_json::json!({"conditions":[{"kind":"regex","pattern":"("}]})).contains("inválida"));
        assert!(bad(serde_json::json!({"conditions":[{"kind":"date"}]})).contains("início"));
        assert!(bad(serde_json::json!({"conditions":[{"kind":"text","values":[""]}]})).contains("texto"));
        let _: Condition = serde_json::from_value(serde_json::json!({"kind":"ordinal","unit":"line","to":3,"negate":true})).unwrap();
        assert_eq!(translate("dd/MM/yyyy HH:mm:ss.SSS 'T' XXX"), "%d/%m/%Y %H:%M:%S.%3f T %:z");
    }
}
