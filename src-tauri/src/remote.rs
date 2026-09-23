//! Read-only Elasticsearch snapshots. Configuration contains no plaintext secrets;
//! Windows credentials are protected by user-scoped DPAPI, others are session-only.
use crate::operations;
use parking_lot::Mutex;
use reqwest::{blocking::Client, Method, Url};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

const MAX_RECORDS: usize = 5_000_000;
const BATCH_SIZE: usize = 500;
const RESPONSE_LIMIT: u64 = 32 * 1024 * 1024;
const METADATA_FIELD: &str = "_loginsight_remote";
static STORE_LOCK: Mutex<()> = Mutex::new(());
static SESSION: LazyLock<Mutex<HashMap<String, Secret>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteKind {
    Elasticsearch,
    Kibana,
}

fn default_time_field() -> String {
    "@timestamp".into()
}
fn default_limit() -> usize {
    100_000
}
fn default_kibana_version() -> String {
    "auto".into()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteConfig {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: RemoteKind,
    pub url: String,
    pub index: String,
    #[serde(default = "default_time_field")]
    pub time_field: String,
    #[serde(default)]
    pub username: String,
    #[serde(default = "default_limit")]
    pub max_records: usize,
    #[serde(default)]
    pub query: Option<Value>,
    #[serde(default = "default_kibana_version")]
    pub kibana_version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    #[serde(flatten)]
    connection: RemoteConfig,
    has_password: bool,
    password_saved: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteList {
    connections: Vec<SavedConnection>,
    persistent_secrets: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Secret {
    identity: String,
    password: String,
}

#[derive(Serialize)]
pub struct TestResult {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    path: String,
    count: usize,
    total: Option<u64>,
    total_relation: String,
    limited: bool,
    bytes: u64,
    metadata_field: String,
    warning: Option<String>,
    order: String,
}

fn validate(mut c: RemoteConfig) -> Result<RemoteConfig, String> {
    c.name = c.name.trim().to_owned();
    c.url = c.url.trim().trim_end_matches('/').to_owned();
    c.index = c.index.trim().to_owned();
    c.time_field = c.time_field.trim().to_owned();
    if c.name.is_empty() || c.name.len() > 160 {
        return Err("Informe um nome com até 160 caracteres.".into());
    }
    if !c.id.is_empty()
        && (c.id.len() > 80 || !c.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    {
        return Err("Identificador de conexão inválido.".into());
    }
    if c.url.len() > 4096 {
        return Err("URL excede o limite de 4096 caracteres.".into());
    }
    let url = Url::parse(&c.url)
        .map_err(|_| "URL inválida. Use http:// ou https:// e o endereço base do serviço.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use uma URL HTTP(S) base, sem usuário, senha, parâmetros ou fragmento. Informe as credenciais nos campos próprios.".into());
    }
    c.url = url.as_str().trim_end_matches('/').to_owned();
    if c.index.is_empty()
        || c.index.len() > 512
        || !c
            .index
            .chars()
            .all(|ch| ch.is_alphanumeric() || "-_.:*,".contains(ch))
        || c.index == "."
        || c.index == ".."
    {
        return Err("Informe índices, aliases ou padrões como logs-*. Separe índices por vírgula; caminhos e parâmetros não são aceitos.".into());
    }
    if c.username.len() > 512 || c.username.contains([':', '\r', '\n']) {
        return Err("Usuário inválido para autenticação Basic.".into());
    }
    if c.time_field.len() > 256 || c.time_field.chars().any(char::is_control) {
        return Err("Campo de horário inválido.".into());
    }
    if !(1..=MAX_RECORDS).contains(&c.max_records) {
        return Err("O limite deve estar entre 1 e 5.000.000 registros.".into());
    }
    if let Some(query) = c.query.as_ref().filter(|v| !v.is_null()) {
        let object = query
            .as_object()
            .ok_or("A consulta deve ser um objeto Query DSL JSON.")?;
        if query.to_string().len() > 64 * 1024
            || [
                "query",
                "size",
                "pit",
                "sort",
                "search_after",
                "from",
                "aggs",
                "aggregations",
            ]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return Err("Informe apenas a cláusula Query DSL (por exemplo {\"match_all\":{}}), sem query, paginação ou agregações no nível superior; máximo 64 KiB.".into());
        }
    } else {
        c.query = None;
    }
    if c.kibana_version.trim().is_empty() {
        c.kibana_version = "auto".into();
    }
    Ok(c)
}

fn identity(c: &RemoteConfig) -> String {
    json!([c.kind, c.url, c.username]).to_string()
}

fn database(root: &Path) -> Result<Connection, String> {
    fs::create_dir_all(root).map_err(|e| format!("Não foi possível preparar as conexões: {e}"))?;
    let db = Connection::open(root.join("remote-connections.sqlite3"))
        .map_err(|e| format!("Não foi possível abrir as conexões: {e}"))?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA secure_delete=ON; CREATE TABLE IF NOT EXISTS remote_connections (id TEXT PRIMARY KEY, config TEXT NOT NULL, secret BLOB);")
        .map_err(|e| format!("Não foi possível preparar as conexões: {e}"))?;
    Ok(db)
}

fn saved(c: RemoteConfig, encrypted: bool) -> SavedConnection {
    let session = SESSION
        .lock()
        .get(&c.id)
        .is_some_and(|secret| secret.identity == identity(&c));
    SavedConnection {
        connection: c,
        has_password: encrypted || session,
        password_saved: encrypted,
    }
}

fn list_impl(root: &Path) -> Result<RemoteList, String> {
    let _lock = STORE_LOCK.lock();
    let db = database(root)?;
    let mut statement = db
        .prepare("SELECT config, secret IS NOT NULL FROM remote_connections ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut connections = Vec::new();
    for row in rows {
        let (config, encrypted) = row.map_err(|e| e.to_string())?;
        let c = serde_json::from_str(&config).map_err(|_| {
            "Uma conexão salva está inválida. O arquivo de configuração foi preservado."
        })?;
        connections.push(saved(c, encrypted));
    }
    Ok(RemoteList {
        connections,
        persistent_secrets: cfg!(windows),
    })
}

fn password_for(
    root: &Path,
    c: &RemoteConfig,
    supplied: Option<String>,
) -> Result<Option<String>, String> {
    if let Some(password) = supplied {
        if password.len() > 16_384 {
            return Err("Senha excede o tamanho permitido.".into());
        }
        if c.username.is_empty() && !password.is_empty() {
            return Err("Informe o usuário da conexão.".into());
        }
        return Ok(Some(password));
    }
    if c.username.is_empty() {
        return Ok(None);
    }
    if let Some(secret) = SESSION
        .lock()
        .get(&c.id)
        .filter(|secret| secret.identity == identity(c))
    {
        return Ok(Some(secret.password.clone()));
    }
    if !c.id.is_empty() {
        let _lock = STORE_LOCK.lock();
        let db = database(root)?;
        let bytes: Option<Vec<u8>> = db
            .query_row(
                "SELECT secret FROM remote_connections WHERE id=?1",
                [&c.id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        if let Some(bytes) = bytes {
            let plain = unprotect(&bytes)?;
            let secret: Secret = serde_json::from_slice(&plain)
                .map_err(|_| "Credencial protegida inválida. Informe a senha novamente.")?;
            if secret.identity == identity(c) {
                return Ok(Some(secret.password));
            }
        }
    }
    Ok(None)
}

fn client_for(
    root: &Path,
    connection: RemoteConfig,
    password: Option<String>,
) -> Result<RemoteClient, String> {
    let c = validate(connection)?;
    let password = password_for(root, &c, password)?;
    if !c.username.is_empty() && password.is_none() {
        return Err("Informe a senha. A credencial salva só pode ser reutilizada com o mesmo endereço e usuário.".into());
    }
    RemoteClient::new(c, password)
}

fn save_impl(
    root: &Path,
    connection: RemoteConfig,
    password: Option<String>,
    remember: bool,
) -> Result<SavedConnection, String> {
    let mut c = validate(connection)?;
    if remember && !cfg!(windows) {
        return Err("Salvar senha de forma protegida está disponível no Windows. Desmarque Salvar senha para usar apenas nesta sessão.".into());
    }
    let password = password_for(root, &c, password)?;
    if c.id.is_empty() {
        c.id = uuid::Uuid::new_v4().to_string();
    }
    let secret = password.map(|password| Secret {
        identity: identity(&c),
        password,
    });
    let encrypted = if remember {
        secret
            .as_ref()
            .map(|secret| protect(&serde_json::to_vec(secret).map_err(|e| e.to_string())?))
            .transpose()?
    } else {
        None
    };
    let _lock = STORE_LOCK.lock();
    let db = database(root)?;
    db.execute("INSERT INTO remote_connections(id,config,secret) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET config=excluded.config,secret=excluded.secret", params![c.id, serde_json::to_string(&c).map_err(|e| e.to_string())?, encrypted])
        .map_err(|e| format!("Não foi possível salvar a conexão: {e}"))?;
    if let Some(secret) = secret {
        SESSION.lock().insert(c.id.clone(), secret);
    } else {
        SESSION.lock().remove(&c.id);
    }
    operations::commit();
    Ok(saved(c, remember && encrypted.is_some()))
}

#[cfg(windows)]
fn crypt(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // User-scoped DPAPI: never CRYPTPROTECT_LOCAL_MACHINE; no plaintext file.
    unsafe {
        let result = if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                windows::core::w!("LogInsight remote"),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        result.map_err(|_| "Não foi possível acessar a senha protegida pelo Windows. Use o mesmo usuário do Windows ou informe a senha novamente.")?;
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(result)
    }
}
#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crypt(bytes, false)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crypt(bytes, true)
}
#[cfg(not(windows))]
fn protect(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("Proteção persistente de senhas indisponível neste sistema.".into())
}
#[cfg(not(windows))]
fn unprotect(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("Esta senha foi protegida pelo Windows. Informe a senha para esta sessão.".into())
}

struct RemoteClient {
    client: Client,
    connection: RemoteConfig,
    password: Option<String>,
}
impl RemoteClient {
    fn new(connection: RemoteConfig, password: Option<String>) -> Result<Self, String> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("LogInsight/0.1")
            .build()
            .map_err(|_| "Não foi possível iniciar o cliente HTTP com TLS.")?;
        Ok(Self {
            client,
            connection,
            password,
        })
    }
    fn request(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&Value>,
        cleanup: bool,
    ) -> Result<Value, String> {
        if !cleanup {
            operations::check()?;
        }
        let mut url = Url::parse(&self.connection.url).map_err(|_| "Endereço inválido.")?;
        let is_kibana = self.connection.kind == RemoteKind::Kibana;
        let actual_method = if is_kibana {
            url.set_path(&format!(
                "{}/api/console/proxy",
                url.path().trim_end_matches('/')
            ));
            url.query_pairs_mut()
                .append_pair("path", endpoint.trim_start_matches('/'))
                .append_pair("method", method.as_str());
            Method::POST
        } else {
            let (path, query) = endpoint.split_once('?').unwrap_or((endpoint, ""));
            url.set_path(&format!("{}{}", url.path().trim_end_matches('/'), path));
            if !query.is_empty() {
                url.set_query(Some(query));
            }
            method
        };
        let mut request = self
            .client
            .request(actual_method, url)
            .header("Accept", "application/json");
        if cleanup {
            request = request.timeout(Duration::from_secs(5));
        }
        if is_kibana {
            request = request
                .header("kbn-xsrf", "true")
                .header("x-elastic-product-origin", "kibana");
            if self.connection.kibana_version == "v9" || self.connection.kibana_version == "auto" {
                request = request
                    .header("elastic-api-version", "2023-10-31")
                    .header("Accept", "application/vnd.elasticsearch+json; compatible-with=8, application/json");
            }
        }
        if !self.connection.username.is_empty() {
            request = request.basic_auth(&self.connection.username, self.password.as_deref());
        }
        if let Some(body) = body {
            request = request.json(body);
        } else if is_kibana {
            request = request
                .header("Content-Type", "application/json")
                .body("{}");
        }
        let response = request.send().map_err(|e| {
            if e.is_timeout() {
                "Tempo de conexão esgotado. Verifique endereço e disponibilidade do serviço."
            } else {
                "Falha de conexão HTTP/TLS. Verifique endereço, rede e certificado do serviço."
            }
        })?;
        let status = if response.status().is_success() && self.connection.kind == RemoteKind::Kibana
        {
            response
                .headers()
                .get("x-console-proxy-status-code")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(response.status().as_u16())
        } else {
            response.status().as_u16()
        };
        if !(200..300).contains(&status) {
            return Err(match status { 401 => "Autenticação recusada (401). Revise usuário e senha; login SSO não é suportado.", 403 => "Acesso negado (403). A conta precisa de permissão de leitura/PIT e, no Kibana, acesso ao Console.", 301..=399 => "O serviço redirecionou a conexão. Use a URL final diretamente; credenciais não são encaminhadas a redirecionamentos.", 404 => "Endpoint ou índice não encontrado (404). Confirme o índice, Elasticsearch 7.10+ e, no Kibana, URL base/space e Console habilitado.", _ => "O serviço recusou a consulta. Confira o índice, Query DSL e permissões de leitura." }.into());
        }
        let mut bytes = Vec::new();
        response
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Falha ao ler a resposta do serviço.")?;
        if bytes.len() as u64 > RESPONSE_LIMIT {
            return Err("Uma página excedeu 32 MiB. Restrinja a consulta ou reduza o tamanho dos documentos.".into());
        }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| "O serviço não retornou JSON válido. Confirme o endereço da API; páginas de login/SSO não são aceitas.")?;
        if data.get("error").is_some_and(|e| !e.is_null()) {
            return Err("O serviço retornou erro na consulta. Confira a Query DSL, o índice e as permissões de leitura.".into());
        }
        Ok(data)
    }
}

struct PointInTime<'a> {
    remote: &'a RemoteClient,
    id: Option<String>,
}
impl<'a> PointInTime<'a> {
    fn open(remote: &'a RemoteClient) -> Result<Self, String> {
        let data = remote.request(
            Method::POST,
            &format!("/{}/_pit?keep_alive=2m", remote.connection.index),
            None,
            false,
        )?;
        let id = data.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("O serviço não retornou um PIT. É necessário Elasticsearch 7.10+ com permissão de leitura/PIT.")?.to_owned();
        let pit = Self {
            remote,
            id: Some(id),
        };
        ensure_complete(&data)?;
        Ok(pit)
    }
    fn update(&mut self, data: &Value) {
        if let Some(id) = data
            .get("pit_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.id = Some(id.to_owned());
        }
    }
    fn close(&mut self) -> Result<(), String> {
        let Some(id) = self.id.take() else {
            return Ok(());
        };
        let response =
            self.remote
                .request(Method::DELETE, "/_pit", Some(&json!({"id": id})), true)?;
        if response.get("succeeded").and_then(Value::as_bool) != Some(true) {
            return Err("O serviço não confirmou o fechamento do PIT.".into());
        }
        Ok(())
    }
}
impl Drop for PointInTime<'_> {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn ensure_complete(data: &Value) -> Result<(), String> {
    if data.get("timed_out").and_then(Value::as_bool) == Some(true)
        || data.get("terminated_early").and_then(Value::as_bool) == Some(true)
        || data
            .pointer("/_shards/failed")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        || ["failed", "partial", "skipped"].iter().any(|key| {
            data.get("_clusters")
                .and_then(|c| c.get(*key))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
        })
    {
        return Err("A consulta remota retornou resultados parciais (timeout, interrupção antecipada, shards ou clusters indisponíveis). Nenhum snapshot foi publicado; restrinja o período e tente novamente.".into());
    }
    Ok(())
}

