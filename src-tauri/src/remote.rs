//! Read-only Elasticsearch, Kibana and Wazuh snapshots. Configuration contains no
//! plaintext secrets; Windows credentials are protected by user-scoped DPAPI,
//! others are session-only.
use crate::operations;
use parking_lot::Mutex;
use reqwest::{blocking::Client, Method, Url};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

const MAX_RECORDS: usize = 5_000_000;
pub(crate) const BATCH_SIZE: usize = 500;
const RESPONSE_LIMIT: u64 = 32 * 1024 * 1024;
const CA_LIMIT: u64 = 1024 * 1024;
pub(crate) const METADATA_FIELD: &str = "_loginsight_remote";
static STORE_LOCK: Mutex<()> = Mutex::new(());
static SESSION: LazyLock<Mutex<HashMap<String, Secret>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteKind {
    Elasticsearch,
    Kibana,
    Ssh,
    Winrm,
    /// Wazuh indexer (OpenSearch, or Elasticsearch before Wazuh 4.3).
    Wazuh,
    /// Wazuh server API (port 55000), authenticated with a JWT.
    WazuhApi,
    /// Wazuh dashboard (OpenSearch Dashboards): the indexer through its Dev Tools console proxy.
    WazuhWeb,
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
fn default_auth() -> String {
    "basic".into()
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
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub key_path: String,
    #[serde(default = "default_file_limit")]
    pub max_bytes: u64,
    /// "basic" (user and password) or, for Wazuh, "token" (Bearer token in the password slot).
    #[serde(default = "default_auth")]
    pub auth: String,
    /// Local PEM/DER file trusted in addition to the system roots (for example Wazuh root-ca.pem).
    #[serde(default)]
    pub ca_path: String,
    /// Explicit opt-in that disables certificate validation for this endpoint.
    #[serde(default)]
    pub insecure_tls: bool,
}
fn default_file_limit() -> u64 { 512 * 1024 * 1024 }

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
    pub(crate) ok: bool,
    pub(crate) message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub(crate) path: String,
    pub(crate) count: usize,
    pub(crate) total: Option<u64>,
    pub(crate) total_relation: String,
    pub(crate) limited: bool,
    pub(crate) bytes: u64,
    pub(crate) metadata_field: String,
    pub(crate) warning: Option<String>,
    pub(crate) order: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<String>,
}

fn is_wazuh(kind: &RemoteKind) -> bool {
    matches!(kind, RemoteKind::Wazuh | RemoteKind::WazuhApi | RemoteKind::WazuhWeb)
}

