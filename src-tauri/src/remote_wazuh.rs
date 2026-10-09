//! Wazuh sources. Alerts and events are read from the Wazuh indexer, directly or
//! through the dashboard's Dev Tools console proxy, with the
//! scroll API, which OpenSearch 1.x-3.x (Wazuh 4.3-5.x) and Elasticsearch 7+
//! (Wazuh 4.0-4.2 with Elastic Stack) all support, unlike their PIT APIs.
//! The agent inventory comes from the Wazuh server API (JWT), whose /agents
//! endpoint is stable from 4.0 to 5.0; /manager/logs changed in 4.3 and is gone in 5.0.
use crate::operations;
use crate::remote::{
    ensure_complete, hits_total, query, Failure, ImportResult, RemoteClient, RemoteConfig,
    Snapshot, TestResult, BATCH_SIZE, METADATA_FIELD,
};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::{collections::HashSet, path::Path};

const KEEP_ALIVE: &str = "2m";

/// Index families written by Wazuh 4.x and 5.x, probed by the access test.
struct Family {
    pattern: &'static str,
    noun: &'static str,
    /// Generation revealed by data in the family; vulnerabilities exist in both.
    generation: Option<&'static str>,
}
const FAMILIES: &[Family] = &[
    Family { pattern: "wazuh-alerts-*", noun: "alertas", generation: Some("Wazuh 4.x") },
    Family { pattern: "wazuh-archives-*", noun: "eventos arquivados", generation: Some("Wazuh 4.x") },
    Family { pattern: "wazuh-findings-v5*", noun: "achados", generation: Some("Wazuh 5.x") },
    Family { pattern: "wazuh-events-v5*", noun: "eventos", generation: Some("Wazuh 5.x") },
    Family { pattern: "wazuh-states-vulnerabilities*", noun: "vulnerabilidades", generation: None },
];

fn family_of(index: &str) -> Option<&'static Family> {
    FAMILIES
        .iter()
        .find(|family| index.starts_with(family.pattern.trim_end_matches('*')))
}

pub(crate) struct ApiDataset {
    pub(crate) id: &'static str,
    endpoint: &'static str,
    /// URL-encoded: "+" and "-" select the Wazuh API sort direction.
    sort: &'static str,
    noun: &'static str,
    order: &'static str,
    /// Field identifying an item across pages, so shifted offsets never duplicate it.
    key: Option<&'static str>,
    page: usize,
}
pub(crate) const API_DATASETS: &[ApiDataset] = &[
    // Wazuh 4.0-4.2 accept at most 500 items per page.
    ApiDataset { id: "agents", endpoint: "/agents", sort: "%2Bid", noun: "agentes", order: "ID do agente crescente", key: Some("id"), page: 500 },
];

pub(crate) fn api_dataset(id: &str) -> Result<&'static ApiDataset, String> {
    API_DATASETS
        .iter()
        .find(|dataset| dataset.id == id)
        .ok_or_else(|| "Escolha os dados da API do servidor Wazuh: agentes.".into())
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    out
}

fn insecure_note(c: &RemoteConfig) -> &'static str {
    if c.insecure_tls { " Atenção: o certificado do servidor não foi validado." } else { "" }
}

// ------------------------------------------------------------------ indexer

