//! Exact grouped timelines over an explicitly admitted analysis/source snapshot.
//! This module never resolves the mutable active Case. Admission owns revisions,
//! visibility and transforms; both SQL and event fallbacks receive that scope.
use crate::{analysis_context::Identity, model::Event, query::AnalyticsBudget, querylang};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

type Result<T> = std::result::Result<T, String>;
pub(crate) const DEFAULT_LIMIT: usize = 12;
pub(crate) const MAX_LIMIT: usize = 24;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Grid {
    pub start: i64,
    pub bucket_ms: i64,
    pub bucket_count: usize,
}
impl Grid {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.bucket_count > 240
            || self.bucket_ms < 0
            || (self.bucket_count == 0) != (self.bucket_ms == 0)
        {
            return Err("Grade da linha do tempo inválida: use até 240 faixas de largura positiva, ou uma grade vazia.".into());
        }
        Ok(())
    }
    pub(crate) fn end(&self) -> i128 {
        i128::from(self.start) + i128::from(self.bucket_ms) * self.bucket_count as i128
    }
    fn bucket(&self, timestamp: i64) -> Option<usize> {
        let offset = i128::from(timestamp) - i128::from(self.start);
        if self.bucket_ms == 0 || offset < 0 || i128::from(timestamp) >= self.end() {
            return None;
        }
        Some((offset / i128::from(self.bucket_ms)) as usize)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Context {
    pub analysis: Option<Identity>,
    pub source_generation: Option<u64>,
    pub case_key: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Spec {
    pub field: String,
    pub grid: Grid,
    pub limit: usize,
    pub context: Context,
}
impl Spec {
    pub(crate) fn new(
        field: String,
        grid: Grid,
        limit: Option<usize>,
        context: Context,
    ) -> Result<Self> {
        grid.validate()?;
        Ok(Self {
            field,
            grid,
            limit: limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
            context,
        })
    }
    pub(crate) fn validate(&self) -> Result<()> {
        self.grid.validate()?;
        if !(1..=MAX_LIMIT).contains(&self.limit) {
            return Err("Limite de séries temporais inválido.".into());
        }
        Ok(())
    }
    pub(crate) fn base_bytes(&self) -> usize {
        self.field
            .len()
            .saturating_mul(2)
            .saturating_add(
                self.grid
                    .bucket_count
                    .saturating_mul(3 * std::mem::size_of::<usize>()),
            )
            .saturating_add(1024)
    }
    pub(crate) fn key_bytes(&self, key: &str) -> usize {
        // Own full keys plus sort/SQL-literal working space and the exact grid.
        key.len()
            .saturating_mul(3)
            .saturating_add(
                self.grid
                    .bucket_count
                    .saturating_mul(std::mem::size_of::<usize>()),
            )
            .saturating_add(192)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Counts {
    pub count: usize,
    pub buckets: Vec<usize>,
}
impl Counts {
    pub(crate) fn new(buckets: usize) -> Self {
        Self {
            count: 0,
            buckets: vec![0; buckets],
        }
    }
    pub(crate) fn add(&mut self, bucket: usize, count: usize) -> Result<()> {
        let target = self
            .buckets
            .get_mut(bucket)
            .ok_or("Faixa temporal fora da grade.")?;
        *target = target
            .checked_add(count)
            .ok_or("Contagem temporal excedeu o limite.")?;
        self.count = self
            .count
            .checked_add(count)
            .ok_or("Contagem temporal excedeu o limite.")?;
        Ok(())
    }
    fn merge(&mut self, other: &Counts) -> Result<()> {
        if self.buckets.len() != other.buckets.len() {
            return Err("Grades temporais incompatíveis.".into());
        }
        for (bucket, count) in other.buckets.iter().enumerate() {
            self.add(bucket, *count)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Series {
    pub key: String,
    pub count: usize,
    pub buckets: Vec<usize>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Response {
    pub field: String,
    pub grid: Grid,
    pub total: Counts,
    pub series: Vec<Series>,
    pub other: Counts,
    pub missing: Counts,
    pub untimed: usize,
    pub outside_grid: usize,
    pub limit: usize,
    pub selection: String,
    pub context: Context,
}
impl Response {
    pub(crate) fn empty(spec: &Spec) -> Result<Self> {
        spec.validate()?;
        Ok(Self {
            field: spec.field.clone(),
            grid: spec.grid,
            total: Counts::new(spec.grid.bucket_count),
            series: Vec::new(),
            other: Counts::new(spec.grid.bucket_count),
            missing: Counts::new(spec.grid.bucket_count),
            untimed: 0,
            outside_grid: 0,
            limit: spec.limit,
            selection: "top".into(),
            context: spec.context.clone(),
        })
    }
}

/// The fallback retains exact complete keys only within the shared analytic
/// byte budget. A limit failure aborts the operation; finish never returns an
/// approximate or partially accumulated chart after a rejected row.
pub(crate) struct Accumulator {
    spec: Spec,
    field: querylang::FieldRef,
    result: Response,
    groups: HashMap<String, Counts>,
    budget: AnalyticsBudget,
    failure: Option<String>,
}
impl Accumulator {
    pub(crate) fn new(spec: &Spec) -> Result<Self> {
        let mut budget = AnalyticsBudget::new();
        budget.charge(spec.base_bytes())?;
        Ok(Self {
            spec: spec.clone(),
            field: querylang::field_ref(&spec.field),
            result: Response::empty(spec)?,
            groups: HashMap::new(),
            budget,
            failure: None,
        })
    }
    pub(crate) fn add_event(&mut self, event: &Event) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.add_event_inner(event, false);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    /// Compact indexed metadata reserves zero for missing time. Case/memory
    /// Events instead retain the full Option<i64> domain, including the epoch.
    pub(crate) fn add_indexed_event(&mut self, event: &Event) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.add_event_inner(event, true);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn add_event_inner(&mut self, event: &Event, indexed: bool) -> Result<()> {
        crate::operations::check()?;
        let Some(timestamp) = event.timestamp.filter(|&t| !indexed || t != 0) else {
            self.result.untimed = self
                .result
                .untimed
                .checked_add(1)
                .ok_or("Contagem temporal excedeu o limite.")?;
            return Ok(());
        };
        let Some(bucket) = self.spec.grid.bucket(timestamp) else {
            self.result.outside_grid = self
                .result
                .outside_grid
                .checked_add(1)
                .ok_or("Contagem temporal excedeu o limite.")?;
            return Ok(());
        };
        let context = querylang::Ctx::new(event);
        if let Some(key) = context.get(&self.field) {
            if let Some(counts) = self.groups.get_mut(key.as_ref()) {
                counts.add(bucket, 1)?;
            } else {
                self.budget.charge(self.spec.key_bytes(&key))?;
                let mut counts = Counts::new(self.spec.grid.bucket_count);
                counts.add(bucket, 1)?;
                self.groups.insert(key.into_owned(), counts);
            }
        } else {
            self.result.missing.add(bucket, 1)?;
        }
        self.result.total.add(bucket, 1)
    }
    pub(crate) fn finish(mut self) -> Result<Response> {
        crate::operations::check()?;
        if let Some(error) = self.failure {
            return Err(error);
        }
        let mut groups: Vec<_> = self.groups.into_iter().collect();
        groups.sort_unstable_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));
        for (index, (key, counts)) in groups.into_iter().enumerate() {
            if index % 1024 == 0 {
                crate::operations::check()?;
            }
            if index < self.spec.limit {
                self.result.series.push(Series {
                    key,
                    count: counts.count,
                    buckets: counts.buckets,
                });
            } else {
                self.result.other.merge(&counts)?;
            }
        }
        Ok(self.result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn spec(field: &str, limit: Option<usize>) -> Spec {
        Spec::new(
            field.into(),
            Grid {
                start: -10,
                bucket_ms: 10,
                bucket_count: 3,
            },
            limit,
            Context::default(),
        )
        .unwrap()
    }
    fn event(timestamp: Option<i64>, key: Option<Value>) -> Event {
        let mut event = Event::empty();
        event.timestamp = timestamp;
        if let Some(value) = key {
            event.fields.insert("group".into(), value);
        }
        event
    }
    fn collect(spec: &Spec, events: &[Event]) -> Response {
        let mut accumulator = Accumulator::new(spec).unwrap();
        for event in events {
            accumulator.add_indexed_event(event).unwrap();
        }
        accumulator.finish().unwrap()
    }
    fn reconciles(response: &Response) {
        assert_eq!(
            response.total.count,
            response.total.buckets.iter().sum::<usize>()
        );
        assert_eq!(
            response.other.count,
            response.other.buckets.iter().sum::<usize>()
        );
        assert_eq!(
            response.missing.count,
            response.missing.buckets.iter().sum::<usize>()
        );
        for series in &response.series {
            assert_eq!(series.count, series.buckets.iter().sum::<usize>());
        }
        for i in 0..response.grid.bucket_count {
            assert_eq!(
                response.total.buckets[i],
                response.other.buckets[i]
                    + response.missing.buckets[i]
                    + response.series.iter().map(|s| s.buckets[i]).sum::<usize>()
            );
        }
    }
    #[test]
    fn epoch_is_present_for_events_and_missing_only_for_compact_indexes() {
        let spec = spec("group", None);
        let events = [event(Some(0), Some(json!("epoch"))), event(None, None)];
        let mut generic = Accumulator::new(&spec).unwrap();
        for event in &events {
            generic.add_event(event).unwrap();
        }
        let generic = generic.finish().unwrap();
        assert_eq!((generic.total.count, generic.untimed), (1, 1));
        assert_eq!(generic.total.buckets, vec![0, 1, 0]);
        let indexed = collect(&spec, &events);
        assert_eq!((indexed.total.count, indexed.untimed), (0, 2));
        reconciles(&generic);
        reconciles(&indexed);
    }
    #[test]
    fn exact_top_groups_keep_blank_keys_missing_and_outside_counts_separate() {
        let events = vec![
            event(Some(-10), Some(json!(""))),
            event(Some(-1), Some(json!(" "))),
            event(Some(1), Some(json!("alpha"))),
            event(Some(9), Some(json!("alpha"))),
            event(Some(10), Some(json!("beta"))),
            event(Some(19), None),
            event(Some(20), Some(json!("outside"))),
            event(Some(-11), None),
            event(Some(0), Some(json!("sentinel"))),
            event(None, None),
        ];
        let response = collect(&spec("group", Some(2)), &events);
        assert_eq!(
            response
                .series
                .iter()
                .map(|s| (s.key.as_str(), s.count))
                .collect::<Vec<_>>(),
            vec![("alpha", 2), ("", 1)]
        );
        assert_eq!(response.total.buckets, vec![2, 2, 2]);
        assert_eq!(response.other.buckets, vec![1, 0, 1]);
        assert_eq!(response.missing.buckets, vec![0, 0, 1]);
        assert_eq!((response.untimed, response.outside_grid), (2, 2));
        reconciles(&response);
        let mut reversed = events;
        reversed.reverse();
        assert_eq!(collect(&spec("group", Some(2)), &reversed), response);
    }
    #[test]
    fn canonical_values_preserve_long_text_json_null_case_and_nested_lookup() {
        let long_a = format!("{}a", "x".repeat(400));
        let long_b = format!("{}b", "x".repeat(400));
        let mut events = vec![
            event(Some(1), Some(json!(long_a))),
            event(Some(1), Some(json!(long_b))),
            event(Some(1), Some(Value::Null)),
        ];
        let mut case = Event::empty();
        case.timestamp = Some(1);
        case.fields.insert("GROUP".into(), json!("case value"));
        events.push(case);
        let result = collect(&spec("group", None), &events);
        assert_eq!(result.series.len(), 4);
        assert!(result.series.iter().any(|s| s.key == "null"));
        assert!(result.series.iter().any(|s| s.key == long_a));
        assert!(result.series.iter().any(|s| s.key == long_b));
        let mut nested = Event::empty();
        nested.timestamp = Some(1);
        nested
            .fields
            .insert("payload".into(), json!({"items":[{"name":"child"}]}));
        assert_eq!(
            collect(&spec("payload.items.0.name", None), &[nested]).series[0].key,
            "child"
        );
        let mut alias = Event::empty();
        alias.timestamp = Some(1);
        alias.source = "api".into();
        assert_eq!(
            collect(&spec("origem", None), &[alias]).series[0].key,
            "api"
        );
        reconciles(&result);
    }
    #[test]
    fn grid_arithmetic_is_half_open_overflow_safe_and_empty_grid_is_explicit() {
        let grid = Grid {
            start: i64::MAX - 1,
            bucket_ms: i64::MAX,
            bucket_count: 2,
        };
        grid.validate().unwrap();
        assert_eq!(grid.bucket(i64::MAX), Some(0));
        assert_eq!(grid.bucket(i64::MIN), None);
        assert!(Grid {
            start: 0,
            bucket_ms: 0,
            bucket_count: 1
        }
        .validate()
        .is_err());
        assert!(Grid {
            start: 0,
            bucket_ms: 1,
            bucket_count: 241
        }
        .validate()
        .is_err());
        let spec = Spec::new(
            "group".into(),
            Grid {
                start: 0,
                bucket_ms: 0,
                bucket_count: 0,
            },
            None,
            Context::default(),
        )
        .unwrap();
        let result = collect(&spec, &[event(Some(1), None), event(None, None)]);
        assert_eq!(
            (result.total.count, result.untimed, result.outside_grid),
            (0, 1, 1)
        );
        assert!(result.total.buckets.is_empty());
    }
    #[test]
    fn budget_failure_cannot_be_finished_as_a_partial_chart() {
        crate::resources::with_analytics_limit(2048, || {
            let mut accumulator = Accumulator::new(&spec("group", None)).unwrap();
            accumulator
                .add_event(&event(Some(1), Some(json!("small"))))
                .unwrap();
            assert!(accumulator
                .add_event(&event(Some(1), Some(json!("x".repeat(1000)))))
                .unwrap_err()
                .contains("LOGINSIGHT_ANALYTICS_LIMIT_MB"));
            assert!(accumulator
                .finish()
                .unwrap_err()
                .contains("LOGINSIGHT_ANALYTICS_LIMIT_MB"));
        });
    }
    #[test]
    fn response_echoes_admitted_context_without_looking_up_active_state() {
        let mut spec = spec("group", Some(999));
        spec.context = Context {
            analysis: Some(Identity {
                case_id: "case-a".into(),
                analysis_id: "analysis-b".into(),
                config_revision: 7,
                visibility_revision: 9,
            }),
            source_generation: Some(42),
            case_key: Some("saved-c".into()),
        };
        let response = collect(&spec, &[event(Some(1), Some(json!("a")))]);
        assert_eq!(response.limit, 24);
        assert_eq!(response.context, spec.context);
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["context"]["analysis"]["configRevision"], 7);
        assert_eq!(value["context"]["sourceGeneration"], 42);
        assert_eq!(value["selection"], "top");
    }
}
