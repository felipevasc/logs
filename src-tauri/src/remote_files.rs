//! Bounded read-only acquisition through system OpenSSH or Windows PSRemoting.
//! Credentials travel over stdin, paths are data, and snapshots publish atomically.
use crate::remote::{RemoteConfig, RemoteKind};
use std::{fs::{self, File}, io::Write, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant}};
use base64::Engine;

const MAX_FILES: usize = 2048;
pub(crate) struct Imported { pub paths: Vec<String>, pub bytes: u64, pub warning: Option<String> }

pub(crate) fn validate(c: &mut RemoteConfig) -> Result<(), String> {
    let url = reqwest::Url::parse(&c.url).map_err(|_| "Informe ssh://host:22 ou https://host:5986/wsman.")?;
    let ssh = c.kind == RemoteKind::Ssh;
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
        || ssh && (url.scheme() != "ssh" || !matches!(url.path(), "" | "/"))
        || !ssh && (url.scheme() != "https" || !matches!(url.path(), "/wsman" | "/" | "")) {
        return Err("Use ssh://host:22 para SSH ou https://host:5986/wsman para WinRM, com credenciais nos campos próprios.".into());
    }
    if c.username.len() > 256 || c.username.chars().any(char::is_control) || c.username.starts_with('-') || ssh && c.username.is_empty() {
        return Err("Informe um usuário válido para o acesso remoto.".into());
    }
    if !(1_048_576..=4_294_967_296).contains(&c.max_bytes) { return Err("O limite da coleta deve estar entre 1 MiB e 4 GiB.".into()); }
    if c.paths.is_empty() || c.paths.len() > 64 { return Err("Escolha de 1 a 64 arquivos ou pastas para coletar.".into()); }
    for path in &mut c.paths {
        *path = path.trim().to_owned();
        if path.len() > 4096 || path.chars().any(char::is_control) || if ssh { !path.starts_with('/') } else {
            let bytes = path.as_bytes(); bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\'
        } { return Err("Informe caminhos absolutos, um por linha, sem curingas ou comandos.".into()); }
    }
    c.paths.sort(); c.paths.dedup();
    if c.key_path.len() > 4096 || c.key_path.chars().any(char::is_control) { return Err("Caminho de chave SSH inválido.".into()); }
    c.url = url.to_string(); c.query = None; c.index.clear(); c.time_field.clear();
    Ok(())
}