struct Scroll<'a> {
    remote: &'a RemoteClient,
    id: Option<String>,
}
impl<'a> Scroll<'a> {
    fn start(remote: &'a RemoteClient, body: &Value) -> Result<(Self, Value), String> {
        let data = remote.request(
            Method::POST,
            &format!("/{}/_search?scroll={KEEP_ALIVE}", remote.connection.index),
            Some(body),
            false,
        )?;
        let mut scroll = Self { remote, id: None };
        scroll.update(&data);
        if scroll.id.is_none() {
            return Err("O Wazuh indexer não retornou um cursor de scroll; nenhum snapshot foi publicado.".into());
        }
        Ok((scroll, data))
    }
    fn update(&mut self, data: &Value) {
        if let Some(id) = data
            .get("_scroll_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.id = Some(id.to_owned());
        }
    }
    fn next(&mut self) -> Result<Value, String> {
        let id = self.id.clone().ok_or("Cursor de scroll indisponível.")?;
        let data = self.remote.request(
            Method::POST,
            "/_search/scroll",
            Some(&json!({"scroll": KEEP_ALIVE, "scroll_id": id})),
            false,
        )?;
        self.update(&data);
        Ok(data)
    }
    fn close(&mut self) -> Result<(), String> {
        let Some(id) = self.id.take() else {
            return Ok(());
        };
        let response = self.remote.request(
            Method::DELETE,
            "/_search/scroll",
            Some(&json!({"scroll_id": [id]})),
            true,
        )?;
        if response.get("succeeded").and_then(Value::as_bool) != Some(true) {
            return Err("O Wazuh indexer não confirmou o fechamento do scroll.".into());
        }
        Ok(())
    }
}
impl Drop for Scroll<'_> {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn sort(c: &RemoteConfig) -> Value {
    if c.time_field.is_empty() {
        json!(["_doc"])
    } else {
        json!([{c.time_field.clone(): {"order":"desc","unmapped_type":"date","missing":"_last"}}])
    }
}

/// "OpenSearch 2.19.5", "Elasticsearch 7.17.0", or a note when the role cannot read it.
fn engine(remote: &RemoteClient) -> Result<String, String> {
    let info = match remote.send(Method::GET, "/", None, false) {
        Ok(info) => info,
        // The root needs cluster:monitor/main, which read-only roles may lack.
        Err(Failure::Status(403)) => return Ok("versão não informada a este usuário".into()),
        Err(failure) => return Err(failure.message(&remote.connection)),
    };
    let number = info
        .pointer("/version/number")
        .and_then(Value::as_str)
        .ok_or("O endereço não respondeu como um Wazuh indexer (OpenSearch ou Elasticsearch). Informe a URL do indexer, geralmente na porta 9200.")?;
    let opensearch = info.pointer("/version/distribution").and_then(Value::as_str) == Some("opensearch")
        || info
            .get("tagline")
            .and_then(Value::as_str)
            .is_some_and(|tagline| tagline.contains("OpenSearch"));
    if opensearch {
        // Wazuh 4.x reports 7.10.2 at the root for Filebeat; nodes report the real version.
        let real = remote
            .send(Method::GET, "/_nodes/_local?filter_path=nodes.*.version", None, false)
            .ok()
            .and_then(|data| {
                data.get("nodes")?
                    .as_object()?
                    .values()
                    .find_map(|node| node.get("version")?.as_str().map(str::to_owned))
            });
        operations::check()?;
        return Ok(match real {
            Some(version) => format!("OpenSearch {version}"),
            None if number == "7.10.2" => "OpenSearch, versão mascarada como 7.10.2".into(),
            None => format!("OpenSearch {number}"),
        });
    }
    let major = number
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(0);
    if major < 7 {
        return Err(format!("Elasticsearch {number} não é suportado. Use o Wazuh indexer (Wazuh 4.3 ou superior) ou Elasticsearch 7 ou superior."));
    }
    Ok(format!("Elasticsearch {number}"))
}

enum Presence {
    Records(u64),
    Denied,
}

/// Other Wazuh families visible to this user; failures other than 403 are skipped.
fn probe(remote: &RemoteClient) -> Result<Vec<(&'static Family, Presence)>, String> {
    let selected = family_of(&remote.connection.index).map(|family| family.pattern);
    let mut found = Vec::new();
    for family in FAMILIES.iter().filter(|family| Some(family.pattern) != selected) {
        operations::check()?;
        let body = json!({"size": 0, "track_total_hits": true, "query": {"match_all": {}}});
        match remote.send(Method::POST, &format!("/{}/_search", family.pattern), Some(&body), false) {
            Ok(data) => {
                if let (Some(n @ 1..), _) = hits_total(&data) {
                    found.push((family, Presence::Records(n)));
                }
            }
            Err(Failure::Status(403)) => found.push((family, Presence::Denied)),
            Err(_) => {}
        }
    }
    operations::check()?;
    Ok(found)
}

