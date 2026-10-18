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
    /// status, reset, rollback, accept_expected, export, import, download or publish.
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
pub fn execute(full: &Results, request: Request) -> Result<Value, String> {
    match request.action.as_str() {
        "status" => Ok(full.metadata["investigation"]["learning"].clone()),
        "reset" => crate::local_learning::reset(),
        "rollback" => crate::local_learning::rollback(),
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
        "download" | "publish" => synchronize(full, &request),
        _ => {
            Err("Ação deve ser status, reset, rollback, accept_expected, export, import, download ou publish".into())
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
    if request.action == "download" {
        let response = auth(
            client
                .get(format!(
                    "{endpoint}/contents/models/shared-model.json?ref={}",
                    settings.github_branch
                ))
                .header("Accept", "application/vnd.github.raw+json"),
        )
        .send()
        .map_err(|_| {
            "Não foi possível consultar a base no GitHub; o aprendizado local permanece disponível"
        })?;
        let content = bounded_json(response)?;
        let model: SharedModel = serde_json::from_value(content).map_err(|e| e.to_string())?;
        return store_shared(&model);
    }
    token.ok_or("Publicar exige token temporário com escrita no repositório configurado")?;
    let contributions = full.learning_contributions()?;
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
    let mut published = 0;
    let mut already = 0;
    for contribution in contributions {
        crate::operations::check()?;
        contribution.validate()?;
        let url = format!(
            "{endpoint}/contents/models/contributions/{}.json",
            contribution.id
        );
        let existing = auth(client.get(format!("{url}?ref={}", settings.github_branch)))
            .send()
            .map_err(|_| "Falha ao conferir contribuição no GitHub")?;
        if existing.status().is_success() {
            let remote: log_learning::Contribution =
                serde_json::from_slice(&github_content(&bounded_json(existing)?)?)
                    .map_err(|_| "A contribuição existente no Git não tem o formato esperado")?;
            remote.validate()?;
            if remote.id != contribution.id {
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
