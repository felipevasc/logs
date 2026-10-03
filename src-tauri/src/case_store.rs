//! Transactional, versioned case store. Only modified cases are rewritten.
use rusqlite::{params, Connection};
use serde_json::{json, Value};
pub(crate) fn connect(dir: &std::path::Path) -> Result<Connection, String> {
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(dir.join("investigations.sqlite3")).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS cases(id TEXT PRIMARY KEY, body TEXT NOT NULL, position INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(|e|e.to_string())?;
    crate::analysis_context::schema(&conn)?;
    Ok(conn)
}
/// Initializes only schema/migration metadata for context admission. Already
/// migrated requests never deserialize the (potentially large) case evidence.
pub(crate) fn context_connection(dir: &std::path::Path) -> Result<Connection, String> {
    crate::case_evidence::bootstrap(dir)?;
    let mut conn = connect(dir)?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='revision')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        drop(conn);
        load_at(dir)?;
        return connect(dir);
    }
    let initialized: bool = conn
        .query_row(
            "SELECT count(*)=2 FROM metadata WHERE key IN ('case-analysis-v1','case-interpretation-v1')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !initialized {
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        crate::analysis_context::initialize(&tx, dir)?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(conn)
}
struct CaseBody<'a> {
    value: &'a Value,
    preserve_context: bool,
}
impl serde::Serialize for CaseBody<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self
            .value
            .as_object()
            .expect("case validated before serialization")
        {
            if key != crate::case_archive::TOKEN_FIELD
                && (key != "analysisContext" || self.preserve_context)
            {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}
pub fn load() -> Result<Value, String> {
    load_at(&crate::config_dir())
}
pub(crate) fn load_at(dir: &std::path::Path) -> Result<Value, String> {
    crate::case_evidence::bootstrap(dir)?;
    let mut conn = connect(dir)?;
    let version: i64 = conn
        .query_row(
            "SELECT count(*) FROM metadata WHERE key='revision'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if version == 0 {
        return Err("CASE_BOOTSTRAP_INCOMPLETE: Inicialização incompleta; os arquivos originais foram preservados.".into());
    }

    {
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        crate::case_evidence::require_legacy_store(&tx)?;
        crate::analysis_context::initialize(&tx, dir)?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    // A read transaction keeps case bodies, configurations and revision together.
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    crate::case_evidence::require_legacy_store(&conn)?;
    let revision: u64 = conn
        .query_row("SELECT value FROM metadata WHERE key='revision'", [], |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?
        .parse()
        .unwrap_or(0);
    let active: String = conn
        .query_row("SELECT value FROM metadata WHERE key='active'", [], |r| {
            r.get(0)
        })
        .unwrap_or("null".into());
    let mut stmt = conn
        .prepare("SELECT body FROM cases ORDER BY position")
        .map_err(|e| e.to_string())?;
    let mut cases = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|r| {
            r.map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for case in &mut cases {
        crate::analysis_context::attach(&conn, case)?;
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(
        json!({"schemaVersion":2,"revision":revision,"active":serde_json::from_str::<Value>(&active).unwrap_or(Value::Null),"cases":cases}),
    )
}
pub fn save(data: Value) -> Result<Value, String> {
    save_at(&crate::config_dir(), data)
}
pub(crate) fn save_at(dir: &std::path::Path, data: Value) -> Result<Value, String> {
    save_initial_at(dir, data, false)
}
fn save_initial_at(dir: &std::path::Path, data: Value, legacy: bool) -> Result<Value, String> {
    crate::case_images::references(&data)?;
    if data.get("imageAssets").is_some() {
        return Err("Importe as imagens antes de salvar a investigação; o Caso armazena somente referências.".into());
    }
    let cases = data
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("Investigação inválida: lista de casos ausente.")?;
    if data
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        > 2
    {
        return Err("Esta investigação foi criada por uma versão mais recente.".into());
    }
    let mut ids = std::collections::HashSet::new();
    for case in cases {
        reject_projected_evidence(case)?;
        let id = case
            .get("id")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or("Caso sem identificador.")?;
        if !ids.insert(id) {
            return Err("Identificadores de caso duplicados.".into());
        }
    }
    let mut conn = connect(dir)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    crate::case_evidence::require_legacy_store(&tx)?;
    let current: u64 = tx
        .query_row("SELECT value FROM metadata WHERE key='revision'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap_or("0".into())
        .parse()
        .unwrap_or(0);
    if current > 0 && data.get("revision").and_then(Value::as_u64).is_none() {
        return Err("Reabra a investigação antes de salvar: revisão ausente.".into());
    }
    if let Some(expected) = data.get("revision").and_then(Value::as_u64) {
        if expected != current {
            return Err(
                "A investigação foi alterada em outra sessão. Reabra-a antes de salvar.".into(),
            );
        }
    }
    if !legacy {
        crate::analysis_context::initialize(&tx, dir)?;
    }
    let mut analysis_contexts = Vec::new();
    for (position, case) in cases.iter().enumerate() {
        let id = case["id"].as_str().unwrap();
        // Borrow large evidence arrays; stripping metadata must not clone them.
        let body = serde_json::to_string(&CaseBody {
            value: case,
            preserve_context: legacy,
        })
        .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO cases(id,body,position) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET body=excluded.body,position=excluded.position WHERE cases.body<>excluded.body OR cases.position<>excluded.position",params![id,body,position as i64]).map_err(|e|e.to_string())?;
        if !legacy {
            let imported = crate::case_archive::consume_prepared(dir, &tx, case)?;
            if let Some(snapshot) = match imported {
                Some(snapshot) => Some(snapshot),
                None => crate::analysis_context::ensure_case(&tx, case)?,
            } {
                analysis_contexts.push(snapshot);
            }
        }
    }
    if legacy {
        crate::analysis_context::initialize(&tx, dir)?;
    }
    let stored: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT id FROM cases")
            .map_err(|e| e.to_string())?;
        let values = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        values
    };
    for id in stored {
        if !ids.contains(id.as_str()) {
            tx.execute("DELETE FROM case_analysis WHERE case_id=?1", [&id])
                .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM cases WHERE id=?1", [id])
                .map_err(|e| e.to_string())?;
        }
    }
    let revision = current
        .checked_add(1)
        .ok_or("Revisão da investigação excedeu o limite.")?;
    tx.execute("INSERT INTO metadata(key,value) VALUES('revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[revision.to_string()]).map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO metadata(key,value) VALUES('active',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[data.get("active").unwrap_or(&Value::Null).to_string()]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    crate::case_archive::retire_committed(dir, &data);
    Ok(json!({"revision":revision,"schemaVersion":2,"analysisContexts":analysis_contexts}))
}

/// A preview is never an evidence record. Inspect only actual Event containers,
/// so an original log's nested fields may use these words without false rejection.
fn reject_projected_evidence(case: &Value) -> Result<(), String> {
    let reject = |value: &Value| -> Result<(), String> {
        if crate::page_projection::is_projected_row(value)
            || matches!(
                value.get("kind").and_then(Value::as_str),
                Some("projected_page" | "exact_field")
            )
        {
            Err("Uma prévia de tabela não pode ser salva como evidência. Carregue o registro completo antes de salvar.".into())
        } else {
            Ok(())
        }
    };
    let rows = |value: &Value| -> Result<(), String> {
        reject(value)?;
        if let Some(values) = value.as_array() {
            for value in values {
                reject(value)?;
            }
        }
        Ok(())
    };
    for key in ["rows", "events"] {
        if let Some(value) = case.get(key) {
            rows(value)?;
        }
    }
    if let Some(items) = case.get("items").and_then(Value::as_array) {
        for item in items {
            for key in ["rows", "events"] {
                if let Some(value) = item.get(key) {
                    rows(value)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("loginsight-cases-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn migration_roundtrip_and_conflicting_revision() {
        let dir = Directory::new();
        let original = json!({"active":"c1","cases":[{"id":"c1","name":"Investigação","items":[{"note":"evidência"}]}]});
        std::fs::write(dir.0.join("cases.json"), original.to_string()).unwrap();
        let mut loaded = load_at(&dir.0).unwrap();
        let mut unchanged = loaded["cases"].clone();
        unchanged[0]
            .as_object_mut()
            .unwrap()
            .remove("analysisContext");
        assert_eq!(unchanged, original["cases"]);
        assert_eq!(loaded["cases"][0]["analysisContext"]["caseId"], "c1");
        assert_eq!(loaded["schemaVersion"], 2);
        let stale = loaded.clone();
        loaded["cases"][0]["name"] = json!("Atualizada");
        assert_eq!(save_at(&dir.0, loaded).unwrap()["revision"], 2);
        assert!(save_at(&dir.0, stale).is_err());
        assert_eq!(load_at(&dir.0).unwrap()["cases"][0]["name"], "Atualizada");
        assert!(dir.0.join("cases.json").exists());
    }
    #[test]
    fn corrupted_legacy_is_never_silently_replaced() {
        let dir = Directory::new();
        std::fs::write(dir.0.join("cases.json"), "broken").unwrap();
        assert!(load_at(&dir.0).is_err());
        std::fs::write(
            dir.0.join("cases.backup.json"),
            json!({"active":null,"cases":[]}).to_string(),
        )
        .unwrap();
        assert!(load_at(&dir.0).is_ok());
    }
    #[test]
    fn projected_rows_cannot_replace_evidence_but_nested_log_fields_remain_valid() {
        let dir = Directory::new();
        let original = json!({"active":"case","cases":[{"id":"case","items":[{"rows":[{
            "id":1,"event_ref":"source:1","fields":{"kind":"projected_row","version":1,"cells":["original data"]}
        }]}]}]});
        save_at(&dir.0, original.clone()).unwrap();
        let before = load_at(&dir.0).unwrap();
        let preview = json!({"kind":"projected_row","version":1,"row":{"id":1,"eventRef":"source:1"},"cells":[{"state":"preview","text":"partial"}]});
        for kind in ["projected_row", "projected_page", "exact_field"] {
            let mut preview = preview.clone();
            preview["kind"] = json!(kind);
            for container in ["items", "rows", "events"] {
                let mut candidate = before.clone();
                if container == "items" {
                    candidate["cases"][0]["items"][0]["rows"][0] = preview.clone();
                } else {
                    candidate["cases"][0][container] = json!([preview.clone()]);
                }
                assert!(save_at(&dir.0, candidate)
                    .unwrap_err()
                    .contains("registro completo"));
                assert_eq!(load_at(&dir.0).unwrap(), before);
            }
        }
        assert_eq!(before["cases"][0]["items"], original["cases"][0]["items"]);
    }
}
