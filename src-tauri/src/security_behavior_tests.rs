//! Behavior, HTTP outcome, location and session rules (v0.18).
use crate::{
    detections::{self, Settings, Source, Triage},
    model::Event,
};
use serde_json::{json, Value};

const START: i64 = 1_700_000_000_000;

fn event(id: usize, seconds: i64, fields: Value) -> Event {
    let mut e = Event::empty();
    e.id = id;
    e.event_ref = format!("behavior:{id:06}");
    e.timestamp = Some(START + seconds * 1000);
    e.source = "behavior-test".into();
    e.fields = fields.as_object().unwrap().clone();
    e
}

fn request(id: usize, seconds: i64, ip: &str, method: &str, url: &str, status: u16, bytes: u64) -> Event {
    event(
        id,
        seconds,
        json!({"event.category":"web","event.action":"http_request","host.name":"web-a","source.ip":ip,
            "url.original":url,"http.request.method":method,"http.response.status_code":status,
            "http.response.body.bytes":bytes}),
    )
}

fn settings() -> Settings {
    Settings { threats: false, ..Default::default() }
}

fn run(events: &[Event]) -> Triage {
    let rules = detections::builtin_ruleset().unwrap();
    let settings = settings();
    detections::run(
        &detections::Inputs { rules: &rules, settings: &settings, catalog: None },
        &Source::Events(events.iter().collect()),
    )
    .unwrap()
}

/// The production path: correlation store, checkpoints and stored results.
fn run_stored(events: &[Event]) -> Vec<Value> {
    let rules = detections::builtin_ruleset().unwrap();
    let settings = settings();
    let result = detections::run_stored(
        &detections::Inputs { rules: &rules, settings: &settings, catalog: None },
        &Source::Events(events.iter().collect()),
    )
    .unwrap();
    result.page(1, 0, 100, None, None, None).unwrap()["detections"].as_array().unwrap().clone()
}

fn rules(t: &Triage) -> Vec<&str> {
    t.detections.iter().map(|d| d.rule.as_str()).collect()
}

fn site() -> Vec<Event> {
    let mut events = Vec::new();
    let mut id = 0;
    let mut next = || {
        id += 1;
        id
    };
    // Ordinary traffic during five hours: many visitors, stable response sizes.
    for n in 0..60 {
        let ip = format!("198.51.100.{}", n % 30 + 1);
        events.push(request(next(), n * 300, &ip, "GET", "/item.php?id=7", 200, 1000 + (n as u64 % 5) * 40));
        events.push(request(next(), n * 300 + 5, &ip, "GET", "/index.php", 200, 5000));
    }
    // A payload that returns far more data than the endpoint normally does.
    events.push(request(next(), 9000, "203.0.113.9", "GET", "/item.php?id=1%20UNION%20SELECT%20username,password%20FROM%20users--", 200, 250_000));
    // The same payload with an ordinary size is not an outlier.
    events.push(request(next(), 9100, "203.0.113.9", "GET", "/item.php?id=1%20UNION%20SELECT%20username,password%20FROM%20users--", 200, 1040));
    // A payload that breaks the application.
    events.push(request(next(), 9200, "203.0.113.9", "GET", "/item.php?id=1%27%20OR%20%271%27=%271%27--", 500, 300));
    // Webshell: probe (404), upload, then the new script answers and receives commands.
    events.push(request(next(), 9500, "203.0.113.9", "GET", "/uploads/avatar.php", 404, 200));
    events.push(request(next(), 9600, "203.0.113.9", "POST", "/profile/upload-avatar", 200, 120));
    for (n, seconds) in [9700, 9800, 9900, 10000].into_iter().enumerate() {
        let method = if n == 0 { "GET" } else { "POST" };
        events.push(request(next(), seconds, "203.0.113.9", method, "/uploads/avatar.php", 200, 64));
    }
    events
}

