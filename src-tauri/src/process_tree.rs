//! Disk-backed process instances and evidence. A PID is never an identity by
//! itself. Unknown boot context deliberately prevents PID parent inference.
use crate::{
    enrichment_maps::{field, text},
    model::Event,
    query::Filter,
    text_reader::Owner,
    AppState,
};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Instant,
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Debug)]
struct Fact {
    host: String,
    boot: String,
    guid: String,
    pid: String,
    parent_guid: String,
    parent_pid: String,
    image: String,
    command: String,
    user: String,
    start: bool,
    end: bool,
}
fn normalized(event: &Event, key: &str, aliases: &[&str]) -> String {
    let canonical = format!("norm.{key}");
    let value = text(event, &[&canonical]);
    if !value.is_empty() {
        return value;
    }
    let suffix = format!(".{key}");
    let values: std::collections::BTreeSet<_> = event
        .fields
        .iter()
        .filter_map(|(name, value)| {
            let prefix = name.strip_suffix(&suffix)?;
            event
                .fields
                .contains_key(&format!("{prefix}._provenance.map"))
                .then(|| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
        })
        .collect();
    if values.len() == 1 {
        return values.into_iter().next().unwrap();
    }
    text(event, aliases)
}
fn pid(value: String) -> String {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
            .map(|v| v.to_string())
            .unwrap_or_default()
    } else {
        value
            .parse::<u64>()
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}
fn fact(event: &Event) -> Option<Fact> {
    let provider = text(
        event,
        &[
            "Provider",
            "Provider.Name",
            "Event.System.Provider.Name",
            "winlog.provider_name",
            "provider",
        ],
    );
    let provider = if provider.is_empty() {
        event.source.as_str()
    } else {
        &provider
    };
    let channel = text(
        event,
        &["Channel", "Event.System.Channel", "winlog.channel"],
    );
    let sysmon = provider.eq_ignore_ascii_case("Microsoft-Windows-Sysmon")
        && channel.eq_ignore_ascii_case("Microsoft-Windows-Sysmon/Operational");
    let security = provider.eq_ignore_ascii_case("Microsoft-Windows-Security-Auditing")
        && channel.eq_ignore_ascii_case("Security");
    let kind = normalized(event, "event.kind", &["event.kind"]);
    let start = (sysmon && event.code == "1")
        || (security && event.code == "4688")
        || kind == "process_start";
    let end = (sysmon && event.code == "5")
        || (security && event.code == "4689")
        || kind == "process_end";
    let guid = normalized(
        event,
        "process.guid",
        if sysmon {
            &["ProcessGuid", "process.entity_id"]
        } else {
            &["process.entity_id"]
        },
    )
    .to_lowercase();
    let process_pid = if security && start {
        text(event, &["NewProcessId"])
    } else if sysmon || security && end {
        text(event, &["ProcessId"])
    } else {
        normalized(event, "process.pid", &["process.pid"])
    };
    if guid.is_empty() && (!start && !end || process_pid.is_empty()) {
        return None;
    }
    let host = normalized(
        event,
        "host",
        &["Computer", "host.name", "hostname", "Hostname", "host"],
    )
    .to_lowercase();
    if host.is_empty() {
        return None;
    }
    if host.len() > 512 || guid.len() > 512 {
        return None;
    }
    let boot = text(event, &["host.boot.id", "boot_id", "BootId", "boot.id"]);
    if boot.len() > 512 {
        return None;
    }
    Some(Fact {
        host,
        boot,
        guid,
        pid: pid(process_pid),
        parent_guid: normalized(
            event,
            "process.parent_guid",
            &["ParentProcessGuid", "process.parent.entity_id"],
        )
        .to_lowercase(),
        parent_pid: pid(normalized(
            event,
            "process.parent_pid",
            if security {
                &["CreatorProcessId", "ProcessId"]
            } else {
                &["ParentProcessId", "process.parent.pid"]
            },
        )),
        image: normalized(
            event,
            "process.image",
            if security {
                &["NewProcessName"]
            } else {
                &["Image", "process.executable"]
            },
        ),
        command: normalized(
            event,
            "process.command_line",
            &["CommandLine", "ProcessCommandLine", "process.command_line"],
        ),
        user: normalized(
            event,
            "process.user",
            &["User", "TargetUserName", "SubjectUserName", "user.name"],
        ),
        start,
        end,
    })
}
fn key(f: &Fact, event: &Event) -> Option<String> {
    if !f.guid.is_empty() {
        Some(format!(
            "g:{}:{}:{}",
            f.host,
            f.guid,
            event
                .timestamp
                .map(|n| n.to_string())
                .unwrap_or_else(|| event.event_ref.clone())
        ))
    } else if f.start && !f.pid.is_empty() {
        event.timestamp.map(|start| {
            format!(
                "p:{}:{}:{}:{}:{}",
                f.host, f.boot, f.pid, start, event.event_ref
            )
        })
    } else {
        None
    }
}
struct Graph {
    owner: Owner,
    conn: Mutex<Connection>,
    _file: tempfile::NamedTempFile,
    touched: Mutex<Instant>,
    skipped: u64,
}
static GRAPHS: LazyLock<Mutex<HashMap<String, Arc<Graph>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Node {
    pub key: String,
    pub host: String,
    pub boot: String,
    pub guid: String,
    pub pid: String,
    pub started: Option<i64>,
    pub ended: Option<i64>,
    pub image: String,
    pub command: String,
    pub user: String,
    pub parent: Option<String>,
    pub relation: String,
    pub candidates: u64,
    pub event_id: usize,
    pub event_ref: String,
    pub children: u64,
    pub events: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub graph: String,
    pub nodes: Vec<Node>,
    pub total: u64,
    pub offset: u64,
    pub skipped: u64,
    pub evidence: Vec<Value>,
    pub ancestors: Vec<Node>,
}
fn build_graph(state: &AppState, filters: &[Filter]) -> Result<Graph, String> {
    let file = tempfile::NamedTempFile::new().map_err(err)?;
    let mut conn = Connection::open(file.path()).map_err(err)?;
    conn.execute_batch("PRAGMA journal_mode=MEMORY;PRAGMA cache_size=-8192;PRAGMA max_page_count=1048576;
        CREATE TABLE nodes(k TEXT PRIMARY KEY,host TEXT,boot TEXT,guid TEXT,pid TEXT,start INTEGER,end INTEGER,image TEXT,command TEXT,user TEXT,pguid TEXT,ppid TEXT,parent TEXT,relation TEXT DEFAULT 'unresolved',candidates INTEGER DEFAULT 0,eid INTEGER,eref TEXT,selected INTEGER DEFAULT 0);
        CREATE TABLE activity(host TEXT,boot TEXT,guid TEXT,pid TEXT,at INTEGER,kind INTEGER,eid INTEGER,eref TEXT,selected INTEGER,k TEXT);
        CREATE TABLE boots(host TEXT,at INTEGER,id TEXT);
        CREATE INDEX instances ON nodes(host,boot,pid,start);CREATE INDEX byguid ON nodes(host,guid);CREATE INDEX activity_guid ON activity(host,guid,at);CREATE INDEX activity_pid ON activity(host,boot,pid,at);CREATE INDEX activity_node ON activity(k);CREATE INDEX parent ON nodes(parent);CREATE INDEX boot_time ON boots(host,at);").map_err(err)?;
    let prepared = crate::query::prepare(filters);
    let mut skipped = 0;
    let mut seen = 0usize;
    let tx = conn.transaction().map_err(err)?;
    crate::workspace::with_selection(state, &[], |selection| {
        for event in selection.iter() {
            crate::operations::check()?;
            seen += 1;
            let selected = prepared.iter().all(|f| crate::query::matches(&event, f));
            // Boot markers describe the host, never a logging provider's process.
            if event.code == "6005"
                && text(&event, &["Channel", "winlog.channel"]).eq_ignore_ascii_case("System")
                && (event.source.eq_ignore_ascii_case("EventLog")
                    || text(
                        &event,
                        &["Provider", "Provider.Name", "winlog.provider_name"],
                    )
                    .eq_ignore_ascii_case("EventLog"))
            {
                let host = text(&event, &["Computer", "host.name"]).to_lowercase();
                if let Some(at) = event.timestamp {
                    if !host.is_empty() {
                        tx.execute(
                            "INSERT INTO boots VALUES(?1,?2,?3)",
                            params![host, at, event.event_ref],
                        )
                        .map_err(err)?;
                    }
                }
            }
            let Some(f) = fact(&event) else {
                if field(&event, &["ProcessId", "NewProcessId", "process.pid"]).is_some() {
                    skipped += 1;
                }
                continue;
            };
            let k = key(&f, &event);
            if f.start {
                if let Some(k) = &k {
                    tx.execute("INSERT INTO nodes(k,host,boot,guid,pid,start,image,command,user,pguid,ppid,eid,eref,selected) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(k) DO UPDATE SET selected=MAX(selected,excluded.selected)",params![k,f.host,f.boot,f.guid,f.pid,event.timestamp,f.image,f.command,f.user,f.parent_guid,f.parent_pid,event.id,event.event_ref,selected]).map_err(err)?;
                } else {
                    skipped += 1;
                }
            }
            tx.execute("INSERT INTO activity(host,boot,guid,pid,at,kind,eid,eref,selected,k) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![f.host,f.boot,f.guid,f.pid,event.timestamp,if f.end{2}else if f.start{1}else{0},event.id,event.event_ref,selected,if f.start{k}else{None}]).map_err(err)?;
            if seen % 2048 == 0 {
                crate::operations::report_progress(
                    "processos",
                    "process-scan",
                    "Lendo instâncias de processos",
                    seen,
                    0,
                    "eventos",
                    0,
                );
            }
        }
        Ok::<_, String>(())
    })??;
    tx.commit().map_err(err)?;
    let cancellation = crate::operations::current_token();
    conn.progress_handler(1000, Some(move || cancellation.cancelled()));
    conn.execute_batch("UPDATE nodes SET boot=COALESCE((SELECT id FROM boots b WHERE b.host=nodes.host AND b.at<=nodes.start ORDER BY b.at DESC LIMIT 1),'') WHERE boot='';
        UPDATE activity SET boot=COALESCE((SELECT id FROM boots b WHERE b.host=activity.host AND b.at<=activity.at ORDER BY b.at DESC LIMIT 1),'') WHERE boot='';
        UPDATE nodes SET end=(SELECT MIN(at) FROM activity a WHERE a.host=nodes.host AND (a.boot='' OR nodes.boot='' OR a.boot=nodes.boot) AND a.at>=nodes.start AND a.kind=2 AND ((nodes.guid<>'' AND a.guid=nodes.guid) OR (nodes.guid='' AND nodes.boot<>'' AND a.boot=nodes.boot AND a.pid=nodes.pid)));
        UPDATE nodes SET end=MIN(COALESCE(end,9223372036854775807),COALESCE((SELECT MIN(start) FROM nodes n WHERE n.host=nodes.host AND n.boot=nodes.boot AND nodes.boot<>'' AND n.pid=nodes.pid AND n.start>nodes.start),9223372036854775807)) WHERE start IS NOT NULL;
        UPDATE nodes SET end=NULL WHERE end=9223372036854775807;
        UPDATE nodes SET candidates=(SELECT COUNT(*) FROM nodes p WHERE p.host=nodes.host AND p.guid=nodes.pguid AND (p.boot='' OR nodes.boot='' OR p.boot=nodes.boot) AND p.k<>nodes.k AND (p.start IS NULL OR nodes.start IS NULL OR p.start<=nodes.start) AND (p.end IS NULL OR nodes.start IS NULL OR p.end>=nodes.start)) WHERE pguid<>'';
        UPDATE nodes SET parent=(SELECT p.k FROM nodes p WHERE p.host=nodes.host AND p.guid=nodes.pguid AND (p.boot='' OR nodes.boot='' OR p.boot=nodes.boot) AND p.k<>nodes.k AND (p.start IS NULL OR nodes.start IS NULL OR p.start<=nodes.start) AND (p.end IS NULL OR nodes.start IS NULL OR p.end>=nodes.start) LIMIT 1),relation='explicit_guid' WHERE pguid<>'' AND candidates=1;
        UPDATE nodes SET relation=CASE WHEN candidates>1 THEN 'ambiguous_guid' ELSE 'missing_guid_parent' END WHERE pguid<>'' AND parent IS NULL;
        UPDATE nodes SET candidates=(SELECT COUNT(*) FROM nodes p WHERE p.host=nodes.host AND p.boot=nodes.boot AND p.pid=nodes.ppid AND p.k<>nodes.k AND p.start<=nodes.start AND (p.end IS NULL OR p.end>nodes.start)) WHERE pguid='' AND ppid<>'' AND boot<>'' AND start IS NOT NULL;
        UPDATE nodes SET parent=(SELECT p.k FROM nodes p WHERE p.host=nodes.host AND p.boot=nodes.boot AND p.pid=nodes.ppid AND p.k<>nodes.k AND p.start<=nodes.start AND (p.end IS NULL OR p.end>nodes.start) LIMIT 1),relation='inferred_pid_lifetime' WHERE pguid='' AND ppid<>'' AND boot<>'' AND candidates=1;
        UPDATE nodes SET relation=CASE WHEN ppid='' THEN 'root' WHEN boot='' THEN 'unknown_boot' WHEN candidates>1 THEN 'ambiguous_pid' ELSE 'missing_pid_parent' END WHERE pguid='' AND parent IS NULL;
        UPDATE activity SET k=(SELECT n.k FROM nodes n WHERE n.host=activity.host AND n.guid=activity.guid AND (n.boot='' OR activity.boot='' OR n.boot=activity.boot) AND (n.start IS NULL OR activity.at IS NULL OR n.start<=activity.at) AND (n.end IS NULL OR activity.at IS NULL OR n.end>=activity.at) LIMIT 1) WHERE k IS NULL AND guid<>'' AND (SELECT COUNT(*) FROM nodes n WHERE n.host=activity.host AND n.guid=activity.guid AND (n.boot='' OR activity.boot='' OR n.boot=activity.boot) AND (n.start IS NULL OR activity.at IS NULL OR n.start<=activity.at) AND (n.end IS NULL OR activity.at IS NULL OR n.end>=activity.at))=1;
        UPDATE activity SET k=(SELECT n.k FROM nodes n WHERE n.host=activity.host AND n.boot=activity.boot AND n.pid=activity.pid AND n.start<=activity.at AND (n.end IS NULL OR n.end>activity.at) LIMIT 1) WHERE k IS NULL AND guid='' AND boot<>'' AND (SELECT COUNT(*) FROM nodes n WHERE n.host=activity.host AND n.boot=activity.boot AND n.pid=activity.pid AND n.start<=activity.at AND (n.end IS NULL OR n.end>activity.at))=1;
        UPDATE nodes SET selected=1 WHERE k IN(SELECT k FROM activity WHERE selected=1 AND k IS NOT NULL);").map_err(err)?;
    // Break cycles rather than rendering a convincing but impossible lineage.
    let cycles: Vec<String> = {
        let mut s=conn.prepare("WITH RECURSIVE walk(origin,k,parent,depth) AS(SELECT k,k,parent,0 FROM nodes WHERE parent IS NOT NULL UNION ALL SELECT w.origin,n.k,n.parent,w.depth+1 FROM walk w JOIN nodes n ON n.k=w.parent WHERE w.depth<256 AND n.k<>w.origin) SELECT DISTINCT origin FROM walk WHERE parent=origin OR depth=256").map_err(err)?;
        let values = s
            .query_map([], |r| r.get(0))
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        values
    };
    for k in cycles {
        conn.execute(
            "UPDATE nodes SET parent=NULL,relation='cycle_or_depth_limit' WHERE k=?1",
            [k],
        )
        .map_err(err)?;
    }
    crate::operations::check()?;
    conn.progress_handler(0, None::<fn() -> bool>);
    Ok(Graph {
        owner: Owner::current()?,
        conn: Mutex::new(conn),
        _file: file,
        touched: Mutex::new(Instant::now()),
        skipped,
    })
}
pub(crate) fn build(state: &AppState, filters: &[Filter]) -> Result<Page, String> {
    crate::workspace::validate(filters)?;
    let graph = Arc::new(build_graph(state, filters)?);
    graph.owner.validate()?;
    let id = uuid::Uuid::new_v4().to_string();
    {
        let mut graphs = GRAPHS.lock();
        graphs.retain(|_, g| g.touched.lock().elapsed().as_secs() < 1800);
        if graphs.len() >= 3 {
            if let Some(old) = graphs
                .iter()
                .min_by_key(|(_, g)| *g.touched.lock())
                .map(|(id, _)| id.clone())
            {
                graphs.remove(&old);
            }
        }
        graphs.insert(id.clone(), graph);
    }
    page(&id, None, "", 0, None)
}
fn node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    fn preview(mut value: String, limit: usize) -> String {
        if value.len() > limit {
            let mut end = limit;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
            value.shrink_to_fit();
            value.push_str(" … [prévia; abra o evento original]");
        }
        value
    }
    Ok(Node {
        key: row.get(0)?,
        host: row.get(1)?,
        boot: row.get(2)?,
        guid: row.get(3)?,
        pid: row.get(4)?,
        started: row.get(5)?,
        ended: row.get(6)?,
        image: preview(row.get(7)?, 2048),
        command: preview(row.get(8)?, 12000),
        user: preview(row.get(9)?, 512),
        parent: row.get(10)?,
        relation: row.get(11)?,
        candidates: row.get(12)?,
        event_id: row.get(13)?,
        event_ref: row.get(14)?,
        children: row.get(15)?,
        events: row.get(16)?,
    })
}
const PROJECTION:&str="n.k,n.host,n.boot,n.guid,n.pid,n.start,n.end,n.image,n.command,n.user,n.parent,n.relation,n.candidates,n.eid,n.eref,(SELECT COUNT(*) FROM nodes c WHERE c.parent=n.k),(SELECT COUNT(*) FROM activity a WHERE a.k=n.k)";
struct PageProgress<'a>(&'a Connection);
impl Drop for PageProgress<'_> {
    fn drop(&mut self) {
        self.0.progress_handler(0, None::<fn() -> bool>);
    }
}
pub(crate) fn page(
    id: &str,
    parent: Option<&str>,
    search: &str,
    offset: u64,
    selected: Option<&str>,
) -> Result<Page, String> {
    if search.len() > 1024 {
        return Err("Busca de processo excessiva.".into());
    }
    let graph = GRAPHS
        .lock()
        .get(id)
        .cloned()
        .ok_or("Árvore expirada. Reconstrua a análise.")?;
    graph.owner.validate()?;
    *graph.touched.lock() = Instant::now();
    let conn = graph.conn.lock();
    let token = crate::operations::current_token();
    conn.progress_handler(1000, Some(move || token.cancelled()));
    let _progress = PageProgress(&conn);
    let visible="WITH RECURSIVE visible(k) AS(SELECT k FROM nodes WHERE selected=1 AND (?1='' OR instr(lower(image||' '||command||' '||user||' '||pid||' '||guid),lower(?1))>0) UNION SELECT n.parent FROM nodes n JOIN visible v ON n.k=v.k WHERE n.parent IS NOT NULL)";
    let predicate =
        "n.k IN(SELECT k FROM visible) AND ((?2 IS NULL AND n.parent IS NULL) OR n.parent=?2)";
    let total = conn
        .query_row(
            &format!("{visible} SELECT COUNT(*) FROM nodes n WHERE {predicate}"),
            params![search, parent],
            |r| r.get(0),
        )
        .map_err(err)?;
    let nodes = {
        let mut s=conn.prepare(&format!("{visible} SELECT {PROJECTION} FROM nodes n WHERE {predicate} ORDER BY n.start,n.k LIMIT 80 OFFSET ?3")).map_err(err)?;
        let rows = s
            .query_map(params![search, parent, offset], node)
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        rows
    };
    let mut evidence = Vec::new();
    let mut ancestors = Vec::new();
    if let Some(k) = selected {
        let mut s=conn.prepare("SELECT eid,eref,at,kind FROM activity WHERE k=?1 ORDER BY at,eid LIMIT 80 OFFSET ?2").map_err(err)?;
        for row in s.query_map(params![k,offset],|r|Ok(json!({"id":r.get::<_,usize>(0)?,"event_ref":r.get::<_,String>(1)?,"timestamp":r.get::<_,Option<i64>>(2)?,"kind":r.get::<_,i32>(3)?}))).map_err(err)?{evidence.push(row.map_err(err)?);}
        let mut next = Some(k.to_owned());
        let mut seen = std::collections::HashSet::new();
        while let Some(k) = next.take() {
            if !seen.insert(k.clone()) || ancestors.len() >= 128 {
                break;
            }
            let n = conn
                .query_row(
                    &format!("SELECT {PROJECTION} FROM nodes n WHERE k=?1"),
                    [k],
                    node,
                )
                .optional()
                .map_err(err)?;
            if let Some(n) = n {
                next = n.parent.clone();
                let mut n = n;
                if !ancestors.is_empty() {
                    n.command.clear();
                }
                ancestors.push(n);
            } else {
                break;
            }
        }
    }
    graph.owner.validate()?;
    Ok(Page {
        graph: id.into(),
        nodes,
        total,
        offset,
        skipped: graph.skipped,
        evidence,
        ancestors,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ev(code: &str) -> Event {
        let mut e = Event::empty();
        e.code = code.into();
        e.source = "Microsoft-Windows-Security-Auditing".into();
        e.fields=serde_json::from_value(json!({"Channel":"Security","Computer":"host","NewProcessId":"0x40","ProcessId":"0x20","NewProcessName":"child.exe"})).unwrap();
        e.timestamp = Some(100);
        e.event_ref = "test:1".into();
        e
    }
    #[test]
    fn security_creator_is_not_the_new_process() {
        let f = fact(&ev("4688")).unwrap();
        assert_eq!(f.pid, "64");
        assert_eq!(f.parent_pid, "32");
        assert!(f.start);
    }
    #[test]
    fn producer_pid_and_cross_provider_codes_never_create_nodes() {
        let mut e = ev("4688");
        e.source = "another-provider".into();
        assert!(fact(&e).is_none());
        let mut e = ev("4624");
        e.fields.insert("ProcessID".into(), json!(99));
        assert!(fact(&e).is_none());
    }
    #[test]
    fn pid_instances_require_timestamp_and_include_machine_and_evidence() {
        let e = ev("4688");
        let f = fact(&e).unwrap();
        let first = key(&f, &e).unwrap();
        let mut other = ev("4688");
        other.timestamp = Some(200);
        assert_ne!(first, key(&f, &other).unwrap());
        other.timestamp = None;
        assert!(key(&f, &other).is_none());
    }
    fn creation(id: usize, pid: &str, parent: &str, time: i64, boot: &str, host: &str) -> Event {
        let mut e = ev("4688");
        e.id = id;
        e.event_ref = format!("fixture:{id}");
        e.timestamp = Some(time);
        e.fields.insert("NewProcessId".into(), json!(pid));
        e.fields.insert("ProcessId".into(), json!(parent));
        e.fields.insert("host.boot.id".into(), json!(boot));
        e.fields.insert("Computer".into(), json!(host));
        e
    }
    #[test]
    fn disk_graph_uses_latest_pid_lifetime_with_machine_and_boot_isolation() {
        let events = vec![
            creation(0, "100", "0", 100, "boot-a", "a"),
            creation(1, "100", "0", 250, "boot-a", "a"),
            creation(2, "200", "100", 300, "boot-a", "a"),
            creation(3, "100", "0", 280, "boot-a", "b"),
            creation(4, "300", "100", 310, "boot-b", "a"),
            creation(5, "400", "100", 320, "", "a"),
        ];
        crate::local_analysis::tests::visible(events, |state| {
            let graph = build_graph(state, &[]).unwrap();
            let conn = graph.conn.lock();
            let parent: usize = conn
                .query_row(
                    "SELECT p.eid FROM nodes n JOIN nodes p ON p.k=n.parent WHERE n.eid=2",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(parent, 1);
            let other: String = conn
                .query_row("SELECT relation FROM nodes WHERE eid=4", [], |r| r.get(0))
                .unwrap();
            assert_eq!(other, "missing_pid_parent");
            let unknown: String = conn
                .query_row("SELECT relation FROM nodes WHERE eid=5", [], |r| r.get(0))
                .unwrap();
            assert_eq!(unknown, "unknown_boot");
        });
    }
    #[test]
    fn ambiguous_pid_is_never_joined_to_an_arbitrary_parent() {
        let events = vec![
            creation(0, "100", "0", 100, "b", "a"),
            creation(1, "100", "0", 100, "b", "a"),
            creation(2, "200", "100", 200, "b", "a"),
        ];
        crate::local_analysis::tests::visible(events, |state| {
            let graph = build_graph(state, &[]).unwrap();
            let conn = graph.conn.lock();
            let (parent, relation): (Option<String>, String) = conn
                .query_row("SELECT parent,relation FROM nodes WHERE eid=2", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert!(parent.is_none());
            assert_eq!(relation, "ambiguous_pid");
        });
    }
}
