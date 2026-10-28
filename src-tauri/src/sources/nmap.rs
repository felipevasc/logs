//! nmap greppable output (`-oG`), as a profile on top of the `labeled` reader: a host line is
//! `Host: 10.0.0.1 (name)<TAB>Ports: 22/open/tcp//ssh//OpenSSH 8.9/, 80/closed/tcp//http///<TAB>Ignored State: …`.
//!
//! Everything is added next to the generic reading (`label.*` and the generic fields stay as they
//! are): `destination.ip` and `host.name` (the canonical roles of the entities), `nmap.status`,
//! `nmap.ports` (a list with one object per port), counts per state and the open ports, services
//! and versions as plain text for search. Nothing happens unless the line has the shape of an
//! nmap host, so no other file is touched.
use crate::model::Event;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

fn entry_start() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|,\s+)(\d{1,5}/(?:open|closed|filtered|unfiltered)[|a-z]*/)").unwrap())
}

/// Entries are separated by ", " and start with `port/state/`; commas inside a version do not split.
fn entries(text: &str) -> Vec<&str> {
    let starts: Vec<usize> = entry_start().captures_iter(text).filter_map(|c| c.get(1)).map(|m| m.start()).collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            text[start..end].trim_end().trim_end_matches(',').trim_end()
        })
        .collect()
}

fn host_shape() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(\S+) \((.*)\)$").unwrap())
}

fn command_shape() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bnmap\s+(-\S.*)$").unwrap())
}

fn version_shape() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^#\s*nmap\s+(\d+(?:\.\d+)+\S*)\s+scan initiated").unwrap())
}

/// nmap writes `|` where the field would hold `/` (`ssl|http`, `mod_perl|2.0.11`).
fn slash(text: &str) -> String {
    text.replace('|', "/")
}

/// One `port/state/protocol/owner/service/rpc/version/` entry.
fn port(entry: &str) -> Option<Value> {
    let mut parts = entry.trim().split('/');
    let number: u32 = parts.next()?.trim().parse().ok().filter(|n| *n <= 65535)?;
    let state = parts.next()?.trim();
    if state.is_empty() {
        return None;
    }
    let protocol = parts.next().unwrap_or("").trim();
    let owner = parts.next().unwrap_or("").trim();
    let service = parts.next().unwrap_or("").trim();
    let rpc = parts.next().unwrap_or("").trim();
    let version = parts.next().unwrap_or("").trim();
    let mut object = serde_json::Map::new();
    object.insert("port".into(), json!(number));
    object.insert("state".into(), json!(slash(state)));
    for (key, value) in [("protocol", protocol), ("owner", owner), ("service", service), ("rpc", rpc), ("version", version)] {
        if !value.is_empty() {
            object.insert(key.into(), json!(slash(value)));
        }
    }
    Some(Value::Object(object))
}

fn text<'a>(ev: &'a Event, key: &str) -> Option<&'a str> {
    ev.fields.get(key).and_then(Value::as_str)
}

/// Adds the nmap fields to a `labeled` record (host lines and comment lines).
pub(super) fn derive(ev: &mut Event, line: &str) {
    if line.trim_start().starts_with('#') {
        return comment(ev, line);
    }
    let Some(host) = text(ev, "label.host").map(str::to_owned) else { return };
    let Some(shape) = host_shape().captures(&host) else { return };
    let address = shape[1].to_string();
    if address.parse::<std::net::IpAddr>().is_err() {
        return;
    }
    let ports_text = text(ev, "label.ports").map(str::to_owned);
    let status = text(ev, "label.status").map(str::to_owned);
    if ports_text.is_none() && status.is_none() {
        return;
    }
    ev.fields.entry("destination.ip").or_insert_with(|| Value::from(address));
    let name = shape[2].trim();
    if !name.is_empty() {
        ev.fields.entry("host.name").or_insert_with(|| Value::from(name));
    }
    if let Some(status) = status {
        ev.fields.entry("nmap.status").or_insert_with(|| Value::from(status));
    }
    for (label, key) in [("label.ignored_state", "nmap.ignored_state"), ("label.os", "nmap.os")] {
        if let Some(value) = text(ev, label).map(str::to_owned) {
            ev.fields.entry(key).or_insert_with(|| Value::from(value));
        }
    }
    let Some(ports_text) = ports_text else { return };
    let ports: Vec<Value> = entries(&ports_text).into_iter().filter_map(port).collect();
    if ports.is_empty() {
        return;
    }
    let in_state = |state: &str| ports.iter().filter(|p| p["state"] == state).collect::<Vec<_>>();
    let open = in_state("open");
    let list = |values: Vec<String>| values.join(",");
    let mut services: Vec<String> = Vec::new();
    for p in &open {
        if let Some(service) = p["service"].as_str() {
            if !services.iter().any(|s| s == service) {
                services.push(service.to_owned());
            }
        }
    }
    let detail: Vec<String> = open
        .iter()
        .map(|p| {
            let service = p["service"].as_str().unwrap_or("");
            match p["version"].as_str() {
                Some(version) => format!("{}/{} {service} ({version})", p["port"], p["protocol"].as_str().unwrap_or("")),
                None => format!("{}/{} {service}", p["port"], p["protocol"].as_str().unwrap_or("")),
            }
        })
        .collect();
    let mut add = |key: &str, value: Value| {
        ev.fields.entry(key.to_owned()).or_insert(value);
    };
    add("nmap.ports_open", json!(list(open.iter().map(|p| p["port"].to_string()).collect())));
    add("nmap.open_count", json!(open.len()));
    add("nmap.closed_count", json!(in_state("closed").len()));
    add("nmap.filtered_count", json!(in_state("filtered").len()));
    add("nmap.ports_total", json!(ports.len()));
    if !services.is_empty() {
        add("nmap.services", json!(list(services)));
    }
    if !detail.is_empty() {
        add("nmap.open", json!(detail.join("; ")));
    }
    add("nmap.ports", Value::Array(ports));
}

