//! Translation of the app's filters into SQL over the engine's columns.
//!
//! Exact text equality and safe integer ranges use native SQL predicates so
//! DuckDB can prune row groups. Other tests run the app's Rust comparisons
//! (registered through [`Tests`]). Conditions that the columns cannot decide
//! exactly become supersets whose candidates are confirmed on the parsed
//! events, so results never depend on the translation being complete.
use super::build::{role_column, QUERY_TOOL, SEP};
use super::udf::Tests;
use crate::entities::{self, Role};
use crate::query::{is_numeric_op, number_matches, reads_all_as_column, value_matches, PreparedFilter};
use crate::querylang::{Expr, Term, TermKind};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Where each field's value lives, across the parts of a session.
#[derive(Default)]
pub(crate) struct Schema {
    /// Field name → SQL expression of its text (NULL when absent).
    pub fields: HashMap<String, String>,
    /// ASCII-lowercase name → field names sharing it.
    pub lower: HashMap<String, Vec<String>>,
    pub structured: HashSet<String>,
    pub ci_multi: HashSet<String>,
}

pub(crate) fn lit(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Conditions of a filter list.
pub(crate) struct Plan {
    /// SQL condition; with `verify`/`lines` empty it selects exactly the matches.
    pub sql: String,
    /// Filters confirmed on each candidate's parsed event.
    pub verify: Vec<usize>,
    /// Filters answered by the line engine (raw-line searches).
    pub lines: Vec<usize>,
    /// The condition reads names or descriptions (catalog enrichment).
    pub names: bool,
    pub tests: Tests,
}

impl Plan {
    pub(crate) fn exact(&self) -> bool {
        self.verify.is_empty() && self.lines.is_empty()
    }
}

const NULL_TEXT: &str = "CAST(NULL AS VARCHAR)";

/// Value a test reads. DuckDB replaces a call whose argument is a constant
/// NULL by NULL without running it, so absent values are decided here.
enum Val {
    Sql(String),
    Absent,
    /// Only the raw line has it.
    Unsupported,
}

fn literal(value: bool) -> String {
    if value { "TRUE" } else { "FALSE" }.into()
}

/// Comparing an integer through f64 is exact around these bounds. Convert
/// fractional bounds to integer boundaries without casting the stored column:
/// casting it would hide its zonemaps from the optimizer. Exotic/non-finite
/// bounds keep the established Rust comparison path.
fn integer_range(column: &str, pf: &PreparedFilter) -> Option<String> {
    let (first, second) = pf.numeric_bounds();
    integer_bounds(column, &pf.f.op, first, second)
}

fn integer_bounds(column: &str, op: &str, first: Option<f64>, second: Option<f64>) -> Option<String> {
    const EXACT: f64 = 9_007_199_254_740_991.0;
    let first = match first {
        Some(n) if n.is_finite() && n.abs() <= EXACT => n,
        None => return Some("FALSE".into()),
        _ => return None,
    };
    let comparison = match op {
        "gt" => format!("{column} > {}", first.floor() as i64),
        "gte" => format!("{column} >= {}", first.ceil() as i64),
        "lt" => format!("{column} < {}", first.ceil() as i64),
        "lte" => format!("{column} <= {}", first.floor() as i64),
        "between" => {
            let second = match second {
                Some(n) if n.is_finite() && n.abs() <= EXACT => n,
                None => return Some("FALSE".into()),
                _ => return None,
            };
            format!("{column} >= {} AND {column} <= {}", first.ceil() as i64, second.floor() as i64)
        }
        _ => return None,
    };
    Some(format!("({comparison})"))
}

/// These predicates occur as conjuncts in a filter list, where SQL NULL and
/// false both reject a row. Negative equality must explicitly retain NULLs.
fn exact_text(value: &str, pf: &PreparedFilter) -> Option<String> {
    let f = &pf.f;
    let (needle, negative) = match f.op.as_str() {
        "equals_exact" => (f.value.as_str(), false),
        "not_equals_exact" => (f.value.as_str(), true),
        // ASCII-insensitive equality is literal equality for needles without
        // ASCII letters (common event codes, ports, and numeric identifiers).
        "equals" | "not_equals" if !f.value.trim().bytes().any(|c| c.is_ascii_alphabetic()) => {
            (f.value.trim(), f.op == "not_equals")
        }
        "in_exact" => {
            let mut items: Vec<&str> = f.value.lines().filter(|v| !v.is_empty()).collect();
            items.sort_unstable();
            items.dedup();
            return Some(if items.is_empty() {
                "FALSE".into()
            } else {
                format!("({value} IN ({}))", items.into_iter().map(lit).collect::<Vec<_>>().join(", "))
            });
        }
        _ => return None,
    };
    Some(if negative {
        format!("({value} IS NULL OR {value} <> {})", lit(needle))
    } else {
        format!("({value} = {})", lit(needle))
    })
}

impl Schema {
    /// Text of a field (exact key), or NULL.
    pub(crate) fn field(&self, name: &str) -> String {
        self.fields.get(name).cloned().unwrap_or_else(|| NULL_TEXT.into())
    }

    /// `Event::col_ref(column)`; `None` when the store lacks the value (raw line).
    pub(crate) fn column(&self, column: &str, names: &mut bool) -> Option<String> {
        Some(match column {
            "id" => "CAST(id AS VARCHAR)".into(),
            "event_ref" => "event_ref".into(),
            "timestamp" => "li_iso(ts)".into(),
            "source" | "level" | "code" | "message" => column.into(),
            "name" | "description" => {
                *names = true;
                column.into()
            }
            "raw" => return None,
            other if other.starts_with('@') && entities::role_of_column(other).is_some() => {
                format!("COALESCE({}, {})", self.field(other), role_column(other))
            }
            other => self.field(other),
        })
    }

    fn value(&self, column: &str, names: &mut bool) -> Val {
        match self.column(column, names) {
            None => Val::Unsupported,
            Some(sql) if sql == NULL_TEXT => Val::Absent,
            Some(sql) => Val::Sql(sql),
        }
    }

    /// Ordinary scalar fields are included verbatim in the indexed free-text
    /// values. A complete hex word of an exact ASCII literal is therefore
    /// guaranteed to contribute that token, including within a UUID.
    /// Metadata, role aliases, skipped path fields and structured values do
    /// not have that coverage proof and retain their existing SQL predicate.
    /// A dot in an exact stored key is literal here (`Event::col_ref`), not
    /// query-language nested traversal; the full key must exist in `fields`.
    fn exact_hex_field(&self, column: &str, value: &str) -> bool {
        super::text::exact_field_hex_word(value).is_some()
            && !column.starts_with(['@', '_'])
            && !matches!(column, "id" | "event_ref" | "timestamp" | "source" | "level" | "code" | "name" | "description" | "message" | "raw" | "arquivo" | "caminho")
            && self.fields.contains_key(column)
            && !self.structured.contains(column)
    }

    /// Exact grouping text as Ctx::get(field_ref(...)) sees it. Unsupported
    /// nested/ambiguous fields keep the bounded event path; NULL stays absent,
    /// while empty and whitespace-only strings remain real keys.
    pub(crate) fn grouped_field(&self, field: &crate::querylang::FieldRef, names: &mut bool) -> Option<String> {
        let (name, role) = field.parts();
        match self.ctx_value(name, role, names) {
            Val::Sql(value) => Some(value),
            Val::Absent => Some(NULL_TEXT.into()),
            Val::Unsupported => None,
        }
    }

    /// Value the search language reads for a field (`Ctx::field`).
    fn ctx_value(&self, name: &str, role: Option<Role>, names: &mut bool) -> Val {
        if name.starts_with('@') {
            if let Some(found) = entities::role_of_column(name) {
                return Val::Sql(format!("COALESCE({}, {})", self.field(name), query_role(found)));
            }
        }
        if matches!(
            name,
            "id" | "event_ref" | "timestamp" | "source" | "level" | "code" | "name" | "description" | "message" | "raw"
        ) {
            return self.value(name, names);
        }
        let mut parts: Vec<String> = self.fields.get(name).cloned().into_iter().collect();
        let lower = name.to_ascii_lowercase();
        // Two keys differing only in case: the record's key order decides.
        if self.ci_multi.contains(&lower) {
            return Val::Unsupported;
        }
        if let Some(variants) = self.lower.get(&lower) {
            parts.extend(variants.iter().filter(|v| v.as_str() != name).map(|v| self.field(v)));
        }
        // Dotted paths can reach into object and array values.
        if name.match_indices('.').any(|(i, _)| self.structured.contains(&name[..i])) {
            return Val::Unsupported;
        }
        if let Some(role) = role {
            parts.push(query_role(role));
        }
        match parts.len() {
            0 => Val::Absent,
            1 => Val::Sql(parts.remove(0)),
            _ => Val::Sql(format!("COALESCE({})", parts.join(", "))),
        }
    }

    pub(crate) fn plan(&self, pfs: &[PreparedFilter]) -> Plan {
        let mut plan = Plan {
            sql: String::new(),
            verify: Vec::new(),
            lines: Vec::new(),
            names: false,
            tests: Tests::default(),
        };
        let mut conditions = Vec::new();
        for (i, pf) in pfs.iter().enumerate() {
            match self.filter(pf, &mut plan.tests, &mut plan.names) {
                Decision::Sql(sql) => conditions.push(sql),
                Decision::Superset(sql) => {
                    conditions.push(sql);
                    plan.verify.push(i);
                }
                Decision::Event => plan.verify.push(i),
                Decision::Lines => plan.lines.push(i),
            }
        }
        plan.sql = if conditions.is_empty() {
            "TRUE".into()
        } else {
            conditions.join(" AND ")
        };
        plan
    }

    fn filter(&self, pf: &PreparedFilter, tests: &mut Tests, names: &mut bool) -> Decision {
        let f = &pf.f;
        let op = f.op.as_str();
        match op {
            "query" => {
                return match &pf.expr {
                    Some(expr) => {
                        let (sql, exact) = self.expr(expr, tests, names);
                        if exact {
                            Decision::Sql(sql)
                        } else {
                            Decision::Superset(sql)
                        }
                    }
                    None => Decision::Sql("FALSE".into()),
                };
            }
            "threat_rule" | "detection" => return Decision::Event,
            "pattern" => return Decision::Sql(format!("(pat = {})", lit(&f.value))),
            _ => {}
        }
        if f.column == "_all" && !reads_all_as_column(op) {
            return match op {
                "regex" | "contains" | "not_contains" => Decision::Lines,
                _ => Decision::Sql("FALSE".into()),
            };
        }
        let owned = Arc::new(crate::query::prepare(std::slice::from_ref(f)).remove(0));
        if is_numeric_op(op) {
            let direct = match f.column.as_str() {
                "timestamp" => Some("ts"),
                "id" => Some("id"),
                _ => None,
            };
            if let Some(column) = direct {
                if let Some(sql) = integer_range(column, pf) {
                    return Decision::Sql(if column == "ts" { format!("(ts <> 0 AND {sql})") } else { sql });
                }
                let value = if column == "ts" { "NULLIF(ts, 0)" } else { column };
                return Decision::Sql(tests.number(value, Arc::new(move |n| number_matches(&owned, n))));
            }
            return match self.value(&f.column, names) {
                Val::Sql(value) => Decision::Sql(tests.text(
                    &value,
                    Arc::new(move |v| number_matches(&owned, v.and_then(crate::model::text_number))),
                )),
                Val::Absent => Decision::Sql(literal(number_matches(&owned, None))),
                Val::Unsupported => Decision::Event,
            };
        }
        match self.value(&f.column, names) {
            Val::Sql(value) => {
                let native = exact_text(&value, pf);
                if op == "equals_exact" && self.exact_hex_field(&f.column, &f.value) {
                    let word = super::text::exact_field_hex_word(&f.value).expect("field coverage checked").to_ascii_lowercase();
                    Decision::Sql(tests.hex_field(owned, native.expect("exact equality is native"), word))
                } else {
                    Decision::Sql(native.unwrap_or_else(|| tests.text(&value, Arc::new(move |v| value_matches(&owned, v)))))
                }
            }
            Val::Absent => Decision::Sql(literal(value_matches(&owned, None))),
            Val::Unsupported => Decision::Event,
        }
    }

    /// SQL of a search expression and whether it is exact (otherwise a superset).
    fn expr(&self, expr: &Expr, tests: &mut Tests, names: &mut bool) -> (String, bool) {
        match expr {
            Expr::All => ("TRUE".into(), true),
            Expr::And(items) | Expr::Or(items) => {
                let joiner = if matches!(expr, Expr::And(_)) { " AND " } else { " OR " };
                let mut exact = true;
                let parts: Vec<String> = items
                    .iter()
                    .map(|item| {
                        let (sql, e) = self.expr(item, tests, names);
                        exact &= e;
                        sql
                    })
                    .collect();
                (format!("({})", parts.join(joiner)), exact)
            }
            Expr::Not(inner) => match self.expr(inner, tests, names) {
                (sql, true) => (format!("(NOT {sql})"), true),
                // The complement of a superset says nothing.
                (_, false) => ("TRUE".into(), false),
            },
            Expr::Term(term) => self.term(term, tests, names),
        }
    }

    fn term(&self, term: &Term, tests: &mut Tests, names: &mut bool) -> (String, bool) {
        let Some((name, role, _)) = term.field() else {
            return self.free_text(term, tests, names);
        };
        if name == "_all" {
            return ("TRUE".into(), false);
        }
        match term.kind() {
            TermKind::Event => ("TRUE".into(), false),
            TermKind::Number if name == "timestamp" => {
                if let Some((op, first, second)) = term.numeric_bounds() {
                    if let Some(comparison) = integer_bounds("ts", op, Some(first), second) {
                        // Unlike top-level filter chips, a term may appear
                        // below NOT/OR. Rust treats an absent timestamp as
                        // false, so retain a two-valued expression here.
                        return (format!("(ts IS NOT NULL AND ts <> 0 AND {comparison})"), true);
                    }
                }
                let term = term.clone();
                let test = tests.number("NULLIF(ts, 0)", Arc::new(move |n| n.is_some_and(|n| term.number_matches(n))));
                (format!("(ts IS NOT NULL AND ts <> 0 AND {test})"), true)
            }
            _ => match self.ctx_value(name, role, names) {
                Val::Sql(value) => {
                    let term = term.clone();
                    (tests.text(&value, Arc::new(move |v| v.is_some_and(|v| term.value_matches(v)))), true)
                }
                // `Ctx::field` finds nothing: the term never matches.
                Val::Absent => ("FALSE".into(), true),
                Val::Unsupported => ("TRUE".into(), false),
            },
        }
    }

    /// Free text looks at every value (`querylang::any_value`).
    fn free_text(&self, term: &Term, tests: &mut Tests, names: &mut bool) -> (String, bool) {
        let needle = match term.kind() {
            TermKind::Contains(needle) if !needle.contains(SEP) => needle.to_string(),
            _ if term.is_pattern() => {
                // A wildcard's literal must appear in some value. Unicode case
                // folding maps `ſ` to `s`, so only literals without `s` qualify.
                return match term
                    .wildcard_literal()
                    .filter(|l| l.is_ascii() && !l.contains(['s', 'S']) && !l.contains(SEP))
                {
                    Some(literal) => (any_value(&literal, tests, names), false),
                    None => ("TRUE".into(), false),
                };
            }
            TermKind::Contains(_) => return ("TRUE".into(), false),
            // Other matchers without a field never match.
            _ => return ("FALSE".into(), true),
        };
        (any_value(&needle, tests, names), true)
    }
}

/// Some value (lowercase free-text column, name or description) contains `needle`.
fn any_value(needle: &str, tests: &mut Tests, _names: &mut bool) -> String {
    tests.free_text(needle)
}

/// Column of a role as the search language resolves it (`Ctx::role`).
fn query_role(role: Role) -> String {
    if role == Role::Tool {
        QUERY_TOOL.into()
    } else {
        role_column(entities::info(role).column)
    }
}

enum Decision {
    /// Exact condition.
    Sql(String),
    /// Condition every match satisfies; candidates are confirmed per event.
    Superset(String),
    /// Decided only on the parsed event.
    Event,
    /// Decided by the line engine.
    Lines,
}

#[cfg(test)]
mod native_predicate_tests {
    use super::*;
    use crate::query::Filter;

    fn filter(column: &str, op: &str, value: &str, value2: Option<&str>) -> PreparedFilter {
        crate::query::prepare(&[Filter { column: column.into(), op: op.into(), value: value.into(), value2: value2.map(str::to_string) }]).remove(0)
    }

    #[test]
    fn exact_hex_candidates_require_stored_scalar_field_coverage() {
        let mut schema = Schema::default();
        let hex = "0000000000000000000000006585cfa1";
        schema.fields.insert("trace_id".into(), "w0".into());
        let plan = schema.plan(&[filter("trace_id", "equals_exact", hex, None)]);
        assert!(plan.exact());
        assert_eq!(plan.tests.hex_fields.len(), 1);
        assert!(plan.tests.hex_fields[0].fallback_sql.contains("w0 ="));
        for column in ["id", "event_ref", "timestamp", "source", "level", "code", "name", "description", "message", "raw", "arquivo", "caminho", "@user", "_meta"] {
            schema.fields.insert(column.into(), "w1".into());
            assert!(!schema.exact_hex_field(column, hex), "{column}");
        }
        assert!(!schema.exact_hex_field("missing", hex));
        assert!(!schema.exact_hex_field("trace_id", "dead"));
        assert!(!schema.exact_hex_field("trace_id", &"f".repeat(65)));
        assert!(schema.exact_hex_field("trace_id", "deadbeef-12345678"));
        assert!(!schema.exact_hex_field("trace_id", "prefixdeadbeef"));
        assert!(schema.plan(&[filter("trace_id", "equals", hex, None)]).tests.hex_fields.is_empty());
        assert!(schema.plan(&[filter("trace_id", "not_equals_exact", hex, None)]).tests.hex_fields.is_empty());
        schema.structured.insert("trace_id".into());
        assert!(!schema.exact_hex_field("trace_id", hex));
    }

    #[test]
    fn dotted_hex_equality_requires_the_exact_stored_scalar_key() {
        let mut schema = Schema::default();
        let hex = "0123456789abcdef0123456789abcdef";
        schema.fields.insert("trace.id".into(), "w0".into());
        schema.fields.insert("Trace.ID".into(), "w1".into());
        schema.fields.insert("trace".into(), "w2".into());
        // A structured parent cannot manufacture a literal child column. It
        // also cannot hide a separately stored exact child key.
        schema.structured.insert("trace".into());
        for (column, sql_column) in [("trace.id", "w0"), ("Trace.ID", "w1")] {
            let plan = schema.plan(&[filter(column, "equals_exact", hex, None)]);
            assert!(plan.exact());
            assert_eq!(plan.tests.hex_fields.len(), 1);
            let candidate = &plan.tests.hex_fields[0];
            assert_eq!(candidate.filter.f.column, column);
            assert_eq!(candidate.fallback_sql, format!("({sql_column} = '{hex}')"));
        }
        for column in ["trace.ID", "trace.nested.id", "missing.child", "@trace.id", "_trace.id"] {
            assert!(!schema.exact_hex_field(column, hex), "{column}");
        }
        schema.structured.insert("trace.id".into());
        assert!(schema.plan(&[filter("trace.id", "equals_exact", hex, None)]).tests.hex_fields.is_empty());
        assert!(schema.exact_hex_field("Trace.ID", hex));
    }

    #[test]
    fn dotted_query_expression_terms_never_establish_required_hex_equality() {
        let mut schema = Schema::default();
        let hex = "0123456789abcdef0123456789abcdef";
        schema.fields.insert("trace.id".into(), "w0".into());
        for op in ["equals", "not_equals_exact", "contains", "in_exact"] {
            assert!(schema.plan(&[filter("trace.id", op, hex, None)]).tests.hex_fields.is_empty(), "{op}");
        }
        for expression in [
            format!("trace.id:\"{hex}\""),
            format!("NOT trace.id:\"{hex}\""),
            format!("trace.id:\"{hex}\" OR source:other"),
            format!("NOT (trace.id:\"{hex}\" OR NOT source:other)"),
            format!("trace.id:\"{hex}\" AND source:other"),
        ] {
            let query = filter("_all", "query", &expression, None);
            assert!(query.expr.is_some(), "fixture must compile: {expression}");
            assert!(schema.plan(std::slice::from_ref(&query)).tests.hex_fields.is_empty(), "{expression}");
            let plan = schema.plan(&[filter("trace.id", "equals_exact", hex, None), query]);
            assert_eq!(plan.tests.hex_fields.len(), 1, "only the required top-level equality: {expression}");
        }
    }

    #[test]
    fn integer_bounds_keep_columns_uncast_and_preserve_fractional_edges() {
        for (op, expected) in [("gt", "(ts > 10)"), ("gte", "(ts >= 11)"), ("lt", "(ts < 11)"), ("lte", "(ts <= 10)")] {
            assert_eq!(integer_range("ts", &filter("timestamp", op, "10.5", None)).as_deref(), Some(expected));
        }
        assert_eq!(integer_range("id", &filter("id", "between", "-1.5", Some("2.5"))).as_deref(), Some("(id >= -1 AND id <= 2)"));
        assert_eq!(integer_range("ts", &filter("timestamp", "gt", "invalid", None)).as_deref(), Some("FALSE"));
        assert!(integer_range("ts", &filter("timestamp", "gt", "NaN", None)).is_none());
        assert!(integer_range("ts", &filter("timestamp", "gte", "9007199254740992", None)).is_none());
    }

    #[test]
    fn exact_lists_and_null_negative_semantics_are_preserved() {
        assert_eq!(exact_text("code", &filter("code", "equals", " 404 ", None)).as_deref(), Some("(code = '404')"));
        assert_eq!(exact_text("x", &filter("x", "not_equals_exact", "O'Reilly", None)).as_deref(), Some("(x IS NULL OR x <> 'O''Reilly')"));
        assert_eq!(exact_text("x", &filter("x", "in_exact", " a \nb,c\n a \n", None)).as_deref(), Some("(x IN (' a ', 'b,c'))"));
        assert!(exact_text("x", &filter("x", "equals", "Admin", None)).is_none());
        assert!(exact_text("x", &filter("x", "in", "Admin", None)).is_none());
    }

    #[test]
    fn plans_expose_safe_ranges_and_equality_to_duckdb() {
        let schema = Schema::default();
        let plan = schema.plan(&[filter("timestamp", "gte", "1000.5", None), filter("code", "equals_exact", "4625", None)]);
        assert!(plan.exact());
        assert!(plan.sql.contains("ts >= 1001"));
        assert!(plan.sql.contains("code = '4625'"));
        assert!(!plan.sql.contains("li_test"));
        assert!(!plan.sql.contains("li_ntest"));
    }

    fn timestamp_connection() -> (duckdb::Connection, Vec<Option<i64>>) {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        super::super::udf::register(&connection).unwrap();
        connection.execute_batch("SET threads=1; CREATE TABLE times (ordinal INTEGER, ts BIGINT)").unwrap();
        let times = vec![None, Some(i64::MIN), Some(-9_007_199_254_740_993),
            Some(-9_007_199_254_740_992), Some(-9_007_199_254_740_991),
            Some(-2), Some(-1), Some(0), Some(1), Some(2), Some(10), Some(11),
            Some(1_700_000_000_000), Some(9_007_199_254_740_991),
            Some(9_007_199_254_740_992), Some(9_007_199_254_740_993), Some(i64::MAX)];
        for (ordinal, timestamp) in times.iter().enumerate() {
            connection.execute("INSERT INTO times VALUES (?, ?)", duckdb::params![ordinal as i32, timestamp]).unwrap();
        }
        (connection, times)
    }

    fn assert_timestamp_expression(connection: &duckdb::Connection, times: &[Option<i64>], expression: &Expr, native: bool) {
        let mut tests = Tests::default();
        let (sql, exact) = Schema::default().expr(expression, &mut tests, &mut false);
        assert!(exact);
        assert_eq!(!sql.contains("li_ntest"), native, "{expression:?}: {sql}");
        let actual: Vec<Option<bool>> = connection.prepare(&format!("SELECT {sql} FROM times ORDER BY ordinal"))
            .unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        let expected: Vec<_> = times.iter().map(|timestamp| {
            let mut event = crate::model::Event::empty();
            event.timestamp = *timestamp;
            Some(expression.matches(&event))
        }).collect();
        assert_eq!(actual, expected, "{expression:?}: {sql}");
    }

    #[test]
    fn timestamp_terms_use_native_bounds_with_exact_nested_boolean_semantics() {
        let (connection, times) = timestamp_connection();
        for source in ["timestamp>10.5", "timestamp>=-1.5", "timestamp<10.5", "timestamp<=-1.5",
            "timestamp:-1.5..2.5", "timestamp>=0", "timestamp<=0",
            "time>=1700000000000", "timestamp>=9007199254740991", "timestamp<=-9007199254740991"] {
            for text in [source.to_string(), format!("NOT ({source})"),
                format!("NOT (NOT ({source}))"),
                format!("({source}) OR timestamp>=1700000000000"),
                format!("NOT (({source}) OR NOT (timestamp>=0))"),
                format!("({source}) AND NOT (timestamp:0..2)")] {
                let expression = crate::querylang::compile_rule(&text).unwrap();
                assert_timestamp_expression(&connection, &times, &expression, true);
            }
        }
    }

    #[test]
    fn timestamp_terms_keep_rust_comparison_for_extreme_and_nonfinite_bounds() {
        use crate::querylang::{not, or, term, Spec};
        let (connection, times) = timestamp_connection();
        for bound in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 9_007_199_254_740_992.0,
            -9_007_199_254_740_992.0, i64::MAX as f64, i64::MIN as f64] {
            for operation in [Spec::Gt as fn(f64) -> Spec, Spec::Gte, Spec::Lt, Spec::Lte] {
                for shape in 0..3 {
                    let expression = term(Some("timestamp"), None, operation(bound)).unwrap();
                    let expression = match shape {
                        0 => expression,
                        1 => not(expression),
                        _ => not(or(vec![expression, term(Some("timestamp"), None, Spec::Gte(0.0)).unwrap()])),
                    };
                    assert_timestamp_expression(&connection, &times, &expression, false);
                }
            }
        }
        for source in ["timestamp:0..9007199254740992", "NOT timestamp:-9007199254740992..0"] {
            assert_timestamp_expression(&connection, &times, &crate::querylang::compile_rule(source).unwrap(), false);
        }
    }

    #[test]
    fn timestamp_chips_and_event_verification_treat_zero_as_missing_but_ids_do_not() {
        let (connection, times) = timestamp_connection();
        let schema = Schema::default();
        for (op, value, second) in [("gt", "-1000", None), ("gte", "0", None),
            ("lt", "1000", None), ("lte", "0", None), ("between", "-1000", Some("1000")),
            ("lt", "9007199254740992", None), ("gt", "-9007199254740992", None)] {
            let pf = filter("timestamp", op, value, second);
            let plan = schema.plan(std::slice::from_ref(&pf));
            assert!(plan.exact());
            let actual: Vec<i32> = connection.prepare(&format!("SELECT ordinal FROM times WHERE {} ORDER BY ordinal", plan.sql))
                .unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
            let expected: Vec<i32> = times.iter().enumerate().filter_map(|(ordinal, timestamp)| {
                let mut event = crate::model::Event::empty(); event.timestamp = *timestamp;
                crate::query::matches(&event, &pf).then_some(ordinal as i32)
            }).collect();
            assert_eq!(actual, expected, "{op} {value}");
            assert!(!actual.contains(&7), "epoch zero is absent for numeric timestamp filters");
        }
        assert!(crate::query::number_matches(&filter("id", "gte", "0", None), Some(0.0)));
        assert_eq!(integer_range("id", &filter("id", "gte", "0", None)).as_deref(), Some("(id >= 0)"));
        // A field called timestamp inside a nested object is not event metadata.
        assert!(crate::querylang::compile_rule("timestamp>=0").unwrap().matches_object(
            &serde_json::json!({"timestamp":0}).as_object().unwrap().clone()));
    }
}
