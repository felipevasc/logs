//! A derived security view. The original event and its fields are never changed.
use crate::{
    entities::{self, Role},
    evidence,
    model::Event,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceMapping {
    /// Exact source name; never a regex applied to attacker-controlled content.
    pub source: String,
    /// Semantic name -> original field path (e.g. actor -> principal.id).
    pub fields: BTreeMap<String, String>,
    pub actions: BTreeMap<String, String>,
    pub outcomes: BTreeMap<String, String>,
    pub timestamp_unit: Option<String>,
    pub timezone: Option<String>,
}
pub const MAPPABLE: &[&str] = &[
    "timestamp",
    "actor",
    "target",
    "namespace",
    "host",
    "service",
    "action",
    "outcome",
    "request",
    "session",
    "connection",
    "process",
    "parent",
    "file",
    "command",
    "credential",
    "created_credential",
    "resource",
    "request_command",
    "url",
    "persistence_target",
    "application",
    "grant",
    "token",
    "repository",
    "pipeline",
    "run",
    "revision",
    "secret",
    "certificate",
    "certificate_issuer",
    "certificate_subject",
    "requester",
    "beneficiary",
    "delegator",
    "resource_spn",
    "destination",
    "artifact",
    "logon",
    "remote_session",
    "principal",
    "source_address",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub field: String,
    pub value: String,
    pub method: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Normalized {
    pub version: String,
    pub product: String,
    pub producer: String,
    pub time: TimeProvenance,
    pub values: BTreeMap<String, Provenance>,
    pub conflicts: Vec<String>,
    pub limitations: Vec<String>,
    pub dedup_key: Option<String>,
    pub event_refs: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimeProvenance {
    pub field: String,
    pub original: Option<String>,
    pub epoch_ms: Option<i64>,
    pub timezone: Option<String>,
    pub resolution_ms: Option<u64>,
    pub ambiguity: Option<String>,
}
impl Normalized {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|v| v.value.as_str())
    }
    fn put(&mut self, key: &str, field: &str, value: String, method: &str) {
        if value.trim().is_empty() || value == "-" {
            return;
        }
        if let Some(old) = self.values.get(key) {
            if old.value != value {
                self.conflicts.push(format!("{key}: {} e {field} divergem", old.field));
            }
            return;
        }
        self.values.insert(key.into(), Provenance { field: field.into(), value, method: method.into() });
    }
    fn alias(&mut self, ev: &Event, key: &str, fields: &[&str]) {
        // IDs, display names and alternative representations are not conflicts.
        if let Some((field, value)) =
            fields.iter().find_map(|f| field_text(ev, f).filter(|v| !v.is_empty()).map(|v| (*f, v)))
        {
            self.put(key, field, value, "adapter");
        }
    }
}

pub fn field_value<'a>(ev: &'a Event, path: &str) -> Option<&'a Value> {
    if let Some(v) =
        ev.fields.get(path).or_else(|| ev.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(path)).map(|(_, v)| v))
    {
        return Some(v);
    }
    // Parsers can retain a literal dotted prefix containing a nested object.
    let (mut value, remaining) =
        path.match_indices('.').rev().find_map(|(i, _)| ev.fields.get(&path[..i]).map(|v| (v, &path[i + 1..])))?;
    let parts = remaining.split('.');
    for part in parts {
        value = match value {
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => value.get(part)?,
        };
    }
    Some(value)
}
pub fn field_text(ev: &Event, path: &str) -> Option<String> {
    match path {
        "source" => return Some(ev.source.clone()),
        "message" => return Some(ev.message.clone()),
        "code" => return Some(ev.code.clone()),
        "event_ref" => return Some(event_ref(ev)),
        _ => {}
    }
    match field_value(ev, path)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
pub fn event_ref(ev: &Event) -> String {
    if !ev.event_ref.is_empty() {
        return ev.event_ref.clone();
    }
    evidence::stable_id(
        "event",
        [
            ev.id.to_string(),
            ev.source.clone(),
            ev.timestamp.map(|t| t.to_string()).unwrap_or_default(),
            ev.code.clone(),
            ev.raw.clone(),
            ev.message.clone(),
            serde_json::to_string(&ev.fields).unwrap_or_default(),
        ],
    )
}
fn joined(values: &[&str]) -> String {
    serde_json::to_string(values).unwrap_or_default()
}

pub fn product(ev: &Event) -> &'static str {
    let hint = format!(
        "{} {} {}",
        ev.source,
        field_text(ev, "winlog.provider_name").unwrap_or_default(),
        field_text(ev, "channel").unwrap_or_default()
    )
    .to_ascii_lowercase();
    if hint.contains("sysmon") {
        "sysmon"
    } else if hint.contains("powershell") {
        "powershell"
    } else if hint.contains("security-auditing") || hint == "security  " || field_text(ev, "TargetUserSid").is_some() {
        "windows-security"
    } else if hint.contains("service control manager")
        || hint.contains("taskscheduler")
        || hint.contains("windows defender")
    {
        "windows"
    } else if field_text(ev, "eventSource").is_some_and(|s| s.ends_with(".amazonaws.com")) {
        "aws"
    } else if field_text(ev, "protoPayload.methodName").is_some() {
        "gcp"
    } else if field_text(ev, "auditID").is_some() && field_text(ev, "objectRef.resource").is_some() {
        "kubernetes"
    } else if field_text(ev, "eventType").is_some() && (hint.contains("okta") || field_text(ev, "uuid").is_some()) {
        "okta"
    } else if field_text(ev, "tenantId").is_some() || field_text(ev, "TenantId").is_some() {
        "azure"
    } else if hint.contains("github") && field_text(ev, "action").is_some() {
        "github"
    } else if field_text(ev, "audit_serial").is_some() || hint.contains("auditd") {
        "auditd"
    } else if field_text(ev, "event_type").is_some() && field_text(ev, "flow_id").is_some() {
        "suricata"
    } else if field_text(ev, "id.orig_h").is_some() {
        "zeek"
    } else if field_text(ev, "http.request.method").is_some()
        || field_text(ev, "request_method").is_some()
        || field_text(ev, "method").is_some() && entities::value(ev, Role::Url).is_some()
    {
        "web"
    } else if hint.contains("sshd")
        || field_text(ev, "process").is_some_and(|v| v == "sshd")
        || field_text(ev, "program").is_some_and(|v| v == "sshd")
        || ev.message.contains("sshd[")
    {
        "sshd"
    } else if field_text(ev, "event.category").is_some() && field_text(ev, "event.action").is_some() {
        "ecs"
    } else {
        "generic"
    }
}