fn validate(mut c: RemoteConfig) -> Result<RemoteConfig, String> {
    c.name = c.name.trim().to_owned();
    c.url = c.url.trim().trim_end_matches('/').to_owned();
    c.index = c.index.trim().to_owned();
    c.time_field = c.time_field.trim().to_owned();
    c.auth = c.auth.trim().to_ascii_lowercase();
    c.ca_path = c.ca_path.trim().to_owned();
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
    if matches!(c.kind, RemoteKind::Ssh | RemoteKind::Winrm) {
        c.auth = default_auth();
        c.ca_path.clear();
        c.insecure_tls = false;
        crate::remote_files::validate(&mut c)?;
        return Ok(c);
    }
    if c.auth.is_empty() {
        c.auth = default_auth();
    }
    if c.auth != "basic" && !(is_wazuh(&c.kind) && c.auth == "token") {
        return Err("Autenticação inválida. Use usuário e senha ou, no Wazuh, um token.".into());
    }
    if c.auth == "token" {
        // A token stands alone; a user name would only make the identity ambiguous.
        c.username.clear();
    }
    if c.ca_path.len() > 4096 || c.ca_path.chars().any(char::is_control) {
        return Err("Caminho do certificado CA inválido.".into());
    }
    let mut url = Url::parse(&c.url)
        .map_err(|_| "URL inválida. Use http:// ou https:// e o endereço base do serviço.")?;
    if c.kind == RemoteKind::WazuhWeb {
        // A dashboard address copied from the browser keeps only its base path.
        url.set_fragment(None);
        url.set_query(None);
        if let Some(base) = url.path().find("/app/").map(|i| url.path()[..i].to_owned()) {
            url.set_path(&base);
        }
    }
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
    c.paths.clear(); c.key_path.clear();
    if c.kind == RemoteKind::WazuhApi {
        // The server API serves fixed inventories, not indices, Query DSL or periods.
        crate::remote_wazuh::api_dataset(&c.index)?;
        c.time_field.clear();
        c.query = None;
    } else if c.index.is_empty()
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
    if c.kind == RemoteKind::WazuhApi && c.auth == "basic" && c.username.is_empty() {
        return Err("Informe o usuário da API do servidor Wazuh ou escolha autenticação por token.".into());
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
    match c.kind {
        RemoteKind::Ssh => json!([c.kind, c.url, c.username, c.key_path]).to_string(),
        // The mode is part of the identity: a saved password is never sent as a token.
        RemoteKind::Wazuh | RemoteKind::WazuhApi | RemoteKind::WazuhWeb => json!([c.kind, c.url, c.username, c.auth]).to_string(),
        _ => json!([c.kind, c.url, c.username]).to_string(),
    }
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
    let token = c.auth == "token";
    if let Some(password) = supplied {
        if password.len() > 16_384 {
            return Err("Senha excede o tamanho permitido.".into());
        }
        if token && password.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
            return Err("Token inválido. Cole somente o valor do token, sem espaços ou quebras de linha.".into());
        }
        if c.username.is_empty() && !password.is_empty() && !token {
            return Err("Informe o usuário da conexão.".into());
        }
        return Ok(Some(password));
    }
    if c.username.is_empty() && c.kind != RemoteKind::Winrm && !token {
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
    if c.auth == "token" && password.as_deref().is_none_or(str::is_empty) {
        return Err("Informe o token. O token salvo só pode ser reutilizado com o mesmo endereço e tipo de autenticação.".into());
    }
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
    // The vault holds only access. Collection paths are authored by each Case.
    if matches!(c.kind, RemoteKind::Ssh | RemoteKind::Winrm) { c.paths.clear(); }
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

/// A failed remote request. Statuses stay typed so callers can tolerate an
/// optional permission; everything else becomes a message without echoing the body.
pub(crate) enum Failure {
    Status(u16),
    Connect { timeout: bool },
    NotJson,
    /// HTTP success carrying a failure document.
    Rejected,
    Other(String),
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::Other(message.into())
    }
}
impl Failure {
    pub(crate) fn message(self, c: &RemoteConfig) -> String {
        let text: String = match self {
            Failure::Status(status) => status_message(&c.kind, status).into(),
            Failure::Connect { timeout: true } => "Tempo de conexão esgotado. Verifique endereço e disponibilidade do serviço.".into(),
            Failure::Connect { timeout: false } => {
                let mut text = String::from("Falha de conexão HTTP/TLS. Verifique endereço, rede e certificado do serviço.");
                if is_wazuh(&c.kind) && c.url.starts_with("https:") && c.ca_path.is_empty() && !c.insecure_tls {
                    text.push_str(" Instalações do Wazuh costumam usar certificado próprio: informe o root-ca.pem em Certificado CA.");
                }
                text
            }
            Failure::NotJson => "O serviço não retornou JSON válido. Confirme o endereço da API; páginas de login/SSO não são aceitas.".into(),
            Failure::Rejected if c.kind == RemoteKind::WazuhApi => "A API do servidor Wazuh informou falha total ou parcial. Confira as permissões RBAC do usuário.".into(),
            Failure::Rejected => "O serviço retornou erro na consulta. Confira a Query DSL, o índice e as permissões de leitura.".into(),
            Failure::Other(text) => return text,
        };
        format!("{text}{}", endpoint_hint(c))
    }
}

fn status_message(kind: &RemoteKind, status: u16) -> &'static str {
    match (kind, status) {
        (_, 301..=399) => "O serviço redirecionou a conexão. Use a URL final diretamente; credenciais não são encaminhadas a redirecionamentos.",
        (RemoteKind::Wazuh, 401) => "Autenticação recusada pelo Wazuh indexer (401). Use um usuário do indexer, como o do painel, e não o da API do servidor.",
        (RemoteKind::Wazuh, 403) => "Acesso negado (403). O usuário precisa de leitura nos índices wazuh-* (permissões read e scroll).",
        (RemoteKind::Wazuh, 404) => "Índice ou endpoint não encontrado (404). Confirme a URL do Wazuh indexer e o índice.",
        (RemoteKind::WazuhWeb, 401) => "Autenticação recusada pelo painel Wazuh (401). Use usuário e senha do painel; login SSO do navegador não é reutilizado.",
        (RemoteKind::WazuhWeb, 403) => "Acesso negado (403). A conta do painel precisa ler os índices wazuh-* e usar o console de Dev Tools (permissões read e scroll).",
        (RemoteKind::WazuhWeb, 404) => "Índice ou console não encontrado (404). Use o endereço base do painel Wazuh e confirme o índice; o Dev Tools do painel precisa estar habilitado.",
        (RemoteKind::WazuhWeb, 400) => "O painel Wazuh recusou a consulta (400). Confira o índice, o filtro e se o endereço é o do painel.",
        (RemoteKind::WazuhApi, 401) => "Autenticação recusada pela API do servidor Wazuh (401). Revise usuário e senha ou gere um novo token.",
        (RemoteKind::WazuhApi, 403) => "Acesso negado (403). O usuário da API precisa da permissão RBAC agent:read.",
        (RemoteKind::WazuhApi, 404) => "Endpoint não encontrado (404). Informe a URL da API do servidor Wazuh 4.x ou 5.x, por exemplo https://servidor:55000.",
        (RemoteKind::WazuhApi, 429) => "Limite de requisições da API do servidor Wazuh atingido (429). Aguarde um minuto e tente novamente.",
        (_, 401) => "Autenticação recusada (401). Revise usuário e senha; login SSO não é suportado.",
        (_, 403) => "Acesso negado (403). A conta precisa de permissão de leitura/PIT e, no Kibana, acesso ao Console.",
        (_, 404) => "Endpoint ou índice não encontrado (404). Confirme o índice, Elasticsearch 7.10+ e, no Kibana, URL base/space e Console habilitado.",
        (RemoteKind::WazuhApi, _) => "A API do servidor Wazuh recusou a consulta. Confira a URL e as permissões do usuário.",
        _ => "O serviço recusou a consulta. Confira o índice, Query DSL e permissões de leitura.",
    }
}

