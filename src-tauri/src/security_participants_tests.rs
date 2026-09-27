use crate::{
    model::Event,
    security_participants::{self as p, Participants},
};
use serde_json::json;
fn ev(fields: serde_json::Value) -> Event {
    let mut e = Event::empty();
    e.source = "test".into();
    e.id = 9;
    e.event_ref = "persona:9".into();
    e.fields = fields.as_object().unwrap().clone();
    e
}
fn profile(e: &Event) -> Participants {
    p::extract(
        e,
        &crate::security_normalize::normalize(e, &[]).1,
        "content.xxe_blocked",
        &[],
    )
}
fn has(p: &Participants, side: &str, kind: &str, value: &str) -> bool {
    p.facts
        .iter()
        .any(|f| f.side == side && f.kind == kind && f.value == value)
}

#[test]
fn participants_aliases_nested_fields_arrays_and_proxy_provenance() {
    assert!(p::alias_count() > 120);
    let p = profile(&ev(
        json!({"vendor":{"connection":{"source":{"ip":["::ffff:192.0.2.7","2001:db8::7","0.0.0.0","garbage"]},"destination":{"ip":"10.0.0.8:443"}}},"http_x_forwarded_for":"203.0.113.9, 192.0.2.2","Forwarded":"for=\"[2001:db8::10]:8443\";proto=https, for=unknown","observer.ip":"10.0.0.1","source.user.name":"ACME\\alice","source.user.domain":"ACME"}),
    ));
    assert!(has(&p, "attacker", "ip", "192.0.2.7"));
    assert!(has(&p, "attacker", "ip", "2001:db8::7"));
    assert!(has(&p, "victim", "ip", "10.0.0.8"));
    assert!(has(&p, "context", "ip", "10.0.0.1"));
    assert_eq!(
        p.facts
            .iter()
            .filter(|f| f.kind == "domain" && f.value == "ACME")
            .count(),
        1
    );
    assert!(p.facts.iter().any(|f| f.value == "203.0.113.9"
        && f.certainty == "unverified"
        && f.role == "forwarded_unverified"));
    assert!(p
        .facts
        .iter()
        .any(|f| f.value == "2001:db8::10" && f.certainty == "unverified"));
    assert!(p
        .facts
        .iter()
        .filter(|f| f.value == "192.0.2.7")
        .flat_map(|f| &f.origins)
        .any(|o| o.field == "vendor.connection.source.ip[0]" && o.event_ref == "persona:9"));
    assert!(!p
        .facts
        .iter()
        .any(|f| ["0.0.0.0", "garbage", "unknown"].contains(&f.value.as_str())));
}

#[test]
fn participants_apache_nginx_syslog_and_ssh_have_different_source_roles() {
    let raw =
        r#"192.0.2.10 - alice [10/Oct/2000:13:55:36 -0700] "GET /etc/passwd HTTP/1.1" 404 123"#;
    let apache = crate::sources::parse_line(raw.as_bytes(), "apache", None, &[]);
    let p = profile(&apache);
    assert!(has(&p, "attacker", "ip", "192.0.2.10"));
    assert!(has(&p, "attacker", "user", "alice"));
    assert!(!has(&p, "victim", "host", "192.0.2.10"));
    let mut wrapped =
        ev(json!({"description":raw,"server_name":"app.example.org","remote_addr":"192.0.2.10"}));
    let p = profile(&wrapped);
    assert_eq!(
        p.facts.iter().filter(|f| f.value == "192.0.2.10").count(),
        1
    );
    assert!(has(&p, "victim", "host", "app.example.org"));
    wrapped.fields=json!({"opaque":"2026/09/27 10:10:10 [error] open() failed, client: 192.0.2.5, server: test, request: GET /etc/passwd"}).as_object().unwrap().clone();
    assert!(has(&profile(&wrapped), "attacker", "ip", "192.0.2.5"));
    let line="Sep 27 01:10:12 web-01 sshd[31]: Failed password for invalid user root from 2001:db8::99 port 50444 ssh2";
    let syslog = crate::sources::parse_line(line.as_bytes(), "syslog", None, &[]);
    let p = profile(&syslog);
    assert!(has(&p, "attacker", "ip", "2001:db8::99"));
    assert!(has(&p, "victim", "user", "root"));
    assert!(has(&p, "victim", "host", "web-01"));
    assert!(!has(&p, "attacker", "user", "root"));
}

