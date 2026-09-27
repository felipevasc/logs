//! Roles evidenced in an operation, not attribution to a real person.
use crate::{model::Event, security_normalize::Normalized};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::LazyLock,
};

pub const VERSION: &str = "participants-1";
const MAX_FACTS: usize = 64;
const MAX_ORIGINS: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Origin {
    pub event_ref: String,
    pub event_id: usize,
    pub field: String,
    pub method: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    pub side: String,
    pub kind: String,
    pub value: String,
    pub role: String,
    pub certainty: String,
    pub namespace: String,
    pub origins: Vec<Origin>,
    pub origins_limited: bool,
}
impl Fact {
    fn key(&self) -> String {
        serde_json::to_string(&(
            &self.side,
            &self.kind,
            &self.value,
            &self.role,
            &self.certainty,
            &self.namespace,
        ))
        .unwrap()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Participants {
    pub version: String,
    pub facts: Vec<Fact>,
    pub limited: bool,
}
impl Default for Participants {
    fn default() -> Self {
        Self {
            version: VERSION.into(),
            facts: vec![],
            limited: false,
        }
    }
}
impl Participants {
    pub fn merge(&mut self, other: &Self) {
        self.limited |= other.limited;
        for fact in &other.facts {
            self.insert(fact.clone());
        }
    }
    fn insert(&mut self, mut fact: Fact) {
        if let Some(old) = self.facts.iter_mut().find(|v| {
            v.side == fact.side
                && v.kind == fact.kind
                && v.value == fact.value
                && v.role == fact.role
                && v.namespace == fact.namespace
        }) {
            if fact.certainty == "observed" {
                old.certainty = fact.certainty.clone();
            }
            old.origins.append(&mut fact.origins);
            old.origins.sort();
            old.origins.dedup();
            old.origins_limited |= fact.origins_limited || old.origins.len() > MAX_ORIGINS;
            old.origins.truncate(MAX_ORIGINS);
            self.facts.sort_by_key(Fact::key);
        } else {
            self.facts.push(fact);
            self.facts.sort_by_key(Fact::key);
            let mut counts = BTreeMap::new();
            self.facts.retain(|f| {
                let count = counts.entry(f.side.clone()).or_insert(0);
                *count += 1;
                *count <= MAX_FACTS
            });
            self.limited |= counts.values().any(|&n| n > MAX_FACTS);
        }
    }
    /// Grouping must never conceal different identities discovered by this resolver.
    pub fn signature(&self) -> String {
        crate::evidence::stable_id(VERSION, self.facts.iter().map(Fact::key))
    }
}
#[derive(Deserialize)]
struct Alias {
    side: String,
    kind: String,
    role: String,
    context: String,
    fields: Vec<String>,
}
static ALIASES: LazyLock<Vec<Alias>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../resources/participant-fields.json"))
        .expect("participant alias catalog")
});
static LOOKUP: LazyLock<BTreeMap<String, Vec<usize>>> = LazyLock::new(|| {
    let mut map: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, a) in ALIASES.iter().enumerate() {
        for f in &a.fields {
            map.entry(f.to_ascii_lowercase()).or_default().push(i);
        }
    }
    map
});
static SSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\bsshd(?:\[\d+\])?:\s+)(?:Failed|Accepted) (?:password|publickey|keyboard-interactive(?:/pam)?) for (?:invalid user )?(?P<user>[^\s]{1,256}) from (?P<ip>[0-9a-fA-F:.]+) port \d+").unwrap()
});
static ACCESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?P<ip>\S+)\s+\S+\s+(?P<user>\S+)\s+\[\d{2}/[A-Za-z]{3}/\d{4}:[^\]]+\]\s+"(?:GET|POST|PUT|DELETE|HEAD|OPTIONS|PATCH)\s+[^\r\n]+\s+HTTP/\d(?:\.\d)?"\s+\d{3}\b"#).unwrap()
});
static SYSLOG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:<\d{1,3}>)?(?:1\s+\S+|[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+(?P<host>[^\s]{1,256})\s+").unwrap()
});
static CLIENT_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\[client (?P<ip>[0-9a-fA-F:.]+)\]|(?:^|, )client: (?P<nginx>[0-9a-fA-F:.]+), server:",
    )
    .unwrap()
});
static FORWARDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(?:^|[;,]\s*)for=(?:"([^"]+)"|([^;,\s]+))"#).unwrap());
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:^|\s)(src|src_ip|dst|dst_ip|suser|duser|rhost|addr|acct)=(?:"([^"\r\n]{1,256})"|'([^'\r\n]{1,256})'|([^\s"']{1,256}))"#).unwrap()
});