/// Default Wazuh ports identify the usual mix-up between indexer, server API and dashboard.
fn endpoint_hint(c: &RemoteConfig) -> &'static str {
    let port = Url::parse(&c.url).ok().and_then(|url| url.port_or_known_default());
    match (&c.kind, port) {
        (RemoteKind::Wazuh, Some(55000)) => " Esse endereço parece ser a API do servidor Wazuh: alertas e eventos ficam no Wazuh indexer (porta 9200). Para agentes, escolha Wazuh · API do servidor.",
        (RemoteKind::Wazuh, Some(443)) => " Use o endereço do Wazuh indexer (geralmente porta 9200), não o do painel web.",
        (RemoteKind::WazuhApi, Some(9200)) => " Esse endereço parece ser o Wazuh indexer: a API do servidor usa a porta 55000. Para alertas, escolha Wazuh · indexer.",
        (RemoteKind::WazuhWeb, Some(9200)) => " Esse endereço parece ser o Wazuh indexer: escolha Wazuh · indexer, ou informe o endereço do painel web.",
        (RemoteKind::WazuhWeb, Some(55000)) => " Esse endereço parece ser a API do servidor Wazuh: informe o endereço do painel web, o mesmo aberto no navegador.",
        (RemoteKind::WazuhApi, Some(443)) => " Use o endereço da API do servidor Wazuh (geralmente porta 55000), não o do painel web.",
        _ => "",
    }
}