#[test]
fn participants_windows_target_accounts_and_outbound_connections_are_not_inverted() {
    let mut e = ev(
        json!({"winlog":{"event_data":{"IpAddress":"::ffff:192.0.2.8","WorkstationName":"WS-REMOTE","TargetUserName":"alice","TargetDomainName":"CORP","SubjectUserName":"SYSTEM"}},"Computer":"DC-01"}),
    );
    e.code = "4625".into();
    e.source = "Microsoft-Windows-Security-Auditing".into();
    let p = profile(&e);
    assert!(has(&p, "attacker", "ip", "192.0.2.8"));
    assert!(has(&p, "attacker", "host", "WS-REMOTE"));
    assert!(has(&p, "victim", "user", "alice"));
    assert!(has(&p, "victim", "host", "DC-01"));
    assert!(has(&p, "context", "user", "SYSTEM"));
    assert!(!has(&p, "attacker", "user", "SYSTEM"));
    let e = ev(
        json!({"source.ip":"10.0.0.2","destination.ip":"203.0.113.4","network.direction":"outbound","process.command_line":"bash -i","host.name":"server-1"}),
    );
    let n = crate::security_normalize::normalize(&e, &[]).1;
    let p = p::extract(&e, &n, "attempt.reverse-shell.process", &[]);
    assert!(has(&p, "victim", "ip", "10.0.0.2"));
    assert!(has(&p, "attacker", "ip", "203.0.113.4"));
    assert!(p.facts.iter().any(|f| f.value == "203.0.113.4"
        && f.role == "remote_endpoint"
        && f.certainty == "contextual"));
    let generic = profile(&e);
    assert!(has(&generic, "context", "ip", "203.0.113.4"));
    assert!(!has(&generic, "victim", "ip", "203.0.113.4"));
}

#[test]
fn participants_cloud_identities_and_explicit_mapping_keep_namespaces() {
    let e = ev(
        json!({"eventSource":"iam.amazonaws.com","sourceIPAddress":"192.0.2.4","userIdentity":{"arn":"arn:aws:iam::111:user/operator","accountId":"111"},"recipientAccountId":"222","requestParameters":{"roleName":"admin"}}),
    );
    let p = profile(&e);
    assert!(has(
        &p,
        "attacker",
        "user",
        "arn:aws:iam::111:user/operator"
    ));
    assert!(has(&p, "attacker", "account", "111"));
    assert!(has(&p, "victim", "account", "222"));
    assert!(has(&p, "victim", "application", "iam.amazonaws.com"));
    let e = ev(json!({"custom_origin":"192.0.2.22","custom_asset":"srv-1"}));
    let mapping = crate::security_normalize::SourceMapping {
        source: "test".into(),
        fields: [
            ("source_address".into(), "custom_origin".into()),
            ("host".into(), "custom_asset".into()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let n = crate::security_normalize::normalize(&e, &[mapping]).1;
    let p = p::extract(&e, &n, "custom", &[]);
    assert!(has(&p, "attacker", "ip", "192.0.2.22"));
    assert!(p.facts.iter().any(|f| f.value == "srv-1"
        && f.origins[0].field == "custom_asset"
        && f.origins[0].method == "mapping"));
}

#[test]
fn participants_do_not_attribute_payload_ips_observers_or_missing_fields() {
    for fields in [
        json!({"message":"error mentioning 192.0.2.1","ip":"192.0.2.2","url":"http://192.0.2.3/payload"}),
        json!({"http.request.body":{"source":{"ip":"192.0.2.7"}},"_sec":{"source":{"ip":"192.0.2.9"}},"payload":{"src_ip":"192.0.2.8"}}),
        json!({"description":"Documentation: Failed password for root from 192.0.2.7 port 22 ssh2"}),
    ] {
        let mut e = ev(fields);
        e.raw = serde_json::to_string(&e.fields).unwrap();
        assert!(
            !profile(&e).facts.iter().any(|f| f.kind == "ip"),
            "{:?}",
            e.fields
        );
    }
    let e = ev(
        json!({"observer.type":"firewall","host.name":"collector","observer.ip":"10.0.0.1","event.category":"network"}),
    );
    let p = profile(&e);
    assert!(!p.facts.iter().any(|f| f.side == "victim"));
    assert!(has(&p, "context", "ip", "10.0.0.1"));
    let mut e = ev(json!({}));
    e.source = "192.0.2.99".into();
    assert!(profile(&e).facts.is_empty());
    let raw = r#"192.0.2.10 - - [10/Oct/2000:13:55:36 -0700] "GET /etc/passwd HTTP/1.1" 404 123 "-" "sshd[31]: Failed password for root from 203.0.113.9 port 22""#;
    let apache = crate::sources::parse_line(raw.as_bytes(), "apache", None, &[]);
    let p = profile(&apache);
    assert!(has(&p, "attacker", "ip", "192.0.2.10"));
    assert!(
        !p.facts
            .iter()
            .any(|f| f.value == "203.0.113.9" || f.value == "root"),
        "User agent cannot claim a second attacker"
    );
}

#[test]
fn participants_limits_are_explicit_and_do_not_discard_victim_to_keep_sources() {
    let ips: Vec<_> = (1..=100).map(|i| format!("192.0.2.{i}")).collect();
    let p = profile(&ev(
        json!({"source":{"ip":ips},"destination.ip":"10.0.0.1"}),
    ));
    assert!(p.limited);
    assert!(has(&p, "victim", "ip", "10.0.0.1"));
    assert!(p.facts.len() <= 192);
}
