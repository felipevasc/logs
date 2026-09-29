//! The columnar engine must answer exactly as the line engine: same lines,
//! counts, groups, histograms, charts and summaries for every filter.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::fmt::Write as _;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

fn json_lines(seed: u64, count: usize) -> String {
    let mut rng = Rng(seed);
    let mut out = String::new();
    let base = 1_772_359_200_000i64; // 2026-03-01T10:00:00Z
    for i in 0..count {
        let mut record = serde_json::Map::new();
        let ts = base + (i as i64) * 5_000 + rng.below(4_000) as i64;
        match rng.below(40) {
            0 => {}
            1 => {
                record.insert("timestamp".into(), json!(ts));
            }
            2 => {
                record.insert("timestamp".into(), json!("sem data"));
            }
            3 => {
                record.insert("timestamp".into(), json!(0));
            }
            _ => {
                let t = chrono::DateTime::from_timestamp_millis(ts).unwrap();
                record.insert("@timestamp".into(), json!(t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()));
            }
        }
        let level = rng.pick(&["error", "ERROR", "warn", "info", "info", "info", "debug", "SEVERE", "notice", "E", "3", "", "Crítico", "trace"]);
        if !level.is_empty() {
            record.insert("level".into(), json!(level));
        }
        let service = rng.pick(&["api", "auth", "billing", "", "10.1.2.3", "Ação", "API"]);
        if !service.is_empty() {
            record.insert("service".into(), json!(service));
        }
        match rng.below(8) {
            0 => {}
            1 => {
                record.insert("event_id".into(), json!(7045));
            }
            _ => {
                record.insert("event_id".into(), json!(rng.pick(&["4625", "4624", "404", "500", "200", "4625"])));
            }
        }
        let user = rng.pick(&["root", "Admin", "admin", "ROOT", "joão", "", "svc_backup", "Σοφία"]);
        let ip = rng.pick(&["10.0.0.5", "10.0.3.7", "192.168.1.9", "2001:db8::1", "45.90.12.3", "not-ip"]);
        let n = rng.below(1000);
        let message = match rng.below(14) {
            0 => format!("Failed password for {user} from {ip} port 22 ssh2"),
            1 => format!("Accepted password for {user} from {ip} port 22"),
            2 => format!("GET /api/pay?id={n}&user={user} took {} ms", rng.below(900)),
            3 => format!("sudo: {user} : TTY=pts/0 ; PWD=/root ; USER=root ; COMMAND=/usr/bin/curl http://evil/{n}"),
            4 => format!("Timeout for request {n}"),
            5 => format!("Ação concluída com sucesso {n}"),
            6 => format!("İstanbul ΣΑΣ σας {n}"),
            7 => format!("linha\tcom\ttabs {n}"),
            8 => String::new(),
            9 => format!("Erro crítico: disco {}% cheio", rng.below(100)),
            10 => format!("{} fim", "x".repeat(600)),
            11 => format!("Timeout ao conectar em {ip} após 30s (tentativa {n})"),
            12 => format!("user {user} logged in from {ip}"),
            _ => format!("uuid 3f2a1b4c-1111-2222-3333-444455556666 req {n} 0x1f"),
        };
        if !message.is_empty() || rng.below(2) == 0 {
            record.insert("message".into(), json!(message));
        }
        match rng.below(9) {
            0 => {}
            1 => {
                record.insert("user".into(), Value::Null);
            }
            2 => {
                record.insert("User".into(), json!(user));
            }
            3 => {
                record.insert("user".into(), json!(user));
                record.insert("User".into(), json!("outro"));
            }
            _ => {
                record.insert("user".into(), json!(user));
            }
        }
        match rng.below(4) {
            0 => {
                record.insert("status".into(), json!(rng.pick(&["500", "404", " 200 ", "abc"])));
            }
            1 => {}
            _ => {
                record.insert("status".into(), json!([200, 404, 500, 302][rng.below(4)]));
            }
        }
        match rng.below(7) {
            0 => {
                record.insert("bytes".into(), json!("10 MB"));
            }
            1 => {
                record.insert("bytes".into(), json!("2,5 KB"));
            }
            2 => {
                record.insert("bytes".into(), json!(rng.below(5000)));
            }
            3 => {
                record.insert("bytes".into(), json!("abc"));
            }
            4 => {
                record.insert("bytes".into(), json!(rng.pick(&["-0", "0", "1e3", "12.345"])));
            }
            _ => {}
        }
        if rng.below(3) == 0 {
            record.insert("duration_ms".into(), json!(rng.below(2000) as f64 / 7.0));
        }
        match rng.below(6) {
            0 => {
                record.insert("latency".into(), json!("120 ms"));
            }
            1 => {
                record.insert("latency".into(), json!("1.5 s"));
            }
            2 => {
                record.insert("latency".into(), json!(rng.below(300)));
            }
            _ => {}
        }
        if rng.below(2) == 0 {
            record.insert("src_ip".into(), json!(ip));
        }
        if rng.below(4) == 0 {
            record.insert("dst_port".into(), json!([22, 443, 8080][rng.below(3)]));
        }
        if rng.below(5) == 0 {
            record.insert("user_agent".into(), json!(rng.pick(&["curl/7.68.0", "sqlmap/1.5#stable", "Mozilla/5.0 (Windows NT 10.0)"])));
        }
        if rng.below(6) == 0 {
            record.insert("url".into(), json!(format!("/api/pay?id={n}&user={user}")));
        }
        if rng.below(7) == 0 {
            record.insert("tags".into(), json!(["a", rng.pick(&["b", "c"])]));
        }
        if rng.below(11) == 0 {
            record.insert("meta".into(), json!({}));
        }
        if rng.below(5) == 0 {
            record.insert("flag".into(), json!(rng.below(2) == 0));
        }
        if rng.below(9) == 0 {
            record.insert("a".into(), json!({"c": n, "d": {"e": "fundo"}}));
        }
        if rng.below(13) == 0 {
            record.insert("@user".into(), json!("literal"));
        }
        if rng.below(3) == 0 {
            record.insert("path".into(), json!(rng.pick(&["/api/pay", "/API/pay", "/health"])));
        }
        if rng.below(4) == 0 {
            record.insert("host".into(), json!(format!("srv-{}", rng.below(4))));
        }
        out.push_str(&Value::Object(record).to_string());
        out.push('\n');
    }
    out
}

