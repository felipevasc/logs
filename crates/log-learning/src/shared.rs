//! The public wire format accepts only finite enumerations and integer counts.
//! No raw text, hashes of private strings, timestamps, entities or template IDs.
use crate::{digest, Histogram, SCHEMA};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionBundle {
    pub schema_version: u32,
    pub contributions: Vec<Contribution>,
}
impl ContributionBundle {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA
            || self.contributions.is_empty()
            || self.contributions.len() > 7
        {
            return Err("Pacote de contribuições inválido".into());
        }
        let mut ids = BTreeSet::new();
        for contribution in &self.contributions {
            contribution.validate()?;
            if !ids.insert(&contribution.id) {
                return Err("Contribuição repetida no pacote".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Application,
    Web,
    Authentication,
    Network,
    Database,
    OperatingSystem,
    Other,
}
impl Domain {
    pub fn from_product(product: &str) -> Self {
        match product {
            "nginx" | "apache" | "iis" | "http" => Self::Web,
            "okta" | "entra" | "authentication" => Self::Authentication,
            "zeek" | "suricata" | "firewall" | "dns" => Self::Network,
            "postgresql" | "mysql" | "mssql" | "oracle" => Self::Database,
            "windows" | "sysmon" | "auditd" | "syslog" | "powershell" => Self::OperatingSystem,
            "java" | "python" | "application" | "dotnet" => Self::Application,
            _ => Self::Other,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicState {
    pub action: u8,
    pub outcome: u8,
    pub latency: u8,
    pub bytes: u8,
    pub length: u8,
}
impl PublicState {
    pub fn new(
        action: &str,
        outcome: &str,
        latency: Option<f64>,
        bytes: Option<f64>,
        words: usize,
    ) -> Self {
        let action = match action {
            "logon" => 1,
            "read" | "file_read" => 2,
            "write" | "file_write" => 3,
            "process_start" | "script_execution" => 4,
            "request" | "http_request" => 5,
            "connect" | "network_connection" => 6,
            "delete" => 7,
            "change" | "password_change" => 8,
            _ => 0,
        };
        let outcome = match outcome {
            "success" => 1,
            "failure" => 2,
            _ => 0,
        };
        let bucket = |v: Option<f64>| {
            v.and_then(Histogram::bin)
                .map_or(0, |b| 1 + (b / 2).min(15) as u8)
        };
        Self {
            action,
            outcome,
            latency: bucket(latency),
            bytes: bucket(bytes),
            length: if words == 0 {
                0
            } else {
                (usize::BITS - words.leading_zeros()).min(6) as u8
            },
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.action > 8
            || self.outcome > 2
            || self.latency > 16
            || self.bytes > 16
            || self.length > 6
        {
            Err("Estado compartilhável fora do vocabulário fechado".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateCount {
    pub state: PublicState,
    pub count: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionCount {
    pub from: PublicState,
    pub to: PublicState,
    pub count: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub schema_version: u32,
    pub id: String,
    pub domain: Domain,
    pub states: Vec<StateCount>,
    pub transitions: Vec<TransitionCount>,
    pub discarded: u64,
}
impl Contribution {
    pub fn new(domain: Domain) -> Self {
        Self {
            schema_version: SCHEMA,
            id: String::new(),
            domain,
            states: Vec::new(),
            transitions: Vec::new(),
            discarded: 0,
        }
    }
    pub fn observe(&mut self, state: PublicState, previous: Option<PublicState>) {
        if self.total() >= 10_000 {
            self.discarded = self.discarded.saturating_add(1);
            return;
        }
        if let Some(entry) = self.states.iter_mut().find(|v| v.state == state) {
            entry.count += 1;
        } else if self.states.len() < 256 {
            self.states.push(StateCount { state, count: 1 });
        } else {
            self.discarded = self.discarded.saturating_add(1);
            return;
        }
        if let Some(from) = previous {
            if let Some(entry) = self
                .transitions
                .iter_mut()
                .find(|v| v.from == from && v.to == state)
            {
                entry.count += 1;
            } else if self.transitions.len() < 512 {
                self.transitions.push(TransitionCount {
                    from,
                    to: state,
                    count: 1,
                });
            }
        }
    }
    pub fn total(&self) -> u64 {
        self.states
            .iter()
            .fold(0u64, |n, v| n.saturating_add(v.count))
    }
    pub fn seal(&mut self) {
        self.states.sort_by_key(|s| s.state);
        self.transitions.sort_by_key(|s| (s.from, s.to));
        self.id.clear();
        self.id = digest(&serde_json::to_vec(self).expect("finite integer protocol"));
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA
            || self.states.len() > 256
            || self.transitions.len() > 512
            || self.total() == 0
            || self.total() > 10_000
            || self.discarded > 1_000_000_000
        {
            return Err("Contribuição incompatível, vazia ou acima do orçamento".into());
        }
        let mut states = BTreeMap::new();
        for entry in &self.states {
            entry.state.validate()?;
            if entry.count == 0
                || entry.count > 10_000
                || states.insert(entry.state, entry.count).is_some()
            {
                return Err("Contagens repetidas ou inválidas".into());
            }
        }
        let mut transitions = BTreeSet::new();
        let mut outgoing = BTreeMap::<PublicState, u64>::new();
        let mut incoming = BTreeMap::<PublicState, u64>::new();
        for entry in &self.transitions {
            entry.from.validate()?;
            entry.to.validate()?;
            if entry.count == 0
                || entry.count > 10_000
                || !transitions.insert((entry.from, entry.to))
                || !states.contains_key(&entry.from)
                || !states.contains_key(&entry.to)
            {
                return Err("Transições repetidas ou inválidas".into());
            }
            *outgoing.entry(entry.from).or_default() += entry.count;
            *incoming.entry(entry.to).or_default() += entry.count;
        }
        if outgoing
            .iter()
            .chain(incoming.iter())
            .any(|(s, n)| *n > states[s])
        {
            return Err("Transições excedem as observações".into());
        }
        let mut sealed = self.clone();
        sealed.seal();
        if sealed.id != self.id {
            return Err("Hash ou ordenação da contribuição divergente".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainModel {
    pub states: Vec<StateCount>,
    pub transitions: Vec<TransitionCount>,
    pub observations: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedModel {
    pub schema_version: u32,
    pub contributions: BTreeSet<String>,
    pub domains: BTreeMap<Domain, DomainModel>,
}
impl Default for SharedModel {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA,
            contributions: BTreeSet::new(),
            domains: BTreeMap::new(),
        }
    }
}
impl SharedModel {
    pub fn merge(&mut self, c: &Contribution) -> Result<bool, String> {
        c.validate()?;
        self.validate()?;
        if self.contributions.contains(&c.id) {
            return Ok(false);
        }
        if self.contributions.len() >= 4096 {
            return Err(
                "Base compartilhada atingiu 4096 contribuições; inicie uma revisão de época".into(),
            );
        }
        let mut next = self.clone();
        let domain = next.domains.entry(c.domain).or_default();
        for state in &c.states {
            if let Some(entry) = domain.states.iter_mut().find(|v| v.state == state.state) {
                entry.count += state.count;
            } else if domain.states.len() < 4096 {
                domain.states.push(state.clone());
            } else {
                return Err(
                    "Vocabulário compartilhado excedeu o orçamento; revisão necessária".into(),
                );
            }
        }
        for transition in &c.transitions {
            if let Some(entry) = domain
                .transitions
                .iter_mut()
                .find(|v| v.from == transition.from && v.to == transition.to)
            {
                entry.count += transition.count;
            } else if domain.transitions.len() < 8192 {
                domain.transitions.push(transition.clone());
            } else {
                return Err("Transições compartilhadas excederam o orçamento".into());
            }
        }
        domain.observations += c.total();
        domain.states.sort_by_key(|s| s.state);
        domain.transitions.sort_by_key(|s| (s.from, s.to));
        next.contributions.insert(c.id.clone());
        next.validate()?;
        *self = next;
        Ok(true)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA
            || self.contributions.len() > 4096
            || self.domains.len() > 7
            || self
                .contributions
                .iter()
                .any(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("Base compartilhada incompatível".into());
        }
        for domain in self.domains.values() {
            if domain.states.len() > 4096
                || domain.transitions.len() > 8192
                || domain.observations > 40_960_000
            {
                return Err("Base compartilhada acima do orçamento".into());
            }
            let mut states = BTreeMap::new();
            let mut total = 0u64;
            for entry in &domain.states {
                entry.state.validate()?;
                if entry.count == 0
                    || entry.count > 40_960_000
                    || states.insert(entry.state, entry.count).is_some()
                {
                    return Err("Estado compartilhado inválido".into());
                }
                total = total.saturating_add(entry.count);
            }
            if total != domain.observations {
                return Err("Total compartilhado divergente".into());
            }
            let mut transitions = BTreeSet::new();
            let mut outgoing = BTreeMap::<PublicState, u64>::new();
            let mut incoming = BTreeMap::<PublicState, u64>::new();
            for t in &domain.transitions {
                t.from.validate()?;
                t.to.validate()?;
                if t.count == 0
                    || t.count > domain.observations
                    || !transitions.insert((t.from, t.to))
                    || !states.contains_key(&t.from)
                    || !states.contains_key(&t.to)
                {
                    return Err("Transição compartilhada inválida".into());
                }
                *outgoing.entry(t.from).or_default() += t.count;
                *incoming.entry(t.to).or_default() += t.count;
            }
            if outgoing
                .iter()
                .chain(incoming.iter())
                .any(|(s, n)| *n > states[s])
            {
                return Err("Transições compartilhadas excedem observações".into());
            }
        }
        Ok(())
    }
    pub fn support(&self, domain: Domain, state: PublicState) -> (u64, u64) {
        self.domains
            .get(&domain)
            .map(|d| {
                (
                    d.states
                        .iter()
                        .find(|s| s.state == state)
                        .map_or(0, |s| s.count),
                    d.observations,
                )
            })
            .unwrap_or((0, 0))
    }
    pub fn revision(&self) -> String {
        digest(&serde_json::to_vec(self).expect("finite integer protocol"))
    }
}