#[test]
fn web_outcome_and_webshell_behaviors_are_found_in_both_paths() {
    let events = site();
    let found = run(&events);
    let names = rules(&found);
    for rule in [
        "web.attack.size-outlier",
        "web.attack.server-error",
        "web.script-appeared",
        "webshell.upload-dir-script",
        "webshell.new-script",
        "webshell.upload-then-script",
    ] {
        assert!(names.contains(&rule), "{rule} ausente em {names:?}");
    }
    let outlier: Vec<_> = found.detections.iter().filter(|d| d.rule == "web.attack.size-outlier").collect();
    assert_eq!(outlier.len(), 1, "somente a resposta fora do padrão");
    assert_eq!(outlier[0].measurements["bytes"], 250000.0);
    // Ordinary, widely used scripts never look like webshells.
    let new_scripts: Vec<_> = found.detections.iter().filter(|d| d.rule == "webshell.new-script").collect();
    assert_eq!(new_scripts.len(), 1);
    assert_eq!(new_scripts[0].count, 4);
    assert_eq!(new_scripts[0].measurements["sources"], json!(["203.0.113.9"]));

    let stored = run_stored(&events);
    let stored_rules: Vec<&str> = stored.iter().filter_map(|d| d["rule"].as_str()).collect();
    for rule in ["web.attack.size-outlier", "web.script-appeared", "webshell.new-script", "webshell.upload-then-script"] {
        assert!(stored_rules.contains(&rule), "{rule} ausente no armazenamento: {stored_rules:?}");
    }
    let sentence = |rule: &str| {
        stored.iter().find(|d| d["rule"] == rule).and_then(|d| d["measurements"]["narrative"].as_str()).unwrap_or_default().to_string()
    };
    assert!(sentence("web.attack.size-outlier").contains("250000 bytes"), "{}", sentence("web.attack.size-outlier"));
    assert!(sentence("webshell.new-script").contains("100% POST") || sentence("webshell.new-script").contains("75% POST"), "{}", sentence("webshell.new-script"));
    assert!(sentence("webshell.upload-then-script").contains("envio de arquivo (1) → acesso ao script"), "{}", sentence("webshell.upload-then-script"));
}

#[test]
fn quiet_sites_produce_no_behavior_findings() {
    let mut events = Vec::new();
    for n in 0..80 {
        let ip = format!("198.51.100.{}", n % 20 + 1);
        events.push(request(n, n as i64 * 200, &ip, "GET", "/index.php", 200, 5000 + n as u64));
        events.push(request(1000 + n, n as i64 * 200 + 3, &ip, "POST", "/login.php", 200, 900));
    }
    // A script used by many visitors from the start, and a static image in uploads.
    events.push(request(5000, 10, "198.51.100.1", "GET", "/uploads/logo.png", 200, 30_000));
    let found = run(&events);
    assert!(found.detections.is_empty(), "{:?}", rules(&found));
}

const GEO: &str = "177.0.0.0\t177.0.255.255\t28573\tBR\tClaro NXT Telecomunicacoes Ltda\n\
    8.8.8.0\t8.8.8.255\t15169\tUS\tGOOGLE\n\
    133.0.0.0\t133.0.255.255\t2907\tJP\tSINET\n";

fn login(id: usize, seconds: i64, user: &str, ip: &str) -> Event {
    event(id, seconds, json!({"event.action":"logon","event.outcome":"success","user.name":user,"source.ip":ip,"host.name":"vpn"}))
}