fn csv_lines(seed: u64, count: usize) -> String {
    let mut rng = Rng(seed);
    let mut out = String::from("Data;Hora;Nível;Origem;Mensagem;Usuário\n");
    for i in 0..count {
        let _ = writeln!(
            out,
            "{:02}/03/2026;{:02}:{:02}:{:02};{};{};{};{}",
            1 + i % 3,
            8 + (i / 60) % 10,
            i % 60,
            rng.below(60),
            rng.pick(&["Erro", "Aviso", "Informação", "falha"]),
            rng.pick(&["ERP", "Portal", ""]),
            rng.pick(&["Pedido 12 enviado", "Falha ao gravar nota 7", "Timeout no banco", "Usuário bloqueado"]),
            rng.pick(&["ana", "Bruno", "", "root"]),
        );
    }
    out
}

fn syslog_lines(seed: u64, count: usize) -> String {
    let mut rng = Rng(seed);
    let mut out = String::new();
    for i in 0..count {
        let _ = writeln!(
            out,
            "Mar  {} 1{}:{:02}:{:02} {} sshd[{}]: {} for {} from {} port 22 ssh2",
            1 + i % 2,
            i % 10,
            i % 60,
            rng.below(60),
            rng.pick(&["web01", "db02"]),
            1000 + rng.below(9000),
            rng.pick(&["Failed password", "Accepted publickey", "Invalid user"]),
            rng.pick(&["root", "admin", "deploy"]),
            rng.pick(&["45.90.12.3", "10.0.0.8", "192.168.1.20"]),
        );
    }
    out
}

/// First difference between two JSON values, as a path and both sides.
fn difference(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys()) {
                let (va, vb) = (x.get(key).unwrap_or(&Value::Null), y.get(key).unwrap_or(&Value::Null));
                if let Some(d) = difference(va, vb, &format!("{path}.{key}")) {
                    return Some(d);
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (va, vb)) in x.iter().zip(y).enumerate() {
                if let Some(d) = difference(va, vb, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            (x.len() != y.len()).then(|| format!("{path}: {} vs {} itens", x.len(), y.len()))
        }
        _ if a == b => None,
        _ => Some(format!(
            "{path}: colunar={} linhas={}",
            a.to_string().chars().take(300).collect::<String>(),
            b.to_string().chars().take(300).collect::<String>()
        )),
    }
}

fn same(what: &str, columnar: Value, lines: Value) {
    if let Some(d) = difference(&columnar, &lines, "") {
        panic!("{what}\n  {d}");
    }
}

/// Stats levels with equal counts may come in any order from the line engine.
fn sorted_levels(mut v: Value) -> Value {
    if let Some(levels) = v.get_mut("levels").and_then(Value::as_array_mut) {
        levels.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
    }
    v
}