fn query(c: &RemoteConfig, from: Option<&str>, to: Option<&str>) -> Result<Value, String> {
    let base = c.query.clone().unwrap_or_else(|| json!({"match_all": {}}));
    if from.is_none() && to.is_none() {
        return Ok(base);
    }
    if c.time_field.is_empty() {
        return Err("Informe o campo de horário para consultar um período.".into());
    }
    let mut range = serde_json::Map::new();
    for (key, value) in [("gte", from), ("lte", to)] {
        if let Some(value) = value {
            if value.trim().is_empty() || value.len() > 128 {
                return Err(
                    "Limite de período inválido. Use ISO 8601 ou data math do Elasticsearch."
                        .into(),
                );
            }
            range.insert(key.into(), Value::String(value.to_owned()));
        }
    }
    if let (Some(from), Some(to)) = (from, to) {
        if let (Ok(a), Ok(b)) = (
            chrono::DateTime::parse_from_rfc3339(from),
            chrono::DateTime::parse_from_rfc3339(to),
        ) {
            if a > b {
                return Err("O início do período deve ser anterior ao fim.".into());
            }
        }
    }
    Ok(json!({"bool": {"filter": [base, {"range": {c.time_field.clone(): range}}]}}))
}

fn test_impl(remote: &RemoteClient) -> Result<TestResult, String> {
    let mut pit = PointInTime::open(remote)?;
    let data = remote.request(Method::POST, "/_search", Some(&json!({"size":0,"track_total_hits":false,"pit":{"id":pit.id,"keep_alive":"2m"},"query":query(&remote.connection,None,None)?})), false)?;
    pit.update(&data);
    ensure_complete(&data)?;
    if data
        .pointer("/hits/hits")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err("A resposta de busca não é compatível com Elasticsearch.".into());
    }
    operations::check()?;
    pit.close()?;
    Ok(TestResult {
        ok: true,
        message: "Conexão, autenticação e leitura do índice verificadas com PIT.".into(),
    })
}

