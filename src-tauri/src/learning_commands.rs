//! Explicit synchronization. Tokens live only for this request; neither tokens
//! nor private profiles are saved in the Case or sent to the repository.
use crate::security_results::Results;
use base64::Engine;
use log_learning::{SharedModel, MAX_DOCUMENT_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{Read, Write};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Local learning and diagnostic memory/knowledge actions. Publication is explicit.
    pub action: String,
    #[serde(default)]
    pub signal_id: Option<String>,
    #[serde(default)]
    pub document: Option<Value>,
    #[serde(default)]
    pub token: Option<String>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningRequest")
            .field("action", &self.action)
            .field("document_present", &self.document.is_some())
            .field("token_present", &self.token.is_some())
            .finish()
    }
}
fn store_shared(model: &SharedModel) -> Result<Value, String> {
    model.validate()?;
    let directory = crate::config_dir();
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, model).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(directory.join("learning-shared.json"))
        .map_err(|e| e.error.to_string())?;
    Ok(
        json!({"status":"imported","revision":model.revision(),"contributions":model.contributions.len(),"apply":"next_explicit_analysis"}),
    )
}
fn bounded_json(response: reqwest::blocking::Response) -> Result<Value, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GitHub respondeu HTTP {}", status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_DOCUMENT_BYTES as u64)
    {
        return Err("Resposta do Git excedeu 16 MiB".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("Resposta do Git excedeu 16 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Resposta do Git fora do formato esperado".into())
}
fn github_content(content: &Value) -> Result<Vec<u8>, String> {
    let encoded = content["content"]
        .as_str()
        .ok_or("Arquivo compartilhado sem conteúdo")?
        .split_whitespace()
        .collect::<String>();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Conteúdo compartilhado inválido")?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("Base compartilhada excedeu 16 MiB".into());
    }
    Ok(bytes)
}
fn diagnostic_workflow(content: &Value) -> Result<(), String> {
    let bytes = github_content(content)?;
    let workflow = std::str::from_utf8(&bytes).map_err(|_| "Workflow de aprendizado inválido")?;
    if !workflow.contains("diagnostic-consolidate") || !workflow.contains("models/diagnostic-contributions/") {
        return Err("Atualize a branch de aprendizado com o workflow e o núcleo desta versão antes de publicar diagnósticos".into());
    }
    Ok(())
}
pub fn execute(full: &Results, request: Request) -> Result<Value, String> {
    match request.action.as_str() {
        "status" => Ok(full.metadata["investigation"]["learning"].clone()),
        "reset" => crate::local_learning::reset(),
        "rollback" => crate::local_learning::rollback(),
        "diagnostic_memory_status" => crate::diagnostic_memory::status(),
        "diagnostic_feedback" => full.diagnostic_feedback(request.signal_id.as_deref().ok_or("Escolha o episódio a revisar")?,request.document.ok_or("Forneça o resultado da investigação")?),
        "diagnostic_export"=>serde_json::to_value(crate::diagnostic_memory::contributions()?).map_err(|e|e.to_string()),
        "diagnostic_import"=>crate::diagnostic_knowledge::store(request.document.ok_or("Forneça a base diagnóstica")?),
        "diagnostic_rollback"=>crate::diagnostic_knowledge::rollback(),
        "accept_expected" => full.learn_expected(
            request
                .signal_id
                .as_deref()
                .ok_or("Escolha o diagnóstico a revisar")?,
        ),
        "export" => Ok(
            json!({"schema_version":log_learning::SCHEMA,"contributions":full.learning_contributions()?,"meaning":"Somente estados de vocabulário fechado e contagens. A publicação continua sendo opcional."}),
        ),
        "import" => {
            let document = request.document.ok_or("Forneça uma base compartilhada")?;
            if document.to_string().len() > MAX_DOCUMENT_BYTES {
                return Err("Base compartilhada excedeu 16 MiB".into());
            }
            let model: SharedModel = serde_json::from_value(document).map_err(|e| e.to_string())?;
            store_shared(&model)
        }
        "download" | "publish" | "diagnostic_download" | "diagnostic_publish" => synchronize(full, &request),
        _ => {
            Err("Ação desconhecida. Use status/reset/rollback/accept_expected/export/import/download/publish ou diagnostic_memory_status/feedback/export/import/rollback/download/publish com prefixo diagnostic_.".into())
        }
    }
}
fn synchronize(full: &Results, request: &Request) -> Result<Value, String> {
    let settings = crate::detections::load_settings().investigation.learning;
    settings.validate()?;
    let repository = settings
        .github_repository
        .as_deref()
        .ok_or("Configure o repositório nas opções do aprendizado")?;
    let token = request.token.as_deref().filter(|s| !s.is_empty());
    if token.is_some_and(|s| s.len() > 256 || s.chars().any(char::is_whitespace)) {
        return Err("Token do Git fora do formato esperado".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("LogInsight-local-learning")
        .build()
        .map_err(|e| e.to_string())?;
    let endpoint = format!("https://api.github.com/repos/{repository}");
    let auth = |builder: reqwest::blocking::RequestBuilder| {
        if let Some(token) = token {
            builder.bearer_auth(token)
        } else {
            builder
        }
    };
    let diagnostic=request.action.starts_with("diagnostic_");
    if request.action == "download" || request.action=="diagnostic_download" {
        let filename=if diagnostic{"diagnostic-knowledge.json"}else{"shared-model.json"};
        let response = auth(
            client
                .get(format!(
                    "{endpoint}/contents/models/{filename}?ref={}",
                    settings.github_branch
                ))
                .header("Accept", "application/vnd.github.raw+json"),
        )
        .send()
        .map_err(|_| {
            "Não foi possível consultar a base no GitHub; o aprendizado local permanece disponível"
        })?;
        let content = bounded_json(response)?;
        if diagnostic{
            let _lane=crate::security_store::working_lane()?;
            return crate::diagnostic_knowledge::store(content);
        }
        let model: SharedModel = serde_json::from_value(content).map_err(|e| e.to_string())?;
        return store_shared(&model);
    }
    token.ok_or("Publicar exige token temporário com escrita no repositório configurado")?;
    let contributions=if diagnostic{
        crate::diagnostic_memory::contributions()?.contributions.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?
    }else{full.learning_contributions()?.into_iter().map(serde_json::to_value).collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?};
    if contributions.is_empty() {
        return Err("Não há contribuição validada nesta análise".into());
    }
    crate::operations::check()?;
    let branch = auth(client.get(format!(
        "{endpoint}/git/ref/heads/{}",
        settings.github_branch
    )))
    .send()
    .map_err(|_| "Falha ao consultar a branch de aprendizado")?;
    if branch.status() == reqwest::StatusCode::NOT_FOUND {
        let repository_info = bounded_json(
            auth(client.get(&endpoint))
                .send()
                .map_err(|_| "Falha ao consultar o repositório")?,
        )?;
        let default = repository_info["default_branch"]
            .as_str()
            .ok_or("Repositório sem branch principal")?;
        let base = bounded_json(
            auth(client.get(format!("{endpoint}/git/ref/heads/{default}")))
                .send()
                .map_err(|_| "Falha ao consultar a revisão principal")?,
        )?;
        let sha = base["object"]["sha"]
            .as_str()
            .ok_or("Revisão principal sem identidade")?;
        bounded_json(
            auth(client.post(format!("{endpoint}/git/refs")))
                .json(&json!({"ref":format!("refs/heads/{}",settings.github_branch),"sha":sha}))
                .send()
                .map_err(|_| "Falha ao criar a branch de aprendizado")?,
        )?;
    } else {
        bounded_json(branch)?;
    }
    if diagnostic {
        let response = auth(client.get(format!("{endpoint}/contents/.github/workflows/learning.yml?ref={}", settings.github_branch)))
            .send().map_err(|_| "Falha ao verificar a consolidação diagnóstica na branch")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err("A branch de aprendizado precisa do workflow desta versão antes de receber diagnósticos".into());
        }
        diagnostic_workflow(&bounded_json(response)?)?;
    }
    let mut published = 0;
    let mut already = 0;
    for contribution in contributions {
        crate::operations::check()?;
        let id=contribution["id"].as_str().ok_or("Contribuição sem identidade pública")?;
        let folder=if diagnostic{"diagnostic-contributions"}else{"contributions"};
        let url = format!(
            "{endpoint}/contents/models/{folder}/{id}.json"
        );
        let existing = auth(client.get(format!("{url}?ref={}", settings.github_branch)))
            .send()
            .map_err(|_| "Falha ao conferir contribuição no GitHub")?;
        if existing.status().is_success() {
            let bytes=github_content(&bounded_json(existing)?)?;
            let remote_id=if diagnostic{let remote:log_learning::DiagnosticContribution=serde_json::from_slice(&bytes).map_err(|_|"Contribuição diagnóstica inválida no Git")?;remote.validate()?;remote.id}
                else{let remote:log_learning::Contribution=serde_json::from_slice(&bytes).map_err(|_|"Contribuição inválida no Git")?;remote.validate()?;remote.id};
            if remote_id != id {
                return Err("O arquivo no Git diverge da contribuição identificada".into());
            }
            already += 1;
            continue;
        }
        if existing.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(format!(
                "Consulta da contribuição respondeu HTTP {}",
                existing.status().as_u16()
            ));
        }
        let bytes = serde_json::to_vec_pretty(&contribution).map_err(|e| e.to_string())?;
        let content = base64::engine::general_purpose::STANDARD.encode(bytes);
        bounded_json(auth(client.put(url)).json(&json!({"message":"learning: add reviewed aggregate contribution","branch":settings.github_branch,"content":content})).send().map_err(|_|"Falha ao publicar contribuição agregada")?)?;
        published += 1;
    }
    Ok(
        json!({"status":"published","published":published,"already_present":already,"repository":repository,"branch":settings.github_branch,"consolidation":"GitHub Actions validates and consolidates the next shared revision; download it after that workflow completes","raw_logs_sent":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_publication_requires_a_compatible_consolidation_workflow() {
        let wire = |text: &str| json!({"content":base64::engine::general_purpose::STANDARD.encode(text)});
        assert!(diagnostic_workflow(&wire("consolidate models/contributions")).is_err());
        assert!(diagnostic_workflow(&wire(include_str!("../../.github/workflows/learning.yml"))).is_ok());
    }
    #[test]
    fn credentials_are_never_rendered_by_debug_and_repository_paths_are_bounded() {
        let request = Request {
            action: "publish".into(),
            signal_id: None,
            document: None,
            token: Some("never-log-this-token".into()),
        };
        assert!(!format!("{request:?}").contains("never-log"));
        let mut settings = crate::local_learning::Settings::default();
        settings.github_repository = Some("owner/project".into());
        settings.validate().unwrap();
        settings.github_repository = Some("owner/../private".into());
        assert!(settings.validate().is_err());
        settings.github_repository = None;
        settings.github_branch = "branch?token=secret".into();
        assert!(settings.validate().is_err());
    }
}