/// Compares both engines on one filter list; `full` also covers every page
/// order, grouping, chart, pivot, summary and comparison.
fn check_filters(src: &Source, filters: &str, full: bool) {
    testkit::clear_caches();
    let ids = src.matches(Engine::Columnar, filters);
    let lines = src.matches(Engine::Lines, filters);
    assert_eq!(ids.len(), lines.len(), "quantidade de linhas: {filters}");
    assert_eq!(ids, lines, "linhas selecionadas: {filters}");
    assert_eq!(src.count(Engine::Columnar, filters), lines.len(), "contagem: {filters}");
    same(
        &format!("estatísticas {filters}"),
        sorted_levels(src.stats(Engine::Columnar, filters)),
        sorted_levels(src.stats(Engine::Lines, filters)),
    );
    let sorts: &[(&str, &str)] = if full {
        &[("", "asc"), ("timestamp", "desc"), ("timestamp", "asc"), ("level", "desc"), ("source", "asc"), ("code", "desc"), ("@user", "asc"), ("status", "desc"), ("bytes", "asc"), ("name", "asc"), ("message", "desc")]
    } else {
        &[("timestamp", "desc"), ("status", "desc")]
    };
    let offsets: &[usize] = if full { &[0, 37] } else { &[0] };
    for &(sort, dir) in sorts {
        for &offset in offsets {
            same(
                &format!("página {sort} {dir} {offset} {filters}"),
                src.query(Engine::Columnar, filters, sort, dir, offset, 40),
                src.query(Engine::Lines, filters, sort, dir, offset, 40),
            );
        }
    }
    let mut explore_c = src.explore(Engine::Columnar, filters, "timestamp", "desc");
    let mut explore_l = src.explore(Engine::Lines, filters, "timestamp", "desc");
    explore_c["stats"] = sorted_levels(explore_c["stats"].take());
    explore_l["stats"] = sorted_levels(explore_l["stats"].take());
    same(&format!("explorar {filters}"), explore_c, explore_l);
    let count = r#"[{"func":"count","column":"*","alias":"n"}]"#;
    let groups: &[&str] = if full {
        &["source", "level", "timestamp", "code", "@user", "user", "status", "name", "@src_scope", "@tool", "a.c", "tags", "*", "arquivo"]
    } else {
        &["@user", "status"]
    };
    for group in groups {
        same(
            &format!("agrupar {group} {filters}"),
            src.aggregate(Engine::Columnar, filters, group, count),
            src.aggregate(Engine::Lines, filters, group, count),
        );
    }
    let columns = ["source", "level", "code", "@user", "status", "timestamp", "user_agent"];
    same(
        &format!("facetas {filters}"),
        src.multi_count(Engine::Columnar, filters, &columns),
        src.multi_count(Engine::Lines, filters, &columns),
    );
    if !full {
        return;
    }
    let rich = r#"[{"func":"count","column":"*","alias":""},{"func":"count_distinct","column":"src_ip","alias":"ips"},
        {"func":"sum","column":"bytes","alias":"b"},{"func":"avg","column":"duration_ms","alias":"d"},
        {"func":"min","column":"timestamp","alias":"primeiro"},{"func":"max","column":"latency","alias":"lat"},
        {"func":"string_agg","column":"user","alias":"users"},{"func":"avg","column":"status","alias":"st"},
        {"func":"count_distinct","column":"timestamp","alias":"tempos"},{"func":"sum","column":"id","alias":"ids"}]"#;
    for group in ["@user", "level", "source", "timestamp"] {
        same(
            &format!("resumir {group} {filters}"),
            src.aggregate(Engine::Columnar, filters, group, rich),
            src.aggregate(Engine::Lines, filters, group, rich),
        );
    }
    let meta = r#"[{"func":"count_distinct","column":"level","alias":"n"},{"func":"min","column":"timestamp","alias":"t"},{"func":"sum","column":"level","alias":"s"}]"#;
    same(
        &format!("metadados {filters}"),
        src.aggregate(Engine::Columnar, filters, "level", meta),
        src.aggregate(Engine::Lines, filters, "level", meta),
    );
    for spec in [
        r#"{"chart":"time","metric":"count"}"#,
        r#"{"chart":"time","metric":"count","split":"source"}"#,
        r#"{"chart":"time","metric":"avg","field":"bytes"}"#,
        r#"{"chart":"time","metric":"sum","field":"latency","unit":"auto"}"#,
        r#"{"chart":"time","metric":"max","field":"duration_ms","interval_ms":3600000,"split":"level"}"#,
        r#"{"chart":"time","metric":"distinct","field":"user","split":"source"}"#,
        r#"{"chart":"terms","metric":"count","field":"status"}"#,
        r#"{"chart":"terms","metric":"count"}"#,
        r#"{"chart":"terms","metric":"count","field":"timestamp","limit":5}"#,
        r#"{"chart":"terms","metric":"sum","field":"bytes","limit":3}"#,
        r#"{"chart":"terms","metric":"distinct","field":"user"}"#,
        r#"{"chart":"terms","metric":"avg","field":"status","split":"level"}"#,
        r#"{"chart":"terms","metric":"min","unit":"bytes","field":"bytes"}"#,
    ] {
        same(
            &format!("gráfico {spec} {filters}"),
            src.series(Engine::Columnar, filters, spec),
            src.series(Engine::Lines, filters, spec),
        );
    }
    for spec in [
        r#"{"rows":["source","level"],"cols":["status"],"values":[{"func":"count","column":"*","alias":"n"},{"func":"avg","column":"bytes","alias":"b"}]}"#,
        r#"{"rows":["user"],"cols":[],"values":[{"func":"count_distinct","column":"src_ip","alias":""},{"func":"string_agg","column":"host","alias":"h"},{"func":"max","column":"timestamp","alias":"t"}],"limit_rows":5}"#,
        r#"{"rows":["timestamp"],"cols":["name"],"values":[{"func":"sum","column":"latency","alias":"l"}]}"#,
    ] {
        same(
            &format!("pivô {spec} {filters}"),
            src.pivot(Engine::Columnar, filters, spec),
            src.pivot(Engine::Lines, filters, spec),
        );
    }
    same(
        &format!("resumo {filters}"),
        src.overview(Engine::Columnar, filters),
        src.overview(Engine::Lines, filters),
    );
    let day = 86_400_000;
    let start = 1_772_359_200_000i64;
    same(
        &format!("comparação {filters}"),
        src.compare(Engine::Columnar, filters, (start, start + day / 4), (start + day / 4 + 1, start + day)),
        src.compare(Engine::Lines, filters, (start, start + day / 4), (start + day / 4 + 1, start + day)),
    );
}

