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
    const EXACT: f64 = 9_007_199_254_740_991.0;
    let (first, second) = pf.numeric_bounds();
    let first = match first {
        Some(n) if n.is_finite() && n.abs() <= EXACT => n,
        None => return Some("FALSE".into()),
        _ => return None,
    };
    let comparison = match pf.f.op.as_str() {
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
                    return Decision::Sql(sql);
                }
                return Decision::Sql(tests.number(column, Arc::new(move |n| number_matches(&owned, n))));
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
            Val::Sql(value) => Decision::Sql(exact_text(&value, pf).unwrap_or_else(|| {
                tests.text(&value, Arc::new(move |v| value_matches(&owned, v)))
            })),
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
                let term = term.clone();
                (tests.number("ts", Arc::new(move |n| n.is_some_and(|n| term.number_matches(n)))), true)
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
}