pub fn normalize(ev: &Event, mappings: &[SourceMapping]) -> (Event, Normalized) {
    let mut n = Normalized {
        version: evidence::NORMALIZATION_VERSION.into(),
        product: product(ev).into(),
        producer: ev.source.clone(),
        event_refs: vec![event_ref(ev)],
        time: TimeProvenance { field: "timestamp".into(), epoch_ms: ev.timestamp, ..Default::default() },
        ..Default::default()
    };
    for field in ["@timestamp", "eventTime", "TimeCreated", "timestamp", "time", "ts"] {
        if let Some(original) = field_text(ev, field) {
            n.time.field = field.into();
            n.time.original = Some(original);
            break;
        }
    }
    if let Some(original) = n.time.original.as_ref() {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(original) {
            n.time.timezone = Some(dt.offset().to_string());
            n.time.resolution_ms = Some(if original.contains('.') { 1 } else { 1000 });
        } else if original.parse::<f64>().is_ok() {
            n.time.timezone = Some("epoch UTC".into());
        } else {
            n.time.ambiguity = Some("Fuso e resolução dependem da configuração de importação da fonte".into());
        }
    }
    n.alias(
        ev,
        "namespace",
        &[
            "tenantId",
            "TenantId",
            "organization.id",
            "org_id",
            "userIdentity.accountId",
            "protoPayload.resourceName.project",
            "resource.labels.project_id",
            "cluster.uid",
            "cluster.name",
            "TargetDomainName",
            "SubjectDomainName",
            "user.domain",
        ],
    );
    n.alias(
        ev,
        "actor",
        &[
            "userIdentity.principalId",
            "protoPayload.authenticationInfo.principalEmail",
            "actor.id",
            "user.id",
            "SubjectUserSid",
            "SubjectUserName",
        ],
    );
    n.alias(
        ev,
        "target",
        &[
            "TargetUserSid",
            "TargetUserName",
            "requestParameters.userName",
            "objectRef.uid",
            "objectRef.name",
            "target.id",
        ],
    );
    n.alias(ev, "host", &["host.id", "agent.id", "Computer", "computer", "host.name", "hostname"]);
    n.alias(ev, "service", &["eventSource", "protoPayload.serviceName", "service.name", "appId", "appDisplayName"]);
    n.alias(
        ev,
        "request",
        &["request.id", "http.request.id", "requestId", "requestID", "trace.id", "trace_id", "auditID"],
    );
    n.alias(
        ev,
        "session",
        &[
            "TargetLogonId",
            "SubjectLogonId",
            "winlog.logon.id",
            "session.id",
            "authenticationContext.externalSessionId",
        ],
    );
    n.alias(ev, "connection", &["network.community_id", "connection.id", "flow_id", "uid"]);
    n.alias(ev, "process", &["ProcessGuid", "process.entity_id"]);
    n.alias(ev, "parent", &["ParentProcessGuid", "process.parent.entity_id"]);
    n.alias(ev, "credential", &["userIdentity.accessKeyId", "credential.id", "servicePrincipalCredentialKeyId"]);
    n.alias(ev, "created_credential", &["responseElements.accessKey.accessKeyId", "createdCredential.id"]);
    n.alias(
        ev,
        "resource",
        &["resource.id", "objectRef.uid", "protoPayload.resourceName", "requestParameters.resourceArn"],
    );
    n.alias(ev, "command", &["CommandLine", "process.command_line", "ScriptBlockText", "cmdline", "command_line"]);
    n.alias(ev, "file", &["TargetFilename", "file.path", "target_file", "download.path"]);
    n.alias(ev, "request_command", &["http.request.body.command", "http.request.body.cmd"]);
    n.alias(ev, "url", &["url.original", "url.full", "http.url", "download.url", "request_uri", "uri"]);
    if n.get("request_command").is_none() && n.product == "web" {
        if let Some(url) = n.get("url").map(str::to_string) {
            if let Some(query) = url.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or(q)) {
                if let Some((parameter, value)) = query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| {
                    ["cmd", "command", "exec", "execute", "shell"].contains(&k.to_ascii_lowercase().as_str())
                }) {
                    let field = n.values["url"].field.clone();
                    n.put(
                        "request_command",
                        &field,
                        decode_request(&value.replace('+', " ")),
                        &format!("query parameter {parameter}: form/percent decoding (max 2 passes)"),
                    );
                }
            }
        }
        if n.get("request_command").is_none() {
            if let Some(body) = field_text(ev, "http.request.body").filter(|s| s.len() <= 65536) {
                if let Ok(value) = serde_json::from_str::<Value>(&body) {
                    for key in ["cmd", "command", "exec"] {
                        if let Some(command) = value.get(key).and_then(Value::as_str) {
                            n.put(
                                "request_command",
                                "http.request.body",
                                command.into(),
                                &format!("JSON object command field: {key}"),
                            );
                            break;
                        }
                    }
                }
            }
        }
    }
    n.alias(
        ev,
        "persistence_target",
        &["service.executable", "task.executable", "registry.data.path", "systemd.exec_start.path"],
    );
    for (key, aliases) in [
        ("application", &["appId", "ApplicationId", "application.id", "servicePrincipalId"][..]),
        ("grant", &["oauth.grant.id", "grant.id"][..]),
        ("token", &["token_id", "token.id", "hashed_token", "UniqueTokenIdentifier"][..]),
        ("repository", &["repository_id", "repo_id", "repository.id", "repo"][..]),
        ("pipeline", &["workflow_id", "pipeline.id", "workflow.id"][..]),
        ("run", &["run_id", "workflow_run_id", "pipeline.run.id", "workflow.run.id"][..]),
        ("revision", &["head_sha", "commit_id", "git.commit.id", "workflow.sha"][..]),
        ("secret", &["secret.id", "secret.name", "secret_name"][..]),
        ("certificate", &["CertThumbprint", "certificate.fingerprint.sha1", "certificate.thumbprint"][..]),
        ("certificate_issuer", &["CertIssuerName", "certificate.issuer"][..]),
        ("certificate_subject", &["certificate.subject.upn", "certificate.san.upn"][..]),
        ("requester", &["certificate.requester.upn", "requester.upn"][..]),
        ("beneficiary", &["delegation.beneficiary.sid", "delegation.beneficiary_sid"][..]),
        ("delegator", &["delegation.delegator.sid", "delegation.delegator_sid"][..]),
        ("resource_spn", &["delegation.resource_spn", "ServiceName", "service.spn"][..]),
        ("destination", &["destination.address", "destination.ip", "IpAddress", "remote.host"][..]),
        ("artifact", &["artifact.sha256", "file.hash.sha256", "process.hash.sha256", "SHA256"][..]),
        ("logon", &["TargetLogonId", "SubjectLogonId", "winlog.logon.id", "logon.id"][..]),
        ("remote_session", &["ssh.session.id", "remote.session.id", "ses"][..]),
        ("source_address", &["source.ip", "SourceIp", "IpAddress", "src_ip"][..]),
    ] {
        n.alias(ev, key, aliases);
    }
    if n.get("certificate").is_none() {
        if let (Some(issuer), Some(serial)) = (
            n.get("certificate_issuer").map(str::to_string),
            field_text(ev, "CertSerialNumber").or_else(|| field_text(ev, "certificate.serial_number")),
        ) {
            n.put("certificate", "certificate.issuer+serial_number", joined(&[&issuer, &serial]), "adapter");
        }
    }

    let (action, outcome) = entities::action_outcome(ev);
    n.alias(
        ev,
        "principal",
        if matches!(action, Some("logon" | "auth_success")) {
            &["user.id", "TargetUserSid", "auid"]
        } else {
            &["user.id", "SubjectUserSid", "auid"]
        },
    );
    if action == Some("process_start") {
        if let Some(id) = field_text(ev, "SubjectLogonId") {
            n.values.remove("logon");
            n.put("logon", "SubjectLogonId", id, "adapter");
        }
    }
    if n.get("artifact").is_none() {
        if let Some(hashes) = field_text(ev, "Hashes") {
            if let Some(hash) = hashes
                .split(',')
                .find_map(|p| p.trim().strip_prefix("SHA256="))
                .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            {
                n.put("artifact", "Hashes", hash.to_ascii_lowercase(), "SHA256 entry");
            }
        }
    }
    if n.get("remote_session").is_some_and(|s| ["-1", "4294967295"].contains(&s)) {
        n.values.remove("remote_session");
    }
    let action_field = ["event.action", "eventName", "eventType", "Operation", "type", "verb"]
        .into_iter()
        .find(|f| field_value(ev, f).is_some())
        .unwrap_or(if ev.code.is_empty() { "message" } else { "code" });
    let outcome_field = [
        "event.outcome",
        "responseElements.ConsoleLogin",
        "result",
        "ResultType",
        "errorCode",
        "responseStatus.code",
        "success",
        "res",
        "Status",
    ]
    .into_iter()
    .find(|f| field_value(ev, f).is_some())
    .unwrap_or(action_field);
    if n.product != "generic" {
        if let Some(a) = action {
            n.put("action", action_field, a.into(), "adapter");
        }
        if let Some(o) = outcome {
            n.put("outcome", outcome_field, o.into(), "adapter");
        }
    }
    // Explicit mappings fill gaps. They cannot silently override a known producer.
    for m in mappings.iter().filter(|m| m.source == ev.source) {
        for (role, field) in &m.fields {
            if let Some(mut value) = field_text(ev, field) {
                if role == "timestamp" {
                    let parsed = mapped_time(&value, m);
                    if n.product != "generic" && n.time.epoch_ms.is_some() {
                        if parsed != n.time.epoch_ms {
                            n.conflicts.push(format!("timestamp: adaptador e {field} divergem"));
                        }
                        continue;
                    }
                    n.time = TimeProvenance {
                        field: field.clone(),
                        original: Some(value),
                        epoch_ms: parsed,
                        timezone: m.timezone.clone(),
                        resolution_ms: Some(if m.timestamp_unit.as_deref() == Some("s") { 1000 } else { 1 }),
                        ambiguity: parsed
                            .is_none()
                            .then(|| "Horário mapeado sem formato, unidade ou fuso inequívoco".into()),
                    };
                    continue;
                }
                if role == "action" {
                    value = m.actions.get(&value).cloned().unwrap_or(value);
                }
                if role == "outcome" {
                    value = m.outcomes.get(&value).cloned().unwrap_or(value);
                }
                if n.product == "generic" && n.values.get(role).is_some_and(|p| p.method == "adapter") {
                    n.values.remove(role);
                }
                n.put(role, field, value, "mapping");
            }
        }
    }
    for (key, role) in
        [("host", Role::Host), ("actor", Role::User), ("command", Role::CommandLine), ("file", Role::File)]
    {
        if !n.values.contains_key(key) {
            if let Some(v) = entities::value(ev, role) {
                n.put(key, entities::info(role).column, v.into_owned(), "alias");
            }
        }
    }
    // PID reuse: bind a PID only when its recorded start identity exists.
    if n.get("process").is_none() {
        if let (Some(pid), Some(start)) = (field_text(ev, "process.pid"), field_text(ev, "process.start")) {
            n.put("process", "process.pid+process.start", joined(&[&pid, &start]), "adapter");
        }
    }
    if n.get("action").is_none() {
        if let Some(a) = action {
            n.put("action", action_field, a.into(), "inference");
        }
    }
    if n.get("outcome").is_none() {
        if let Some(o) = outcome {
            n.put("outcome", outcome_field, o.into(), "inference");
        }
    }
    if n.product == "generic" && n.get("action").is_some() {
        n.limitations.push("Ação inferida em fonte genérica; confirme o mapeamento.".into());
    }

    let protection = ["event.outcome", "verdict.action", "alert.action", "event.disposition", "action", "act"]
        .iter()
        .find_map(|f| {
            field_text(ev, f)
                .filter(|v| {
                    matches!(
                        v.to_ascii_lowercase().as_str(),
                        "blocked" | "block" | "drop" | "dropped" | "deny" | "denied" | "quarantined"
                    )
                })
                .map(|v| (*f, v))
        });
    if let Some((field, v)) = protection {
        n.values.remove("outcome");
        n.put("outcome", field, "blocked".into(), "adapter");
        n.put("protection", field, v, "adapter");
    }
    if n.time.epoch_ms.is_none() {
        n.limitations.push("Registro sem horário inequívoco; não participa de correlações temporais.".into());
    }

    let host = n.get("host").unwrap_or("").to_string();
    // Local accounts are namespaced by host, never by their display name alone.
    if n.get("namespace").is_none()
        && !host.is_empty()
        && !matches!(n.product.as_str(), "aws" | "gcp" | "azure" | "okta" | "kubernetes")
    {
        n.put("namespace", "host", format!("host:{host}"), "derived");
    }
    let namespace = n.get("namespace").unwrap_or("").to_string();
    if n.get("service").is_none() && matches!(n.product.as_str(), "sshd" | "windows-security") {
        n.put("service", "producer", n.product.clone(), "adapter");
    }
    if let Some(request_command) = n.get("request_command").map(str::to_string) {
        n.put("command_key", "http.request.body.command", request_command, "derived");
    } else if let Some(command) = n.get("command").map(str::to_string) {
        n.put("command_key", "command", command, "derived");
    }
    for key in ["process", "parent", "session", "connection"] {
        if let Some(v) = n.get(key).map(str::to_string) {
            if host.is_empty() {
                n.limitations.push(format!("{key} sem host: vínculo não utilizável"));
                n.values.remove(key);
            } else {
                n.values.get_mut(key).unwrap().value = joined(&[&namespace, &host, &v]);
            }
        }
    }
    if let Some(v) = n.get("request").map(str::to_string) {
        let service = n.get("service").unwrap_or("").to_string();
        if namespace.is_empty() && host.is_empty() && service.is_empty() {
            n.values.remove("request");
            n.limitations.push("Requisição sem escopo verificável".into());
        } else {
            n.values.get_mut("request").unwrap().value = joined(&[&namespace, &host, &service, &v]);
        }
    }
    for key in [
        "application",
        "grant",
        "token",
        "repository",
        "pipeline",
        "run",
        "revision",
        "certificate",
        "beneficiary",
        "delegator",
        "resource_spn",
    ] {
        if let Some(v) = n.get(key).map(str::to_string) {
            let v = if key == "certificate" && v.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
                v.replace(':', "").to_ascii_lowercase()
            } else {
                v
            };
            if namespace.is_empty() {
                n.values.remove(key);
            } else {
                n.values.get_mut(key).unwrap().value = joined(&[&namespace, &v]);
            }
        }
    }
    if let Some(v) = n.get("artifact").map(str::to_string).filter(|s| s.chars().all(|c| c.is_ascii_hexdigit())) {
        n.values.get_mut("artifact").unwrap().value = v.to_ascii_lowercase();
    }
    for key in ["logon", "remote_session"] {
        if let Some(v) = n.get(key).map(str::to_string) {
            if host.is_empty() {
                n.values.remove(key);
            } else {
                n.values.get_mut(key).unwrap().value = joined(&[&namespace, &host, &v]);
            }
        }
    }
    if n.get("certificate_subject").zip(n.get("requester")).is_some_and(|(a, b)| !a.eq_ignore_ascii_case(b)) {
        n.put(
            "certificate_identity_mismatch",
            "certificate.subject.upn+certificate.requester.upn",
            "true".into(),
            "derived",
        );
    }
    if let Some(user) = entities::value(ev, Role::User) {
        n.put("identity", "namespace+@user", joined(&[&namespace, &user]), "derived");
    }
    for key in ["credential", "created_credential"] {
        if let Some(v) = n.get(key).map(str::to_string) {
            if namespace.is_empty() {
                n.values.remove(key);
                n.limitations.push(format!("{key} sem conta/tenant"));
            } else {
                n.values.get_mut(key).unwrap().value = joined(&[&namespace, &v]);
            }
        }
    }
    for (key, v) in [
        ("file_key", n.get("file").map(str::to_string)),
        ("persistence_key", n.get("persistence_target").map(str::to_string)),
        ("image_key", entities::value(ev, Role::Process).map(|v| v.into_owned())),
    ] {
        if let Some(v) = v {
            if !host.is_empty() {
                n.put(
                    key,
                    "namespace+host+path",
                    joined(&[&namespace, &host, &canonical_path(&v, &n.product)]),
                    "derived",
                );
            }
        }
    }
    let unique = match n.product.as_str() {
        "aws" => field_text(ev, "eventID")
            .map(|v| joined(&["aws", &namespace, &field_text(ev, "recipientAccountId").unwrap_or_default(), &v])),
        "kubernetes" => field_text(ev, "auditID")
            .map(|v| joined(&["kubernetes", &namespace, &host, &v, &field_text(ev, "stage").unwrap_or_default()])),
        "okta" => field_text(ev, "uuid").map(|v| joined(&["okta", &namespace, &v])),
        _ => field_text(ev, "EventRecordID")
            .or_else(|| field_text(ev, "winlog.record_id"))
            .filter(|_| !host.is_empty())
            .map(|v| {
                joined(&[
                    &n.product,
                    &namespace,
                    &host,
                    &field_text(ev, "channel").unwrap_or_default(),
                    &v,
                    &ev.timestamp.map(|t| t.to_string()).unwrap_or_default(),
                ])
            }),
    };
    n.dedup_key = unique;
    let mut derived = ev.clone();
    derived.timestamp = n.time.epoch_ms;
    derived.event_ref = event_ref(ev);
    derived.name.clear();
    derived.description.clear();
    derived.fields.retain(|k, _| !k.starts_with("_sec."));
    derived.fields.insert("_sec.product".into(), Value::from(n.product.clone()));
    for (k, p) in &n.values {
        derived.fields.insert(format!("_sec.{k}"), Value::from(p.value.clone()));
    }
    if let Some(cmd) = n.get("command") {
        let mut command_event = Event::empty();
        command_event.fields.insert("CommandLine".into(), Value::from(cmd));
        let decoded = entities::decode_payloads(&command_event);
        if let Some(d) = decoded.first() {
            derived.fields.insert("_sec.decoded_command".into(), Value::from(d.text.clone()));
        }
        if suspicious_command(cmd) || decoded.iter().any(|d| suspicious_command(&d.text)) {
            derived.fields.insert("_sec.suspicious_command".into(), Value::from("true"));
        }
        if !literal_output(cmd) && (reverse_shell(cmd) || decoded.iter().any(|d| reverse_shell(&d.text))) {
            derived.fields.insert("_sec.reverse_shell".into(), Value::from("true"));
        }
    }
    if n.get("request_command").is_some_and(suspicious_command) {
        derived.fields.insert("_sec.suspicious_request".into(), Value::from("true"));
    }
    if let Some(command) = n.get("request_command") {
        let decoded = decode_request(command);
        if reverse_shell(&decoded) && !literal_output(&decoded) {
            derived.fields.insert("_sec.reverse_shell_request".into(), Value::from("true"));
        }
    }
    derived.fields.insert("_sec.namespace".into(), Value::from(namespace));
    derived.fields.insert("_sec.outcome".into(), Value::from(n.get("outcome").unwrap_or("unknown")));
    if field_text(ev, "event.kind").is_some_and(|v| matches!(v.as_str(), "documentation" | "example"))
        || n.get("command").is_some_and(literal_output)
    {
        derived.fields.insert("_sec.literal_output".into(), Value::from("true"));
    }
    (derived, n)
}

