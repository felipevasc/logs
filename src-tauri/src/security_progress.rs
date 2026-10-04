//! Live verification receipts. Only accepted finding counts leave the worker;
//! incomplete evidence is never published as a completed analysis.
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Item {
    pub id: String,
    pub name: String,
    pub state: &'static str,
    pub findings: usize,
    pub counts_by_level: [usize; 5],
    pub applicable: Option<usize>,
    pub eligible: Option<usize>,
    pub coverage: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use serde_json::json;

    #[test]
    fn live_receipts_count_only_enabled_rules_and_accepted_findings() {
        let rule = |id: &str, condition: &str, enabled: bool| serde_json::from_value(json!({
            "id": id, "name": id, "severity": "medium", "kind": "single", "where": condition, "enabled": enabled,
            "evidence": { "level": 3, "maturity": "experimental" }
        })).unwrap();
        let rules = crate::detections::test_ruleset(vec![rule("hit", "message:needle", true), rule("none", "message:absent", true), rule("off", "message:needle", false)], vec![]).unwrap();
        let mut event = crate::model::Event::empty(); event.message = "needle".into(); event.event_ref = "progress:original".into();
        let settings = crate::detections::Settings { threats: false, investigation: crate::investigation::Settings { enabled: false, ..Default::default() }, ..Default::default() };
        let receipts = Arc::new(Mutex::new(Vec::new())); let capture = Arc::clone(&receipts);
        let token = crate::operations::token(Some("triage-progress-test".into())).unwrap();
        let result = crate::operations::run_with_token(token, || crate::operations::with_reporter(Arc::new(move |event| capture.lock().unwrap().push(event)), ||
            crate::detections::run_stored(&crate::detections::Inputs { rules: &rules, catalog: None, settings: &settings }, &crate::detections::Source::Events(vec![&event])))).unwrap().unwrap();
        let receipts = receipts.lock().unwrap();
        assert!(receipts.iter().all(|r| r.operation_id.as_deref() == Some("triage-progress-test")));
        let first = receipts.iter().find_map(|r| r.triage.as_ref()).unwrap();
        assert_eq!(first.total, 2); assert_eq!(first.completed, 0); assert!(first.reset);
        let last = receipts.iter().rev().find_map(|r| r.triage.as_ref()).unwrap();
        assert_eq!(last.completed, 2); assert_eq!(last.total, 2); assert_eq!(last.findings, 1);
        assert_eq!(last.findings, result.page(1, 0, 20, None, None).unwrap()["detections"].as_array().unwrap().len());
        assert!(last.items.iter().all(|item| item.state == "completed"));
        assert!(last.items.iter().any(|item| item.id == "none" && item.findings == 0));
        assert!(!last.items.iter().any(|item| item.id == "off"));
        let detection = crate::detections::run(&crate::detections::Inputs { rules: &rules, catalog: None, settings: &settings }, &crate::detections::Source::Events(vec![&event])).unwrap().detections.into_iter().next().unwrap();
        let mut writer = crate::security_results::Writer::new().unwrap();
        assert!(writer.push(&detection).unwrap());
        assert!(!writer.push(&detection).unwrap(), "a repeated identity must not increment live counts");
    }

    #[test]
    fn tracker_emits_deltas_and_leaves_unfinished_rules_pending() {
        let mut tracker = Tracker { items: vec![Some(Tracker::item("a", "A")), None, Some(Tracker::item("b", "B")), Some(Tracker::item("c", "C"))], dirty: BTreeSet::new(), cursor: 0, revision: 1, completed: 0, findings: 0, total: 3, last: Instant::now() };
        let receipts = Arc::new(Mutex::new(Vec::new())); let capture = Arc::clone(&receipts);
        crate::operations::with_reporter(Arc::new(move |event| capture.lock().unwrap().push(event)), || {
            tracker.start(0); tracker.finding(0, 4); tracker.start(2);
        });
        assert_eq!(tracker.completed, 1); assert_eq!(tracker.items[0].as_ref().unwrap().state, "completed");
        assert_eq!(tracker.items[2].as_ref().unwrap().state, "running");
        assert_eq!(tracker.items[3].as_ref().unwrap().state, "pending");
        assert_eq!(tracker.items[0].as_ref().unwrap().counts_by_level, [0, 0, 0, 1, 0]);
        let receipts = receipts.lock().unwrap();
        let last = receipts.last().unwrap().triage.as_ref().unwrap();
        assert_eq!(last.current.as_deref(), Some("b"));
        assert_eq!(last.completed, 1); assert_eq!(last.findings, 1);
        assert_eq!(last.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    }
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub reset: bool,
    pub revision: usize,
    pub completed: usize,
    pub total: usize,
    pub findings: usize,
    pub current: Option<String>,
    pub items: Vec<Item>,
}
pub(crate) struct Tracker {
    items: Vec<Option<Item>>,
    dirty: BTreeSet<usize>,
    cursor: usize,
    revision: usize,
    completed: usize,
    findings: usize,
    total: usize,
    last: Instant,
}
impl Tracker {
    pub fn new(rules: &crate::detections::RuleSet, categories: &[String]) -> Self {
        let items: Vec<_> = rules.rules.iter().map(|rule| rule.enabled.then(|| Self::item(&rule.def.id, &rule.def.name)))
            .chain(categories.iter().map(|name| Some(Self::item(&format!("threat:{name}"), name)))).collect();
        let total = items.iter().flatten().count();
        let mut tracker = Self { dirty: (0..items.len()).collect(), items, cursor: 0, revision: 0, completed: 0, findings: 0, total, last: Instant::now() };
        tracker.emit("triage-scan", "Lendo registros para as verificações", true);
        tracker
    }
    fn item(id: &str, name: &str) -> Item {
        Item { id: id.into(), name: name.into(), state: "pending", findings: 0, counts_by_level: [0; 5], applicable: None, eligible: None, coverage: None }
    }
    pub fn coverage(&mut self, index: usize, applicable: usize, eligible: usize, coverage: &'static str) {
        if let Some(Some(item)) = self.items.get_mut(index) {
            item.applicable = Some(applicable); item.eligible = Some(eligible); item.coverage = Some(coverage);
            self.dirty.insert(index);
        }
    }
    pub fn start(&mut self, index: usize) {
        // Raw findings are ordered by rule, including the expanded chains.
        debug_assert!(index >= self.cursor);
        self.advance(index);
        let mut changed = false;
        if let Some(Some(item)) = self.items.get_mut(index) {
            if item.state != "running" { item.state = "running"; self.dirty.insert(index); changed = true; }
        }
        self.emit("triage-verify", "Verificando os indícios e suas evidências", changed);
    }
    fn advance(&mut self, next: usize) {
        while self.cursor < next {
            if let Some(item) = self.items[self.cursor].as_mut() {
                item.state = "completed"; self.completed += 1; self.dirty.insert(self.cursor);
            }
            self.cursor += 1;
        }
    }
    pub fn finding(&mut self, index: usize, level: u8) {
        if let Some(Some(item)) = self.items.get_mut(index) {
            item.findings += 1; self.findings += 1;
            if (1..=5).contains(&level) { item.counts_by_level[level as usize - 1] += 1; }
            self.dirty.insert(index);
        }
        self.emit("triage-verify", "Verificando os indícios e suas evidências", false);
    }
    pub fn finish(&mut self) {
        self.advance(self.items.len());
        self.emit("triage-finalize", "Organizando os resultados verificados", true);
    }
    fn emit(&mut self, phase_id: &'static str, phase: &'static str, force: bool) {
        if !force && self.last.elapsed() < Duration::from_millis(120) { return; }
        self.revision += 1;
        let snapshot = Snapshot {
            reset: self.revision == 1, revision: self.revision, completed: self.completed, total: self.total, findings: self.findings,
            current: self.items.get(self.cursor).and_then(Option::as_ref).filter(|item| item.state == "running").map(|item| item.id.clone()),
            items: std::mem::take(&mut self.dirty).into_iter().filter_map(|i| self.items[i].clone()).collect(),
        };
        crate::operations::report_triage(phase_id, phase, snapshot);
        self.last = Instant::now();
    }
}