fn filters_of(conditions: &[(&str, &str, &str)]) -> String {
    let list: Vec<Value> = conditions
        .iter()
        .map(|(column, op, value)| json!({"column": column, "op": op, "value": value}))
        .collect();
    Value::Array(list).to_string()
}

fn query(text: &str) -> String {
    filters_of(&[("_all", "query", text)])
}

const CODES: &str = r#"{"*": {"4625": {"name": "Falha de logon", "description": "Conta falhou ao entrar"},
    "500": {"name": "Erro interno", "description": ""}},
    "auth": {"404": {"name": "Não encontrado", "description": "Timeout do recurso"}}}"#;

fn write(dir: &std::path::Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// Both tests point the app at their own folders through the environment.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn columnar_engine_answers_as_line_engine() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("dados"));
    testkit::set_engine_dir(&dir.path().join("motor").to_string_lossy());
    let files = [
        write(dir.path(), "app.jsonl", &json_lines(7, 1_500)),
        write(dir.path(), "erp.csv", &csv_lines(11, 300)),
        write(dir.path(), "auth.log", &syslog_lines(13, 300)),
        write(dir.path(), "outro.jsonl", &json_lines(99, 500)),
    ];
    let paths: Vec<&str> = files.iter().map(String::as_str).collect();
    let src = Source::open(&paths, CODES, "[]").unwrap();
    src.prepare().unwrap();

    let mut cases = vec![
        "[]".to_string(),
        filters_of(&[("level", "equals", "Erro")]),
        filters_of(&[("level", "not_equals", "erro")]),
        filters_of(&[("level", "equals", "severe")]),
        filters_of(&[("source", "contains", "ap")]),
        filters_of(&[("source", "equals_exact", "Ação")]),
        filters_of(&[("source", "starts_with", "AP")]),
        filters_of(&[("source", "empty", "")]),
        filters_of(&[("code", "not_empty", "")]),
        filters_of(&[("code", "gt", "450")]),
        filters_of(&[("code", "in", "404, 4625\n500")]),
        filters_of(&[("code", "not_in", "404")]),
        filters_of(&[("code", "in_exact", "4625\n200")]),
        filters_of(&[("message", "regex", "^Timeout.*\\d+$")]),
        filters_of(&[("message", "contains", "AÇÃO")]),
        filters_of(&[("message", "not_contains", "timeout")]),
        filters_of(&[("_all", "contains", "timeout")]),
        filters_of(&[("_all", "not_contains", "root")]),
        filters_of(&[("_all", "regex", "port 22")]),
        filters_of(&[("@user", "equals", "root")]),
        filters_of(&[("@src_ip", "cidr", "10.0.0.0/8")]),
        filters_of(&[("src_ip", "not_cidr", "10.0.0.0/8, 2001:db8::/32")]),
        filters_of(&[("bytes", "gt", "1 MB")]),
        filters_of(&[("bytes", "lte", "0")]),
        filters_of(&[("status", "gte", "404")]),
        filters_of(&[("id", "lt", "300")]),
        filters_of(&[("user", "empty", "")]),
        filters_of(&[("User", "not_empty", "")]),
        filters_of(&[("name", "contains", "logon")]),
        filters_of(&[("description", "equals", "Timeout do recurso")]),
        filters_of(&[("tags", "contains", "\"b\"")]),
        filters_of(&[("flag", "equals", "true")]),
        filters_of(&[("event_ref", "contains", ":1")]),
        filters_of(&[("arquivo", "equals", "erp.csv")]),
        filters_of(&[("ausente", "not_contains", "x")]),
        filters_of(&[("ausente", "empty", "")]),
        filters_of(&[("ausente", "not_in", "a, b")]),
        filters_of(&[("ausente", "gt", "3")]),
        filters_of(&[("raw", "contains", "root")]),
        filters_of(&[("level", "equals", "Erro"), ("source", "not_equals", "api")]),
        filters_of(&[("_all", "contains", "failed"), ("@user", "in", "root, admin")]),
        filters_of(&[("user_agent", "contains", "curl"), ("_all", "not_contains", "evil")]),
        query("timeout"),
        query("Timeout for request"),
        query("user:root"),
        query("user:ROOT"),
        query("status>=500"),
        query("status:(404 OR 500)"),
        query("-status:404"),
        query("NOT level:aviso"),
        query("level:erro"),
        query("ip:10.0.0.0/8"),
        query("user:adm*"),
        query("message:/fail\\w+/"),
        query("path=\"/api/pay\""),
        query("path:/api/pay"),
        query("status:*"),
        query("ausente:*"),
        query("@tool:curl"),
        query("@tool:sqlmap"),
        query("tudo:root"),
        query("erro*"),
        query("tim?out"),
        query("/fal[hl]a/"),
        query("data>2026-03-01T12:00:00Z"),
        query("data:2026-03-01*"),
        query("nome:logon"),
        query("descricao:timeout"),
        query("logon"),
        query("Ação"),
        query("istanbul"),
        query("σας"),
        query("ΣΑΣ"),
        query("a.c>500"),
        query("a.d.e:fundo"),
        query("tags.0:a"),
        query("bytes>1MB"),
        query("bytes:1000..20000"),
        query("latency>100"),
        query("id:42"),
        query("@src_scope:privado"),
        query("@action:* AND @outcome:*"),
        query("usuario:ana OR origem:ERP"),
        query("host:srv-1 -user:root"),
        query("deteccao:\"auth.bruteforce.source\""),
        query("\"tabs 1\""),
        query("user:(root admin \"joão\")"),
        query("user!=root"),
        query("codigo:4625 AND (nivel:erro OR nivel:aviso)"),
        query("-ausente:x"),
        query("NOT ausente:*"),
        query("bruto:root"),
    ];
    cases.push(filters_of(&[("message", "pattern", "Timeout for request ‹…›")]));
    cases.push(filters_of(&[("timestamp", "between", "2026-03-01 10:30")]).replace("\"value\":\"2026-03-01 10:30\"", "\"value\":\"2026-03-01 10:30\",\"value2\":\"2026-03-01 12:00\""));
    cases.push(filters_of(&[("timestamp", "empty", "")]));
    for (i, case) in cases.iter().enumerate() {
        check_filters(&src, case, i % 4 == 0);
    }
}

#[test]
fn derived_fields_and_catalog_names_stay_consistent() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("dados"));
    testkit::set_engine_dir(&dir.path().join("motor").to_string_lossy());
    let file = write(dir.path(), "app.jsonl", &json_lines(21, 800));
    let derived = r#"[{"name":"conta","source":"message","rules":[
        {"pattern":"for (\\w+) from","filter":{"column":"name","op":"contains","value":"logon"}},
        {"pattern":"user (\\S+)"}]}]"#;
    let src = Source::open(&[file.as_str()], CODES, derived).unwrap();
    src.prepare().unwrap();
    for case in [
        "[]".to_string(),
        query("conta:root"),
        query("conta:*"),
        filters_of(&[("conta", "not_empty", "")]),
        query("logon"),
        filters_of(&[("name", "equals", "Falha de logon")]),
    ] {
        check_filters(&src, &case, true);
    }
}
