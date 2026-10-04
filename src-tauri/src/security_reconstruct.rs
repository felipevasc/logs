//! Reconstruct only operations with producer identities. Similar text is not identity.
use crate::security_budget::TrackedConnection;
use crate::{
    model::Event,
    security_normalize::{event_ref, field_text},
};
use rusqlite::{params, OptionalExtension};
fn storage_error(e: impl std::fmt::Display) -> String {
    format!("Reconstrução em fluxo: {e}")
}

/// Membership remains complete on disk even when an operation has millions of
/// duplicate fragments. Only the bounded reconstructed content enters RAM.
pub struct Members {
    db: TrackedConnection,
}
impl Members {
    pub fn new() -> Result<Self, String> {
        let db = TrackedConnection::open("").map_err(storage_error)?;
        db.execute_batch("CREATE TABLE members(operation INTEGER,event INTEGER,ref TEXT,position INTEGER,PRIMARY KEY(operation,position));CREATE TABLE operations(id INTEGER PRIMARY KEY,total INTEGER,limitations TEXT);BEGIN").map_err(storage_error)?;
        Ok(Self { db })
    }
    fn add(&self, operation: usize, event: &Event, position: usize) -> Result<(), String> {
        self.db
            .prepare_cached("INSERT INTO members VALUES(?1,?2,?3,?4)")
            .map_err(storage_error)?
            .execute(params![
                operation as i64,
                event.id as i64,
                event_ref(event),
                position as i64
            ])
            .map_err(storage_error)?;
        Ok(())
    }
    fn finish(&self, id: usize, total: usize, limitations: &[String]) -> Result<(), String> {
        self.db
            .execute(
                "INSERT INTO operations VALUES(?1,?2,?3)",
                params![
                    id as i64,
                    total as i64,
                    serde_json::to_string(limitations).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }
    pub fn describe(&self, id: usize) -> Result<Option<(usize, Vec<String>)>, String> {
        let value: Option<(usize, String)> = self
            .db
            .query_row(
                "SELECT total,limitations FROM operations WHERE id=?1",
                [id as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(storage_error)?;
        value
            .map(|(total, text)| Ok((total, serde_json::from_str(&text).map_err(storage_error)?)))
            .transpose()
    }
    pub fn for_each(
        &self,
        id: usize,
        mut consume: impl FnMut(usize, String) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut stmt = self
            .db
            .prepare("SELECT event,ref FROM members WHERE operation=?1 ORDER BY position")
            .map_err(storage_error)?;
        let mut rows = stmt.query([id as i64]).map_err(storage_error)?;
        while let Some(row) = rows.next().map_err(storage_error)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            consume(
                row.get(0).map_err(storage_error)?,
                row.get(1).map_err(storage_error)?,
            )?;
        }
        Ok(())
    }
}
pub struct Fragments {
    db: TrackedConnection,
    last: Option<String>,
}
impl Fragments {
    pub fn new() -> Result<Self, String> {
        let db = TrackedConnection::open("").map_err(storage_error)?;
        db.execute_batch("CREATE TABLE fragments(k TEXT,part INTEGER,ts INTEGER,eid INTEGER,payload TEXT);CREATE INDEX fragment_order ON fragments(k,part,ts,eid);CREATE TABLE parts(n INTEGER PRIMARY KEY,text TEXT);BEGIN").map_err(storage_error)?;
        Ok(Self { db, last: None })
    }
    pub fn push(&self, key: &str, event: &Event) -> Result<(), String> {
        crate::security_budget::check()?;
        let part = field_text(event, "MessageNumber")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        let part =
            i64::try_from(part).map_err(|_| "Número de fragmento excede o intervalo suportado")?;
        self.db
            .prepare_cached("INSERT INTO fragments VALUES(?1,?2,?3,?4,?5)")
            .map_err(storage_error)?
            .execute(params![
                key,
                part,
                event.timestamp,
                event.id as i64,
                serde_json::to_string(event).map_err(storage_error)?
            ])
            .map_err(storage_error)?;
        Ok(())
    }
    pub fn next(&mut self, members: &Members) -> Result<Option<(Event, Vec<String>)>, String> {
        let key: Option<String> = self
            .db
            .query_row(
                "SELECT k FROM fragments WHERE ?1 IS NULL OR k>?1 ORDER BY k LIMIT 1",
                [self.last.as_deref()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let Some(key) = key else { return Ok(None) };
        self.last = Some(key.clone());
        self.db
            .execute("DELETE FROM parts", [])
            .map_err(storage_error)?;
        let mut stmt = self
            .db
            .prepare("SELECT payload FROM fragments WHERE k=?1 ORDER BY part,ts,eid,rowid")
            .map_err(storage_error)?;
        let mut rows = stmt.query([&key]).map_err(storage_error)?;
        let mut base = None::<Event>;
        let mut first = None::<(usize, String)>;
        let mut merged = std::collections::HashMap::new();
        let mut bytes = 0usize;
        let mut count = 0;
        let mut total = 1;
        let mut product = String::new();
        let mut syscall = false;
        let mut selected_exec = false;
        let mut terminal = false;
        let mut conflicting = false;
        while let Some(row) = rows.next().map_err(storage_error)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let text: String = row.get(0).map_err(storage_error)?;
            let event: Event = serde_json::from_str(&text).map_err(storage_error)?;
            if first.is_none() {
                first = Some((event.id, event_ref(&event)));
                product = crate::security_normalize::product(&event).into();
                total = field_text(&event, "MessageTotal")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1);
                base = Some(event.clone());
            }
            members.add(first.as_ref().unwrap().0, &event, count)?;
            count += 1;
            if product == "powershell" {
                let part = field_text(&event, "MessageNumber")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1);
                let part = i64::try_from(part)
                    .map_err(|_| "Número de fragmento excede o intervalo suportado")?;
                let content = field_text(&event, "ScriptBlockText").unwrap_or_default();
                let previous: Option<String> = self
                    .db
                    .query_row("SELECT text FROM parts WHERE n=?1", [part], |r| r.get(0))
                    .optional()
                    .map_err(storage_error)?;
                conflicting |= previous.is_some_and(|p| p != content);
                self.db
                    .execute(
                        "INSERT OR REPLACE INTO parts VALUES(?1,?2)",
                        params![part, content],
                    )
                    .map_err(storage_error)?;
            } else {
                syscall |= field_text(&event, "type").as_deref() == Some("SYSCALL");
                if product == "auditd"
                    && !selected_exec
                    && field_text(&event, "type").as_deref() == Some("EXECVE")
                {
                    base = Some(event.clone());
                    selected_exec = true;
                }
                if product == "kubernetes"
                    && field_text(&event, "stage").as_deref() == Some("ResponseComplete")
                {
                    base = Some(event.clone());
                    terminal = true;
                }
                for (k, v) in event.fields {
                    if !merged.contains_key(&k) {
                        bytes = bytes.saturating_add(
                            k.len() + serde_json::to_vec(&v).map_err(storage_error)?.len() + 64,
                        );
                        if bytes > 2 * 1024 * 1024 || merged.len() >= 8192 {
                            return Err("Conteúdo reconstruído excede 2 MiB ou 8192 campos; originais preservados, análise não publicada".into());
                        }
                        merged.insert(k, v);
                    }
                }
            }
        }
        let mut event = base.ok_or("Operação sem fragmentos")?;
        let mut limitations = Vec::new();
        if product == "powershell" {
            let(parts,min,max,size):(usize,Option<i64>,Option<i64>,usize)=self.db.query_row("SELECT count(*),min(n),max(n),coalesce(sum(length(cast(text AS BLOB))),0) FROM parts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(storage_error)?;
            if conflicting {
                limitations.push("Fragmentos PowerShell conflitantes".into());
            }
            if total > 4096 || parts != total || min != Some(1) || max != i64::try_from(total).ok()
            {
                limitations.push("Script PowerShell incompleto".into());
            }
            if size > 512 * 1024 {
                return Err("Script reconstruído excede 512 KiB; análise interrompida sem executar conteúdo".into());
            }
            let mut text = String::with_capacity(size);
            let mut parts = self
                .db
                .prepare("SELECT text FROM parts ORDER BY n")
                .map_err(storage_error)?;
            for part in parts
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(storage_error)?
            {
                text.push_str(&part.map_err(storage_error)?);
            }
            event.fields.insert("ScriptBlockText".into(), text.into());
        } else {
            for (k, v) in merged {
                event.fields.entry(k).or_insert(v);
            }
            if product == "kubernetes" && !terminal {
                limitations.push("Operação Kubernetes sem ResponseComplete".into());
            }
            if product == "auditd" {
                if !syscall {
                    limitations.push("Operação auditd sem resultado SYSCALL".into());
                }
                if let Some(argc) = field_text(&event, "argc").and_then(|s| s.parse::<usize>().ok())
                {
                    if argc > 4096 {
                        return Err("Operação auditd excede 4096 argumentos".into());
                    }
                    let args = (0..argc)
                        .map(|i| field_text(&event, &format!("a{i}")))
                        .collect::<Vec<_>>();
                    if args.iter().all(Option::is_some) {
                        let text = args.into_iter().flatten().collect::<Vec<_>>().join(" ");
                        if text.len() > 512 * 1024 {
                            return Err("Comando auditd reconstruído excede 512 KiB".into());
                        }
                        event.fields.insert("cmdline".into(), text.into());
                    } else {
                        limitations
                            .push("Argumentos EXECVE incompletos; fragmentos preservados".into());
                    }
                }
            }
        }
        let (id, reference) = first.unwrap();
        event.id = id;
        event.event_ref = reference;
        members.finish(id, count, &limitations)?;
        Ok(Some((event, limitations)))
    }
}

pub fn key(event: &Event) -> Option<String> {
    if !matches!(
        crate::security_normalize::product(event),
        "powershell" | "auditd" | "kubernetes"
    ) {
        return None;
    }
    let (_, n) = crate::security_normalize::normalize(event, &[]);
    let scope = n.get("namespace").unwrap_or("");
    let host = n.get("host").unwrap_or("");
    let identity = match n.product.as_str() {
        "powershell" if !host.is_empty() => {
            field_text(event, "ScriptBlockId").map(|v| ("powershell", v))
        }
        "auditd" if !host.is_empty() && event.timestamp.is_some() => {
            field_text(event, "audit_serial")
                .map(|v| ("auditd", format!("{}:{v}", event.timestamp.unwrap())))
        }
        "kubernetes" if !scope.is_empty() => {
            field_text(event, "auditID").map(|v| ("kubernetes", v))
        }
        _ => None,
    }?;
    Some(serde_json::to_string(&(identity.0, scope, host, identity.1)).unwrap())
}

pub struct Reconstructed {
    pub event: Event,
    pub members: Vec<(usize, String)>,
    pub limitations: Vec<String>,
}
pub fn assemble(mut events: Vec<Event>) -> Result<Reconstructed, String> {
    if events.is_empty() {
        return Err("Operação sem fragmentos".into());
    }
    events.sort_by_key(|e| {
        (
            field_text(e, "MessageNumber")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0),
            e.timestamp,
            e.id,
        )
    });
    let mut event = events[0].clone();
    event.event_ref = event_ref(&event);
    let members = events.iter().map(|e| (e.id, event_ref(e))).collect();
    let mut limitations = Vec::new();
    let product = crate::security_normalize::product(&event);
    if product == "powershell" {
        let total = field_text(&event, "MessageTotal")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);
        let mut parts = std::collections::BTreeMap::new();
        for e in &events {
            let index = field_text(e, "MessageNumber")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1);
            let text = field_text(e, "ScriptBlockText").unwrap_or_default();
            if let Some(previous) = parts.insert(index, text.clone()) {
                if previous != text {
                    limitations.push("Fragmentos PowerShell conflitantes".into());
                }
            }
        }
        if total > 4096 || parts.len() != total || !(1..=total).all(|i| parts.contains_key(&i)) {
            limitations.push("Script PowerShell incompleto".into());
        }
        let bytes: usize = parts.values().map(String::len).sum();
        if bytes > 512 * 1024 {
            return Err(
                "Script reconstruído excede 512 KiB; análise interrompida sem executar conteúdo"
                    .into(),
            );
        }
        event.fields.insert(
            "ScriptBlockText".into(),
            serde_json::Value::from(parts.into_values().collect::<String>()),
        );
    } else {
        if product == "auditd" {
            if let Some(exec) = events
                .iter()
                .find(|e| field_text(e, "type").as_deref() == Some("EXECVE"))
            {
                event = exec.clone();
            }
        }
        // The terminal Kubernetes stage supplies the outcome; RequestReceived cannot prove success.
        if product == "kubernetes" {
            if let Some(last) = events
                .iter()
                .rev()
                .find(|e| field_text(e, "stage").as_deref() == Some("ResponseComplete"))
            {
                event = last.clone();
            } else {
                limitations.push("Operação Kubernetes sem ResponseComplete".into());
            }
        }
        for e in &events {
            for (k, v) in &e.fields {
                event.fields.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        if product == "auditd"
            && !events
                .iter()
                .any(|e| field_text(e, "type").as_deref() == Some("SYSCALL"))
        {
            limitations.push("Operação auditd sem resultado SYSCALL".into());
        }
        if product == "auditd" {
            if let Some(count) = field_text(&event, "argc").and_then(|v| v.parse::<usize>().ok()) {
                if count > 4096 {
                    return Err("Operação auditd excede 4096 argumentos".into());
                }
                let arguments: Vec<_> = (0..count)
                    .map(|i| field_text(&event, &format!("a{i}")))
                    .collect();
                if arguments.iter().all(Option::is_some) {
                    let command = arguments
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if command.len() > 512 * 1024 {
                        return Err("Comando auditd reconstruído excede 512 KiB".into());
                    }
                    event
                        .fields
                        .insert("cmdline".into(), serde_json::Value::from(command));
                } else {
                    limitations
                        .push("Argumentos EXECVE incompletos; fragmentos preservados".into());
                }
            }
        }
    }
    // Keep the first event's addressable identity, even when a terminal stage was selected.
    event.id = events[0].id;
    event.event_ref = event_ref(&events[0]);
    Ok(Reconstructed {
        event,
        members,
        limitations,
    })
}