fn ip(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"');
    let parsed = value
        .parse::<IpAddr>()
        .ok()
        .or_else(|| value.parse::<SocketAddr>().ok().map(|a| a.ip()))
        .or_else(|| {
            value
                .strip_prefix('[')
                .and_then(|v| v.strip_suffix(']'))
                .and_then(|v| v.parse().ok())
        })?;
    let parsed = match parsed {
        IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v)),
        v => v,
    };
    if parsed.is_unspecified()
        || parsed.is_multicast()
        || matches!(parsed, IpAddr::V4(v) if v.is_broadcast())
    {
        return None;
    }
    Some(parsed.to_string())
}
fn blocked_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("_sec.")
        || lower == "_sec"
        || [
            "request.body",
            "response.body",
            "request_body",
            "response_body",
        ]
        .iter()
        .any(|s| lower.contains(s))
        || lower.split('.').any(|s| {
            [
                "payload",
                "command",
                "command_line",
                "commandline",
                "script",
                "scriptblocktext",
                "_sec",
            ]
            .contains(&s)
        })
}

struct Context<'a> {
    ev: &'a Event,
    norm: &'a Normalized,
    auth: bool,
    windows_auth: bool,
    linux_auth: bool,
    web: bool,
    local: bool,
    observer: bool,
    outbound: bool,
    ambiguous_outbound: bool,
    response: bool,
}
impl Context<'_> {
    fn allows(&self, context: &str) -> bool {
        match context {
            "any" => true,
            "auth" => self.auth,
            "windows_auth" => self.windows_auth,
            "linux_auth" => self.linux_auth,
            "web" => self.web,
            "local_action" => self.local && !self.auth,
            "host" => !self.observer && (self.local || self.web || self.auth),
            "component" => !self.observer,
            "kubernetes" => self.norm.product == "kubernetes",
            "cloud" => matches!(self.norm.product.as_str(), "aws" | "azure" | "gcp"),
            _ => false,
        }
    }
    fn add(
        &self,
        out: &mut Participants,
        side: &str,
        kind: &str,
        role: &str,
        raw: &str,
        field: &str,
        method: &str,
    ) {
        let value = raw.trim();
        if value.is_empty()
            || value.len() > 512
            || ["-", "(null)", "null", "unknown", "N/A", "?"].contains(&value)
            || value.chars().any(char::is_control)
        {
            return;
        }
        let value = if kind == "ip" {
            let Some(value) = ip(value) else { return };
            value
        } else {
            value.to_string()
        };
        let (mut side, mut role, mut certainty) = (
            side,
            role,
            if method == "alias" || method == "mapping" {
                "observed"
            } else {
                "contextual"
            },
        );
        // Packet direction is not automatically attack direction.
        if ["source", "destination"].contains(&role) {
            if self.ambiguous_outbound || self.response {
                side = "context";
                certainty = "contextual";
            } else if self.outbound {
                if side == "attacker" {
                    side = "victim";
                    role = "affected_asset";
                } else {
                    side = "attacker";
                    role = "remote_endpoint";
                }
                certainty = "contextual";
            }
        }
        if role == "forwarded_unverified" {
            certainty = "unverified";
        }
        let domain = if kind == "user" && !value.starts_with("arn:") {
            value
                .split_once('\\')
                .map(|(d, _)| d)
                .or_else(|| value.rsplit_once('@').map(|(_, d)| d))
                .filter(|v| !v.is_empty() && v.len() < 256)
                .map(str::to_string)
        } else {
            None
        };
        out.insert(Fact {
            side: side.into(),
            kind: kind.into(),
            value,
            role: role.into(),
            certainty: certainty.into(),
            namespace: self.norm.get("namespace").unwrap_or("").into(),
            origins: vec![Origin {
                event_ref: crate::security_normalize::event_ref(self.ev),
                event_id: self.ev.id,
                field: field.into(),
                method: method.into(),
            }],
            origins_limited: false,
        });
        if let Some(domain) = domain {
            self.add(
                out,
                side,
                "domain",
                role,
                &domain,
                field,
                "domínio registrado na identidade",
            );
        }
    }
    fn field(&self, out: &mut Participants, path: &str, canonical: &str, value: &str) {
        if blocked_path(canonical) {
            return;
        }
        // Known wrappers may change; arbitrary payload objects are not promoted.
        let lower = canonical.to_ascii_lowercase();
        let keys = std::iter::once(lower.as_str())
            .chain(lower.match_indices('.').map(|(i, _)| &lower[i + 1..]));
        for key in keys {
            let Some(aliases) = LOOKUP.get(key) else {
                continue;
            };
            for &i in aliases {
                let a = &ALIASES[i];
                if !self.allows(&a.context) {
                    continue;
                }
                if a.role == "forwarded_unverified" {
                    if key.ends_with("forwarded") {
                        for cap in FORWARDED.captures_iter(value).take(16) {
                            self.add(
                                out,
                                &a.side,
                                &a.kind,
                                &a.role,
                                cap.get(1).or_else(|| cap.get(2)).unwrap().as_str(),
                                path,
                                "header não validado",
                            );
                        }
                    } else {
                        for v in value.split(',').take(16) {
                            self.add(
                                out,
                                &a.side,
                                &a.kind,
                                &a.role,
                                v,
                                path,
                                "header não validado",
                            );
                        }
                    }
                } else {
                    self.add(out, &a.side, &a.kind, &a.role, value, path, "alias");
                }
            }
            break;
        }
    }
    fn text(&self, out: &mut Participants, field: &str, text: &str) {
        // Log-shaped text inside a URI, header or user agent remains client input.
        // Explicit aliases such as X-Forwarded-For are handled separately above.
        if blocked_path(field)
            || field.to_ascii_lowercase().split('.').any(|part| {
                [
                    "url",
                    "uri",
                    "path",
                    "query",
                    "query_string",
                    "querystring",
                    "args",
                    "headers",
                    "referer",
                    "referrer",
                    "agent",
                    "user_agent",
                    "useragent",
                ]
                .contains(&part)
            })
        {
            return;
        }
        let text = text.trim();
        if text.starts_with('{')
            || text.starts_with("[{")
            || ["documentation:", "example:", "exemplo:"]
                .iter()
                .any(|p| text.to_ascii_lowercase().starts_with(p))
        {
            return;
        }
        if let Some(c) = ACCESS.captures(text) {
            self.add(
                out,
                "attacker",
                "ip",
                "source",
                &c["ip"],
                field,
                "Apache/Nginx Common/Combined Log",
            );
            self.add(
                out,
                "attacker",
                "user",
                "identity_used",
                &c["user"],
                field,
                "usuário autenticado no access log",
            );
            // The request and user agent may contain quoted log-looking payloads.
            return;
        }
        if let Some(c) = SSH.captures(text) {
            self.add(
                out,
                "attacker",
                "ip",
                "source",
                &c["ip"],
                field,
                "OpenSSH: peer remoto",
            );
            self.add(
                out,
                "victim",
                "user",
                "target_account",
                &c["user"],
                field,
                "OpenSSH: conta solicitada",
            );
            if let Some(header) = SYSLOG.captures(text) {
                self.add(
                    out,
                    "victim",
                    "host",
                    "affected_asset",
                    &header["host"],
                    field,
                    "host emissor de autenticação SSH",
                );
            }
        }
        if self.web || text.contains("[client ") || text.contains("client: ") {
            for c in CLIENT_ERROR.captures_iter(text).take(4) {
                self.add(
                    out,
                    "attacker",
                    "ip",
                    "source",
                    c.name("ip").or_else(|| c.name("nginx")).unwrap().as_str(),
                    field,
                    "peer no error log web",
                );
            }
        }
        // Structured syslog/audit key-value records only; never a query or command payload.
        if text.starts_with("CEF:")
            || text.starts_with("type=USER_")
            || (SYSLOG.is_match(text)
                && (self.linux_auth || text.contains(" SRC=") || text.contains(" src=")))
        {
            for c in KEY_VALUE.captures_iter(text).take(32) {
                self.field(
                    out,
                    field,
                    &c[1],
                    c.get(2)
                        .or_else(|| c.get(3))
                        .or_else(|| c.get(4))
                        .unwrap()
                        .as_str(),
                );
            }
        }
    }
}

