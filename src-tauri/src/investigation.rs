//! Exact population analytics on compact columnar facts. Statistical signals
//! remain hypotheses; no amount of rarity establishes execution or compromise.
use crate::{
    entities::{self, Role},
    model::Event,
    security_normalize::Normalized,
};
use duckdb::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const VERSION: &str = "investigation-1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub baseline_days: u32,
    pub minimum_history: u32,
    pub rare_max: u32,
    pub profile: Option<String>,
    pub profile_revision: Option<String>,
    pub business: Vec<BusinessPolicy>,
    pub telemetry: Vec<TelemetryPolicy>,
    pub ioc: Option<crate::security_ioc::Catalog>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BusinessPolicy {
    pub id: String,
    pub namespace: String,
    pub source: String,
    pub action: String,
    pub currency: String,
    pub window_ms: u32,
    pub minimum_count: u32,
    pub minimum_amount: Option<f64>,
    #[serde(default)]
    pub account_change_action: Option<String>,
    #[serde(default)]
    pub account_change_source: Option<String>,
    #[serde(default)]
    pub authentication_source: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryPolicy {
    pub source: String,
    pub namespace: String,
    pub maximum_gap_ms: u32,
    pub sequence: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fact(
        index: usize,
        ts: i64,
        tenant: &str,
        user: &str,
        host: &str,
        src: &str,
        dst: &str,
        outcome: &str,
    ) -> (Event, Normalized) {
        let mut e = Event::empty();
        e.id = index;
        e.event_ref = format!("fixture:{index}");
        e.timestamp = Some(ts);
        e.source = "fixture".into();
        e.fields=json!({"_sec.namespace":tenant,"user.name":user,"host.name":host,"source.ip":src,"destination.ip":dst}).as_object().unwrap().clone();
        let mut n = Normalized::default();
        n.time.epoch_ms = e.timestamp;
        n.put("action", "fixture", "logon".into(), "adapter");
        n.put("outcome", "fixture", outcome.into(), "adapter");
        (e, n)
    }
    fn count(p: &Population, kind: &str) -> i64 {
        p.db.query_row("SELECT count(*) FROM signals WHERE kind=?", [kind], |r| {
            r.get(0)
        })
        .unwrap()
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_business_policies_keep_currency_session_order_and_original_support() {
        let settings:Settings=serde_json::from_value(json!({"business":[{"id":"payments","namespace":"tenant","source":"payments","action":"transfer","currency":"BRL","window_ms":300000,"minimum_count":2,"minimum_amount":100,"account_change_action":"password_change","account_change_source":"accounts","authentication_source":"identity"}]})).unwrap();
        for variant in ["positive", "currency", "session", "reverse", "namespace"] {
            let mut p = Population::new(&settings).unwrap();
            for id in 0..4 {
                let (mut e, mut n) = fact(
                    id,
                    1700000000000 + id as i64 * 1000,
                    if variant == "namespace" && id == 3 {
                        "other"
                    } else {
                        "tenant"
                    },
                    "alice",
                    "",
                    "",
                    "",
                    "success",
                );
                e.source = match id {
                    0 => "accounts",
                    1 => "identity",
                    _ => "payments",
                }
                .into();
                n.values.remove("action");
                n.put(
                    "action",
                    "action",
                    match id {
                        0 => "password_change",
                        1 => "logon",
                        _ => "transfer",
                    }
                    .into(),
                    "mapping",
                );
                n.put(
                    "session",
                    "session",
                    if variant == "session" && id >= 2 {
                        "different-session"
                    } else {
                        "session-1"
                    }
                    .into(),
                    "mapping",
                );
                if id >= 2 {
                    n.put(
                        "currency",
                        "currency",
                        if variant == "currency" { "USD" } else { "BRL" }.into(),
                        "mapping",
                    );
                    n.numbers.insert(
                        "amount".into(),
                        crate::security_normalize::NumberProvenance {
                            field: "amount".into(),
                            original: "75".into(),
                            original_unit: "BRL".into(),
                            value: 75.0,
                            unit: "BRL".into(),
                            method: "mapping".into(),
                        },
                    );
                }
                if variant == "reverse" && id == 1 {
                    e.timestamp = Some(1699999999000);
                    n.time.epoch_ms = e.timestamp;
                }
                p.observe(&e, &n).unwrap();
            }
            p.finish().unwrap();
            assert_eq!(
                count(&p, "business_velocity:payments"),
                if matches!(variant, "currency" | "namespace") {
                    0
                } else {
                    1
                },
                "{variant}"
            );
            assert_eq!(
                count(&p, "account_takeover_chain:payments"),
                if variant == "positive" || variant == "namespace" {
                    1
                } else {
                    0
                },
                "{variant}"
            );
            if variant == "positive" {
                let members:i64=p.db.query_row("SELECT count(*) FROM signal_members m JOIN signals s USING(id) WHERE kind='account_takeover_chain:payments'",[],|r|r.get(0)).unwrap();
                assert_eq!(members, 4);
            }
        }
    }
    #[test]
    fn novelty_uses_complete_history_and_keeps_exact_support_in_the_same_namespace() {
        let mut p = Population::new(&Settings {
            baseline_days: 1,
            minimum_history: 5,
            ..Default::default()
        })
        .unwrap();
        for i in 0..30 {
            let (e, n) = fact(
                i,
                1_700_000_000_000 + i as i64 * 1000,
                "tenant-a",
                "alice",
                "old",
                "",
                "",
                "success",
            );
            p.observe(&e, &n).unwrap();
        }
        let (e, n) = fact(
            30,
            1_700_000_000_000 + 2 * 86_400_000,
            "tenant-a",
            "alice",
            "new",
            "",
            "",
            "success",
        );
        p.observe(&e, &n).unwrap();
        let (e, n) = fact(
            31,
            1_700_000_000_000 + 2 * 86_400_000,
            "tenant-b",
            "alice",
            "new",
            "",
            "",
            "success",
        );
        p.observe(&e, &n).unwrap();
        let summary = p.finish().unwrap();
        assert_eq!(summary["population"], 32);
        assert_eq!(count(&p, "new_relationship"), 1);
        let refs:String=p.db.query_row("SELECT string_agg(m.ref,',') FROM signal_members m JOIN signals s USING(id) WHERE s.kind='new_relationship'",[],|r|r.get(0)).unwrap();
        assert_eq!(refs, "fixture:30");
    }
    #[test]
    fn a_single_late_rare_event_after_the_old_sample_cap_is_observed() {
        let mut p = Population::new(&Settings {
            baseline_days: 1,
            minimum_history: 5,
            ..Default::default()
        })
        .unwrap();
        for i in 0..6501 {
            let (e, n) = fact(
                i,
                1_700_000_000_000 + i as i64 * 1000,
                "tenant",
                "alice",
                "old",
                "",
                "",
                "success",
            );
            p.observe(&e, &n).unwrap();
        }
        let (e, n) = fact(
            6501,
            1_700_000_000_000 + 2 * 86_400_000,
            "tenant",
            "alice",
            "unique",
            "",
            "",
            "success",
        );
        p.observe(&e, &n).unwrap();
        assert_eq!(p.finish().unwrap()["population"], 6502);
        assert_eq!(count(&p, "new_relationship"), 1);
    }
    #[test]
    fn sliding_spraying_crosses_calendar_boundaries_but_single_account_failures_do_not_match() {
        let mut p = Population::new(&Settings::default()).unwrap();
        for i in 0..20 {
            let (e, n) = fact(
                i,
                1_700_006_390_000 + i as i64 * 1000,
                "tenant",
                &format!("user-{i}"),
                "",
                "198.51.100.1",
                "",
                "failure",
            );
            p.observe(&e, &n).unwrap();
        }
        for i in 20..120 {
            let (e, n) = fact(
                i,
                1_700_006_390_000 + i as i64 * 1000,
                "tenant",
                "expired-service-account",
                "",
                "198.51.100.2",
                "",
                "failure",
            );
            p.observe(&e, &n).unwrap();
        }
        p.finish().unwrap();
        assert_eq!(count(&p, "password_spraying"), 1);
        let entity: String =
            p.db.query_row(
                "SELECT entity FROM signals WHERE kind='password_spraying'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entity, "198.51.100.1");
    }
    #[test]
    fn late_arrival_and_reverse_order_produce_the_same_signal_id_and_never_promote_evidence() {
        let input = (0..16)
            .map(|i| {
                fact(
                    i,
                    1_700_000_000_000 + i as i64 * 45_000,
                    "tenant",
                    "",
                    "",
                    "192.0.2.1",
                    "203.0.113.1",
                    "success",
                )
            })
            .collect::<Vec<_>>();
        let mut a = Population::new(&Settings::default()).unwrap();
        let mut b = Population::new(&Settings::default()).unwrap();
        for (e, n) in &input {
            a.observe(e, n).unwrap();
        }
        for (e, n) in input.iter().rev() {
            b.observe(e, n).unwrap();
        }
        a.finish().unwrap();
        b.finish().unwrap();
        assert_eq!(count(&a, "beaconing"), 1);
        let payload = |p: &Population| {
            p.db.query_row(
                "SELECT payload FROM signals WHERE kind='beaconing'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        };
        assert_eq!(payload(&a), payload(&b));
        assert_eq!(
            serde_json::from_str::<Value>(&payload(&a)).unwrap()["evidence_level"],
            0
        );
    }
    #[test]
    fn unknown_time_is_reported_and_disabled_analysis_is_not_complete() {
        let mut p = Population::new(&Settings::default()).unwrap();
        let (mut e, n) = fact(1, 1, "tenant", "user", "host", "", "", "success");
        e.timestamp = None;
        p.observe(&e, &n).unwrap();
        assert_eq!(p.finish().unwrap()["timed"], 0);
        assert_eq!(count(&p, "telemetry_parse_quality"), 1);
        let mut p = Population::new(&Settings {
            enabled: false,
            ..Default::default()
        })
        .unwrap();
        p.observe(&e, &n).unwrap();
        assert_eq!(p.finish().unwrap()["status"], "not_evaluated");
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            baseline_days: 7,
            minimum_history: 20,
            rare_max: 3,
            profile: None,
            profile_revision: None,
            business: vec![],
            telemetry: vec![],
            ioc: None,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=365).contains(&self.baseline_days)
            || !(5..=100_000).contains(&self.minimum_history)
            || !(1..=100).contains(&self.rare_max)
        {
            return Err("Histórico: período, suporte ou raridade fora dos limites.".into());
        }
        if self
            .profile
            .as_ref()
            .is_some_and(|p| p.trim().is_empty() || p.len() > 128)
        {
            return Err("O perfil do ambiente exige um nome com até 128 bytes.".into());
        }
        if self.profile_revision.is_some() && self.profile.is_none() {
            return Err("A revisão histórica exige selecionar o perfil no Caso".into());
        }
        if self.profile_revision.as_ref().is_some_and(|r| {
            r.is_empty()
                || r.len() > 128
                || !r.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }) {
            return Err("Revisão do perfil histórico inválida".into());
        }
        if self.business.len() > 64 || self.telemetry.len() > 256 {
            return Err("Políticas excedem 64 modelos de negócio ou 256 fontes".into());
        }
        let mut ids = std::collections::HashSet::new();
        for policy in &self.business {
            if !ids.insert(&policy.id)
                || [
                    &policy.id,
                    &policy.namespace,
                    &policy.source,
                    &policy.action,
                    &policy.currency,
                ]
                .iter()
                .any(|s| s.is_empty() || s.len() > 256)
                || !(1..=86_400_000).contains(&policy.window_ms)
                || policy.minimum_count == 0
                || policy
                    .minimum_amount
                    .is_some_and(|v| !v.is_finite() || v <= 0.0)
            {
                return Err("Política de negócio exige ID único, fonte, namespace, ação, moeda e limiares explícitos".into());
            }
            if [
                &policy.account_change_action,
                &policy.account_change_source,
                &policy.authentication_source,
            ]
            .iter()
            .any(|v| {
                v.as_ref()
                    .is_some_and(|v| v.trim().is_empty() || v.len() > 256)
            }) {
                return Err(
                    "Ação e fontes de tomada de conta devem ser explícitas e limitadas a 256 bytes"
                        .into(),
                );
            }
        }
        for policy in &self.telemetry {
            if policy.source.is_empty() || policy.namespace.is_empty() || policy.maximum_gap_ms == 0
            {
                return Err(
                    "Política de telemetria exige fonte, namespace e intervalo esperado".into(),
                );
            }
        }
        if let Some(ioc) = &self.ioc {
            ioc.validate()?;
        }
        Ok(())
    }
}

pub struct Population {
    _watch: Option<crate::security_budget::DuckWatch>,
    pub db: Connection,
    _disk: crate::security_budget::DiskLease,
    _directory: tempfile::TempDir,
    pending: Vec<Vec<Value>>,
    pending_bytes: usize,
    pub settings: Settings,
    pub count: usize,
    ioc: Option<crate::security_ioc::Index>,
    history: Value,
    sketches: crate::security_sketches::Summaries,
    schema: crate::security_schema::Schema,
}
fn error(e: impl std::fmt::Display) -> String {
    crate::security_budget::failure()
        .unwrap_or_else(|| format!("Investigação da população completa: {e}"))
}
fn role(event: &Event, role: Role) -> String {
    entities::value(event, role)
        .map(|v| v.into_owned())
        .unwrap_or_default()
}
fn number(event: &Event, paths: &[&str]) -> Option<f64> {
    paths
        .iter()
        .find_map(|p| {
            crate::security_normalize::field_text(event, p)?
                .parse::<f64>()
                .ok()
        })
        .filter(|n| n.is_finite())
}
pub fn entropy(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &byte in text.as_bytes() {
        counts[byte as usize] += 1;
    }
    counts
        .iter()
        .filter(|&&n| n > 0)
        .map(|&n| {
            let p = n as f64 / text.len() as f64;
            -p * p.log2()
        })
        .sum()
}
fn template(text: &str) -> String {
    // An explicitly named deterministic template, not a claimed learned Drain model.
    text.split_whitespace()
        .take(128)
        .map(|word| {
            if word.parse::<f64>().is_ok()
                || word.parse::<std::net::IpAddr>().is_ok()
                || (word.len() >= 16 && word.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
            {
                "<value>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Population {
    pub fn schema_rows(&self) -> Value {
        self.schema.describe()
    }
    pub fn new(settings: &Settings) -> Result<Self, String> {
        settings.validate()?;
        let directory = tempfile::tempdir().map_err(error)?;
        let db = Connection::open(directory.path().join("facts.duckdb")).map_err(error)?;
        // One admitted worker: this connection must never multiply the global CPU limit.
        let spill = crate::resources::spill_bytes(false, directory.path())?;
        db.execute_batch(&format!("SET threads=1;SET preserve_insertion_order=false; SET memory_limit='{}MB'; SET max_temp_directory_size='{spill}B';", crate::resources::duckdb_memory_mb().clamp(32,64))).map_err(error)?;
        db.execute_batch("CREATE TABLE facts(ref VARCHAR,ns VARCHAR,ts BIGINT,eid BIGINT,source VARCHAR,
            actor VARCHAR,host VARCHAR,src VARCHAR,dst VARCHAR,process VARCHAR,parent VARCHAR,session VARCHAR,
            resource VARCHAR,action VARCHAR,outcome VARCHAR,bytes DOUBLE,amount DOUBLE,currency VARCHAR,
            template VARCHAR,domain VARCHAR,entropy DOUBLE,code VARCHAR,parse_ok BOOLEAN,time_ok BOOLEAN,
            user_agent VARCHAR,metadata VARCHAR,latency DOUBLE);
            CREATE TABLE signals(id VARCHAR PRIMARY KEY,kind VARCHAR,ns VARCHAR,col VARCHAR,entity VARCHAR,
                priority INTEGER,first BIGINT,last BIGINT,payload VARCHAR);
            CREATE TABLE signal_members(id VARCHAR,ref VARCHAR,eid BIGINT,ts BIGINT);
            CREATE TABLE graph(ns VARCHAR,left_kind VARCHAR,left_value VARCHAR,right_kind VARCHAR,right_value VARCHAR,
                relation VARCHAR,quality VARCHAR,ref VARCHAR,eid BIGINT,ts BIGINT);
            CREATE TABLE supports(kind VARCHAR,ns VARCHAR,entity VARCHAR,ref VARCHAR,eid BIGINT,ts BIGINT);").map_err(error)?;
        db.execute_batch("CREATE TABLE ioc_matches(indicator VARCHAR,ns VARCHAR,source VARCHAR,ref VARCHAR,eid BIGINT,ts BIGINT,payload VARCHAR);").map_err(error)?;
        let ioc = settings
            .ioc
            .as_ref()
            .map(crate::security_ioc::Index::new)
            .transpose()?;
        crate::engine::udf::register(&db).map_err(error)?;
        let watch = crate::security_budget::DuckWatch::new(&db)?;
        Ok(Self {
            _watch: watch,
            db,
            _disk: crate::security_budget::DiskLease::register(directory.path(), true),
            _directory: directory,
            pending: Vec::new(),
            pending_bytes: 0,
            settings: settings.clone(),
            count: 0,
            ioc,
            history: Value::Null,
            sketches: crate::security_sketches::Summaries::new(),
            schema: Default::default(),
        })
    }

    pub fn observe(&mut self, event: &Event, n: &Normalized) -> Result<(), String> {
        if !self.settings.enabled {
            return Ok(());
        }
        crate::operations::check()?;
        let namespace = crate::security_normalize::field_text(event, "_sec.namespace")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("source:{}", event.source));
        let domain = role(event, Role::Domain);
        self.schema.observe(event, &namespace);
        let actor = role(event, Role::User);
        let destination = role(event, Role::DstIp);
        if !actor.is_empty() {
            self.sketches
                .actors
                .insert(&json!([namespace, actor]).to_string());
        }
        if !destination.is_empty() {
            self.sketches
                .destinations
                .insert(&json!([namespace, destination]).to_string());
        }
        if let Some(number) = n.numbers.get("latency") {
            self.sketches.latency.insert(number.value);
        }
        if let Some(number) = n.numbers.get("bytes") {
            self.sketches.bytes.insert(number.value);
        }
        if !event.code.is_empty() {
            self.sketches.codes.insert(&event.code);
            self.sketches.frequent_codes.insert(&event.code);
        }
        if let Some(index) = &self.ioc {
            for (indicator, value, field, time_known) in index.matches(event) {
                let pack = self.settings.ioc.as_ref().unwrap();
                let payload = json!({"indicator":indicator,"exact_value":value,"field":field,"time_known":time_known,"catalog":pack.id,"revision":pack.revision,"source":pack.source,"confirmation":"exact"});
                self.db
                    .execute(
                        "INSERT INTO ioc_matches VALUES(?,?,?,?,?,?,?)",
                        params![
                            indicator.id,
                            namespace,
                            event.source,
                            event.event_ref,
                            event.id as i64,
                            event.timestamp,
                            payload.to_string()
                        ],
                    )
                    .map_err(error)?;
            }
        }
        let metadata = json!({"product":n.product,"time":n.time,"limitations":n.limitations,
            "conflicts":n.conflicts,"content_clipped":n.content_clipped,
            "provenance":n.values,
            "numbers":n.numbers,"service":n.get("service"),"peer_group":n.get("peer_group"),
            "transformations":n.transformations,
            "flags":{"suspicious_command":crate::security_normalize::field_text(event,"_sec.suspicious_command").as_deref()==Some("true") && !n.get("command").is_some_and(crate::security_normalize::literal_output),
                "suspicious_request":crate::security_normalize::field_text(event,"_sec.suspicious_request").as_deref()==Some("true")||n.signals.iter().any(|s|s.key.contains("request")&&!s.key.contains("ambiguous")),
                "documentation":crate::security_normalize::field_text(event,"event.kind").as_deref()==Some("documentation")},
            "semantic":n.values.iter().map(|(key,p)|(key.clone(),p.value.clone())).collect::<std::collections::BTreeMap<_,_>>(),
            "command":n.get("command"),"process_key":n.get("process"),"parent_key":n.get("parent"),
            "credential":n.get("credential"),"artifact":n.get("artifact"),
            "url":role(event,Role::Url),"record_id":crate::security_normalize::field_text(event,"EventRecordID")
                .or_else(||crate::security_normalize::field_text(event,"winlog.record_id"))});
        let row = vec![
            json!(event.event_ref),
            json!(namespace),
            json!(event.timestamp),
            json!(event.id),
            json!(event.source),
            json!(role(event, Role::User)),
            json!(role(event, Role::Host)),
            json!(role(event, Role::SrcIp)),
            json!(role(event, Role::DstIp)),
            json!(role(event, Role::Process)),
            json!(role(event, Role::ParentProcess)),
            json!(n.get("session").unwrap_or("")),
            json!(n.get("resource").or_else(|| n.get("file")).unwrap_or("")),
            json!(n.get("action").unwrap_or("")),
            json!(n.get("outcome").unwrap_or("unknown")),
            json!(n.numbers.get("bytes").map(|v| v.value)),
            json!(n.numbers.get("amount").map(|v| v.value)),
            json!(n
                .get("currency")
                .map(str::to_string)
                .or_else(|| crate::security_normalize::field_text(event, "transaction.currency"))
                .or_else(|| crate::security_normalize::field_text(event, "currency"))
                .unwrap_or_default()),
            json!(template(&event.message)),
            json!(domain),
            json!(entropy(domain.split('.').next().unwrap_or(""))),
            json!(event.code),
            json!(event.parse_status == "parsed"),
            json!(n.time.ambiguity.is_none() && event.timestamp.is_some()),
            json!(role(event, Role::UserAgent)),
            json!(metadata.to_string()),
            json!(n.numbers.get("latency").map(|v| v.value)),
        ];
        self.pending_bytes += row.iter().map(|v| v.to_string().len()).sum::<usize>();
        self.pending.push(row);
        self.count += 1;
        if self.pending.len() >= 1024 || self.pending_bytes >= crate::resources::batch_bytes() {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut appender = self.db.appender("facts").map_err(error)?;
        for row in self.pending.drain(..) {
            crate::operations::check()?;
            appender
                .append_row(params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_i64(),
                    row[3].as_i64(),
                    row[4].as_str(),
                    row[5].as_str(),
                    row[6].as_str(),
                    row[7].as_str(),
                    row[8].as_str(),
                    row[9].as_str(),
                    row[10].as_str(),
                    row[11].as_str(),
                    row[12].as_str(),
                    row[13].as_str(),
                    row[14].as_str(),
                    row[15].as_f64(),
                    row[16].as_f64(),
                    row[17].as_str(),
                    row[18].as_str(),
                    row[19].as_str(),
                    row[20].as_f64(),
                    row[21].as_str(),
                    row[22].as_bool(),
                    row[23].as_bool(),
                    row[24].as_str(),
                    row[25].as_str(),
                    row[26].as_f64()
                ])
                .map_err(error)?;
        }
        appender.flush().map_err(error)?;
        drop(appender);
        self.pending_bytes = 0;
        if self.count % 8192 == 0 {
            self.db.execute_batch("CHECKPOINT").map_err(error)?;
        }
        Ok(())
    }

    fn signal(
        &self,
        kind: &str,
        query: &str,
        column: &str,
        description: &str,
        priority: i32,
    ) -> Result<(), String> {
        // Each query returns scope, entity, start, end, metrics. All memberships
        // are computed on disk and remain accessible through paginated endpoints.
        crate::security_budget::phase("population signals");
        let signal_error = |e: duckdb::Error| error(format!("sinal {kind}: {e}"));
        // Materialize into the bounded on-disk database, never a client-sized
        // result vector. A separate reader keeps writes from invalidating the
        // live Arrow stream and can observe the durable candidate table.
        self.db
            .execute_batch(&format!(
                "DROP TABLE IF EXISTS signal_candidates;CREATE TABLE signal_candidates AS {query}"
            ))
            .map_err(signal_error)?;
        let reader = self.db.try_clone().map_err(signal_error)?;
        let _watch = crate::security_budget::DuckWatch::new(&reader)?;
        let mut stmt = reader
            .prepare("SELECT * FROM signal_candidates")
            .map_err(signal_error)?;
        let mut rows = crate::security_duck_stream::Rows::new(&mut stmt).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let namespace: String = row.get(0).map_err(error)?;
            let entity: String = row.get(1).map_err(error)?;
            let first: Option<i64> = row.get(2).map_err(error)?;
            let last: Option<i64> = row.get(3).map_err(error)?;
            let metrics: String = row.get(4).map_err(error)?;
            let measurements = serde_json::from_str::<Value>(&metrics).map_err(error)?;
            let id = crate::evidence::stable_id(
                VERSION,
                [
                    kind.to_string(),
                    namespace.clone(),
                    column.to_string(),
                    entity.clone(),
                    format!("{first:?}:{last:?}"),
                ],
            );
            let payload = json!({"id":id,"kind":kind,"namespace":namespace,"column":column,"value":entity,
                "priority":priority,"evidence_level":0,"claim":"activity","outcome":"unknown",
                "explanation":description,"measurements":measurements,
                "first":first,"last":last,"version":VERSION,"evaluation":"unassessed",
                "benign_alternatives":["Mudança legítima, manutenção, automação ou particularidade do ambiente"],
                "missing_evidence":["Corroboração da hipótese e validação em corpus representativo"]});
            self.db
                .execute(
                    "INSERT OR REPLACE INTO signals VALUES(?,?,?,?,?,?,?,?,?)",
                    params![
                        id,
                        kind,
                        namespace,
                        column,
                        entity,
                        priority,
                        first,
                        last,
                        payload.to_string()
                    ],
                )
                .map_err(error)?;
            let field = match column {
                "@user" => "actor",
                "@src_ip" => "src",
                "@host" => "host",
                "@process" => "process",
                "@session" => "session",
                "source" => "source",
                _ => return Err("Coluna de sinal inválida".into()),
            };
            let cutoff = self
                .db
                .query_row(
                    "SELECT min(ts) FROM facts WHERE time_ok AND ns=?",
                    [&namespace],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .map_err(error)?
                .and_then(|t| t.checked_add(i64::from(self.settings.baseline_days) * 86_400_000))
                .unwrap_or(i64::MAX);
            let support = match kind {
                "new_relationship" => format!("AND ref IN (SELECT r.ref FROM relations r WHERE r.ns=f.ns AND r.entity=f.actor AND r.col='@user' AND r.ts>={cutoff} AND NOT EXISTS(SELECT 1 FROM relations old WHERE old.ns=r.ns AND old.entity=r.entity AND old.kind=r.kind AND old.target=r.target AND old.ts<{cutoff}))"),
                "contextual_rarity" => format!("AND ref IN (SELECT r.ref FROM relations r WHERE r.ns=f.ns AND r.entity=f.actor AND r.col='@user' AND (SELECT count(*) FROM relations p WHERE p.ns=r.ns AND p.entity=r.entity AND p.kind=r.kind AND p.target=r.target)<={})",self.settings.rare_max),
                "password_spraying" => "AND action='logon' AND outcome='failure' AND actor<>'' AND time_ok".into(),
                "volume_change" => "AND bytes IS NOT NULL AND time_ok AND EXISTS(SELECT 1 FROM volume_deviations v WHERE v.ns=f.ns AND v.actor=f.actor AND v.hour=cast(floor(f.ts/3600000)*3600000 AS BIGINT))".into(),
                "beaconing" => "AND time_ok AND EXISTS(SELECT 1 FROM periodic_destinations p WHERE p.ns=f.ns AND p.src=f.src AND p.dst=f.dst)".into(),
                "dns_tunneling_context" => "AND domain<>'' AND length(split_part(domain,'.',1))>=24 AND entropy>=3.5 AND time_ok".into(),
                "process_parent_rarity" => "AND parent<>'' AND time_ok AND (SELECT count(*) FROM facts p WHERE p.ns=f.ns AND p.process=f.process AND p.parent=f.parent)<=3".into(),
                "telemetry_parse_quality" => "AND (NOT parse_ok OR NOT time_ok)".into(),
                _ => format!("AND ref IN (SELECT ref FROM supports s WHERE s.kind='{}' AND s.ns=f.ns AND s.entity='{}')",kind.replace('\'',"''"),entity.replace('\'',"''")),
            };
            let (support_first, support_last) = if kind == "telemetry_parse_quality" {
                (None, None)
            } else {
                (first, last)
            };
            let entity_filter = if matches!(
                kind,
                "new_relationship"
                    | "contextual_rarity"
                    | "password_spraying"
                    | "session_context_change"
                    | "volume_change"
                    | "beaconing"
                    | "dns_tunneling_context"
                    | "process_parent_rarity"
                    | "telemetry_parse_quality"
            ) {
                format!("{field}=?")
            } else {
                "? IS NOT NULL".into()
            };
            self.db.execute(&format!("INSERT INTO signal_members SELECT ?,ref,eid,ts FROM facts f WHERE ns=? AND {entity_filter} AND (? IS NULL OR ts IS NULL OR ts>=?) AND (? IS NULL OR ts IS NULL OR ts<=?) {support}"),
                params![id,namespace,entity,support_first,support_first,support_last,support_last]).map_err(error)?;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<Value, String> {
        self.flush()?;
        if !self.settings.enabled {
            return Ok(json!({"enabled":false,"complete":false,"status":"not_evaluated"}));
        }
        self.db
            .execute_batch("CHECKPOINT")
            .map_err(|e| error(format!("persistência dos fatos: {e}")))?;
        self.history = crate::security_history::load(&self.db, &self.settings)?;
        self.db.execute_batch(&format!("CREATE TEMP VIEW bounds AS SELECT ns,min(ts)+{}*86400000 AS cutoff FROM facts WHERE time_ok GROUP BY ns;",self.settings.baseline_days)).map_err(error)?;
        crate::operations::report_progress(
            "comprometimentos",
            "behavior",
            "Analisando comportamento e relações",
            self.count,
            self.count,
            "registros",
            0,
        );
        let first: Option<i64> = self
            .db
            .query_row("SELECT min(ts) FROM facts WHERE time_ok", [], |r| r.get(0))
            .map_err(error)?;
        let cutoff =
            first.and_then(|t| t.checked_add(i64::from(self.settings.baseline_days) * 86_400_000));
        self.db.execute_batch("CREATE TABLE relations AS
            SELECT ns,actor AS entity,'@user' AS col,'user_host' AS kind,host AS target,ref,eid,ts FROM facts WHERE actor<>'' AND host<>'' AND time_ok
            UNION ALL SELECT ns,actor,'@user','user_resource',resource,ref,eid,ts FROM facts WHERE actor<>'' AND resource<>'' AND time_ok
            UNION ALL SELECT ns,process,'@process','process_destination',dst,ref,eid,ts FROM facts WHERE process<>'' AND dst<>'' AND time_ok;").map_err(error)?;
        if cutoff.is_some() {
            let history = self.settings.minimum_history;
            self.signal("new_relationship",&format!("SELECT r.ns,r.entity,min(r.ts),max(r.ts),to_json(struct_pack(baseline_end:=min(c.cutoff),new_relations:=count(DISTINCT (r.kind,r.target)),events:=count(*))) FROM relations r JOIN bounds c ON c.ns=r.ns
                JOIN (SELECT r.ns,r.entity,count(DISTINCT ref) n FROM relations r JOIN bounds c ON c.ns=r.ns WHERE r.ts<c.cutoff GROUP BY r.ns,r.entity HAVING count(DISTINCT ref)>={history}) b ON r.ns=b.ns AND r.entity=b.entity
                WHERE r.col='@user' AND r.ts>=c.cutoff AND NOT EXISTS(SELECT 1 FROM relations old WHERE old.ns=r.ns AND old.entity=r.entity AND old.kind=r.kind AND old.target=r.target AND old.ts<c.cutoff) GROUP BY r.ns,r.entity"),
                "@user","Relação nova no histórico disponível, após um período de referência com suporte suficiente.",25)?;
            self.db.execute_batch("CREATE TEMP TABLE volume_deviations AS WITH hours AS (SELECT ns,actor,cast(floor(ts/3600000)*3600000 AS BIGINT) AS hour,sum(bytes) AS amount FROM facts WHERE actor<>'' AND bytes IS NOT NULL AND bytes>=0 AND time_ok GROUP BY ns,actor,hour),
                baseline AS (SELECT h.ns,actor,median(amount) med,count(*) n FROM hours h JOIN bounds c ON c.ns=h.ns WHERE hour<c.cutoff GROUP BY h.ns,actor HAVING count(*)>=5),
                deviations AS (SELECT b.ns,b.actor,b.med,b.n,median(abs(h.amount-b.med)) mad FROM baseline b JOIN hours h ON h.ns=b.ns AND h.actor=b.actor JOIN bounds c ON c.ns=h.ns WHERE h.hour<c.cutoff GROUP BY b.ns,b.actor,b.med,b.n)
                SELECT h.ns,h.actor,h.hour,h.amount,b.med,b.mad,b.n FROM hours h JOIN deviations b ON h.ns=b.ns AND h.actor=b.actor JOIN bounds c ON c.ns=h.ns WHERE h.hour>=c.cutoff AND h.amount>=1048576 AND h.amount>greatest(b.med*5,b.med+6*b.mad)").map_err(error)?;
            self.signal("volume_change","SELECT ns,actor,min(hour),max(hour+3599999),to_json(struct_pack(max_bytes:=max(amount),baseline_median:=max(med),baseline_mad:=max(mad),baseline_hours:=max(n),deviating_hours:=count(*))) FROM volume_deviations GROUP BY ns,actor",
                "@user","Volume horário acima da mediana e dispersão robusta do período de referência; transferência maliciosa ainda não demonstrada.",35)?;
        }
        self.signal("contextual_rarity",&format!("WITH pairs AS (SELECT ns,entity,kind,target,count(*) n,min(ts) AS first_ts,max(ts) AS last_ts FROM relations WHERE col='@user' GROUP BY ns,entity,kind,target), totals AS (SELECT ns,actor AS entity,count(*) total FROM facts WHERE actor<>'' AND time_ok GROUP BY ns,actor)
            SELECT p.ns,p.entity,min(p.first_ts),max(p.last_ts),to_json(struct_pack(rare_pairs:=count(*),max_pair_events:=max(p.n),entity_events:=max(t.total))) FROM pairs p JOIN totals t ON p.ns=t.ns AND p.entity=t.entity WHERE p.n<={} AND t.total>={} GROUP BY p.ns,p.entity",self.settings.rare_max,self.settings.minimum_history),
            "@user","Combinações pouco frequentes para esta entidade no universo analisado; raridade isolada não prova abuso.",15)?;
        self.signal("password_spraying","WITH windows AS (SELECT ns,src,ts,actor,count(DISTINCT actor) OVER(PARTITION BY ns,src ORDER BY ts RANGE BETWEEN 86400000 PRECEDING AND CURRENT ROW) users,
            count(*) OVER(PARTITION BY ns,src ORDER BY ts RANGE BETWEEN 86400000 PRECEDING AND CURRENT ROW) attempts FROM facts WHERE action='logon' AND outcome='failure' AND actor<>'' AND src<>'' AND time_ok)
            SELECT ns,src,min(ts)-86400000,max(ts),to_json(struct_pack(distinct_users:=max(users),attempts:=max(attempts),window_ms:=86400000)) FROM windows WHERE users>=20 AND attempts<=users*5 GROUP BY ns,src",
            "@src_ip","Falhas distribuídas por várias contas em uma janela móvel de 24 horas; investigar spraying e automação legítima.",40)?;
        self.support_signal("session_context_change","WITH windows AS (SELECT ns,session,ts,src,user_agent,count(DISTINCT src) OVER(PARTITION BY ns,session ORDER BY ts RANGE BETWEEN 3600000 PRECEDING AND CURRENT ROW) ips,count(DISTINCT user_agent) OVER(PARTITION BY ns,session ORDER BY ts RANGE BETWEEN 3600000 PRECEDING AND CURRENT ROW) agents FROM facts WHERE session<>'' AND src<>'' AND user_agent<>'' AND time_ok), changed AS(SELECT * FROM windows WHERE ips>1 AND agents>1) SELECT DISTINCT f.ns,f.session AS entity,f.ref,f.eid,f.ts FROM facts f JOIN changed c ON f.ns=c.ns AND f.session=c.session AND f.ts BETWEEN c.ts-3600000 AND c.ts WHERE f.src<>'' AND f.user_agent<>'' AND f.time_ok",
            "@session","Sessão com mudança de origem e agente em janela móvel de uma hora; VPN, proxy e sessões compartilhadas são alternativas.",30)?;
        self.db.execute_batch("CREATE TEMP TABLE periodic_destinations AS WITH gaps AS (SELECT ns,src,dst,ts,ts-lag(ts) OVER(PARTITION BY ns,src,dst ORDER BY ts,ref) gap FROM facts WHERE src<>'' AND dst<>'' AND time_ok),
            medians AS (SELECT ns,src,dst,median(gap) med,count(*) n,min(ts-gap) AS first_ts,max(ts) AS last_ts FROM gaps WHERE gap>0 GROUP BY ns,src,dst HAVING count(*)>=12),
            scores AS (SELECT m.ns,m.src,m.dst,m.med,m.n,m.first_ts,m.last_ts,avg(CASE WHEN abs(g.gap-m.med)<=m.med*0.2 THEN 1.0 ELSE 0.0 END) regularity FROM medians m JOIN gaps g ON m.ns=g.ns AND m.src=g.src AND m.dst=g.dst WHERE g.gap>0 GROUP BY m.ns,m.src,m.dst,m.med,m.n,m.first_ts,m.last_ts)
            SELECT * FROM scores WHERE med>=1000 AND regularity>=0.8").map_err(error)?;
        self.signal("beaconing","SELECT ns,src,min(first_ts),max(last_ts),to_json(struct_pack(destinations:=count(*),median_interval_ms:=median(med),regularity:=max(regularity))) FROM periodic_destinations GROUP BY ns,src",
            "@src_ip","Comunicação periódica com tolerância a jitter; verificar atualização, monitoramento e C2.",25)?;
        self.signal("dns_tunneling_context","SELECT ns,src,min(ts),max(ts),to_json(struct_pack(queries:=count(*),distinct_names:=count(DISTINCT domain),average_label_entropy:=avg(entropy),max_length:=max(length(domain)))) FROM facts WHERE domain<>'' AND src<>'' AND length(split_part(domain,'.',1))>=24 AND entropy>=3.5 AND time_ok GROUP BY ns,src HAVING count(*)>=20 AND count(DISTINCT domain)>=10",
            "@src_ip","Consultas com rótulos longos, diversos e de alta entropia; verificar CDN, identificadores legítimos e túnel DNS.",25)?;
        self.signal("process_parent_rarity","WITH pairs AS (SELECT ns,process,parent,count(*) n,min(ts) AS first_ts,max(ts) AS last_ts FROM facts WHERE process<>'' AND parent<>'' AND time_ok GROUP BY ns,process,parent), totals AS (SELECT ns,process,sum(n) total FROM pairs GROUP BY ns,process)
            SELECT p.ns,p.process,min(first_ts),max(last_ts),to_json(struct_pack(rare_parents:=count(*),max_pair_events:=max(n),process_events:=max(total))) FROM pairs p JOIN totals t ON p.ns=t.ns AND p.process=t.process WHERE n<=3 AND total>=20 GROUP BY p.ns,p.process",
            "@process","Relação pai/filho pouco frequente para este executável; confirmar identidade da instância e finalidade.",20)?;
        self.signal("telemetry_parse_quality","SELECT ns,source,min(ts),max(ts),to_json(struct_pack(records:=count(*),parse_failures:=sum(CASE WHEN parse_ok THEN 0 ELSE 1 END),missing_or_ambiguous_time:=sum(CASE WHEN time_ok THEN 0 ELSE 1 END))) FROM facts GROUP BY ns,source HAVING sum(CASE WHEN parse_ok THEN 0 ELSE 1 END)>0 OR sum(CASE WHEN time_ok THEN 0 ELSE 1 END)>0",
            "source","Fonte com falhas de parsing ou cronologia incompleta; isto limita análises e não demonstra interferência maliciosa.",10)?;
        self.extended_signals()?;
        self.db.execute_batch("INSERT INTO graph SELECT ns,'user',actor,'host',host,'observed_with','contextual',ref,eid,ts FROM facts WHERE actor<>'' AND host<>'';
            INSERT INTO graph SELECT ns,'user',actor,'resource',resource,'access_observed','contextual',ref,eid,ts FROM facts WHERE actor<>'' AND resource<>'';
            INSERT INTO graph SELECT ns,'session',session,'process',json_extract_string(metadata,'$.process_key'),'session_process','explicit',ref,eid,ts FROM facts WHERE session<>'' AND json_extract_string(metadata,'$.process_key') IS NOT NULL;
            INSERT INTO graph SELECT ns,'process',json_extract_string(metadata,'$.parent_key'),'process',json_extract_string(metadata,'$.process_key'),'parent_child','explicit',ref,eid,ts FROM facts WHERE json_extract_string(metadata,'$.parent_key') IS NOT NULL AND json_extract_string(metadata,'$.process_key') IS NOT NULL;
            INSERT INTO graph SELECT ns,'process',process,'destination',dst,'network_observed','contextual',ref,eid,ts FROM facts WHERE process<>'' AND dst<>'';").map_err(error)?;
        self.db.execute_batch("INSERT INTO graph SELECT ns,'user',actor,'credential',json_extract_string(metadata,'$.semantic.created_credential'),'credential_created','explicit',ref,eid,ts FROM facts WHERE actor<>'' AND outcome='success' AND json_extract_string(metadata,'$.semantic.created_credential') IS NOT NULL;
            INSERT INTO graph SELECT ns,'credential',json_extract_string(metadata,'$.semantic.credential'),'resource',resource,'credential_used','explicit',ref,eid,ts FROM facts WHERE resource<>'' AND json_extract_string(metadata,'$.semantic.credential') IS NOT NULL;
            INSERT INTO graph SELECT ns,'session',session,'user',actor,'authentication_observed','explicit',ref,eid,ts FROM facts WHERE session<>'' AND actor<>'' AND action='logon' AND outcome='success';
            INSERT INTO graph SELECT ns,'process',json_extract_string(metadata,'$.process_key'),'file',json_extract_string(metadata,'$.semantic.file_key'),'file_observed','contextual',ref,eid,ts FROM facts WHERE json_extract_string(metadata,'$.process_key') IS NOT NULL AND json_extract_string(metadata,'$.semantic.file_key') IS NOT NULL;
            INSERT INTO graph SELECT ns,'run',json_extract_string(metadata,'$.semantic.run'),'artifact',json_extract_string(metadata,'$.artifact'),'artifact_observed','explicit',ref,eid,ts FROM facts WHERE json_extract_string(metadata,'$.semantic.run') IS NOT NULL AND json_extract_string(metadata,'$.artifact') IS NOT NULL;").map_err(error)?;
        // A unique nearby process is only a candidate explanation when the
        // connection has no process identity. Both original observations are
        // retained; temporal proximity never becomes an explicit causal edge.
        self.db.execute_batch("CREATE TEMP TABLE candidate_connections AS WITH starts AS(SELECT ns,actor,host,ref,eid,ts,process FROM facts WHERE time_ok AND action='process_start' AND actor<>'' AND host<>'' AND process<>''), connections AS(SELECT ns,actor,host,ref,eid,ts,dst FROM facts WHERE time_ok AND action='network_connection' AND actor<>'' AND host<>'' AND dst<>'' AND process='' AND json_extract_string(metadata,'$.process_key') IS NULL), candidates AS(SELECT p.ns,p.ref AS process_ref,p.eid AS process_eid,p.ts AS process_ts,c.ref AS network_ref,c.eid AS network_eid,c.ts AS network_ts,c.dst,count(*) OVER(PARTITION BY c.ns,c.ref) AS candidates FROM starts p JOIN connections c ON p.ns=c.ns AND p.actor=c.actor AND p.host=c.host AND c.ts>=p.ts AND c.ts-p.ts<=120000) SELECT * FROM candidates WHERE candidates=1;
          INSERT INTO graph SELECT ns,'process_observation',process_ref,'ip',dst,'candidate_process_connection','hypothetical',process_ref,process_eid,process_ts FROM candidate_connections;
          INSERT INTO graph SELECT ns,'process_observation',process_ref,'ip',dst,'candidate_process_connection','hypothetical',network_ref,network_eid,network_ts FROM candidate_connections;").map_err(error)?;
        let signals: i64 = self
            .db
            .query_row("SELECT count(*) FROM signals", [], |r| r.get(0))
            .map_err(error)?;
        let edges: i64 = self
            .db
            .query_row("SELECT count(*) FROM graph", [], |r| r.get(0))
            .map_err(error)?;
        let timed: i64 = self
            .db
            .query_row("SELECT count(*) FROM facts WHERE time_ok", [], |r| r.get(0))
            .map_err(error)?;
        let mut sketches = self.sketches.describe();
        if let Some(candidates) = sketches["frequent_event_codes"]["candidates"].as_array_mut() {
            for candidate in candidates {
                let code = candidate["value"]
                    .as_str()
                    .ok_or("Código de candidato inválido")?;
                let exact: i64 = self
                    .db
                    .query_row("SELECT count(*) FROM facts WHERE code=?", [code], |r| {
                        r.get(0)
                    })
                    .map_err(error)?;
                candidate["exact_count"] = json!(exact);
            }
        }
        Ok(
            json!({"enabled":true,"complete":true,"version":VERSION,"population":self.count,"timed":timed,"sketches":sketches,"schema":self.schema.summary(),
            "signals":signals,"edges":edges,"graph_policy":{"qualities":["explicit","contextual","hypothetical"],"hypothetical":"Unique same-user and same-host process observed within 120 seconds before an unattributed connection; both originals retained; no causal attribution or patient zero inference"},"baseline_start":first,"baseline_end":cutoff,
            "history_scope":"current_universe",
            "historical_profile":self.history,"baseline_scope":"per_namespace","packs":crate::security_packs::catalog(),
            "approximate":false,"evidence_policy":"priority_does_not_promote_evidence",
            "limitations":["Histórico cobre apenas o universo analisado; não equivale à vida da entidade.",
                "Sinais comportamentais ainda não têm precisão representativa demonstrada."]}),
        )
    }

    pub(crate) fn support_signal(
        &self,
        kind: &str,
        select: &str,
        column: &str,
        description: &str,
        priority: i32,
    ) -> Result<(), String> {
        self.db
            .execute_batch(&format!(
                "INSERT INTO supports SELECT '{}',ns,entity,ref,eid,ts FROM ({select});",
                kind.replace('\'', "''")
            ))
            .map_err(error)?;
        self.signal(kind,&format!("SELECT ns,entity,min(ts),max(ts),to_json(struct_pack(events:=count(DISTINCT ref))) FROM supports WHERE kind='{}' GROUP BY ns,entity",kind.replace('\'',"''")),column,description,priority)
    }
    fn extended_signals(&self) -> Result<(), String> {
        crate::security_packs::run(self)?;
        if let Some(catalog) = &self.settings.ioc {
            let mut statement = self
                .db
                .prepare("SELECT DISTINCT indicator FROM ioc_matches ORDER BY indicator")
                .map_err(error)?;
            let present = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(error)?
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
                .map_err(error)?;
            for indicator in catalog
                .indicators
                .iter()
                .filter(|i| present.contains(&i.id))
            {
                let id = indicator.id.replace('\'', "''");
                self.support_signal(&format!("offline_ioc:{}",indicator.id),&format!("SELECT ns,source AS entity,ref,eid,ts FROM ioc_matches WHERE indicator='{id}'"),"source",&format!("Correspondência exata com IOC offline {} do catálogo {} revisão {}. Procedência: {}. Contexto: {}. Horários desconhecidos impedem verificar a validade temporal.",indicator.id,catalog.id,catalog.revision,catalog.source,indicator.context),40)?;
            }
        }
        self.db.execute_batch("CREATE TEMP VIEW operational_hours AS SELECT ns,source,floor(ts/3600000)*3600000 AS hour,count(*) AS total,sum(CASE WHEN outcome='failure' OR try_cast(code AS INTEGER)>=500 THEN 1 ELSE 0 END) AS errors,median(latency) AS latency FROM facts WHERE time_ok GROUP BY ns,source,hour;
            CREATE TEMP VIEW operational_baseline AS SELECT h.ns,h.source,median(1.0*errors/total) AS error_rate,median(latency) AS med_latency,count(*) AS hours FROM operational_hours h JOIN bounds b USING(ns) WHERE hour<b.cutoff GROUP BY h.ns,h.source HAVING count(*)>=5;").map_err(error)?;
        if self.history["status"] == "frozen_reference" {
            self.support_signal("historical_relationship",&format!("SELECT r.ns,r.entity,r.ref,r.eid,r.ts FROM relations r WHERE r.col='@user' AND NOT EXISTS(SELECT 1 FROM historical_pairs p WHERE p.ns=r.ns AND p.entity=r.entity AND p.kind=r.kind AND p.target=r.target) AND (SELECT sum(events) FROM historical_hours h WHERE h.ns=r.ns AND h.entity=r.entity)>={} AND r.ts>(SELECT max(hour)+3599999 FROM historical_hours h WHERE h.ns=r.ns AND h.entity=r.entity)",self.settings.minimum_history),"@user","Relação ausente na referência histórica congelada deste Caso; a referência exclui achados e hipóteses prioritárias. Conferir cobertura e mudanças autorizadas.",25)?;
            self.db.execute_batch("CREATE TEMP VIEW seasonal_reference AS SELECT ns,entity,slot,median(bytes) med,count(*) support,max(hour) last_hour FROM historical_hours WHERE bytes IS NOT NULL GROUP BY ns,entity,slot HAVING count(*)>=3;
                CREATE TEMP VIEW seasonal_deviation AS SELECT b.ns,b.entity,b.slot,b.med,b.support,b.last_hour,median(abs(h.bytes-b.med)) mad FROM seasonal_reference b JOIN historical_hours h ON b.ns=h.ns AND b.entity=h.entity AND b.slot=h.slot GROUP BY b.ns,b.entity,b.slot,b.med,b.support,b.last_hour;
                CREATE TEMP VIEW current_hours AS SELECT ns,actor AS entity,cast(floor(ts/3600000)*3600000 AS BIGINT) hour,cast(strftime(to_timestamp(ts/1000.0),'%w') AS INTEGER)*24+cast(strftime(to_timestamp(ts/1000.0),'%H') AS INTEGER) slot,sum(bytes) amount FROM facts WHERE actor<>'' AND time_ok AND bytes IS NOT NULL GROUP BY ns,actor,hour,slot;").map_err(error)?;
            self.support_signal("seasonal_volume_change","SELECT f.ns,f.actor AS entity,f.ref,f.eid,f.ts FROM facts f JOIN current_hours h ON f.ns=h.ns AND f.actor=h.entity AND cast(floor(f.ts/3600000)*3600000 AS BIGINT)=h.hour JOIN seasonal_deviation b ON b.ns=h.ns AND b.entity=h.entity AND b.slot=h.slot WHERE h.hour>b.last_hour AND h.amount>=1048576 AND h.amount>greatest(b.med*5,b.med+6*b.mad) AND f.bytes IS NOT NULL AND f.time_ok","@user","Volume acima da mediana e MAD da mesma hora da semana em UTC, com ao menos três semanas de suporte no snapshot congelado. Sazonalidade não estabelece exfiltração.",35)?;
            self.support_signal("peer_volume_change","WITH peers AS (SELECT ns,peer_group,slot,median(bytes) med,count(DISTINCT entity) peers FROM historical_hours WHERE peer_group<>'' AND bytes IS NOT NULL GROUP BY ns,peer_group,slot HAVING count(DISTINCT entity)>=5) SELECT f.ns,f.actor AS entity,f.ref,f.eid,f.ts FROM facts f JOIN current_hours h ON f.ns=h.ns AND f.actor=h.entity AND cast(floor(f.ts/3600000)*3600000 AS BIGINT)=h.hour JOIN peers p ON p.ns=f.ns AND p.peer_group=json_extract_string(f.metadata,'$.peer_group') AND p.slot=h.slot WHERE h.amount>=1048576 AND h.amount>p.med*5 AND NOT EXISTS(SELECT 1 FROM historical_hours b WHERE b.ns=f.ns AND b.entity=f.actor) AND f.bytes IS NOT NULL AND f.time_ok","@user","Entidade em cold start excedeu a referência de ao menos cinco pares de um grupo declarado explicitamente. Diferenças de função e carga exigem revisão.",20)?;
        }
        self.support_signal("template_novelty","SELECT f.ns,f.source AS entity,f.ref,f.eid,f.ts FROM facts f JOIN bounds b USING(ns) WHERE f.time_ok AND f.ts>=b.cutoff AND f.template<>'' AND NOT EXISTS(SELECT 1 FROM facts old WHERE old.ns=f.ns AND old.source=f.source AND old.template=f.template AND old.ts<b.cutoff) AND (SELECT count(*) FROM facts old WHERE old.ns=f.ns AND old.source=f.source AND old.ts<b.cutoff)>=20","source","Template novo após histórico suficiente da fonte. A generalização é determinística e pode agrupar mensagens distintas; verificar deploy e conteúdo original.",15)?;
        self.support_signal("application_error_change","SELECT f.ns,f.source AS entity,f.ref,f.eid,f.ts FROM facts f JOIN operational_hours h ON f.ns=h.ns AND f.source=h.source AND floor(f.ts/3600000)*3600000=h.hour JOIN operational_baseline base ON h.ns=base.ns AND h.source=base.source JOIN bounds b ON b.ns=h.ns WHERE h.hour>=b.cutoff AND h.total>=20 AND 1.0*h.errors/h.total>greatest(base.error_rate*3,base.error_rate+0.2) AND (f.outcome='failure' OR try_cast(f.code AS INTEGER)>=500)","source","Taxa de erro aumentou em relação ao histórico, usando todas as respostas como denominador. Verificar deploy, demanda e dependências.",25)?;
        self.support_signal("latency_change","SELECT f.ns,f.source AS entity,f.ref,f.eid,f.ts FROM facts f JOIN operational_hours h ON f.ns=h.ns AND f.source=h.source AND floor(f.ts/3600000)*3600000=h.hour JOIN operational_baseline base ON h.ns=base.ns AND h.source=base.source JOIN bounds b ON b.ns=h.ns WHERE h.hour>=b.cutoff AND h.total>=20 AND base.med_latency>0 AND h.latency>base.med_latency*3 AND f.latency IS NOT NULL","source","Mediana da latência horária excedeu três vezes a referência; somente unidades declaradas foram convertidas para milissegundos.",25)?;
        self.support_signal("authentication_chain","WITH windows AS (SELECT ns,actor,src,ts,outcome,count(*) FILTER(WHERE outcome='failure') OVER(PARTITION BY ns,actor,src ORDER BY ts RANGE BETWEEN 1800000 PRECEDING AND CURRENT ROW) AS failures FROM facts WHERE action='logon' AND actor<>'' AND src<>'' AND time_ok), success AS (SELECT * FROM windows WHERE outcome='success' AND failures>=10) SELECT DISTINCT f.ns,f.actor AS entity,f.ref,f.eid,f.ts FROM facts f JOIN success s ON f.ns=s.ns AND f.actor=s.actor AND f.src=s.src AND f.ts BETWEEN s.ts-1800000 AND s.ts WHERE f.action='logon' AND f.outcome IN ('failure','success')","@user","Falhas de autenticação seguidas de sucesso da mesma conta e origem. Confirma atividade; credencial obtida indevidamente ainda depende de corroboração.",45)?;
        for policy in &self.settings.telemetry {
            let source = policy.source.replace('\'', "''");
            let ns = policy.namespace.replace('\'', "''");
            self.support_signal("telemetry_gap",&format!("WITH ordered AS (SELECT ref,ts,lag(ts) OVER(ORDER BY ts,ref) previous_ts,lag(ref) OVER(ORDER BY ts,ref) previous_ref FROM facts WHERE source='{source}' AND ns='{ns}' AND time_ok), gaps AS (SELECT * FROM ordered WHERE ts-previous_ts>{}) SELECT f.ns,f.source AS entity,f.ref,f.eid,f.ts FROM facts f WHERE f.ref IN(SELECT ref FROM gaps UNION SELECT previous_ref FROM gaps)",policy.maximum_gap_ms),"source","Intervalo entre eventos excedeu a política explícita da fonte. Retenção, reinício e falha de coleta são alternativas; o intervalo não prova limpeza maliciosa.",15)?;
            if policy.sequence {
                self.support_signal("telemetry_record_sequence",&format!("WITH ordered AS (SELECT ns,source,ref,eid,ts,try_cast(json_extract_string(metadata,'$.record_id') AS BIGINT) record_id,lag(try_cast(json_extract_string(metadata,'$.record_id') AS BIGINT)) OVER(PARTITION BY host ORDER BY ts,ref) previous_id FROM facts WHERE source='{source}' AND ns='{ns}' AND time_ok) SELECT ns,source AS entity,ref,eid,ts FROM ordered WHERE record_id>previous_id+1 OR record_id<previous_id"),"source","Salto ou reinício da sequência declarada de registros. Conferir filtros, rollover, retenção e cópia parcial antes de atribuir interferência.",15)?;
            }
        }
        for policy in &self.settings.business {
            let source = policy.source.replace('\'', "''");
            let ns = policy.namespace.replace('\'', "''");
            let action = policy.action.replace('\'', "''");
            let currency = policy.currency.replace('\'', "''");
            let amount = policy
                .minimum_amount
                .map_or(String::new(), |value| format!(" AND total_amount>={value}"));
            self.support_signal(&format!("business_velocity:{}",policy.id),&format!("WITH windows AS (SELECT ns,actor,ts,count(*) OVER(PARTITION BY actor ORDER BY ts RANGE BETWEEN {} PRECEDING AND CURRENT ROW) n,sum(amount) OVER(PARTITION BY actor ORDER BY ts RANGE BETWEEN {} PRECEDING AND CURRENT ROW) total_amount FROM facts WHERE ns='{ns}' AND source='{source}' AND action='{action}' AND currency='{currency}' AND outcome='success' AND actor<>'' AND time_ok AND amount>=0), selected AS (SELECT * FROM windows WHERE n>={}{amount}) SELECT DISTINCT f.ns,f.actor AS entity,f.ref,f.eid,f.ts FROM facts f JOIN selected s ON f.ns=s.ns AND f.actor=s.actor AND f.ts BETWEEN s.ts-{} AND s.ts WHERE f.source='{source}' AND f.action='{action}' AND f.currency='{currency}' AND f.outcome='success' AND f.amount>=0",policy.window_ms,policy.window_ms,policy.minimum_count,policy.window_ms),"@user",&format!("Atividade excedeu a política declarada '{}', na moeda {}. Não equivale a classificação de fraude.",policy.id,policy.currency),35)?;
            if let Some(change) = &policy.account_change_action {
                let change = change.replace('\'', "''");
                let change_source = policy
                    .account_change_source
                    .as_deref()
                    .unwrap_or(&policy.source)
                    .replace('\'', "''");
                let auth_source = policy
                    .authentication_source
                    .as_deref()
                    .unwrap_or(&policy.source)
                    .replace('\'', "''");
                self.support_signal(&format!("account_takeover_chain:{}",policy.id),&format!("WITH changes AS(SELECT * FROM facts WHERE ns='{ns}' AND source='{change_source}' AND action='{change}' AND outcome='success' AND actor<>'' AND time_ok),auth AS(SELECT * FROM facts WHERE ns='{ns}' AND source='{auth_source}' AND action='logon' AND outcome='success' AND session<>'' AND time_ok),transactions AS(SELECT * FROM facts WHERE ns='{ns}' AND source='{source}' AND action='{action}' AND currency='{currency}' AND outcome='success' AND session<>'' AND amount>=0 AND time_ok),linked AS(SELECT c.actor AS entity,c.ref c_ref,c.eid c_eid,c.ts c_ts,a.ref a_ref,a.eid a_eid,a.ts a_ts,t.ref t_ref,t.eid t_eid,t.ts t_ts FROM changes c JOIN auth a ON c.actor=a.actor AND a.ts>c.ts AND a.ts-c.ts<=1800000 JOIN transactions t ON a.actor=t.actor AND a.session=t.session AND t.ts>a.ts AND t.ts-a.ts<={}) SELECT '{ns}' AS ns,entity,c_ref AS ref,c_eid AS eid,c_ts AS ts FROM linked UNION SELECT '{ns}',entity,a_ref,a_eid,a_ts FROM linked UNION SELECT '{ns}',entity,t_ref,t_eid,t_ts FROM linked",policy.window_ms),"@user",&format!("Alteração de conta declarada na política '{}' seguida de autenticação e transação da mesma identidade e sessão, na moeda {}. Encadeamento observado não estabelece fraude; conferir autorização e recuperação legítima.",policy.id,policy.currency),45)?;
            }
        }
        Ok(())
    }
}