struct PendingFile {
    path: PathBuf,
    published: bool,
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn import_impl(
    remote: &RemoteClient,
    root: &Path,
    from: Option<String>,
    to: Option<String>,
    mut progress: impl FnMut(usize, usize),
) -> Result<ImportResult, String> {
    let query = query(&remote.connection, from.as_deref(), to.as_deref())?;
    let directory = root.join("remote-snapshots");
    fs::create_dir_all(&directory)
        .map_err(|e| format!("Não foi possível preparar o snapshot: {e}"))?;
    let id = uuid::Uuid::new_v4();
    let path = directory.join(format!("remote-{id}.jsonl"));
    let mut pending = PendingFile {
        path: directory.join(format!("remote-{id}.part")),
        published: false,
    };
    // Declared after the guard so the writer closes before cleanup on Windows.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending.path)
        .map_err(|e| format!("Não foi possível criar o snapshot: {e}"))?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    let mut pit = PointInTime::open(remote)?;
    let mut after: Option<Value> = None;
    let mut count = 0usize;
    let mut total = None;
    let mut total_relation = "unknown".to_owned();
    let mut complete = false;
    let mut collision = false;
    let sort = if remote.connection.time_field.is_empty() {
        json!([{"_shard_doc":"asc"}])
    } else {
        json!([{remote.connection.time_field.clone(): {"order":"desc","unmapped_type":"date","missing":"_last"}},{"_shard_doc":"asc"}])
    };
    while count < remote.connection.max_records {
        operations::check()?;
        let size = BATCH_SIZE.min(remote.connection.max_records - count);
        let mut body = json!({"size":size,"track_total_hits":after.is_none(),"pit":{"id":pit.id,"keep_alive":"2m"},"sort":sort,"query":query,"_source":true});
        if let Some(value) = after.as_ref() {
            body["search_after"] = value.clone();
        }
        let data = remote.request(Method::POST, "/_search", Some(&body), false)?;
        pit.update(&data);
        ensure_complete(&data)?;
        operations::check()?;
        if after.is_none() {
            let hits_total = data.pointer("/hits/total");
            total = hits_total.and_then(|v| {
                v.as_u64()
                    .or_else(|| v.get("value").and_then(Value::as_u64))
            });
            total_relation = if hits_total.is_some_and(Value::is_u64) {
                "eq"
            } else {
                hits_total
                    .and_then(|v| v.get("relation"))
                    .and_then(Value::as_str)
                    .filter(|r| matches!(*r, "eq" | "gte"))
                    .unwrap_or("unknown")
            }
            .into();
        }
        let hits = data
            .pointer("/hits/hits")
            .and_then(Value::as_array)
            .ok_or("A resposta não contém uma lista de registros Elasticsearch.")?;
        if hits.len() > size {
            return Err(
                "O serviço ignorou o limite de paginação; a importação foi interrompida.".into(),
            );
        }
        for hit in hits {
            operations::check()?;
            let mut source = hit.get("_source").and_then(Value::as_object).cloned().ok_or("Um registro não contém _source. Habilite a leitura de _source; nenhum snapshot parcial foi publicado.")?;
            let mut key = METADATA_FIELD.to_owned();
            while source.contains_key(&key) {
                key.push('_');
                collision = true;
            }
            let mut metadata: serde_json::Map<String, Value> = hit
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(key, _)| key.as_str() != "_source")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            metadata.insert(
                "connectionName".into(),
                Value::String(remote.connection.name.clone()),
            );
            source.insert(key, Value::Object(metadata));
            serde_json::to_writer(&mut writer, &source)
                .map_err(|e| format!("Não foi possível gravar o snapshot: {e}"))?;
            writer
                .write_all(b"\n")
                .map_err(|e| format!("Não foi possível gravar o snapshot: {e}"))?;
            count += 1;
        }
        progress(
            count,
            total
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(remote.connection.max_records),
        );
        if total_relation == "eq"
            && total.is_some_and(|n| count as u64 > n || hits.len() < size && (count as u64) < n)
        {
            return Err("A paginação retornou uma quantidade inconsistente de registros; nenhum snapshot foi publicado.".into());
        }
        if hits.len() < size || total_relation == "eq" && total.is_some_and(|n| count as u64 >= n) {
            complete = true;
            break;
        }
        let next = hits
            .last()
            .and_then(|hit| hit.get("sort"))
            .filter(|s| s.as_array().is_some_and(|a| !a.is_empty()))
            .ok_or("A paginação não retornou sort/search_after. O snapshot não foi publicado.")?
            .clone();
        if after.as_ref() == Some(&next) {
            return Err("A paginação repetiu o cursor; o snapshot não foi publicado.".into());
        }
        after = Some(next);
    }
    operations::check()?;
    let mut warnings = Vec::new();
    if let Err(error) = pit.close() {
        warnings.push(format!("Snapshot concluído, mas não foi possível fechar o PIT: {error} Ele expira em até 2 minutos sem uso."));
    }
    if collision {
        warnings.push("Metadados receberam sufixo _ quando o registro já tinha um campo _loginsight_remote; nenhum campo original foi substituído.".into());
    }
    if !complete && remote.connection.time_field.is_empty() {
        warnings.push("O limite foi atingido sem campo de horário: o recorte segue a ordem interna dos shards, não os registros mais recentes.".into());
    }
    writer
        .flush()
        .map_err(|e| format!("Não foi possível concluir o snapshot: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Não foi possível sincronizar o snapshot: {e}"))?;
    let bytes = writer
        .get_ref()
        .metadata()
        .map_err(|e| e.to_string())?
        .len();
    drop(writer);
    operations::check()?;
    fs::rename(&pending.path, &path)
        .map_err(|e| format!("Não foi possível publicar o snapshot: {e}"))?;
    pending.published = true;
    operations::commit();
    let order = if remote.connection.time_field.is_empty() {
        "Ordem interna dos shards".into()
    } else {
        format!(
            "{} decrescente; valores sem horário ao final",
            remote.connection.time_field
        )
    };
    Ok(ImportResult {
        path: path.to_string_lossy().into_owned(),
        count,
        total,
        total_relation,
        limited: !complete,
        bytes,
        metadata_field: METADATA_FIELD.into(),
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
        order,
    })
}

