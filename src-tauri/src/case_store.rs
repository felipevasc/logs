//! Transactional, versioned case store. Only modified cases are rewritten.
use rusqlite::{params, Connection};
use serde_json::{json, Value};
fn connect(dir: &std::path::Path) -> Result<Connection, String> {
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = Connection::open(dir.join("investigations.sqlite3")).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS cases(id TEXT PRIMARY KEY, body TEXT NOT NULL, position INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(|e|e.to_string())?;
    Ok(conn)
}
pub fn load() -> Result<Value, String> {
    load_at(&crate::config_dir())
}
fn load_at(dir: &std::path::Path) -> Result<Value, String> {
    let conn = connect(dir)?;
    let version: i64 = conn
        .query_row(
            "SELECT count(*) FROM metadata WHERE key='revision'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if version == 0 {
        drop(conn);
        let paths = [dir.join("cases.json"), dir.join("cases.backup.json")];
        let mut legacy = None;
        for path in &paths {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    if value.get("cases").is_some_and(Value::is_array) {
                        legacy = Some(value);
                        break;
                    }
                }
            }
        }
        if legacy.is_none() && paths.iter().any(|p| p.exists()) {
            return Err("Não foi possível recuperar os casos salvos. Os arquivos originais foram preservados.".into());
        }
        let mut legacy = legacy.unwrap_or(json!({"active":null,"cases":[]}));
        if let Some(object) = legacy.as_object_mut() {
            object.remove("revision");
        }
        save_at(dir, legacy)?;
        return load_at(dir);
    }
    // A read transaction keeps cases and revision from the same snapshot.
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
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
    let cases = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .map(|r| {
            r.map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(
        json!({"schemaVersion":2,"revision":revision,"active":serde_json::from_str::<Value>(&active).unwrap_or(Value::Null),"cases":cases}),
    )
}
pub fn save(data: Value) -> Result<Value, String> {
    save_at(&crate::config_dir(), data)
}
fn save_at(dir: &std::path::Path, data: Value) -> Result<Value, String> {
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
    for (position, case) in cases.iter().enumerate() {
        let id = case["id"].as_str().unwrap();
        let body = serde_json::to_string(case).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO cases(id,body,position) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET body=excluded.body,position=excluded.position WHERE cases.body<>excluded.body OR cases.position<>excluded.position",params![id,body,position as i64]).map_err(|e|e.to_string())?;
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
            tx.execute("DELETE FROM cases WHERE id=?1", [id])
                .map_err(|e| e.to_string())?;
        }
    }
    let revision = current + 1;
    tx.execute("INSERT INTO metadata(key,value) VALUES('revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[revision.to_string()]).map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO metadata(key,value) VALUES('active',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[data.get("active").unwrap_or(&Value::Null).to_string()]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({"revision":revision,"schemaVersion":2}))
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
        assert_eq!(loaded["cases"], original["cases"]);
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
}