/// Bounded text transformation; never evaluates the decoded payload.
fn decode_request(text: &str) -> String {
    let mut end = text.len().min(65536);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut value = text[..end].to_string();
    for _ in 0..2 {
        value = crate::threats::percent_decode(&value);
    }
    value
}

/// Complete interactive shell + socket/redirection grammar. A network address,
/// /dev/tcp probe, encoded string or shell name alone cannot satisfy this gate.
pub fn reverse_shell(command: &str) -> bool {
    static PATTERNS: std::sync::OnceLock<regex::RegexSet> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| regex::RegexSet::new([
        r"(?i)^\s*(?:/bin/)?(?:bash|sh|zsh)\s+-i\s+(?:[012]?>&|&>)\s*/dev/tcp/[a-z0-9_.:-]+/[0-9]{1,5}\s+0>&[12]\b",
        r"(?i)^\s*(?:/usr/bin/)?(?:nc|ncat|netcat)(?:\.exe)?\s+(?:[a-z0-9_.:-]+\s+[0-9]{1,5}\s+-(?:e|c)\s+(?:/bin/(?:ba)?sh|cmd(?:\.exe)?|powershell(?:\.exe)?)|-(?:e|c)\s+(?:/bin/(?:ba)?sh|cmd(?:\.exe)?|powershell(?:\.exe)?)\s+[a-z0-9_.:-]+\s+[0-9]{1,5})\b",
        r"(?i)^\s*(?:/usr/bin/)?socat\s+(?:tcp(?:4|6)?:[a-z0-9_.:-]+:[0-9]{1,5}\s+exec:(?:/bin/(?:ba)?sh|cmd(?:\.exe)?)|exec:(?:/bin/(?:ba)?sh|cmd(?:\.exe)?)[^\r\n]*\s+tcp(?:4|6)?:[a-z0-9_.:-]+:[0-9]{1,5})",
    ]).unwrap());
    // Only executable shell positions qualify. Semicolons inside arguments,
    // comments, Python strings and PowerShell output are not execution.
    fn executable(text: &str, patterns: &regex::RegexSet, depth: usize) -> bool {
        if depth > 3 || text.contains("<<") {
            return false;
        }
        static LAUNCH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let launch =
            LAUNCH.get_or_init(|| regex::Regex::new(r"(?i)^\s*(?:/bin/)?(?:bash|sh|zsh)\s+-(?:c|lc)\s+").unwrap());
        let mut start = 0;
        let mut quote = None;
        let mut escape = false;
        let mut comment = false;
        let check = |part: &str| {
            if patterns.is_match(part) {
                return true;
            }
            if let Some(m) = launch.find(part) {
                let body = part[m.end()..].trim();
                if let Some(q) = body.chars().next().filter(|q| *q == '\'' || *q == '"') {
                    if body.len() > 1 && body.ends_with(q) {
                        return executable(&body[1..body.len() - 1], patterns, depth + 1);
                    }
                }
            }
            false
        };
        for (i, ch) in text.char_indices() {
            if comment {
                if ch == '\n' {
                    comment = false;
                    start = i + 1;
                }
                continue;
            }
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' && quote != Some('\'') {
                escape = true;
                continue;
            }
            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                }
                continue;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                continue;
            }
            if ch == '#' && (i == 0 || text[..i].chars().next_back().is_some_and(char::is_whitespace)) {
                if check(&text[start..i]) {
                    return true;
                }
                comment = true;
                continue;
            }
            if matches!(ch, ';' | '|' | '&' | '(' | ')' | '\r' | '\n') {
                // Check the complete suffix so redirections such as >& are
                // available to the grammar; a later delimiter cannot make
                // a quoted argument into a command.
                if check(&text[start..]) {
                    return true;
                }
                start = i + ch.len_utf8();
            }
        }
        !comment && check(&text[start..])
    }
    executable(command, patterns, 0)
}

