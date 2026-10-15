//! Separar logs: cuts a large text log into the items that match a rule and,
//! optionally, the others, reading it as a stream. With "remove the original"
//! and little free space the original is consumed from its end while the
//! outputs grow (see `lowspace`).
//!
//! Paths only come from native pickers opened here: the interface receives a
//! token for each chosen file, so page content can never name a file to write,
//! truncate or remove.
mod codec;
mod framing;
mod lowspace;
mod outputs;
mod rule;
pub(crate) mod spec;
mod stream;

use lowspace::{Journal, MapSink, Phase, Stop};
use serde::Serialize;
use spec::{Mode, SplitSpec};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use stream::{Meter, Prepared, Stats};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone)]
struct Grant {
    input: PathBuf,
    matched: Option<PathBuf>,
    rest: Option<PathBuf>,
}

static GRANTS: LazyLock<Mutex<HashMap<String, Grant>>> = LazyLock::new(Default::default);
/// Originals being separated right now.
static BUSY: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

fn journal_dir() -> PathBuf {
    crate::base_config_dir().join("log-split")
}

fn grant(token: &str) -> Result<Grant, String> {
    GRANTS.lock().unwrap_or_else(|e| e.into_inner()).get(token).cloned().ok_or_else(|| "Escolha o arquivo novamente.".to_string())
}

struct Busy(PathBuf);
impl Busy {
    fn claim(path: &Path) -> Result<Busy, String> {
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !BUSY.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()) {
            return Err("Este arquivo já está sendo separado.".into());
        }
        Ok(Busy(key))
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

const TEXT_EXTENSIONS: &[&str] = &["log", "txt", "json", "jsonl", "ndjson", "csv", "tsv", "out", "err", "xml", "trace"];

/// `app.log` → `app.filtrado.log`; `server.log.2026-10-12` → `server.log.2026-10-12.filtrado.log`.
fn default_output(input: &Path, label: &str) -> PathBuf {
    let dir = outputs::parent(input);
    let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "log".into());
    let (stem, ext) = match input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()) {
        Some(ext) if TEXT_EXTENSIONS.contains(&ext.as_str()) => (input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), input.extension().unwrap().to_string_lossy().into_owned()),
        _ => (name, "log".to_string()),
    };
    let mut candidate = dir.join(format!("{stem}.{label}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}.{label}-{n}.{ext}"));
        n += 1;
    }
    candidate
}

fn outputs_of(grant: &Grant) -> (PathBuf, PathBuf) {
    (
        grant.matched.clone().unwrap_or_else(|| default_output(&grant.input, "filtrado")),
        grant.rest.clone().unwrap_or_else(|| default_output(&grant.input, "restante")),
    )
}

/// Whether two paths are on the same disk (the volume of their folders).
fn same_volume(a: &Path, b: &Path) -> bool {
    let volume = |path: &Path| -> Option<String> {
        let dir = fs::canonicalize(outputs::parent(path)).ok()?;
        #[cfg(windows)]
        {
            match dir.components().next()? {
                std::path::Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().to_lowercase()),
                _ => None,
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(fs::metadata(dir).ok()?.dev().to_string())
        }
    };
    matches!((volume(a), volume(b)), (Some(x), Some(y)) if x == y)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    token: String,
    path: String,
    name: String,
    size: u64,
    encoding: String,
    line_break: &'static str,
    lines: Vec<String>,
    framing: String,
    first_date: Option<i64>,
    /// Dates at the start of the file never go back (more than 2 s).
    chronological: Option<bool>,
    free_bytes: Option<u64>,
    matched_path: String,
    rest_path: String,
    matched_chosen: bool,
    rest_chosen: bool,
    /// Both outputs on the original's disk: removing it can work in parts.
    same_volume: bool,
}