pub(crate) fn indexer_test(remote: &RemoteClient) -> Result<TestResult, String> {
    let c = &remote.connection;
    let engine = engine(remote)?;
    // The same read path as the import, including the scroll permissions.
    let body = json!({"size": 1, "track_total_hits": true, "sort": sort(c), "query": query(c, None, None)?, "_source": true});
    let (mut scroll, data) = Scroll::start(remote, &body)?;
    ensure_complete(&data)?;
    if data.pointer("/hits/hits").and_then(Value::as_array).is_none() {
        return Err("A resposta de busca não é compatível com o Wazuh indexer.".into());
    }
    let (total, relation) = hits_total(&data);
    operations::check()?;
    scroll.close()?;
    let found = probe(remote)?;

    let via = if c.kind == crate::remote::RemoteKind::WazuhWeb { " pelo painel web" } else { "" };
    let mut message = format!("Wazuh indexer verificado{via} ({engine}). ");
    let mut generations: Vec<&str> = Vec::new();
    match total {
        Some(0) => message.push_str(&format!("Nenhum registro em {} para este usuário.", c.index)),
        Some(n) => {
            let prefix = if relation == "gte" { "mais de " } else { "" };
            message.push_str(&format!("{}: {prefix}{} registros, leitura por scroll confirmada.", c.index, thousands(n)));
            generations.extend(family_of(&c.index).and_then(|family| family.generation));
        }
        None => message.push_str(&format!("Leitura de {} por scroll confirmada.", c.index)),
    }
    let available: Vec<String> = found
        .iter()
        .filter_map(|(family, presence)| match presence {
            Presence::Records(n) => {
                generations.extend(family.generation);
                Some(format!("{} em {} ({})", family.noun, family.pattern, thousands(*n)))
            }
            Presence::Denied => None,
        })
        .collect();
    let denied: Vec<&str> = found
        .iter()
        .filter_map(|(family, presence)| matches!(presence, Presence::Denied).then_some(family.pattern))
        .collect();
    if !available.is_empty() {
        message.push_str(&format!(" Também disponíveis: {}.", available.join("; ")));
    }
    if !denied.is_empty() {
        message.push_str(&format!(" Sem permissão de leitura: {}.", denied.join(", ")));
    }
    generations.sort_unstable();
    generations.dedup();
    if !generations.is_empty() {
        message.push_str(&format!(" Índices de {}.", generations.join(" e ")));
    }
    if total == Some(0) {
        message.push_str(if available.is_empty() {
            " Nenhum índice Wazuh com registros foi encontrado: confirme a URL do indexer, a ingestão de alertas e a leitura em wazuh-*."
        } else {
            " Escolha um dos conjuntos disponíveis em Dados do Wazuh."
        });
    }
    message.push_str(insecure_note(c));
    Ok(TestResult { ok: true, message })
}