/// `# Nmap 7.94 scan initiated … as: nmap -sV …` and the free-form `#Comando que gerou: nmap …`.
fn comment(ev: &mut Event, line: &str) {
    if let Some(version) = version_shape().captures(line.trim()) {
        ev.fields.entry("nmap.version").or_insert_with(|| Value::from(version[1].to_owned()));
    }
    if let Some(command) = command_shape().captures(line) {
        ev.fields.entry("nmap.command").or_insert_with(|| Value::from(format!("nmap {}", command[1].trim())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> Event {
        let mut ev = Event::empty();
        super::super::labeled::add_fields(&mut ev, line);
        derive(&mut ev, line);
        ev
    }

    #[test]
    fn a_host_line_gets_the_address_name_counts_and_a_list_of_ports() {
        let ev = event("Host: 192.0.2.4 (srv.example.test)\tPorts: 80/open/tcp//http//Apache httpd 2.4.6 ((CentOS) mod_perl|2.0.11)/, 443/open/tcp//ssl|http//nginx 1.20.1/, 8080/filtered/tcp//http-proxy///, 8081/closed/tcp//blackice-icecap///\tIgnored State: filtered (996)");
        assert_eq!(ev.fields["destination.ip"], "192.0.2.4");
        assert_eq!(ev.fields["host.name"], "srv.example.test");
        assert_eq!(ev.fields["nmap.ports_open"], "80,443");
        assert_eq!(ev.fields["nmap.open_count"], 2);
        assert_eq!(ev.fields["nmap.filtered_count"], 1);
        assert_eq!(ev.fields["nmap.closed_count"], 1);
        assert_eq!(ev.fields["nmap.ports_total"], 4);
        assert_eq!(ev.fields["nmap.services"], "http,ssl/http");
        assert_eq!(ev.fields["nmap.ignored_state"], "filtered (996)");
        let ports = ev.fields["nmap.ports"].as_array().unwrap();
        assert_eq!(ports[0]["version"], "Apache httpd 2.4.6 ((CentOS) mod_perl/2.0.11)");
        assert_eq!(ports[1], json!({"port": 443, "state": "open", "protocol": "tcp", "service": "ssl/http", "version": "nginx 1.20.1"}));
        assert!(ev.fields["nmap.open"].as_str().unwrap().starts_with("80/tcp http (Apache httpd 2.4.6"));
        // the generic labeled fields are still there
        assert!(ev.fields["label.host"].as_str().unwrap().starts_with("192.0.2.4 ("));
    }

    #[test]
    fn an_unnamed_host_a_status_line_and_commas_inside_versions() {
        let ev = event("Host: 192.0.2.7 ()\tPorts: 80/open/tcp//http//Server, with comma/, 443/closed/tcp//https///");
        assert!(!ev.fields.contains_key("host.name"));
        assert_eq!(ev.fields["nmap.ports"].as_array().unwrap().len(), 2);
        assert_eq!(ev.fields["nmap.ports"][0]["version"], "Server, with comma");
        let up = event("Host: 192.0.2.7 ()\tStatus: Up");
        assert_eq!((up.fields["destination.ip"].as_str(), up.fields["nmap.status"].as_str()), (Some("192.0.2.7"), Some("Up")));
        let ipv6 = event("Host: 2001:db8::1 (v6.example.test)\tStatus: Up");
        assert_eq!(ipv6.fields["destination.ip"], "2001:db8::1");
    }

    #[test]
    fn lines_that_are_not_nmap_hosts_are_left_alone() {
        for line in ["Host: example.test\tPorts: 80/open/tcp//http///", "Host: not-an-ip (x)\tStatus: Up", "Host: 192.0.2.1 (x)\tColor: red", "Time: 1\tUser: ana"] {
            let ev = event(line);
            assert!(!ev.fields.keys().any(|k| k.starts_with("nmap.") || k == "destination.ip" || k == "host.name"), "{line}: {:?}", ev.fields);
        }
    }

    #[test]
    fn the_command_and_version_come_from_the_header_comment() {
        let free = event("#Comando que gerou:  nmap -Pn -sV -p80,443 -oG out.gnmap 192.0.2.0/24 e depois um grep");
        assert!(free.fields["nmap.command"].as_str().unwrap().starts_with("nmap -Pn -sV -p80,443 -oG out.gnmap"));
        let standard = event("# Nmap 7.94 scan initiated Tue Oct 27 10:00:00 2026 as: nmap -sV -oG - 192.0.2.0/24");
        assert_eq!(standard.fields["nmap.version"], "7.94");
        assert_eq!(standard.fields["nmap.command"], "nmap -sV -oG - 192.0.2.0/24");
    }
}
