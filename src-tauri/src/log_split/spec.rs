//! What the user asked for, as the interface sends it.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FramingSpec {
    /// Records that start with a date when the file shows continuation lines;
    /// otherwise indented lines and stack traces continue the line above.
    Auto,
    Line,
    StartDate,
    Continuation,
    #[serde(rename_all = "camelCase")]
    StartRegex { pattern: String, #[serde(default)] ignore_case: bool },
    #[serde(rename_all = "camelCase")]
    EndRegex { pattern: String, #[serde(default)] ignore_case: bool },
    BlankLine,
    /// Escapes: `\n`, `\r`, `\t`, `\0`, `\\` and `\xNN`.
    Delimiter { text: String },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Combine {
    #[default]
    All,
    Any,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TextOp {
    #[default]
    Contains,
    StartsWith,
    EndsWith,
    Equals,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    #[default]
    Item,
    FirstLine,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Locate {
    /// The first date of the item's first line (then of its next lines).
    #[default]
    Auto,
    /// A date at the very start of the first line (after an optional `[`).
    LineStart,
    /// The text captured by a regex (group `data`, `date` or 1, else the match).
    Regex,
    /// A field of a JSON object line (dotted path).
    Field,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DateOrder {
    #[default]
    Auto,
    DayFirst,
    MonthFirst,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Zone {
    #[default]
    Local,
    Utc,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Missing {
    /// The date of the closest item above that had one.
    #[default]
    Previous,
    Match,
    NoMatch,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    #[default]
    Item,
    Line,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Test {
    #[serde(rename_all = "camelCase")]
    Text {
        #[serde(default)]
        op: TextOp,
        values: Vec<String>,
        #[serde(default)]
        ignore_case: bool,
        #[serde(default)]
        scope: Scope,
    },
    #[serde(rename_all = "camelCase")]
    Regex {
        pattern: String,
        #[serde(default)]
        ignore_case: bool,
        #[serde(default)]
        scope: Scope,
    },
    #[serde(rename_all = "camelCase")]
    Date {
        /// Inclusive lower bound, epoch milliseconds.
        from: Option<i64>,
        /// Exclusive upper bound, epoch milliseconds.
        to: Option<i64>,
        #[serde(default)]
        locate: Locate,
        /// Regex (Locate::Regex) or field path (Locate::Field).
        #[serde(default)]
        pattern: Option<String>,
        /// strftime (`%d/%m/%Y %H:%M:%S`), Java style (`dd/MM/yyyy HH:mm:ss`),
        /// `epoch`, `epoch_s` or `epoch_ms`; empty recognizes any layout.
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        order: DateOrder,
        #[serde(default)]
        zone: Zone,
        #[serde(default)]
        missing: Missing,
    },
    #[serde(rename_all = "camelCase")]
    Ordinal {
        #[serde(default)]
        unit: Unit,
        /// Inclusive, 1-based.
        from: Option<u64>,
        /// Inclusive, 1-based.
        to: Option<u64>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    #[serde(default)]
    pub negate: bool,
    #[serde(flatten)]
    pub test: Test,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSpec {
    #[serde(default)]
    pub combine: Combine,
    pub conditions: Vec<Condition>,
    /// The file is in chronological order: date bounds may be found by
    /// binary search and reading stops once they are passed.
    #[serde(default)]
    pub sorted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Only the items that match.
    Matched,
    /// Matching items and, in a second file, the others.
    Both,
    /// Both files, then the original is removed. With little free space the
    /// original is cut from its end while the outputs grow.
    Move,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitSpec {
    pub framing: FramingSpec,
    pub rule: RuleSpec,
    pub mode: Mode,
}