pub(crate) fn indexer_import(
    remote: &RemoteClient,
    root: &Path,
    from: Option<String>,
    to: Option<String>,
    mut progress: impl FnMut(usize, usize),
) -> Result<ImportResult, String> {
    let c = &remote.connection;
    let query = query(c, from.as_deref(), to.as_deref())?;
    let mut snapshot = Snapshot::create(root)?;
    let size = BATCH_SIZE.min(c.max_records);
    let body = json!({"size": size, "track_total_hits": true, "sort": sort(c), "query": query, "_source": true});
    let (mut scroll, mut data) = Scroll::start(remote, &body)?;
    let (total, total_relation) = hits_total(&data);
    let exact = total.filter(|_| total_relation == "eq");
    let mut complete = false;
    loop {
        ensure_complete(&data)?;
        operations::check()?;
        let hits = data
            .pointer("/hits/hits")
            .and_then(Value::as_array)
            .ok_or("A resposta não contém uma lista de registros do Wazuh indexer.")?;
        if hits.len() > size {
            return Err("O serviço ignorou o limite de paginação; a importação foi interrompida.".into());
        }
        for hit in hits.iter().take(c.max_records - snapshot.count) {
            operations::check()?;
            snapshot.write_hit(hit, &c.name)?;
        }
        let count = snapshot.count as u64;
        progress(
            snapshot.count,
            total
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(c.max_records),
        );
        if exact.is_some_and(|n| count > n) || hits.is_empty() && exact.is_some_and(|n| count < n) {
            return Err("A paginação retornou uma quantidade inconsistente de registros; nenhum snapshot foi publicado.".into());
        }
        if hits.is_empty() || exact.is_some_and(|n| count >= n) {
            complete = true;
            break;
        }
        if snapshot.count >= c.max_records {
            break;
        }
        data = scroll.next()?;
    }
    operations::check()?;
    let mut warnings = Vec::new();
    if let Err(error) = scroll.close() {
        warnings.push(format!("Snapshot concluído, mas não foi possível fechar o scroll: {error} Ele expira em até 2 minutos sem uso."));
    }
    warnings.extend(snapshot.collision_warning());
    if !complete && c.time_field.is_empty() {
        warnings.push("O limite foi atingido sem campo de horário: o recorte segue a ordem interna dos índices, não os registros mais recentes.".into());
    }
    let count = snapshot.count;
    let (path, bytes) = snapshot.publish()?;
    let order = if c.time_field.is_empty() {
        "Ordem interna dos índices".into()
    } else {
        format!("{} decrescente; valores sem horário ao final", c.time_field)
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

// --------------------------------------------------------------- server API

/// Version reported by the server API, accepted from Wazuh 4.0 (the first JWT API).
fn api_version(remote: &RemoteClient) -> Result<(String, u32), String> {
    let info = remote.request(Method::GET, "/", None, false)?;
    let version = info
        .pointer("/data/api_version")
        .and_then(Value::as_str)
        .map(|version| version.trim().trim_start_matches('v'))
        .filter(|version| !version.is_empty())
        .ok_or("O serviço não respondeu como a API do servidor Wazuh. Informe a URL da API, por exemplo https://servidor:55000.")?;
    let major = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .ok_or("Versão da API do servidor Wazuh não reconhecida.")?;
    if major < 4 {
        return Err(format!("API do servidor Wazuh {version} não suportada. É necessário Wazuh 4.0 ou superior."));
    }
    Ok((version.to_owned(), major))
}

fn api_page(remote: &RemoteClient, dataset: &ApiDataset, offset: usize, limit: usize) -> Result<(Vec<Value>, Option<u64>), String> {
    let data = remote.request(
        Method::GET,
        &format!("{}?offset={offset}&limit={limit}&sort={}", dataset.endpoint, dataset.sort),
        None,
        false,
    )?;
    let items = data
        .pointer("/data/affected_items")
        .and_then(Value::as_array)
        .cloned()
        .ok_or("A resposta da API do servidor Wazuh não contém affected_items.")?;
    if items.len() > limit {
        return Err("A API do servidor Wazuh ignorou o limite de paginação; a importação foi interrompida.".into());
    }
    Ok((items, data.pointer("/data/total_affected_items").and_then(Value::as_u64)))
}

pub(crate) fn api_test(remote: &RemoteClient) -> Result<TestResult, String> {
    let c = &remote.connection;
    let dataset = api_dataset(&c.index)?;
    let (version, major) = api_version(remote)?;
    let (_, total) = api_page(remote, dataset, 0, 1)?;
    let session = if c.auth == "token" { "com o token informado" } else { "com sessão JWT" };
    let mut message = format!("API do servidor Wazuh {version} verificada {session}.");
    if let Some(total) = total {
        message.push_str(&format!(" {} {} disponíveis para importar.", thousands(total), dataset.noun));
    }
    if major > 5 {
        message.push_str(" Esta versão é mais nova que as validadas (4.x e 5.x); confira o resultado.");
    }
    message.push_str(insecure_note(c));
    Ok(TestResult { ok: true, message })
}

pub(crate) fn api_import(
    remote: &RemoteClient,
    root: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<ImportResult, String> {
    let c = &remote.connection;
    let dataset = api_dataset(&c.index)?;
    let (version, _) = api_version(remote)?;
    let mut snapshot = Snapshot::create(root)?;
    let mut seen = HashSet::new();
    let (mut offset, mut total, mut complete, mut repeated) = (0usize, None, false, 0usize);
    while snapshot.count < c.max_records {
        operations::check()?;
        let limit = dataset.page.min(c.max_records - snapshot.count);
        let (items, reported) = api_page(remote, dataset, offset, limit)?;
        total = reported;
        for (position, item) in items.iter().enumerate() {
            operations::check()?;
            let record = item
                .as_object()
                .cloned()
                .ok_or("Item inválido na resposta da API do servidor Wazuh.")?;
            // Offsets shift when agents enrol during the import; each one is written once.
            if let Some(id) = dataset.key.and_then(|key| record.get(key)) {
                if !seen.insert(id.to_string()) {
                    repeated += 1;
                    continue;
                }
            }
            let mut metadata = Map::new();
            metadata.insert("wazuhDataset".into(), json!(dataset.id));
            metadata.insert("endpoint".into(), json!(dataset.endpoint));
            metadata.insert("offset".into(), json!(offset + position));
            metadata.insert("apiVersion".into(), json!(version));
            metadata.insert("connectionName".into(), json!(c.name));
            snapshot.write(record, metadata)?;
        }
        offset += items.len();
        progress(
            snapshot.count,
            total
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(c.max_records),
        );
        if items.is_empty() || total.is_some_and(|n| offset as u64 >= n) {
            complete = true;
            break;
        }
    }
    operations::check()?;
    let mut warnings: Vec<String> = snapshot.collision_warning().into_iter().collect();
    if repeated > 0 {
        warnings.push(format!("{repeated} itens repetidos pela paginação foram ignorados: a lista mudou durante a leitura."));
    }
    let count = snapshot.count;
    let (path, bytes) = snapshot.publish()?;
    Ok(ImportResult {
        path,
        count,
        total,
        total_relation: if total.is_some() { "eq" } else { "unknown" }.into(),
        limited: !complete,
        bytes,
        metadata_field: METADATA_FIELD.into(),
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
        order: dataset.order.into(),
        paths: None,
        format: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::tests::{config, ok, snapshot_count, Folder, Reply, Server};
    use crate::remote::RemoteKind;
    use std::fs;

    fn indexer(url: &str) -> RemoteClient {
        let mut c = config(url);
        c.kind = RemoteKind::Wazuh;
        c.index = "wazuh-alerts-*".into();
        c.time_field = "timestamp".into();
        RemoteClient::new(c, Some("pass".into())).unwrap()
    }
    fn api(url: &str, index: &str) -> RemoteClient {
        let mut c = config(url);
        c.kind = RemoteKind::WazuhApi;
        c.index = index.into();
        c.time_field.clear();
        RemoteClient::new(c, Some("pass".into())).unwrap()
    }
    fn api_token(url: &str, index: &str, token: &str) -> RemoteClient {
        let mut c = config(url);
        c.kind = RemoteKind::WazuhApi;
        c.index = index.into();
        c.auth = "token".into();
        c.username.clear();
        RemoteClient::new(c, Some(token.into())).unwrap()
    }
    fn alerts(first: usize, count: usize, total: usize, scroll: &str) -> Value {
        json!({"_scroll_id":scroll,"timed_out":false,"_shards":{"failed":0},"hits":{"total":{"value":total,"relation":"eq"},"hits":(first..first+count).map(|i|json!({"_index":"wazuh-alerts-4.x-2026.10.08","_id":i.to_string(),"sort":[1000000-i],"_source":{"timestamp":"2026-10-08T12:00:00.000+0000","rule":{"level":5,"id":"5710","description":"sshd: Attempt to login using a non-existent user"},"agent":{"id":"001","name":"web01"},"data":{"srcip":"203.0.113.7","srcuser":"admin"},"id":format!("1696766400.{i}")}})).collect::<Vec<_>>()}})
    }
    fn count(total: usize) -> Value {
        json!({"timed_out":false,"_shards":{"failed":0},"hits":{"total":{"value":total,"relation":"eq"},"hits":[]}})
    }
    fn status(code: u16) -> Reply {
        Reply { status: code, headers: vec![], body: json!({"error":"never echoed"}) }
    }
    fn agents(ids: std::ops::Range<usize>, total: usize) -> Value {
        json!({"data":{"affected_items":ids.map(|i|json!({"id":format!("{i:03}"),"name":format!("host-{i}"),"status":"active","lastKeepAlive":"2026-10-08T12:00:00+00:00"})).collect::<Vec<_>>(),"total_affected_items":total,"total_failed_items":0,"failed_items":[]},"message":"ok","error":0})
    }
    const BASIC: &str = "Basic dXNlcjpwYXNz";

    #[test]
    fn indexer_scroll_contract_limits_and_cleanup() {
        let root = Folder::new();
        let server = Server::new(vec![ok(alerts(0, 500, 502, "s1")), ok(alerts(500, 2, 502, "s2")), ok(json!({"succeeded":true,"num_freed":1}))]);
        let result = indexer_import(&indexer(&server.url), &root.0, Some("2026-10-01T00:00:00Z".into()), None, |_, _| {}).unwrap();
        assert_eq!((result.count, result.total, result.limited), (502, Some(502), false));
        assert_eq!(result.order, "timestamp decrescente; valores sem horário ao final");
        let text = fs::read_to_string(&result.path).unwrap();
        assert_eq!(text.lines().count(), 502);
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["rule"]["id"], "5710");
        assert_eq!(first["_loginsight_remote"]["_index"], "wazuh-alerts-4.x-2026.10.08");
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].path, "/wazuh-alerts-*/_search?scroll=2m");
        assert_eq!(requests[0].body["size"], 500);
        assert_eq!(requests[0].body["track_total_hits"], true);
        assert_eq!(requests[0].body["sort"][0]["timestamp"]["order"], "desc");
        assert_eq!(requests[0].body["query"]["bool"]["filter"][1]["range"]["timestamp"]["gte"], "2026-10-01T00:00:00Z");
        assert_eq!((requests[1].path.as_str(), &requests[1].body["scroll_id"]), ("/_search/scroll", &json!("s1")));
        assert_eq!((requests[2].method.as_str(), &requests[2].body["scroll_id"]), ("DELETE", &json!(["s2"])));
        assert!(requests.iter().all(|r| r.headers.get("authorization").map(String::as_str) == Some(BASIC)));

        // A page larger than the remaining budget is truncated and reported as limited.
        let limited = Folder::new();
        let server = Server::new(vec![ok(alerts(0, 500, 2000, "a")), ok(alerts(500, 500, 2000, "b")), ok(json!({"succeeded":true}))]);
        let mut remote = indexer(&server.url);
        remote.connection.max_records = 700;
        let result = indexer_import(&remote, &limited.0, None, None, |_, _| {}).unwrap();
        assert_eq!((result.count, result.limited), (700, true));
        assert_eq!(fs::read_to_string(&result.path).unwrap().lines().count(), 700);
        assert_eq!(server.finish()[2].body["scroll_id"], json!(["b"]));

        // Partial shard results publish nothing and still release the latest cursor.
        let failed = Folder::new();
        let mut partial = alerts(500, 1, 600, "p2");
        partial["_shards"]["failed"] = json!(1);
        let server = Server::new(vec![ok(alerts(0, 500, 600, "p1")), ok(partial), ok(json!({"succeeded":true}))]);
        let error = indexer_import(&indexer(&server.url), &failed.0, None, None, |_, _| {}).err().unwrap();
        assert!(error.contains("parciais"));
        assert_eq!(snapshot_count(&failed.0), 0);
        assert_eq!(server.finish().last().unwrap().body["scroll_id"], json!(["p2"]));

        // Without a time field the scroll uses index order and says so when limited.
        let unordered = Folder::new();
        let server = Server::new(vec![ok(alerts(0, 2, 9, "u")), ok(json!({"succeeded":true}))]);
        let mut remote = indexer(&server.url);
        remote.connection.time_field.clear();
        remote.connection.max_records = 2;
        let result = indexer_import(&remote, &unordered.0, None, None, |_, _| {}).unwrap();
        assert!(result.limited && result.warning.unwrap().contains("ordem interna"));
        assert_eq!(server.finish()[0].body["sort"], json!(["_doc"]));
    }

    #[test]
    fn indexer_test_reports_engine_generation_and_permissions() {
        let server = Server::new(vec![
            ok(json!({"name":"node-1","version":{"number":"7.10.2","build_type":"rpm"},"tagline":"The OpenSearch Project: https://opensearch.org/"})),
            ok(json!({"nodes":{"abc":{"version":"2.19.5"}}})),
            ok(alerts(0, 1, 1_234_567, "t1")),
            ok(json!({"succeeded":true})),
            ok(count(42)),
            ok(count(0)),
            ok(count(0)),
            status(403),
        ]);
        let message = indexer_test(&indexer(&server.url)).unwrap().message;
        for expected in ["OpenSearch 2.19.5", "wazuh-alerts-*: 1.234.567 registros", "eventos arquivados em wazuh-archives-* (42)", "Sem permissão de leitura: wazuh-states-vulnerabilities*", "Índices de Wazuh 4.x."] {
            assert!(message.contains(expected), "{expected} in {message}");
        }
        let requests = server.finish();
        assert_eq!(requests[1].path, "/_nodes/_local?filter_path=nodes.*.version");
        assert_eq!(requests[2].body["size"], 1);
        assert_eq!(requests[4].path, "/wazuh-archives-*/_search");

        // A role without cluster monitoring still tests reads; 5.x data is suggested.
        let server = Server::new(vec![
            status(403),
            ok(alerts(0, 0, 0, "z")),
            ok(json!({"succeeded":true})),
            ok(count(0)),
            ok(count(10)),
            ok(count(0)),
            ok(count(0)),
        ]);
        let message = indexer_test(&indexer(&server.url)).unwrap().message;
        for expected in ["versão não informada", "Nenhum registro em wazuh-alerts-*", "achados em wazuh-findings-v5* (10)", "Índices de Wazuh 5.x.", "Escolha um dos conjuntos"] {
            assert!(message.contains(expected), "{expected} in {message}");
        }
        server.finish();

        let server = Server::new(vec![ok(json!({"version":{"number":"6.8.23"},"tagline":"You Know, for Search"}))]);
        assert!(indexer_test(&indexer(&server.url)).err().unwrap().contains("não é suportado"));
        server.finish();
    }

    #[test]
    fn dashboard_reads_the_indexer_through_the_console_proxy() {
        let root = Folder::new();
        let server = Server::new(vec![ok(alerts(0, 2, 2, "w1")), ok(json!({"succeeded":true}))]);
        let mut c = config(&format!("{}/wazuh", server.url));
        c.kind = RemoteKind::WazuhWeb;
        c.index = "wazuh-alerts-*".into();
        c.time_field = "timestamp".into();
        let remote = RemoteClient::new(c, Some("pass".into())).unwrap();
        let result = indexer_import(&remote, &root.0, None, None, |_, _| {}).unwrap();
        assert_eq!((result.count, result.limited), (2, false));
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        for (request, (path, method)) in requests.iter().zip([("wazuh-alerts-*/_search?scroll=2m", "POST"), ("_search/scroll", "DELETE")]) {
            let url = reqwest::Url::parse(&format!("http://fixture{}", request.path)).unwrap();
            assert_eq!(url.path(), "/wazuh/api/console/proxy");
            let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!((query["path"].as_str(), query["method"].as_str()), (path, method));
            assert_eq!(request.method, "POST");
            assert_eq!(request.headers["osd-xsrf"], "true");
            assert_eq!(request.headers["authorization"], BASIC);
        }
        assert_eq!(requests[1].body["scroll_id"], json!(["w1"]));

        // The dashboard returns the indexer status as is.
        let server = Server::new(vec![status(401)]);
        let mut c = config(&server.url);
        c.kind = RemoteKind::WazuhWeb;
        c.index = "wazuh-alerts-*".into();
        let error = indexer_test(&RemoteClient::new(c, Some("pass".into())).unwrap()).err().unwrap();
        assert!(error.contains("painel Wazuh (401)"), "{error}");
        server.finish();
    }

    #[test]
    fn wrong_wazuh_endpoints_get_specific_guidance() {
        let mut c = config("https://wazuh.example:55000");
        c.kind = RemoteKind::Wazuh;
        let message = Failure::Status(401).message(&c);
        assert!(message.contains("indexer (401)") && message.contains("porta 9200"), "{message}");
        assert!(Failure::Connect { timeout: false }.message(&c).contains("root-ca.pem"));
        c.kind = RemoteKind::WazuhApi;
        c.url = "https://wazuh.example:9200".into();
        assert!(Failure::NotJson.message(&c).contains("porta 55000"));
        c.url = "https://wazuh.example".into();
        assert!(Failure::Status(404).message(&c).contains("painel web"));
        assert!(!Failure::Status(404).message(&config("https://elastic.example:9200")).contains("Wazuh"));
    }

    #[test]
    fn server_api_session_paging_and_versions() {
        let root = Folder::new();
        let server = Server::new(vec![
            status(405),
            ok(json!({"data":{"token":"t1"},"error":0})),
            ok(json!({"data":{"title":"Wazuh API REST","api_version":"v4.3.10"},"error":0})),
            ok(agents(0..500, 502)),
            status(401),
            ok(json!({"data":{"token":"t2"},"error":0})),
            // An agent enrolled meanwhile shifted the offsets: 499 comes back again.
            ok(agents(499..502, 502)),
        ]);
        let result = api_import(&api(&server.url, "agents"), &root.0, |_, _| {}).unwrap();
        assert_eq!((result.count, result.total, result.limited), (502, Some(502), false));
        assert!(result.warning.unwrap().contains("1 itens repetidos"));
        let text = fs::read_to_string(&result.path).unwrap();
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["_loginsight_remote"]["wazuhDataset"], "agents");
        assert_eq!(first["_loginsight_remote"]["apiVersion"], "4.3.10");
        let requests = server.finish();
        assert_eq!((requests[0].method.as_str(), requests[1].method.as_str()), ("POST", "GET"));
        assert_eq!(requests[0].path, "/security/user/authenticate");
        assert_eq!(requests[1].headers["authorization"], BASIC);
        assert_eq!(requests[2].headers["authorization"], "Bearer t1");
        assert_eq!(requests[3].path, "/agents?offset=0&limit=500&sort=%2Bid");
        assert_eq!(requests[4].path, "/agents?offset=500&limit=500&sort=%2Bid");
        assert_eq!(requests[5].headers["authorization"], BASIC);
        assert_eq!(requests[6].headers["authorization"], "Bearer t2");

        // A supplied token is used as is and never renewed.
        let server = Server::new(vec![
            ok(json!({"data":{"api_version":"v5.0.0"},"error":0})),
            ok(agents(0..1, 1500)),
        ]);
        let message = api_test(&api_token(&server.url, "agents", "jwt-x")).unwrap().message;
        assert!(message.contains("5.0.0") && message.contains("1.500 agentes") && message.contains("token informado"), "{message}");
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].headers["authorization"], "Bearer jwt-x");
        assert_eq!(requests[1].path, "/agents?offset=0&limit=1&sort=%2Bid");

        let server = Server::new(vec![status(401)]);
        assert!(api_test(&api_token(&server.url, "agents", "jwt-y")).err().unwrap().contains("(401)"));
        assert_eq!(server.finish().len(), 1);

        let server = Server::new(vec![
            ok(json!({"data":{"token":"t"},"error":0})),
            ok(json!({"data":{"api_version":"v3.13.6"},"error":0})),
        ]);
        assert!(api_test(&api(&server.url, "agents")).err().unwrap().contains("4.0"));
        server.finish();

        let server = Server::new(vec![
            ok(json!({"data":{"token":"t"},"error":0})),
            ok(json!({"data":{"api_version":"4.14.8"},"error":0})),
            ok(json!({"data":{"affected_items":[],"total_affected_items":0,"failed_items":[{"error":{"code":4000}}]},"error":2})),
        ]);
        assert!(api_test(&api(&server.url, "agents")).err().unwrap().contains("falha total ou parcial"));
        server.finish();
    }
}