fn load_ca(path: &Path) -> Result<Vec<reqwest::Certificate>, String> {
    let file = fs::File::open(path).map_err(|e| format!("Não foi possível abrir o certificado CA: {e}"))?;
    let mut bytes = Vec::new();
    file.take(CA_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Não foi possível ler o certificado CA: {e}"))?;
    if bytes.len() as u64 > CA_LIMIT {
        return Err("O certificado CA excede 1 MiB.".into());
    }
    let certificates = if bytes.windows(11).any(|w| w == b"-----BEGIN ") {
        reqwest::Certificate::from_pem_bundle(&bytes).ok()
    } else {
        reqwest::Certificate::from_der(&bytes).ok().map(|c| vec![c])
    };
    certificates
        .filter(|c| !c.is_empty())
        .ok_or_else(|| "O arquivo informado não contém um certificado CA em PEM ou DER.".into())
}

enum Auth<'a> {
    /// Basic credentials, or the configured token as Bearer.
    Configured,
    Bearer(&'a str),
}

pub(crate) struct RemoteClient {
    client: Client,
    pub(crate) connection: RemoteConfig,
    password: Option<String>,
    /// Wazuh server API session token, renewed once when it expires mid-import.
    session: Mutex<Option<String>>,
}
impl RemoteClient {
    pub(crate) fn new(connection: RemoteConfig, password: Option<String>) -> Result<Self, String> {
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("LogInsight/0.1");
        if !connection.ca_path.is_empty() {
            builder = builder.tls_certs_merge(load_ca(Path::new(&connection.ca_path))?);
        }
        if connection.insecure_tls {
            builder = builder.tls_danger_accept_invalid_certs(true);
        }
        let client = builder
            .build()
            .map_err(|_| "Não foi possível iniciar o cliente HTTP com TLS.")?;
        Ok(Self {
            client,
            connection,
            password,
            session: Mutex::new(None),
        })
    }
    pub(crate) fn request(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&Value>,
        cleanup: bool,
    ) -> Result<Value, String> {
        self.send(method, endpoint, body, cleanup)
            .map_err(|failure| failure.message(&self.connection))
    }
    pub(crate) fn send(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&Value>,
        cleanup: bool,
    ) -> Result<Value, Failure> {
        if self.connection.kind != RemoteKind::WazuhApi || self.connection.auth == "token" {
            return self.send_with(method, endpoint, body, cleanup, Auth::Configured);
        }
        let token = self.wazuh_session(false)?;
        match self.send_with(method.clone(), endpoint, body, cleanup, Auth::Bearer(&token)) {
            // Server API tokens expire (900 s by default): renew once with the same credentials.
            Err(Failure::Status(401)) => {
                let token = self.wazuh_session(true)?;
                self.send_with(method, endpoint, body, cleanup, Auth::Bearer(&token))
            }
            other => other,
        }
    }
    fn wazuh_session(&self, renew: bool) -> Result<String, Failure> {
        let mut session = self.session.lock();
        if let Some(token) = session.as_ref().filter(|_| !renew) {
            return Ok(token.clone());
        }
        // POST since Wazuh 4.4; earlier 4.x releases answer only GET.
        let endpoint = "/security/user/authenticate";
        let data = match self.send_with(Method::POST, endpoint, None, false, Auth::Configured) {
            Err(Failure::Status(405)) => self.send_with(Method::GET, endpoint, None, false, Auth::Configured)?,
            other => other?,
        };
        let token = data
            .pointer("/data/token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or("A API do servidor Wazuh não retornou um token de acesso.")?
            .to_owned();
        *session = Some(token.clone());
        Ok(token)
    }
    fn send_with(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&Value>,
        cleanup: bool,
        auth: Auth,
    ) -> Result<Value, Failure> {
        if !cleanup {
            operations::check()?;
        }
        let mut url = Url::parse(&self.connection.url).map_err(|_| "Endereço inválido.")?;
        let is_kibana = self.connection.kind == RemoteKind::Kibana;
        let is_dashboard = self.connection.kind == RemoteKind::WazuhWeb;
        let actual_method = if is_kibana || is_dashboard {
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
        if is_dashboard {
            // OpenSearch Dashboards (1.x-3.x) requires this header and returns the indexer status as is.
            request = request.header("osd-xsrf", "true");
        }
        match auth {
            Auth::Bearer(token) => request = request.bearer_auth(token),
            Auth::Configured if self.connection.auth == "token" => {
                request = request.bearer_auth(self.password.as_deref().unwrap_or_default())
            }
            Auth::Configured if !self.connection.username.is_empty() => {
                request = request.basic_auth(&self.connection.username, self.password.as_deref())
            }
            Auth::Configured => {}
        }
        if let Some(body) = body {
            request = request.json(body);
        } else if is_kibana || is_dashboard {
            request = request
                .header("Content-Type", "application/json")
                .body("{}");
        }
        // This client owns no source/catalog/store guard or producer child here.
        let response = crate::global_scheduler::unreserved_io(|| request.send())
            .map_err(|e| Failure::Connect { timeout: e.is_timeout() })?;
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
            return Err(Failure::Status(status));
        }
        let mut bytes = Vec::new();
        crate::global_scheduler::unreserved_io(|| response
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut bytes))
            .map_err(|_| "Falha ao ler a resposta do serviço.")?;
        if bytes.len() as u64 > RESPONSE_LIMIT {
            return Err("Uma página excedeu 32 MiB. Restrinja a consulta ou reduza o tamanho dos documentos.".into());
        }
        if !cleanup { operations::check()?; }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::NotJson)?;
        let failed = match self.connection.kind {
            // The server API reports 0 for success, 1 for partial and 2 for complete failure.
            RemoteKind::WazuhApi => data.get("error").is_some_and(|e| e.as_u64() != Some(0)),
            _ => data.get("error").is_some_and(|e| !e.is_null()),
        };
        if failed {
            return Err(Failure::Rejected);
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

pub(crate) fn ensure_complete(data: &Value) -> Result<(), String> {
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

pub(crate) fn query(c: &RemoteConfig, from: Option<&str>, to: Option<&str>) -> Result<Value, String> {
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

/// A local JSONL snapshot: records are written to a pending file that is
/// published atomically, or removed when the import fails or is cancelled.
pub(crate) struct Snapshot {
    // Declared before the guard so the writer closes before cleanup on Windows.
    writer: BufWriter<fs::File>,
    pending: PendingFile,
    path: PathBuf,
    pub(crate) count: usize,
    collision: bool,
}
impl Snapshot {
    pub(crate) fn create(root: &Path) -> Result<Self, String> {
        let directory = root.join("remote-snapshots");
        fs::create_dir_all(&directory)
            .map_err(|e| format!("Não foi possível preparar o snapshot: {e}"))?;
        let id = uuid::Uuid::new_v4();
        let pending = PendingFile {
            path: directory.join(format!("remote-{id}.part")),
            published: false,
        };
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending.path)
            .map_err(|e| format!("Não foi possível criar o snapshot: {e}"))?;
        Ok(Self {
            writer: BufWriter::with_capacity(256 * 1024, file),
            pending,
            path: directory.join(format!("remote-{id}.jsonl")),
            count: 0,
            collision: false,
        })
    }
    /// Writes a search hit: its _source plus the remaining hit fields as metadata.
    pub(crate) fn write_hit(&mut self, hit: &Value, connection: &str) -> Result<(), String> {
        let source = hit.get("_source").and_then(Value::as_object).cloned().ok_or("Um registro não contém _source. Habilite a leitura de _source; nenhum snapshot parcial foi publicado.")?;
        let mut metadata: Map<String, Value> = hit
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| key.as_str() != "_source")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        metadata.insert("connectionName".into(), Value::String(connection.to_owned()));
        self.write(source, metadata)
    }
    /// Never replaces an original field: the metadata key gets a _ suffix instead.
    pub(crate) fn write(&mut self, mut record: Map<String, Value>, metadata: Map<String, Value>) -> Result<(), String> {
        let mut key = METADATA_FIELD.to_owned();
        while record.contains_key(&key) {
            key.push('_');
            self.collision = true;
        }
        record.insert(key, Value::Object(metadata));
        serde_json::to_writer(&mut self.writer, &record)
            .map_err(|e| format!("Não foi possível gravar o snapshot: {e}"))?;
        self.writer
            .write_all(b"\n")
            .map_err(|e| format!("Não foi possível gravar o snapshot: {e}"))?;
        self.count += 1;
        Ok(())
    }
    pub(crate) fn collision_warning(&self) -> Option<String> {
        self.collision.then(|| "Metadados receberam sufixo _ quando o registro já tinha um campo _loginsight_remote; nenhum campo original foi substituído.".into())
    }
    /// Returns the published path and its size in bytes.
    pub(crate) fn publish(self) -> Result<(String, u64), String> {
        let Snapshot { mut writer, mut pending, path, .. } = self;
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
        Ok((path.to_string_lossy().into_owned(), bytes))
    }
}

/// hits.total of a first search page: the count and "eq", "gte" or "unknown".
pub(crate) fn hits_total(data: &Value) -> (Option<u64>, String) {
    let hits_total = data.pointer("/hits/total");
    let total = hits_total.and_then(|v| {
        v.as_u64()
            .or_else(|| v.get("value").and_then(Value::as_u64))
    });
    let relation = if hits_total.is_some_and(Value::is_u64) {
        "eq"
    } else {
        hits_total
            .and_then(|v| v.get("relation"))
            .and_then(Value::as_str)
            .filter(|r| matches!(*r, "eq" | "gte"))
            .unwrap_or("unknown")
    };
    (total, relation.into())
}

fn import_impl(
    remote: &RemoteClient,
    root: &Path,
    from: Option<String>,
    to: Option<String>,
    mut progress: impl FnMut(usize, usize),
) -> Result<ImportResult, String> {
    let query = query(&remote.connection, from.as_deref(), to.as_deref())?;
    let mut snapshot = Snapshot::create(root)?;
    let mut pit = PointInTime::open(remote)?;
    let mut after: Option<Value> = None;
    let mut total = None;
    let mut total_relation = "unknown".to_owned();
    let mut complete = false;
    let sort = if remote.connection.time_field.is_empty() {
        json!([{"_shard_doc":"asc"}])
    } else {
        json!([{remote.connection.time_field.clone(): {"order":"desc","unmapped_type":"date","missing":"_last"}},{"_shard_doc":"asc"}])
    };
    while snapshot.count < remote.connection.max_records {
        operations::check()?;
        let size = BATCH_SIZE.min(remote.connection.max_records - snapshot.count);
        let mut body = json!({"size":size,"track_total_hits":after.is_none(),"pit":{"id":pit.id,"keep_alive":"2m"},"sort":sort,"query":query,"_source":true});
        if let Some(value) = after.as_ref() {
            body["search_after"] = value.clone();
        }
        let data = remote.request(Method::POST, "/_search", Some(&body), false)?;
        pit.update(&data);
        ensure_complete(&data)?;
        operations::check()?;
        if after.is_none() {
            (total, total_relation) = hits_total(&data);
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
            snapshot.write_hit(hit, &remote.connection.name)?;
        }
        let count = snapshot.count;
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
    warnings.extend(snapshot.collision_warning());
    if !complete && remote.connection.time_field.is_empty() {
        warnings.push("O limite foi atingido sem campo de horário: o recorte segue a ordem interna dos shards, não os registros mais recentes.".into());
    }
    let count = snapshot.count;
    let (path, bytes) = snapshot.publish()?;
    let order = if remote.connection.time_field.is_empty() {
        "Ordem interna dos shards".into()
    } else {
        format!(
            "{} decrescente; valores sem horário ao final",
            remote.connection.time_field
        )
    };
    Ok(ImportResult {
        path,
        count,
        total,
        total_relation,
        limited: !complete,
        bytes,
        metadata_field: METADATA_FIELD.into(),
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
        order,
        paths: None,
        format: None,
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
    operation_id: Option<String>,
) -> Result<TestResult, String> {
    crate::offload_operation(operation_id, move || {
        let connection = validate(connection)?;
        match connection.kind {
            RemoteKind::Ssh | RemoteKind::Winrm => {
                let password = password_for(&crate::config_dir(), &connection, password)?;
                crate::remote_files::test(&connection, password)?;
                Ok(TestResult { ok: true, message: "Acesso confirmado aos arquivos selecionados.".into() })
            }
            RemoteKind::Wazuh | RemoteKind::WazuhWeb => crate::remote_wazuh::indexer_test(&client_for(&crate::config_dir(), connection, password)?),
            RemoteKind::WazuhApi => crate::remote_wazuh::api_test(&client_for(&crate::config_dir(), connection, password)?),
            RemoteKind::Elasticsearch | RemoteKind::Kibana => test_impl(&client_for(&crate::config_dir(), connection, password)?),
        }
    }).await?
}
#[tauri::command]
pub async fn remote_import(
    connection: RemoteConfig,
    password: Option<String>,
    from: Option<String>,
    to: Option<String>,
    app: tauri::AppHandle,
    operation_id: Option<String>,
) -> Result<ImportResult, String> {
    crate::offload_operation(operation_id, move || {
        let root = crate::config_dir();
        let connection = validate(connection)?;
        if matches!(connection.kind, RemoteKind::Ssh | RemoteKind::Winrm) {
            let password = password_for(&root, &connection, password)?;
            let result = crate::remote_files::import(&connection, password, &root, &app)?;
            return Ok(ImportResult { path: result.paths[0].clone(), count: result.paths.len(),
                total: None, total_relation: "eq".into(), limited: false, bytes: result.bytes,
                metadata_field: String::new(), warning: result.warning, order: "arquivos".into(),
                paths: Some(result.paths), format: Some("auto".into()) });
        }
        let progress = |count, total| {
            crate::emit_progress(
                Some(&app),
                "remoto",
                "Recebendo registros",
                count,
                total,
                "registros",
                true,
            )
        };
        let remote = client_for(&root, connection, password)?;
        match remote.connection.kind {
            RemoteKind::Wazuh | RemoteKind::WazuhWeb => crate::remote_wazuh::indexer_import(&remote, &root, from, to, progress),
            RemoteKind::WazuhApi => {
                if from.is_some() || to.is_some() {
                    return Err("A API do servidor Wazuh não filtra por período. Limpe De e Até, ou use Wazuh · indexer para alertas e eventos.".into());
                }
                crate::remote_wazuh::api_import(&remote, &root, progress)
            }
            _ => import_impl(&remote, &root, from, to, progress),
        }
    })
    .await?
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::Arc,
        thread,
        time::Instant,
    };

    #[derive(Debug)]
    pub(crate) struct Request {
        pub(crate) method: String,
        pub(crate) path: String,
        pub(crate) headers: HashMap<String, String>,
        pub(crate) body: Value,
    }
    pub(crate) struct Reply {
        pub(crate) status: u16,
        pub(crate) headers: Vec<(String, String)>,
        pub(crate) body: Value,
    }
    pub(crate) fn ok(body: Value) -> Reply {
        Reply {
            status: 200,
            headers: vec![],
            body,
        }
    }
    pub(crate) struct Server {
        pub(crate) url: String,
        requests: Arc<Mutex<Vec<Request>>>,
        thread: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        pub(crate) fn new(replies: Vec<Reply>) -> Self {
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
        pub(crate) fn finish(mut self) -> Vec<Request> {
            self.thread.take().unwrap().join().unwrap();
            std::mem::take(&mut *self.requests.lock())
        }
    }
    pub(crate) struct Folder(pub(crate) PathBuf);
    impl Folder {
        pub(crate) fn new() -> Self {
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
    pub(crate) fn config(url: &str) -> RemoteConfig {
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
            paths: vec![], key_path: String::new(), max_bytes: default_file_limit(),
            auth: default_auth(), ca_path: String::new(), insecure_tls: false,
        }
    }
    fn client(url: &str) -> RemoteClient {
        RemoteClient::new(validate(config(url)).unwrap(), Some("pass".into())).unwrap()
    }
    fn page(first: usize, count: usize, total: usize, pit: &str) -> Value {
        json!({"pit_id":pit,"timed_out":false,"_shards":{"failed":0},"hits":{"total":{"value":total,"relation":"eq"},"hits":(first..first+count).map(|i|json!({"_index":"logs-2026","_id":i.to_string(),"sort":[1000000-i,i],"_source":{"@timestamp":"2026-09-22T10:11:12.345Z","message":format!("event {i}"),"_loginsight_remote":{"keep":"original"}}})).collect::<Vec<_>>()}})
    }
    pub(crate) fn snapshot_count(root: &Path) -> usize {
        fs::read_dir(root.join("remote-snapshots")).unwrap().count()
    }

    #[test]
    fn single_slot_page_progresses_while_remote_http_waits_for_body() {
        use crate::global_scheduler::{Scheduler, Priority, with_scheduler};
        let scheduler = Scheduler::new(1);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote = client(&format!("http://{}", listener.local_addr().unwrap()));
        let (received, waiting) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            loop { let mut line = String::new(); reader.read_line(&mut line).unwrap(); if line == "\r\n" { break; } }
            // Send headers first, then keep read_to_end blocked independently
            // of the connection establishment / response-header wait.
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n").unwrap();
            socket.flush().unwrap(); received.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            socket.write_all(b"{\"ok\":true}").unwrap();
        });
        let other = Arc::clone(&scheduler);
        let importer = thread::spawn(move || with_scheduler(other, || {
            let token = operations::token(None).unwrap();
            operations::run_with_token(token, || remote.request(Method::GET, "/", None, false)).unwrap().unwrap()
        }));
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let (page_done, page_result) = std::sync::mpsc::channel();
        let other = Arc::clone(&scheduler);
        let page = thread::spawn(move || with_scheduler(other, || {
            let token = operations::token(None).unwrap().with_priority(Priority::Interactive);
            let value = operations::run_with_token(token, || {
                let conn = duckdb::Connection::open_in_memory().unwrap(); conn.execute_batch("SET threads=1").unwrap();
                conn.query_row("SELECT 42", [], |row| row.get::<_, i64>(0)).unwrap()
            }).unwrap();
            page_done.send(value).unwrap();
        }));
        let value = page_result.recv_timeout(Duration::from_secs(3));
        // Always unblock the server before asserting, including a regression.
        release.send(()).unwrap(); server.join().unwrap();
        assert_eq!(value.unwrap(), 42);
        assert_eq!(importer.join().unwrap(), json!({"ok":true})); page.join().unwrap();
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

    // Throwaway self-signed root (private key discarded) for the CA loader.
    const TEST_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBlDCCATugAwIBAgIUKbIBJqb4c90sdxTBr/M9gfzOG88wCgYIKoZIzj0EAwIw
HzEdMBsGA1UEAwwUTG9nSW5zaWdodCB0ZXN0IHJvb3QwIBcNMjYxMDA5MTcwMDA4
WhgPMjEyNjA5MTUxNzAwMDhaMB8xHTAbBgNVBAMMFExvZ0luc2lnaHQgdGVzdCBy
b290MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAECuNuaqeyCN4kJFo0NapdkzzU
wvj/pQ7uw+ZHAsKl6u7tgHFMCcDhzzBSx7H0Uq/nPEbrpB03xu571NWKh0uvCqNT
MFEwHQYDVR0OBBYEFPAFL2JH6NdxWObxIJUO6NU9SuXzMB8GA1UdIwQYMBaAFPAF
L2JH6NdxWObxIJUO6NU9SuXzMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwID
RwAwRAIgelxooj6qUaGJyJSYzATRlMN/jqaffohYAAq9nEjDjC4CIBPrOM6IQ+iX
t7Oc7pH7LwIiUBsysbgB6p6AdyM/36XG
-----END CERTIFICATE-----
";

    #[test]
    fn wazuh_config_auth_modes_tls_and_secret_identity() {
        let root = Folder::new();
        let mut c = config("https://wazuh.example:9200");
        c.kind = RemoteKind::Wazuh;
        c.index = "wazuh-alerts-*".into();
        c.id = uuid::Uuid::new_v4().to_string();
        assert!(validate(c.clone()).is_ok());
        // Token mode is Wazuh-only and never keeps a user name.
        let mut elastic_token = config("https://elastic.example");
        elastic_token.auth = "token".into();
        assert!(validate(elastic_token).is_err());
        let mut token = c.clone();
        token.auth = " Token ".into();
        let token = validate(token).unwrap();
        assert_eq!((token.auth.as_str(), token.username.as_str()), ("token", ""));
        assert!(password_for(&root.0, &token, Some("abc def".into())).is_err());
        // The server API takes a fixed dataset, never indices, Query DSL or periods.
        let mut api = c.clone();
        api.kind = RemoteKind::WazuhApi;
        assert!(validate(api.clone()).is_err());
        api.index = "agents".into();
        api.query = Some(json!({"match_all":{}}));
        let api = validate(api).unwrap();
        assert!(api.query.is_none() && api.time_field.is_empty());
        let mut anonymous = api.clone();
        anonymous.username.clear();
        assert!(validate(anonymous).err().unwrap().contains("token"));
        // A dashboard address copied from the browser keeps only its base path.
        let mut web = c.clone();
        web.kind = RemoteKind::WazuhWeb;
        web.url = "https://wazuh.example/app/wz-home#/overview/?tab=general".into();
        assert_eq!(validate(web.clone()).unwrap().url, "https://wazuh.example");
        web.url = "https://proxy.example/wazuh/app/discover?_g=()".into();
        assert_eq!(validate(web).unwrap().url, "https://proxy.example/wazuh");
        let mut ssh = c.clone();
        ssh.kind = RemoteKind::Ssh;
        ssh.url = "ssh://linux.example:22".into();
        ssh.paths = vec!["/var/log/auth.log".into()];
        ssh.ca_path = "C:/ca.pem".into();
        ssh.insecure_tls = true;
        let ssh = validate(ssh).unwrap();
        assert!(ssh.ca_path.is_empty() && !ssh.insecure_tls);

        // A password saved for Basic is never sent as a token to the same endpoint.
        let saved = save_impl(&root.0, c.clone(), Some("basic-secret".into()), false).unwrap();
        let mut as_token = saved.connection.clone();
        as_token.auth = "token".into();
        let as_token = validate(as_token).unwrap();
        assert!(password_for(&root.0, &as_token, None).unwrap().is_none());
        assert!(client_for(&root.0, as_token, None).err().unwrap().contains("token"));
        assert_eq!(password_for(&root.0, &saved.connection, None).unwrap().unwrap(), "basic-secret");
        SESSION.lock().remove(&saved.connection.id);

        // CA files must hold a certificate; PEM bundles and DER are accepted.
        let pem = root.0.join("root-ca.pem");
        fs::write(&pem, TEST_CA).unwrap();
        assert_eq!(load_ca(&pem).unwrap().len(), 1);
        let mut with_ca = c.clone();
        with_ca.ca_path = pem.to_string_lossy().into_owned();
        assert!(RemoteClient::new(with_ca, None).is_ok());
        let bad = root.0.join("bad.pem");
        fs::write(&bad, "not a certificate").unwrap();
        assert!(load_ca(&bad).err().unwrap().contains("PEM ou DER"));
        assert!(load_ca(&root.0.join("missing.pem")).err().unwrap().contains("abrir"));
        let mut insecure = c.clone();
        insecure.insecure_tls = true;
        assert!(RemoteClient::new(insecure, None).is_ok());
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