pub fn extract(ev: &Event, norm: &Normalized, rule: &str, tactics: &[String]) -> Participants {
    let mut out = Participants::default();
    let get = |key: &str| crate::security_normalize::field_text(ev, key).unwrap_or_default();
    let code = if ev.code.is_empty() {
        get("winlog.event_id")
    } else {
        ev.code.clone()
    };
    let windows_auth =
        ["4624", "4625", "4648", "4768", "4769", "4771", "4776"].contains(&code.as_str());
    let ssh_record = |text: &str| {
        let text = text.trim();
        !text.starts_with('{')
            && !text.starts_with('[')
            && !ACCESS.is_match(text)
            && SSH.is_match(text)
    };
    let linux_auth = norm.product == "sshd"
        || (norm.product == "auditd"
            && (ev.raw.contains("type=USER_") || get("type").starts_with("USER_")))
        || ssh_record(&ev.message)
        || ssh_record(&ev.raw);
    let auth = windows_auth
        || linux_auth
        || norm
            .get("action")
            .is_some_and(|v| ["logon", "login", "authenticate", "authentication"].contains(&v));
    let web = norm.product == "web" || norm.get("url").is_some() || ACCESS.is_match(&ev.raw);
    let local = norm.get("command").is_some()
        || ["process", "file", "registry"]
            .iter()
            .any(|v| get("event.category").contains(v))
        || norm.product == "sysmon";
    let observer = !get("observer.type").is_empty()
        || !get("observer.ip").is_empty()
        || get("event.module").contains("suricata")
        || get("event.category").contains("network") && !local && !web && !auth;
    let direction = get("network.direction").to_ascii_lowercase();
    let egress = ["outbound", "egress"].contains(&direction.as_str());
    let remote_abuse = rule.contains("reverse-shell")
        || rule.contains("reverse_shell")
        || tactics
            .iter()
            .any(|t| matches!(t.as_str(), "command-and-control" | "exfiltration"));
    let ctx = Context {
        ev,
        norm,
        auth,
        windows_auth,
        linux_auth,
        web,
        local,
        observer,
        outbound: egress && remote_abuse,
        ambiguous_outbound: egress && !remote_abuse,
        response: !get("http.type").is_empty() && get("http.type") == "response",
    };
    // Explicit mappings retain their semantic role and original field provenance.
    for (key, side, kind, role) in [
        ("source_address", "attacker", "ip", "source"),
        ("destination", "victim", "ip", "destination"),
        ("actor", "attacker", "user", "identity_used"),
        ("target", "victim", "user", "target_account"),
        ("host", "victim", "host", "affected_asset"),
        ("service", "victim", "application", "target_application"),
        ("application", "victim", "application", "target_application"),
        ("resource", "victim", "resource", "target_resource"),
    ] {
        if let Some(p) = norm.values.get(key).filter(|p| p.method == "mapping") {
            ctx.add(&mut out, side, kind, role, &p.value, &p.field, "mapping");
        }
    }
    fn walk(
        ctx: &Context<'_>,
        out: &mut Participants,
        path: &str,
        canonical: &str,
        value: &serde_json::Value,
        depth: usize,
        nodes: &mut usize,
        bytes: &mut usize,
    ) {
        if *nodes >= 2048 || *bytes >= 256 * 1024 || depth > 12 {
            out.limited = true;
            return;
        }
        *nodes += 1;
        if blocked_path(canonical) {
            return;
        }
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    let c = if canonical.is_empty() {
                        k.clone()
                    } else {
                        format!("{canonical}.{k}")
                    };
                    walk(ctx, out, &p, &c, v, depth + 1, nodes, bytes);
                    if *nodes >= 2048 || *bytes >= 256 * 1024 {
                        out.limited = true;
                        break;
                    }
                }
            }
            serde_json::Value::Array(items) => {
                out.limited |= items.len() > 64;
                for (i, v) in items.iter().take(64).enumerate() {
                    walk(
                        ctx,
                        out,
                        &format!("{path}[{i}]"),
                        canonical,
                        v,
                        depth + 1,
                        nodes,
                        bytes,
                    );
                }
            }
            serde_json::Value::String(text) => {
                if *bytes + text.len().min(65536) > 256 * 1024 {
                    *bytes = 256 * 1024;
                    out.limited = true;
                    return;
                }
                *bytes += text.len().min(65536);
                if text.len() <= 65536 {
                    ctx.field(out, path, canonical, text);
                    ctx.text(out, path, text);
                } else {
                    out.limited = true;
                }
            }
            serde_json::Value::Number(number) => {
                ctx.field(out, path, canonical, &number.to_string())
            }
            _ => (),
        }
    }
    let (mut nodes, mut bytes) = (0, 0);
    for (k, v) in &ev.fields {
        walk(&ctx, &mut out, k, k, v, 0, &mut nodes, &mut bytes);
    }
    for (field, value) in [("message", ev.message.as_str()), ("raw", ev.raw.as_str())] {
        if value.len() <= 65536 {
            ctx.text(&mut out, field, value);
        } else {
            out.limited = true;
        }
    }
    // The syslog emitter is only an affected host when a local/auth operation supports that role.
    if ctx.allows("host") {
        if let Some(c) = SYSLOG.captures(&ev.raw) {
            ctx.add(
                &mut out,
                "victim",
                "host",
                "affected_asset",
                &c["host"],
                "raw",
                "host emissor syslog em operação local",
            );
        }
    }
    out
}

#[cfg(test)]
pub fn alias_count() -> usize {
    ALIASES.iter().map(|a| a.fields.len()).sum()
}