fn inspect(token: &str) -> Result<Inspection, String> {
    let grant = grant(token)?;
    let mut file = File::open(&grant.input).map_err(|e| format!("Não foi possível abrir {}: {e}", grant.input.display()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let bytes = stream::sample(&mut file, size)?;
    let codec = codec::Codec::sniff(&bytes)?;
    let body = &bytes[codec.bom.min(bytes.len())..];
    let text = codec.decode(&body[..codec.align(body.len())]).into_owned();
    let ctx = stream::calendar(&grant.input, &file, &text);
    let (framing, note) = stream::compile_framing(&spec::FramingSpec::Auto, &codec, &text, ctx)?;
    let line_break = if codec.newline == b'\r' { "CR" } else if text.contains("\r\n") { "CRLF" } else { "LF" };
    let lines = text.lines().take(12).map(|l| l.chars().take(400).collect()).collect();
    // Item dates of the sample, to suggest the chronological option.
    let reader = rule::DateReader::auto(ctx);
    let mut dates = Vec::new();
    let mut items = framing::Reader::new(file.try_clone().map_err(|e| e.to_string())?, codec.bom as u64, (codec.bom + body.len()) as u64, &framing, codec, 1, 1)?;
    while let Some(batch) = items.next_batch()? {
        for span in batch.spans.iter().filter(|s| s.first) {
            if let Some(date) = reader.read(&codec.decode(&batch.data[span.start..span.end])) {
                dates.push(date);
            }
        }
    }
    let chronological = (dates.len() >= 10).then(|| dates.windows(2).all(|w| w[1] + 2_000 >= w[0]));
    let (matched, rest) = outputs_of(&grant);
    Ok(Inspection {
        token: token.to_string(),
        path: grant.input.to_string_lossy().into_owned(),
        name: grant.input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size,
        encoding: codec.name(),
        line_break,
        lines,
        framing: note,
        first_date: dates.first().copied(),
        chronological,
        free_bytes: outputs::free_space(&outputs::parent(&matched)),
        same_volume: same_volume(&grant.input, &matched) && same_volume(&grant.input, &rest),
        matched_chosen: grant.matched.is_some(),
        rest_chosen: grant.rest.is_some(),
        matched_path: matched.to_string_lossy().into_owned(),
        rest_path: rest.to_string_lossy().into_owned(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    path: String,
    bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitResult {
    /// "stream" (one pass) or "parts" (original consumed from its end).
    strategy: &'static str,
    matched: OutputInfo,
    rest: Option<OutputInfo>,
    original_removed: bool,
    stats: Stats,
    framing: String,
    elapsed_ms: u64,
}

fn open_original(path: &Path, exclusive: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    if exclusive {
        options.write(true);
        // Nobody may append to or map the file while it is cut.
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 0);
    }
    options.open(path).map_err(|e| {
        if e.raw_os_error() == Some(32) || e.raw_os_error() == Some(1224) {
            format!("{} está aberto em outro programa ou carregado nesta análise. Feche-o (ou remova-o das fontes) para separar removendo o original.", path.display())
        } else {
            format!("Não foi possível abrir {}: {e}", path.display())
        }
    })
}

fn check_outputs(input: &Path, matched: &Path, rest: Option<&Path>) -> Result<(), String> {
    let canon = |p: &Path| fs::canonicalize(p).ok();
    for output in std::iter::once(matched).chain(rest) {
        if output == input || (output.exists() && canon(output) == canon(input)) {
            return Err("O arquivo de saída não pode ser o próprio arquivo original.".into());
        }
        if output.is_dir() {
            return Err(format!("{} é uma pasta.", output.display()));
        }
    }
    if rest.is_some_and(|r| r == matched) {
        return Err("Escolha nomes diferentes para o arquivo filtrado e o restante.".into());
    }
    Ok(())
}

fn written(written: outputs::Written) -> (OutputInfo, Option<OutputInfo>) {
    let info = |(path, bytes): (PathBuf, u64)| OutputInfo { path: path.to_string_lossy().into_owned(), bytes };
    (info(written.matched), written.rest.map(info))
}

fn run_stream(prep: &Prepared, matched: &Path, rest: Option<&Path>, started: Instant) -> Result<SplitResult, String> {
    let bom = {
        let mut bom = vec![0; prep.codec.bom];
        stream::read_at(&mut prep.handle()?, 0, &mut bom)?;
        bom
    };
    let mut sink = outputs::Outputs::create(matched, rest, &bom, prep.handle()?)?;
    let mut meter = Meter::new("split-stream", "Separando", prep.start(), prep.len);
    let stats = stream::run(prep, &mut sink, &mut meter)?;
    crate::operations::check()?;
    // Past this point the separation completes even if cancelled.
    crate::operations::commit();
    let (matched, rest) = written(sink.finish()?);
    Ok(SplitResult { strategy: "stream", matched, rest, original_removed: false, stats, framing: prep.framing_note.clone(), elapsed_ms: started.elapsed().as_millis() as u64 })
}

fn finish_parts(journal: &mut Journal, dir: &Path, started: Instant) -> Result<SplitResult, String> {
    lowspace::assemble(journal, dir)?;
    let size = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    Ok(SplitResult {
        strategy: "parts",
        matched: OutputInfo { path: journal.matched.to_string_lossy().into_owned(), bytes: size(&journal.matched) },
        rest: Some(OutputInfo { path: journal.rest.to_string_lossy().into_owned(), bytes: size(&journal.rest) }),
        original_removed: true,
        stats: journal.stats,
        framing: journal.framing.clone(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Carving, then assembling; a cancellation while carving puts the original back.
fn carve_and_finish(journal: &mut Journal, dir: &Path, mut original: File, started: Instant) -> Result<SplitResult, String> {
    let carved = lowspace::carve(journal, dir, &mut original);
    drop(original);
    match carved {
        Ok(()) => {
            crate::operations::commit();
            finish_parts(journal, dir, started)
        }
        Err(stop) => {
            crate::operations::commit();
            let reason = match stop {
                Stop::Cancelled => "Separação cancelada".to_string(),
                Stop::Failed(error) => error,
            };
            match lowspace::undo(journal, dir) {
                Ok(()) => Err(format!("{reason}. O arquivo original foi restaurado.")),
                Err(error) => Err(format!("{reason}. Não foi possível restaurar o original agora ({error}); abra Separar logs para tentar de novo.")),
            }
        }
    }
}

fn run_parts(prep: Prepared, matched: &Path, rest: &Path, dir: &Path, started: Instant) -> Result<SplitResult, String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let work = outputs::parent(&prep.path).join(format!(".loginsight-separacao-{}", &id[..12]));
    fs::create_dir_all(&work).map_err(|e| format!("Não foi possível criar a pasta de trabalho {}: {e}", work.display()))?;
    let mut bom = vec![0; prep.codec.bom];
    stream::read_at(&mut prep.handle()?, 0, &mut bom)?;
    let mut journal = Journal {
        version: 1, id, original: prep.path.clone(), matched: matched.to_path_buf(), rest: rest.to_path_buf(), work,
        bom, original_len: prep.len, phase: Phase::Decide, cut: prep.len, head_end: prep.len, head_matched: false,
        chunks: Vec::new(), renamed: false, assembled: Default::default(), matched_bytes: 0, rest_bytes: 0,
        stats: Stats::default(), framing: prep.framing_note.clone(), started_ms: chrono::Utc::now().timestamp_millis(),
    };
    let decided = (|| {
        journal.save(dir)?;
        let mut sink = MapSink::create(&journal.map_path())?;
        let mut meter = Meter::new("split-decide", "Analisando o arquivo (nada é alterado nesta etapa)", prep.start(), prep.len);
        let stats = stream::run(&prep, &mut sink, &mut meter)?;
        crate::operations::check()?;
        journal.stats = stats;
        journal.matched_bytes = sink.matched;
        journal.rest_bytes = sink.rest;
        sink.finish()?;
        lowspace::locate_head(&mut journal)?;
        journal.phase = Phase::Carve;
        journal.save(dir)
    })();
    if let Err(error) = decided {
        journal.discard(dir);
        return Err(error);
    }
    let Prepared { file, .. } = prep;
    carve_and_finish(&mut journal, dir, file, started)
}

/// `force_parts` works in parts even with room to spare (tests).
fn run_impl(token: &str, spec: &SplitSpec, dir: &Path, force_parts: bool) -> Result<SplitResult, String> {
    let started = Instant::now();
    let grant = grant(token)?;
    let _busy = Busy::claim(&grant.input)?;
    let (matched, rest) = outputs_of(&grant);
    let rest = (spec.mode != Mode::Matched).then_some(rest);
    check_outputs(&grant.input, &matched, rest.as_deref())?;
    let exclusive = spec.mode == Mode::Move;
    let file = open_original(&grant.input, exclusive)?;
    let prep = stream::prepare(&grant.input, file, spec)?;
    let need = prep.len + prep.codec.bom as u64;
    let free = outputs::free_space(&outputs::parent(&matched));
    match spec.mode {
        Mode::Matched => run_stream(&prep, &matched, None, started),
        Mode::Both => {
            let rest = rest.unwrap();
            if same_volume(&matched, &rest) && free.is_some_and(|f| f < need + outputs::RESERVE) {
                return Err(format!(
                    "Espaço insuficiente: os dois arquivos somam cerca de {} e há {} livres. Escolha outra pasta ou use “remover o original”, que separa em partes sem duplicar o volume.",
                    outputs::mib(need),
                    outputs::mib(free.unwrap_or(0))
                ));
            }
            run_stream(&prep, &matched, Some(&rest), started)
        }
        Mode::Move => {
            let rest = rest.unwrap();
            let local = same_volume(&grant.input, &matched) && same_volume(&grant.input, &rest);
            let roomy = !force_parts && free.is_some_and(|f| f >= need + outputs::RESERVE);
            if roomy || !local {
                if !roomy {
                    return Err(format!(
                        "Espaço insuficiente na pasta de destino ({} livres para cerca de {}). Para separar sem espaço extra, salve os dois arquivos no mesmo disco do original.",
                        outputs::mib(free.unwrap_or(0)),
                        outputs::mib(need)
                    ));
                }
                // Removing needs no handle of ours on the file.
                let result = run_stream(&prep, &matched, Some(&rest), started)?;
                drop(prep);
                fs::remove_file(&grant.input).map_err(|e| format!("Os arquivos foram gerados, mas não foi possível remover o original: {e}"))?;
                return Ok(SplitResult { original_removed: true, ..result });
            }
            run_parts(prep, &matched, &rest, dir, started)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    id: String,
    original: String,
    matched: String,
    rest: String,
    phase: Phase,
    original_len: u64,
    carved: u64,
    started_ms: i64,
    can_undo: bool,
}

fn pending_impl(dir: &Path) -> Vec<Pending> {
    let busy: HashSet<PathBuf> = BUSY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut list = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(journal) = Journal::load(&path) else { continue };
        let key = fs::canonicalize(&journal.original).unwrap_or_else(|_| journal.original.clone());
        if busy.contains(&key) || busy.contains(&journal.original) {
            continue;
        }
        if journal.phase == Phase::Decide {
            // Interrupted before anything was changed.
            journal.discard(dir);
            continue;
        }
        list.push(Pending {
            id: journal.id.clone(),
            original: journal.original.to_string_lossy().into_owned(),
            matched: journal.matched.to_string_lossy().into_owned(),
            rest: journal.rest.to_string_lossy().into_owned(),
            phase: journal.phase,
            original_len: journal.original_len,
            carved: journal.carved(),
            started_ms: journal.started_ms,
            can_undo: journal.phase == Phase::Carve,
        });
    }
    list.sort_by_key(|p| p.started_ms);
    list
}

fn recover_impl(dir: &Path, id: &str, undo: bool) -> Result<Option<SplitResult>, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Separação desconhecida.".into());
    }
    let started = Instant::now();
    let mut journal = Journal::load(&Journal::path(dir, id))?;
    let _busy = Busy::claim(&journal.original)?;
    match (journal.phase, undo) {
        (Phase::Carve, true) => {
            crate::operations::commit();
            lowspace::undo(&mut journal, dir).map(|_| None)
        }
        (Phase::Carve, false) => {
            let original = open_original(&journal.original, true)?;
            carve_and_finish(&mut journal, dir, original, started).map(Some)
        }
        (Phase::Assemble, _) => {
            crate::operations::commit();
            finish_parts(&mut journal, dir, started).map(Some)
        }
        (Phase::Decide, _) => {
            journal.discard(dir);
            Ok(None)
        }
    }
}

fn pick_input(app: &tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    let mut picker = app.dialog().file().set_title("Escolher o log a separar");
    if let Some(window) = app.get_webview_window("main") {
        picker = picker.set_parent(&window);
    }
    picker.blocking_pick_file().map(|p| p.into_path().map_err(|_| "Escolha um arquivo local.".to_string())).transpose()
}

/// Progress of the work below as `operation-progress` events.
fn reporting<T>(app: tauri::AppHandle, f: impl FnOnce() -> T) -> T {
    crate::operations::with_reporter(std::sync::Arc::new(move |progress: crate::operations::Progress| {
        let _ = app.emit("operation-progress", progress);
    }), f)
}

#[tauri::command]
pub async fn split_choose_input(app: tauri::AppHandle) -> Result<Option<Inspection>, String> {
    crate::offload(move || {
        let Some(path) = pick_input(&app)? else { return Ok(None) };
        if !path.is_file() {
            return Err("Escolha um arquivo.".into());
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        GRANTS.lock().unwrap_or_else(|e| e.into_inner()).insert(token.clone(), Grant { input: path, matched: None, rest: None });
        inspect(&token).map(Some)
    })
    .await?
}

#[tauri::command]
pub async fn split_inspect(token: String) -> Result<Inspection, String> {
    crate::offload(move || inspect(&token)).await?
}

#[tauri::command]
pub async fn split_choose_output(app: tauri::AppHandle, token: String, side: String) -> Result<Option<Inspection>, String> {
    crate::offload(move || {
        let current = grant(&token)?;
        let matched_side = match side.as_str() {
            "matched" => true,
            "rest" => false,
            _ => return Err("Saída desconhecida.".into()),
        };
        let (matched, rest) = outputs_of(&current);
        let suggested = if matched_side { matched } else { rest };
        let mut picker = app
            .dialog()
            .file()
            .set_title(if matched_side { "Salvar os itens filtrados" } else { "Salvar o restante" })
            .set_directory(outputs::parent(&suggested))
            .set_file_name(suggested.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        if let Some(window) = app.get_webview_window("main") {
            picker = picker.set_parent(&window);
        }
        let Some(chosen) = picker.blocking_save_file() else { return Ok(None) };
        let chosen = chosen.into_path().map_err(|_| "Escolha um arquivo local.".to_string())?;
        check_outputs(&current.input, &chosen, None)?;
        let mut grants = GRANTS.lock().unwrap_or_else(|e| e.into_inner());
        let entry = grants.get_mut(&token).ok_or("Escolha o arquivo novamente.")?;
        if matched_side { entry.matched = Some(chosen) } else { entry.rest = Some(chosen) }
        drop(grants);
        inspect(&token).map(Some)
    })
    .await?
}

#[tauri::command]
pub async fn split_preview(app: tauri::AppHandle, token: String, spec: SplitSpec, operation_id: Option<String>) -> Result<stream::Preview, String> {
    crate::offload_operation(operation_id, move || reporting(app, || {
        let grant = grant(&token)?;
        let file = open_original(&grant.input, false)?;
        let prep = stream::prepare(&grant.input, file, &spec)?;
        stream::preview(&prep)
    }))
    .await?
}

#[tauri::command]
pub async fn split_run(app: tauri::AppHandle, token: String, spec: SplitSpec, operation_id: Option<String>) -> Result<SplitResult, String> {
    crate::offload_operation(operation_id, move || reporting(app, || run_impl(&token, &spec, &journal_dir(), false))).await?
}

#[tauri::command]
pub async fn split_pending() -> Result<Vec<Pending>, String> {
    crate::offload(|| pending_impl(&journal_dir())).await
}

#[tauri::command]
pub async fn split_recover(app: tauri::AppHandle, id: String, undo: bool, operation_id: Option<String>) -> Result<Option<SplitResult>, String> {
    crate::offload_operation(operation_id, move || reporting(app, || recover_impl(&journal_dir(), &id, undo))).await?
}

#[cfg(test)]
mod tests;