fn mapped_time(value: &str, mapping: &SourceMapping) -> Option<i64> {
    if let Ok(number) = value.parse::<f64>() {
        let multiplier = match mapping.timestamp_unit.as_deref()? {
            "s" => 1000.0,
            "ms" => 1.0,
            "us" => 0.001,
            "ns" => 0.000001,
            _ => return None,
        };
        let ms = number * multiplier;
        return (ms.is_finite() && ms >= i64::MIN as f64 && ms <= i64::MAX as f64).then_some(ms as i64);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(dt.timestamp_millis());
    }
    let zone = mapping.timezone.as_deref()?;
    chrono::DateTime::parse_from_rfc3339(&format!("{}{zone}", value.replace(' ', "T")))
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn literal_output(command: &str) -> bool {
    static PRINT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let print=PRINT.get_or_init(||regex::Regex::new(r#"(?i)^\s*(?:(?:cmd(?:\.exe)?\s+/[ck]|(?:powershell|pwsh)(?:\.exe)?\s+-(?:command|c))\s+)?["']?(?:echo|printf|write-output|write-host)\b"#).unwrap());
    if !print.is_match(command) {
        return false;
    }
    let mut quote = None;
    let mut escape = false;
    for ch in command.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '`' || ch == '\\' {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if matches!(ch, ';' | '&' | '|' | '>') {
            return false;
        }
    }
    true
}

/// Specific combinations, never a tool name, base64, a public address or rarity alone.
pub fn suspicious_command(command: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"(?i)(sekurlsa::|lsadump::|/dev/tcp/|\b(?:nc|ncat)\b[^\r\n]*\s-[ec]\s|(?:curl|wget)\b[^\r\n]*\|\s*(?:ba)?sh\b|(?:iex|invoke-expression)\b[^\r\n]*(?:downloadstring|invoke-webrequest)|(?:downloadstring|invoke-webrequest)[^\r\n]*\b(?:iex|invoke-expression)\b|vssadmin\b[^\r\n]*delete\s+shadows|wmic\b[^\r\n]*shadowcopy\s+delete|bcdedit\b[^\r\n]*recoveryenabled\s+no|regsvr32\b[^\r\n]*/i:https?://|mshta\s+(?:https?://|javascript:)|rundll32[^\r\n]*comsvcs[^\r\n]*minidump|reg\s+save\s+hklm\\(?:sam|security|system)\b|dd\s+[^\r\n]*if=/dev/(?:zero|urandom)[^\r\n]*of=/dev/(?:sd|nvme))").expect("reviewed command expression")).is_match(command)
}

fn canonical_path(path: &str, product: &str) -> String {
    let p = path.trim().trim_matches('"');
    if matches!(product, "sysmon" | "windows" | "windows-security" | "powershell") {
        p.replace('/', "\\").to_lowercase()
    } else {
        p.to_string()
    }
}

pub fn validate_mappings(mappings: &[SourceMapping]) -> Result<(), String> {
    if mappings.len() > 100 {
        return Err("Máximo de 100 mapeamentos de fonte".into());
    }
    let mut sources = std::collections::HashSet::new();
    for m in mappings {
        if m.timestamp_unit.as_deref().is_some_and(|u| !["s", "ms", "us", "ns"].contains(&u)) {
            return Err("timestamp_unit deve ser s, ms, us ou ns".into());
        }
        if m.timezone
            .as_deref()
            .is_some_and(|z| chrono::DateTime::parse_from_rfc3339(&format!("2000-01-01T00:00:00{z}")).is_err())
        {
            return Err("timezone deve ser Z ou deslocamento explícito, como -03:00".into());
        }
        if m.source.trim().is_empty() || !sources.insert(&m.source) {
            return Err("Fonte vazia ou duplicada no mapeamento".into());
        }
        if m.fields.keys().any(|k| !MAPPABLE.contains(&k.as_str())) {
            return Err("Papel não suportado no mapeamento".into());
        }
        if m.fields.values().any(|f| f.is_empty() || f.starts_with("_sec.") || f.len() > 256) {
            return Err("Campo inválido no mapeamento".into());
        }
        if m.outcomes.values().any(|v| !["success", "failure", "blocked", "unknown"].contains(&v.as_str())) {
            return Err("Resultado mapeado inválido".into());
        }
    }
    Ok(())
}
