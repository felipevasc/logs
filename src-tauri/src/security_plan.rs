//! One evaluation of each shared AST node per event, including nested predicates.
use crate::querylang::{Ctx, Expr};
use std::collections::HashMap;

enum Node {
    And(Vec<usize>),
    Or(Vec<usize>),
    Not(usize),
    Leaf(Expr),
}
#[derive(Default)]
pub struct Plan {
    nodes: Vec<Node>,
    keys: HashMap<String, usize>,
}
impl Plan {
    pub fn add(&mut self, expr: &Expr) -> usize {
        let key = expr.shared_key();
        if let Some(&id) = key.as_ref().and_then(|key| self.keys.get(key)) {
            return id;
        }
        let node = match expr {
            Expr::And(parts) => Node::And(parts.iter().map(|part| self.add(part)).collect()),
            Expr::Or(parts) => Node::Or(parts.iter().map(|part| self.add(part)).collect()),
            Expr::Not(part) => Node::Not(self.add(part)),
            other => Node::Leaf(other.clone()),
        };
        let id = self.nodes.len();
        self.nodes.push(node);
        if let Some(key) = key {
            self.keys.insert(key, id);
        }
        id
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn signature(&self) -> Option<String> {
        let mut keys = self
            .keys
            .iter()
            .map(|(key, id)| format!("{id}:{key}"))
            .collect::<Vec<_>>();
        keys.sort();
        keys.push(format!("layout:{}", self.nodes.len()));
        Some(crate::evidence::stable_id("predicate-plan-3", keys))
    }
    pub fn cache_values(&self, memo: &[Option<bool>]) -> Vec<Option<bool>> {
        let mut result = vec![None; memo.len()];
        for id in self.keys.values() {
            result[*id] = memo[*id];
        }
        result
    }
    pub fn matches(&self, id: usize, ctx: &Ctx<'_>, memo: &mut [Option<bool>]) -> bool {
        if let Some(value) = memo[id] {
            return value;
        }
        let value = match &self.nodes[id] {
            Node::And(parts) => parts.iter().all(|&part| self.matches(part, ctx, memo)),
            Node::Or(parts) => parts.iter().any(|&part| self.matches(part, ctx, memo)),
            Node::Not(part) => !self.matches(*part, ctx, memo),
            Node::Leaf(expr) => expr.matches_ctx(ctx),
        };
        memo[id] = Some(value);
        value
    }
}

/// A bounded vector executor over resolved typed facts. Only scalar terms
/// enter SQL; predicates needing the original event keep semantic confirmation.
/// Every shared scalar leaf is evaluated once per vector, then reused by all
/// boolean branches and rules. No SQL approximation can suppress a candidate.
pub struct Columnar {
    projections: Vec<crate::querylang::Term>,
    leaves: Vec<usize>,
    select: String,
    _tests: crate::engine::udf::Tests,
    pub batches: usize,
    pub rows: usize,
    pub reused_batches: usize,
}
impl Plan {
    pub fn columnar(&self, _db: &duckdb::Connection) -> Result<Columnar, String> {
        let mut projections = Vec::new();
        let mut fields = HashMap::new();
        let mut leaves = Vec::new();
        let mut tests = crate::engine::udf::Tests::default();
        let mut expressions = std::collections::BTreeMap::<usize, Vec<String>>::new();
        for (node, entry) in self.nodes.iter().enumerate() {
            let Node::Leaf(Expr::Term(term)) = entry else {
                continue;
            };
            let Some(key) = term.projection_key() else {
                continue;
            };
            let column = *fields.entry(key).or_insert_with(|| {
                let id = projections.len();
                projections.push(term.clone());
                id
            });
            expressions.entry(column).or_default().push(format!(
                "struct_pack(node:={node},matched:={})",
                crate::engine::sql::resolved_term(term, "value", &mut tests)
            ));
            leaves.push(node);
        }
        let select = if expressions.is_empty() {
            "SELECT row_id,0 AS node,FALSE AS matched FROM predicate_batch WHERE FALSE".into()
        } else {
            let arms = expressions
                .iter()
                .map(|(projection, items)| format!("WHEN {projection} THEN [{}]", items.join(",")))
                .collect::<Vec<_>>()
                .join(" ");
            format!("SELECT row_id,result.node AS node,result.matched AS matched FROM (SELECT row_id,unnest(CASE projection_id {arms} ELSE [] END) AS result FROM predicate_batch)")
        };
        Ok(Columnar {
            projections,
            leaves,
            select,
            _tests: tests,
            batches: 0,
            rows: 0,
            reused_batches: 0,
        })
    }
}
impl Columnar {
    pub fn evaluate(
        &mut self,
        db: &duckdb::Connection,
        events: &[crate::model::Event],
        memos: &mut [Vec<Option<bool>>],
    ) -> Result<(), String> {
        if events.len() != memos.len() {
            return Err("Lote de predicados incompatível".into());
        }
        if memos
            .iter()
            .all(|memo| self.leaves.iter().all(|&node| memo[node].is_some()))
        {
            self.reused_batches += 1;
            self.rows += events.len();
            return Ok(());
        }
        db.execute_batch("DROP TABLE IF EXISTS predicate_batch;CREATE TEMP TABLE predicate_batch(row_id BIGINT,projection_id INTEGER,value VARCHAR)").map_err(|e|format!("Vetor de predicados: {e}"))?;
        let mut appender = db.appender("predicate_batch").map_err(|e| e.to_string())?;
        for (row, event) in events.iter().enumerate() {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let ctx = Ctx::new(event);
            for (projection, term) in self.projections.iter().enumerate() {
                let value = term.projected_value(&ctx);
                appender
                    .append_row(duckdb::params![row as i64, projection as i32, value])
                    .map_err(|e| format!("Importação do vetor de predicados: {e}"))?;
            }
        }
        appender.flush().map_err(|e| e.to_string())?;
        drop(appender);
        let mut statement = db.prepare(&self.select).map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            crate::operations::check()?;
            let index: usize = row.get(0).map_err(|e| e.to_string())?;
            let memo = memos
                .get_mut(index)
                .ok_or("Índice inválido na avaliação colunar")?;
            let node: usize = row.get(1).map_err(|e| e.to_string())?;
            let value: bool = row.get(2).map_err(|e| e.to_string())?;
            let slot = memo
                .get_mut(node)
                .ok_or("Predicado desconhecido na avaliação colunar")?;
            if let Some(previous) = *slot {
                if previous != value {
                    return Err("Checkpoint divergiu da semântica colunar".into());
                }
            }
            *slot = Some(value);
            count += 1;
        }
        if count != events.len() * self.leaves.len() {
            return Err("Lote incompleto na avaliação colunar".into());
        }
        self.batches += 1;
        self.rows += events.len();
        Ok(())
    }
    pub fn leaves(&self) -> usize {
        self.leaves.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_columnar_shared_semantics_cover_roles_paths_null_unicode_numeric_and_negation() {
        let expressions = [
            "a:ação AND NOT b:*",
            "a:ação OR b:x",
            "NOT (a:x OR b:*)",
            "@host=server",
            "metric>2",
            "nested.value=x",
            "timestamp>0",
            "value|exists:true",
        ];
        let expressions = expressions[..7]
            .iter()
            .map(|s| crate::querylang::compile_rule(s).unwrap())
            .collect::<Vec<_>>();
        let mut plan = Plan::default();
        let ids = expressions.iter().map(|e| plan.add(e)).collect::<Vec<_>>();
        let db = duckdb::Connection::open_in_memory().unwrap();
        crate::engine::udf::register(&db).unwrap();
        let mut columnar = plan.columnar(&db).unwrap();
        let mut events = Vec::new();
        for (i, value) in [
            serde_json::Value::Null,
            serde_json::json!("AÇÃO"),
            serde_json::json!("x"),
            serde_json::json!(""),
        ]
        .into_iter()
        .enumerate()
        {
            let mut event = crate::model::Event::empty();
            event.id = i;
            event.timestamp = Some(i as i64);
            event.fields.insert("a".into(), value);
            event.fields.insert("host.name".into(), "server".into());
            event.fields.insert("metric".into(), "3ms".into());
            event
                .fields
                .insert("nested".into(), serde_json::json!({"value":"x"}));
            events.push(event);
        }
        let mut memos = vec![vec![None; plan.len()]; events.len()];
        columnar.evaluate(&db, &events, &mut memos).unwrap();
        assert!(columnar.leaves() > 3);
        for (event, memo) in events.iter().zip(memos.iter_mut()) {
            let ctx = Ctx::new(event);
            for (expr, &id) in expressions.iter().zip(&ids) {
                assert_eq!(
                    plan.matches(id, &ctx, memo),
                    expr.matches_ctx(&ctx),
                    "{expr:?}"
                );
            }
        }
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn shared_nested_plan_preserves_missing_fields_negations_and_unicode() {
        let expressions = ["a:ação AND NOT b:*", "a:ação OR b:x", "NOT (a:x OR b:*)"]
            .map(|s| crate::querylang::compile_rule(s).unwrap());
        let mut plan = Plan::default();
        let ids = expressions.iter().map(|e| plan.add(e)).collect::<Vec<_>>();
        assert_eq!(plan.add(&expressions[0]), ids[0]);
        for field in [None, Some("AÇÃO"), Some("x")] {
            let mut event = crate::model::Event::empty();
            if let Some(field) = field {
                event.fields.insert("a".into(), field.into());
            }
            let ctx = Ctx::new(&event);
            let mut memo = vec![None; plan.len()];
            for (expr, &id) in expressions.iter().zip(&ids) {
                assert_eq!(plan.matches(id, &ctx, &mut memo), expr.matches_ctx(&ctx));
            }
        }
    }
}