#[tauri::command]
pub async fn remote_list() -> Result<RemoteList, String> {
    crate::offload(|| list_impl(&crate::config_dir())).await?
}
#[tauri::command]
pub async fn remote_save(
    connection: RemoteConfig,
    password: Option<String>,
    remember_password: bool,
) -> Result<SavedConnection, String> {
    crate::offload(move || {
        save_impl(
            &crate::config_dir(),
            connection,
            password,
            remember_password,
        )
    })
    .await?
}
#[tauri::command]
pub async fn remote_delete(id: String) -> Result<(), String> {
    crate::offload(move || {
        let _lock = STORE_LOCK.lock();
        database(&crate::config_dir())?
            .execute("DELETE FROM remote_connections WHERE id=?1", [&id])
            .map_err(|e| e.to_string())?;
        SESSION.lock().remove(&id);
        operations::commit();
        Ok(())
    })
    .await?
}
#[tauri::command]
pub async fn remote_test(
    connection: RemoteConfig,
    password: Option<String>,
) -> Result<TestResult, String> {
    crate::offload(move || test_impl(&client_for(&crate::config_dir(), connection, password)?))
        .await?
}
#[tauri::command]
pub async fn remote_import(
    connection: RemoteConfig,
    password: Option<String>,
    from: Option<String>,
    to: Option<String>,
    app: tauri::AppHandle,
) -> Result<ImportResult, String> {
    crate::offload(move || {
        let root = crate::config_dir();
        import_impl(
            &client_for(&root, connection, password)?,
            &root,
            from,
            to,
            |count, total| {
                crate::emit_progress(
                    Some(&app),
                    "remoto",
                    "Recebendo registros",
                    count,
                    total,
                    "registros",
                    true,
                )
            },
        )
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::Arc,
        thread,
        time::Instant,
    };

    #[derive(Debug)]
    struct Request {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Value,
    }
    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Value,
    }
    fn ok(body: Value) -> Reply {
        Reply {
            status: 200,
            headers: vec![],
            body,
        }
    }
    struct Server {
        url: String,
        requests: Arc<Mutex<Vec<Request>>>,
        thread: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let thread = thread::spawn(move || {
                for reply in replies {
                    let deadline = Instant::now() + Duration::from_secs(8);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error)
                                if error.kind() == std::io::ErrorKind::WouldBlock
                                    && Instant::now() < deadline =>
                            {
                                thread::sleep(Duration::from_millis(2))
                            }
                            Err(error) => {
                                panic!("local fixture did not receive expected request: {error}")
                            }
                        }
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut headers = HashMap::new();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        if let Some((key, value)) = line.split_once(':') {
                            headers.insert(key.to_ascii_lowercase(), value.trim().to_owned());
                        }
                    }
                    let length = headers
                        .get("content-length")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let mut bytes = vec![0u8; length];
                    reader.read_exact(&mut bytes).unwrap();
                    let body = if bytes.is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes).unwrap()
                    };
                    let mut parts = first.split_whitespace();
                    captured.lock().push(Request {
                        method: parts.next().unwrap().into(),
                        path: parts.next().unwrap().into(),
                        headers,
                        body,
                    });
                    let bytes = serde_json::to_vec(&reply.body).unwrap();
                    write!(stream,"HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",reply.status,bytes.len()).unwrap();
                    for (key, value) in reply.headers {
                        write!(stream, "{key}: {value}\r\n").unwrap();
                    }
                    stream.write_all(b"\r\n").unwrap();
                    stream.write_all(&bytes).unwrap();
                }
            });
            Self {
                url,
                requests,
                thread: Some(thread),
            }
        }
        fn finish(mut self) -> Vec<Request> {
            self.thread.take().unwrap().join().unwrap();
            std::mem::take(&mut *self.requests.lock())
        }
    }
    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("loginsight-remote-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn config(url: &str) -> RemoteConfig {
        RemoteConfig {
            id: String::new(),
            name: "Local fixture".into(),
            kind: RemoteKind::Elasticsearch,
            url: url.into(),
            index: "logs-*".into(),
            time_field: "@timestamp".into(),
            username: "user".into(),
            max_records: 100_000,
            query: None,
            kibana_version: "auto".into(),
        }
    }
    fn client(url: &str) -> RemoteClient {
        RemoteClient::new(validate(config(url)).unwrap(), Some("pass".into())).unwrap()
    }
    fn page(first: usize, count: usize, total: usize, pit: &str) -> Value {
        json!({"pit_id":pit,"timed_out":false,"_shards":{"failed":0},"hits":{"total":{"value":total,"relation":"eq"},"hits":(first..first+count).map(|i|json!({"_index":"logs-2026","_id":i.to_string(),"sort":[1000000-i,i],"_source":{"@timestamp":"2026-09-22T10:11:12.345Z","message":format!("event {i}"),"_loginsight_remote":{"keep":"original"}}})).collect::<Vec<_>>()}})
    }
    fn snapshot_count(root: &Path) -> usize {
        fs::read_dir(root.join("remote-snapshots")).unwrap().count()
    }

    // One test keeps cancellation-generation changes isolated from other remote cases.
    #[test]
    fn remote_http_contract_and_atomic_snapshots() {
        let root = Folder::new();
        let server = Server::new(vec![
            ok(json!({"id":"pit-1"})),
            ok(page(0, 500, 502, "pit-2")),
            ok(page(500, 2, 502, "pit-3")),
            ok(json!({"succeeded":true})),
        ]);
        let result = import_impl(
            &client(&server.url),
            &root.0,
            Some("2026-09-01T00:00:00Z".into()),
            Some("2026-09-30T23:59:59Z".into()),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(result.count, 502);
        assert_eq!(result.total, Some(502));
        assert!(!result.limited);
        let text = fs::read_to_string(&result.path).unwrap();
        assert_eq!(text.lines().count(), 502);
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["_loginsight_remote"]["keep"], "original");
        assert_eq!(first["_loginsight_remote_"]["_id"], "0");
        let requests = server.finish();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].path, "/logs-*/_pit?keep_alive=2m");
        assert_eq!(requests[3].method, "DELETE");
        assert_eq!(requests[3].body["id"], "pit-3");
        assert_eq!(requests[2].body["search_after"], json!([999501, 499]));
        assert_eq!(requests[2].body["pit"]["id"], "pit-2");
        assert_eq!(requests[1].body["sort"][0]["@timestamp"]["order"], "desc");
        assert_eq!(
            requests[1].body["query"]["bool"]["filter"][1]["range"]["@timestamp"]["gte"],
            "2026-09-01T00:00:00Z"
        );
        for request in &requests {
            assert_eq!(
                request.headers.get("authorization").unwrap(),
                "Basic dXNlcjpwYXNz"
            );
        }
        assert_eq!(snapshot_count(&root.0), 1);

        let limited_root = Folder::new();
        let server = Server::new(vec![
            ok(json!({"id":"p"})),
            ok(page(0, 2, 99, "p2")),
            ok(json!({"succeeded":true})),
        ]);
        let mut remote = client(&server.url);
        remote.connection.max_records = 2;
        remote.connection.time_field.clear();
        let result = import_impl(&remote, &limited_root.0, None, None, |_, _| {}).unwrap();
        assert!(result.limited);
        assert_eq!(result.count, 2);
        assert!(result.warning.unwrap().contains("ordem interna"));
        server.finish();

        let failed_root = Folder::new();
        let mut incomplete = page(500, 1, 600, "p2");
        incomplete["_shards"]["failed"] = json!(1);
        let server = Server::new(vec![
            ok(json!({"id":"p"})),
            ok(page(0, 500, 600, "p1")),
            ok(incomplete),
            ok(json!({"succeeded":true})),
        ]);
        assert!(
            import_impl(&client(&server.url), &failed_root.0, None, None, |_, _| {})
                .err()
                .unwrap()
                .contains("parciais")
        );
        assert_eq!(snapshot_count(&failed_root.0), 0);
        let requests = server.finish();
        assert_eq!(requests.last().unwrap().body["id"], "p2");

        let server = Server::new(vec![Reply {
            status: 401,
            headers: vec![],
            body: json!({"error":"do not echo sensitive details"}),
        }]);
        let error = test_impl(&client(&server.url)).err().unwrap();
        assert!(error.contains("401"));
        assert!(!error.contains("sensitive"));
        server.finish();

        let server = Server::new(vec![Reply {
            status: 200,
            headers: vec![("x-console-proxy-status-code".into(), "403".into())],
            body: json!({"error":"restricted"}),
        }]);
        let mut remote = client(&format!("{}/base/s/team", server.url));
        remote.connection.kind = RemoteKind::Kibana;
        assert!(test_impl(&remote).err().unwrap().contains("403"));
        let requests = server.finish();
        let url = Url::parse(&format!("http://fixture{}", requests[0].path)).unwrap();
        assert_eq!(url.path(), "/base/s/team/api/console/proxy");
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "path").unwrap().1,
            "logs-*/_pit?keep_alive=2m"
        );
        assert_eq!(requests[0].headers.get("kbn-xsrf").unwrap(), "true");
        assert_eq!(
            requests[0].headers.get("x-elastic-product-origin").unwrap(),
            "kibana"
        );

        let server = Server::new(vec![Reply {
            status: 302,
            headers: vec![("Location".into(), "http://127.0.0.1:1/never-request".into())],
            body: json!({}),
        }]);
        assert!(test_impl(&client(&server.url))
            .err()
            .unwrap()
            .contains("redirecionou"));
        server.finish();

        let server = Server::new(vec![
            ok(json!({"id":"test-pit"})),
            ok(json!({"hits":{"hits":[]}})),
            ok(json!({"succeeded":true})),
        ]);
        assert!(test_impl(&client(&server.url)).unwrap().ok);
        server.finish();

        // HTTP 200 alone does not prove the server released its PIT resource.
        let close_root = Folder::new();
        let server = Server::new(vec![
            ok(json!({"id":"unclosed"})),
            ok(page(0, 0, 0, "unclosed")),
            ok(json!({"succeeded":false})),
        ]);
        let result =
            import_impl(&client(&server.url), &close_root.0, None, None, |_, _| {}).unwrap();
        assert!(result.warning.unwrap().contains("não confirmou"));
        assert_eq!(snapshot_count(&close_root.0), 1);
        server.finish();
        assert!(ensure_complete(&json!({"terminated_early":true})).is_err());

        let cancelled_root = Folder::new();
        let server = Server::new(vec![
            ok(json!({"id":"cancel-pit"})),
            ok(page(0, 500, 900, "cancel-latest")),
            ok(json!({"succeeded":true})),
        ]);
        let remote = client(&server.url);
        let generation = operations::generation();
        let result = operations::run(generation, || {
            import_impl(&remote, &cancelled_root.0, None, None, |_, _| {
                operations::cancel()
            })
        });
        assert!(result.err().unwrap().contains("cancelada"));
        assert_eq!(snapshot_count(&cancelled_root.0), 0);
        let requests = server.finish();
        assert_eq!(requests.last().unwrap().body["id"], "cancel-latest");
    }

    #[test]
    fn remote_config_and_secret_persistence() {
        let root = Folder::new();
        let mut c = config("https://elastic.example/base/");
        c.id = uuid::Uuid::new_v4().to_string();
        assert!(validate({
            let mut x = c.clone();
            x.url = "https://user:pass@host".into();
            x
        })
        .is_err());
        assert!(validate({
            let mut x = c.clone();
            x.url = "file:///secret".into();
            x
        })
        .is_err());
        assert!(validate({
            let mut x = c.clone();
            x.index = "logs/_delete_by_query".into();
            x
        })
        .is_err());
        assert!(validate({
            let mut x = c.clone();
            x.query = Some(json!({"query":{"match_all":{}}}));
            x
        })
        .is_err());
        assert!(validate({
            let mut x = c.clone();
            x.max_records = MAX_RECORDS + 1;
            x
        })
        .is_err());
        let saved = save_impl(
            &root.0,
            c.clone(),
            Some("session-fixture-secret".into()),
            false,
        )
        .unwrap();
        assert!(saved.has_password);
        assert!(!saved.password_saved);
        let c = saved.connection;
        assert_eq!(
            password_for(&root.0, &c, None).unwrap().unwrap(),
            "session-fixture-secret"
        );
        let mut other = c.clone();
        other.url = "https://other.example".into();
        assert!(password_for(&root.0, &other, None).unwrap().is_none());
        let bytes = fs::read(root.0.join("remote-connections.sqlite3")).unwrap();
        assert!(!bytes
            .windows(b"session-fixture-secret".len())
            .any(|w| w == b"session-fixture-secret"));
        SESSION.lock().remove(&c.id);
        assert!(!list_impl(&root.0).unwrap().connections[0].has_password);
        #[cfg(windows)]
        {
            let saved = save_impl(
                &root.0,
                c.clone(),
                Some("persisted-fixture-secret".into()),
                true,
            )
            .unwrap();
            assert!(saved.password_saved);
            SESSION.lock().remove(&c.id);
            assert_eq!(
                password_for(&root.0, &c, None).unwrap().unwrap(),
                "persisted-fixture-secret"
            );
            let bytes = fs::read(root.0.join("remote-connections.sqlite3")).unwrap();
            assert!(!bytes
                .windows(b"persisted-fixture-secret".len())
                .any(|w| w == b"persisted-fixture-secret"));
        }
        #[cfg(not(windows))]
        assert!(save_impl(&root.0, c.clone(), Some("pass".into()), true).is_err());
    }
}