#[test]
fn location_rules_need_history_distance_and_the_installed_base() {
    let mut events: Vec<Event> = (0..6).map(|n| login(n, n as i64 * 3600, "ana", "177.0.1.10")).collect();
    // 30 minutes after the last Brazilian login, a login from a US cloud provider.
    events.push(login(10, 5 * 3600 + 1800, "ana", "8.8.8.8"));
    // Another account seen only once from Japan: no history, no finding.
    events.push(login(11, 100, "bia", "133.0.0.5"));
    let without = run(&events);
    assert!(!rules(&without).iter().any(|r| r.starts_with("geo.")), "sem base nenhuma regra de país");
    crate::geoip::with_test_database(GEO, || {
        let found = run(&events);
        let names = rules(&found);
        for rule in ["geo.new-country", "geo.impossible-travel", "geo.hosting-login"] {
            assert!(names.contains(&rule), "{rule} ausente em {names:?}");
        }
        let travel = found.detections.iter().find(|d| d.rule == "geo.impossible-travel").unwrap();
        assert_eq!(travel.measurements["from"], "BR");
        assert_eq!(travel.measurements["to"], "US");
        assert_eq!(travel.count, 2);
        assert_eq!(found.detections.iter().filter(|d| d.rule == "geo.new-country").count(), 1);
        let stored = run_stored(&events);
        let travel = stored.iter().find(|d| d["rule"] == "geo.impossible-travel").unwrap();
        assert!(travel["measurements"]["narrative"].as_str().unwrap().starts_with("BR → US"));
    });
}

#[test]
fn a_session_used_by_another_client_is_reported() {
    let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36";
    let mut events = Vec::new();
    for n in 0..5 {
        events.push(event(n, n as i64 * 60, json!({"source.ip":"198.51.100.4","url.original":"/app","http.response.status_code":200,
            "http.request.cookie":"PHPSESSID=0123456789abcdef","user_agent.original":chrome})));
    }
    events.push(event(9, 400, json!({"source.ip":"203.0.113.50","url.original":"/app/export","http.response.status_code":200,
        "http.request.cookie":"PHPSESSID=0123456789abcdef","user_agent.original":"curl/8.4.0"})));
    // Another session of the same browser stays quiet.
    events.push(event(10, 500, json!({"source.ip":"198.51.100.5","url.original":"/app","http.response.status_code":200,
        "http.request.cookie":"PHPSESSID=ffffffffffffffff","user_agent.original":chrome})));
    let found = run(&events);
    let hijack: Vec<_> = found.detections.iter().filter(|d| d.rule == "session.multi-browser").collect();
    assert_eq!(hijack.len(), 1, "{:?}", rules(&found));
    assert_eq!(hijack[0].distinct, 2);
}

#[test]
fn brute_force_success_is_told_as_a_sentence() {
    let mut events: Vec<Event> = (0..8)
        .map(|n| event(n, n as i64 * 10, json!({"event.category":"authentication","event.action":"logon","event.outcome":"failure","service.name":"ssh","user.name":"root","source.ip":"203.0.113.7","host.name":"srv"})))
        .collect();
    events.push(event(20, 200, json!({"event.category":"authentication","event.action":"logon","event.outcome":"success","service.name":"ssh","user.name":"root","source.ip":"203.0.113.7","host.name":"srv"})));
    let stored = run_stored(&events);
    let finding = stored.iter().find(|d| d["rule"] == "auth.bruteforce.success").expect("sequência");
    let sentence = finding["measurements"]["narrative"].as_str().unwrap();
    // The finding keeps the failures that complete the step, followed by the success.
    assert!(sentence.starts_with("5 falhas → 1 sucesso em "), "{sentence}");
    assert!(sentence.ends_with("(origem 203.0.113.7, conta root)"), "{sentence}");
}

#[test]
fn parallel_batches_keep_the_serial_result() {
    // The same input split across many batches must give identical findings.
    let events = site();
    let first = run(&events);
    let second = run(&events);
    let summary = |t: &Triage| {
        let mut v: Vec<(String, usize, Vec<usize>)> =
            t.detections.iter().map(|d| (d.rule.clone(), d.count, d.event_ids.clone())).collect();
        v.sort();
        v
    };
    assert_eq!(summary(&first), summary(&second));
    assert_eq!(first.total, events.len());
}