const SSH_COLLECTOR: &str = include_str!("remote_collect.py");
const WINRM_COLLECTOR: &str = include_str!("remote_collect.ps1");
fn quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }
fn command(c: &RemoteConfig) -> Result<Command, String> {
    let url = reqwest::Url::parse(&c.url).map_err(|e| e.to_string())?;
    let mut command = if c.kind == RemoteKind::Ssh {
        let mut command = Command::new("ssh");
        command.args(["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=20", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2"]);
        command.args(["-p", &url.port().unwrap_or(22).to_string(), "-l", &c.username]);
        if !c.key_path.is_empty() { command.args(["-i", &c.key_path, "-o", "IdentitiesOnly=yes"]); }
        command.arg("--").arg(url.host_str().unwrap()).arg(format!("python3 -c {}", quote(SSH_COLLECTOR)));
        command
    } else {
        if !cfg!(windows) { return Err("A coleta WinRM usa o cliente PowerShell do Windows. Neste computador, use SSH.".into()); }
        let mut command = Command::new("powershell.exe");
        let bytes: Vec<u8> = WINRM_COLLECTOR.encode_utf16().flat_map(u16::to_le_bytes).collect();
        command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand", &base64::engine::general_purpose::STANDARD.encode(bytes)]);
        command
    };
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    command.stdin(Stdio::piped());
    Ok(command)
}

fn acquire(c: &RemoteConfig, password: Option<String>, directory: &Path, test: bool) -> Result<(), String> {
    if c.kind == RemoteKind::Ssh && password.as_ref().is_some_and(|p| !p.is_empty()) {
        return Err("SSH usa chave privada ou agente OpenSSH. Remova a senha e selecione a chave, se necessário.".into());
    }
    let output = directory.join("transport");
    let errors = directory.join("errors");
    let mut command = command(c)?;
    command.stdout(Stdio::from(File::create(&output).map_err(|e| e.to_string())?));
    command.stderr(Stdio::from(File::create(&errors).map_err(|e| e.to_string())?));
    let mut child = command.spawn().map_err(|e| format!("Não foi possível iniciar o cliente remoto: {e}. SSH requer OpenSSH; o Linux remoto requer Python 3."))?;
    struct ChildGuard<'a>(&'a mut std::process::Child);
    impl Drop for ChildGuard<'_> { fn drop(&mut self) { if self.0.try_wait().ok().flatten().is_none() { let _ = self.0.kill(); let _ = self.0.wait(); } } }
    let guard = ChildGuard(&mut child);
    let request = serde_json::json!({"paths": c.paths, "maxBytes": c.max_bytes, "maxFiles": MAX_FILES, "test": test,
        "url": c.url, "username": c.username, "password": password, "destination": directory.join("files")});
    guard.0.stdin.take().ok_or("Entrada do cliente indisponível")?.write_all(request.to_string().as_bytes()).map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        crate::operations::check()?;
        if started.elapsed() > Duration::from_secs(if test { 60 } else { 600 }) { return Err("O tempo limite da coleta remota foi atingido.".into()); }
        if fs::metadata(&errors).map(|m| m.len()).unwrap_or(0) > 1_048_576 ||
            fs::metadata(&output).map(|m| m.len()).unwrap_or(0) > c.max_bytes.saturating_add(4 * 1024 * 1024) {
            return Err("A transferência excedeu o limite configurado.".into());
        }
        if c.kind == RemoteKind::Winrm {
            let bytes = fs::read_dir(directory.join("files")).ok().into_iter().flatten().filter_map(Result::ok)
                .filter_map(|f| f.metadata().ok()).map(|m| m.len()).fold(0u64, u64::saturating_add);
            if bytes > c.max_bytes { return Err("A transferência excedeu o limite configurado.".into()); }
        }
        if let Some(status) = guard.0.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                let bytes = fs::read(&errors).map_err(|e| e.to_string())?;
                let mut error = String::from_utf8_lossy(&bytes).to_string();
                if let Some(secret) = password.as_ref().filter(|p| !p.is_empty()) { error = error.replace(secret, "[senha]"); }
                return Err(format!("Coleta recusada: {}", error.chars().take(1800).collect::<String>()));
            }
            return Ok(());
        }
        crate::global_scheduler::unreserved_io(|| std::thread::sleep(Duration::from_millis(50)));
    }
}
pub(crate) fn test(c: &RemoteConfig, password: Option<String>) -> Result<(), String> {
    let temporary = tempfile::tempdir().map_err(|e| e.to_string())?;
    acquire(c, password, temporary.path(), true)
}

fn extract(c: &RemoteConfig, temporary: &Path) -> Result<Imported, String> {
    let mut archive = tar::Archive::new(File::open(temporary.join("transport")).map_err(|e| e.to_string())?);
    let mut paths = Vec::new(); let mut bytes = 0u64; let mut warning = None;
    for item in archive.entries().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        let mut item = item.map_err(|e| e.to_string())?;
        let path = item.path().map_err(|e| e.to_string())?.into_owned();
        let name = path.to_str().ok_or("Nome remoto inválido")?;
        if !item.header().entry_type().is_file() { return Err("O pacote remoto contém um tipo de arquivo não permitido.".into()); }
        if name == "manifest.json" {
            use std::io::Read;
            let mut text = String::new(); item.take(256 * 1024).read_to_string(&mut text).map_err(|e| e.to_string())?;
            let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if let Some(skipped) = manifest["skipped"].as_array().filter(|p| !p.is_empty()) { warning = Some(format!("{} caminhos ausentes foram ignorados.", skipped.len())); }
            continue;
        }
        let components: Vec<_> = path.components().collect();
        if components.len() != 3 || !name.starts_with("files/") || name.contains('\\') || components.iter().any(|p| !matches!(p, std::path::Component::Normal(_))) {
            return Err("O pacote remoto contém um caminho não permitido.".into());
        }
        bytes = bytes.checked_add(item.size()).ok_or("Tamanho de coleta inválido")?;
        if bytes > c.max_bytes || paths.len() >= MAX_FILES { return Err("O pacote remoto excede os limites da coleta.".into()); }
        let target = temporary.join(path);
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut item, &mut file).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        paths.push(target.to_string_lossy().into_owned());
    }
    if paths.is_empty() { return Err("Nenhum arquivo regular encontrado nos caminhos selecionados.".into()); }
    Ok(Imported { paths, bytes, warning })
}

pub(crate) fn import(c: &RemoteConfig, password: Option<String>, root: &Path, app: &tauri::AppHandle) -> Result<Imported, String> {
    let parent = root.join("remote-snapshots"); fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let pending = tempfile::Builder::new().prefix(".pending-").tempdir_in(&parent).map_err(|e| e.to_string())?;
    crate::emit_progress(Some(app), "remoto", "Coletando arquivos remotos", 0, 0, "arquivos", true);
    acquire(c, password, pending.path(), false)?;
    let mut result = if c.kind == RemoteKind::Ssh { extract(c, pending.path())? } else {
        let manifest: serde_json::Value = serde_json::from_slice(&fs::read(pending.path().join("transport")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut paths = Vec::new(); let mut bytes = 0u64;
        for name in manifest["files"].as_array().ok_or("Lista remota de arquivos inválida")? {
            let name = name.as_str().ok_or("Nome remoto inválido")?;
            if name.contains(['/', '\\']) || matches!(name, "" | "." | "..") { return Err("Nome remoto inválido".into()); }
            let target = pending.path().join("files").join(name);
            let metadata = fs::symlink_metadata(&target).map_err(|e| e.to_string())?;
            if !metadata.file_type().is_file() { return Err("Arquivo remoto inválido".into()); }
            bytes = bytes.checked_add(metadata.len()).ok_or("Tamanho remoto inválido")?;
            if bytes > c.max_bytes || paths.len() >= MAX_FILES { return Err("A coleta excede os limites configurados.".into()); }
            paths.push(target.to_string_lossy().into_owned());
        }
        if paths.is_empty() { return Err("Nenhum arquivo encontrado nos caminhos selecionados.".into()); }
        Imported { paths, bytes, warning: manifest["warning"].as_str().filter(|s| !s.is_empty()).map(str::to_owned) }
    };
    crate::operations::check()?;
    fs::remove_file(pending.path().join("transport")).map_err(|e| e.to_string())?;
    fs::remove_file(pending.path().join("errors")).map_err(|e| e.to_string())?;
    let destination = parent.join(format!("files-{}", uuid::Uuid::new_v4()));
    let old = pending.path().to_path_buf();
    fs::rename(&old, &destination).map_err(|e| e.to_string())?;
    for path in &mut result.paths { *path = destination.join(PathBuf::from(&*path).strip_prefix(&old).map_err(|e| e.to_string())?).to_string_lossy().into_owned(); }
    crate::operations::commit();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> RemoteConfig { serde_json::from_value(serde_json::json!({"name":"Linux", "kind":"ssh", "url":"ssh://example.test:22", "index":"", "username":"analyst", "paths":["/var/log/auth.log"], "maxBytes":1048576})).unwrap() }
    #[test]
    fn remote_paths_are_data_and_archives_reject_links_and_over_budget_files() {
        let mut c = config(); validate(&mut c).unwrap();
        c.paths = vec!["relative.log".into()]; assert!(validate(&mut c).is_err());
        c = config(); c.url = "ssh://user:secret@example.test".into(); assert!(validate(&mut c).is_err());
        c = config(); c.kind = RemoteKind::Winrm; c.url = "http://example.test:5985/wsman".into(); assert!(validate(&mut c).is_err());
        assert_eq!(quote("a'b"), "'a'\\''b'");
        let dir = tempfile::tempdir().unwrap();
        let mut archive = tar::Builder::new(File::create(dir.path().join("transport")).unwrap());
        let mut header = tar::Header::new_gnu(); header.set_size(4); header.set_mode(0o600); header.set_cksum();
        archive.append_data(&mut header, "files/0000/auth.log", &b"log\n"[..]).unwrap(); archive.finish().unwrap(); drop(archive);
        let imported = extract(&config(), dir.path()).unwrap(); assert_eq!(imported.bytes, 4); assert_eq!(imported.paths.len(), 1);
        let mut small = config(); small.max_bytes = 3;
        assert!(extract(&small, dir.path()).err().unwrap().contains("limites"));
        let mut archive = tar::Builder::new(File::create(dir.path().join("transport")).unwrap());
        let mut header = tar::Header::new_gnu(); header.set_entry_type(tar::EntryType::Symlink); header.set_size(0); header.set_mode(0o600); header.set_link_name("/etc/passwd").unwrap(); header.set_cksum();
        archive.append_data(&mut header, "files/0001/link", std::io::empty()).unwrap(); archive.finish().unwrap(); drop(archive);
        assert!(extract(&config(), dir.path()).err().unwrap().contains("tipo de arquivo"));
    }
}
