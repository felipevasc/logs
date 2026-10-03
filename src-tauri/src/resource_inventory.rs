//! Bounded component accounting. These owned-allocation estimates are a partial
//! inventory, never an alternative measurement of the process working set/RSS.
use crate::{model::Event, AppState, SourceData};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    pub label: String,
    pub memory_bytes: Option<u64>,
    pub mapped_bytes: Option<u64>,
    pub storage_bytes: Option<u64>,
    pub accounted_bytes: Option<u64>,
    pub budget_bytes: Option<u64>,
    pub items: Option<u64>,
    /// measured: known capacities; estimated: sampled/partial owned allocations;
    /// logical: file/map lengths; accounted: credits, not RAM;
    /// configured: limit, not usage; unavailable: no inexpensive current value.
    pub basis: &'static str,
    pub note: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub memory_known_bytes: u64,
    pub partial: bool,
    pub components: Vec<Component>,
}

fn component(
    id: &str,
    label: &str,
    memory_bytes: Option<u64>,
    items: Option<u64>,
    basis: &'static str,
    note: String,
) -> Component {
    Component {
        id: id.into(),
        label: label.into(),
        memory_bytes,
        mapped_bytes: None,
        storage_bytes: None,
        accounted_bytes: None,
        budget_bytes: None,
        items,
        basis,
        note,
    }
}

fn unavailable(id: &str, label: &str, note: &str) -> Component {
    component(id, label, None, None, "unavailable", note.into())
}

fn vec_bytes<T>(capacity: usize) -> u64 {
    (capacity as u64).saturating_mul(std::mem::size_of::<T>() as u64)
}

fn strings_bytes(strings: &[String], capacity: usize, remaining: &mut usize, partial: &mut bool) -> u64 {
    let count = strings.len().min(*remaining);
    *remaining -= count;
    *partial |= count < strings.len();
    strings.iter().take(count).fold(vec_bytes::<String>(capacity), |bytes, value| {
        bytes.saturating_add(value.capacity() as u64)
    })
}

fn accounted(id: &str, label: &str, bytes: u64, limit: Option<u64>, note: &str) -> Component {
    let mut entry = component(id, label, None, None, "accounted", note.into());
    entry.accounted_bytes = Some(bytes);
    entry.budget_bytes = limit;
    entry
}

// A pathological single nested field must not turn a 1 Hz inventory into a
// corpus traversal. String capacities are O(1); maps/arrays have a node budget.
struct EventSample {
    remaining: usize,
    truncated: bool,
}
impl EventSample {
    fn value_heap(&mut self, value: &serde_json::Value, depth: usize) -> u64 {
        if self.remaining == 0 || depth > 8 {
            self.truncated = true;
            return 0;
        }
        self.remaining -= 1;
        match value {
            serde_json::Value::String(value) => value.capacity() as u64,
            serde_json::Value::Array(values) => {
                let mut bytes = vec_bytes::<serde_json::Value>(values.capacity());
                for value in values {
                    if self.remaining == 0 {
                        self.truncated = true;
                        break;
                    }
                    bytes = bytes.saturating_add(self.value_heap(value, depth + 1));
                }
                bytes
            }
            serde_json::Value::Object(values) => self.map_heap(values, depth + 1),
            _ => 0,
        }
    }
    fn map_heap(
        &mut self,
        values: &serde_json::Map<String, serde_json::Value>,
        depth: usize,
    ) -> u64 {
        // serde_json's map implementation and allocator overhead vary. This
        // is an explicit approximation, not an allocator introspection claim.
        let per_entry = (std::mem::size_of::<(String, serde_json::Value)>() + 32) as u64;
        let mut bytes = (values.len() as u64).saturating_mul(per_entry);
        for (key, value) in values {
            if self.remaining == 0 {
                self.truncated = true;
                break;
            }
            bytes = bytes
                .saturating_add(key.capacity() as u64)
                .saturating_add(self.value_heap(value, depth));
        }
        bytes
    }
    fn event_heap(&mut self, event: &Event) -> u64 {
        let text = [
            &event.event_ref,
            &event.parse_status,
            &event.source,
            &event.level,
            &event.code,
            &event.name,
            &event.description,
            &event.message,
            &event.raw,
        ];
        let bytes = text.iter().fold(0u64, |sum, value| {
            sum.saturating_add(value.capacity() as u64)
        });
        bytes.saturating_add(self.map_heap(&event.fields, 0))
    }
}

fn memory_events_estimate(events: &[Event], capacity: usize) -> (u64, usize, bool) {
    let count = events.len().min(64);
    let mut dynamic = 0u64;
    let mut truncated = false;
    for n in 0..count {
        let position = if count <= 1 {
            0
        } else {
            ((n as u128 * (events.len() - 1) as u128) / (count - 1) as u128) as usize
        };
        let mut sample = EventSample {
            remaining: 512,
            truncated: false,
        };
        dynamic = dynamic.saturating_add(sample.event_heap(&events[position]));
        truncated |= sample.truncated;
    }
    let estimated_dynamic = if count == 0 {
        0
    } else {
        dynamic.saturating_mul(events.len() as u64) / count as u64
    };
    (
        vec_bytes::<Event>(capacity).saturating_add(estimated_dynamic),
        count,
        truncated,
    )
}

/// All locks are attempted once. Busy operations produce unavailable entries;
/// the monitor never queues behind ingestion, index preparation or a query.
pub fn snapshot(state: &AppState) -> Inventory {
    let mut components = Vec::new();
    let mut partial = false;
    match state.source.try_read() {
        None => {
            partial = true;
            components.push(unavailable("source", "Fonte atual", "Fonte ocupada; a coleta não aguardou o lock."));
        }
        Some(source) => match &*source {
            SourceData::None => components.push(component("source", "Fonte atual", Some(0), Some(0), "measured", "Nenhuma fonte carregada.".into())),
            SourceData::Memory(events) => {
                let (bytes, samples, clipped) = memory_events_estimate(events, events.capacity());
                partial |= clipped;
                components.push(component("source-events", "Eventos da fonte em memória", Some(bytes), Some(events.len() as u64), "estimated",
                    format!("Capacidade do vetor + conteúdo estimado por até 64 eventos distribuídos ({samples} amostras); overhead de campos aproximado.{}", if clipped { " Campos complexos excederam o limite da amostra." } else { "" })));
            }
            SourceData::Indexed(index) => {
                let metadata = index.lines.resource_usage();
                let mut clipped = metadata.partial || index.parts.len() > 1024;
                let mut text_budget = 4096;
                let mut bytes = (std::mem::size_of::<crate::sources::FileIndex>() as u64)
                    .saturating_add(metadata.heap_bytes)
                    .saturating_add(vec_bytes::<crate::sources::FilePart>(index.parts.capacity()))
                    .saturating_add(strings_bytes(&index.columns, index.columns.capacity(), &mut text_budget, &mut clipped));
                if let Some(order) = index.time_order.get() {
                    bytes = bytes.saturating_add(vec_bytes::<usize>(order.capacity()));
                }
                let mut mapped = 0u64;
                let mut maps = HashSet::new();
                for part in index.parts.iter().take(1024) {
                    if maps.insert(Arc::as_ptr(&part.mmap) as usize) { mapped = mapped.saturating_add(part.mmap.len() as u64); }
                    for value in [&part.path, &part.physical_path, &part.file_name, &part.format, &part.identity, &part.metadata_identity] {
                        bytes = bytes.saturating_add(value.capacity() as u64);
                    }
                    bytes = bytes.saturating_add(part.event_identity.as_ref().map_or(0, |value| value.capacity() as u64));
                    bytes = bytes.saturating_add(strings_bytes(&part.header, part.header.capacity(), &mut text_budget, &mut clipped));
                }
                partial |= clipped;
                let mut rows = component("source-index", "Metadados de linhas e ordenação", Some(bytes), Some(index.lines.len() as u64), "estimated",
                    format!("Capacidades de descritores e metadados próprios; mapas de journals/checkpoints separados do heap. Arcs compartilhados contados uma vez por fonte. Não lê linhas nem inicializa a ordenação; exclui parsers e regex.{}", if clipped { " Limite de 1.024 descritores ou 4.096 textos atingido; subtotal parcial." } else { "" }));
                rows.mapped_bytes = Some(metadata.mapped_bytes);
                components.push(rows);
                let mut files = component("source-maps", "Arquivos mapeados da fonte", None, Some(index.parts.len() as u64), "logical",
                    "Extensão lógica dos mapas mmap observados (até 1.024 partes); páginas podem ser compartilhadas. Não representa RAM residente nem cache em disco do aplicativo.".into());
                files.mapped_bytes = Some(mapped);
                components.push(files);
            }
        },
    }
    let work = crate::case_work_budget::global();
    components.push(accounted("work-credits", "Trabalho analítico em uso", work.used() as u64, Some(work.base_limits().live as u64),
        "Créditos vivos de payloads/scratch admitidos no processo; inclui trabalho de Casos e fontes. Não é medição de RAM, nem reserva física; excluído do subtotal de heap."));
    let cases = crate::case_resources::resource_snapshot();
    partial |= cases.partial;
    components.push(accounted("selection-credits", "IDs de seleções em uso", cases.selection_bytes, Some(cases.selection_limit),
        "Contabilidade global de IDs lógicos; podem estar em tabelas em disco. Inclui créditos compartilhados por Casos; não somar ao detalhamento por Caso ou ao cache do motor."));
    for (index, owner) in cases.owners.iter().enumerate() {
        components.push(accounted(&format!("case-work-{index}"), &format!("Caso {} · análise {}", owner.case_id, owner.analysis_id), owner.accounted_bytes, None,
            "Créditos de trabalho e seleções desta identidade, compartilhados entre revisões. Já incluídos nos totais globais. A cota depende da política capturada por cada operação; não é RAM."));
    }
    if cases.partial { components.push(unavailable("case-owners", "Detalhamento dos Casos", "Registro ocupado ou mais de 128 identidades; detalhamento parcial, totais globais continuam disponíveis.")); }
    match crate::engine::resource_snapshot() {
        Some(engine) => {
            partial |= engine.partial;
            let mut entry = accounted("engine-selections", "Cache de seleções DuckDB", engine.selection_bytes, None,
                "IDs lógicos das seleções nas sessões registradas; não é heap. Sobrepõe a contabilidade global de seleções. Sessões retiradas ainda usadas por consultas não entram neste detalhe. Não executa SQL.");
            entry.items = Some(engine.selections as u64);
            if engine.partial { entry.note.push_str(" Cache ocupado ou limite de entradas atingido: subtotal parcial."); }
            components.push(entry);
            components.push(component("engine-readers", "Motor DuckDB + Tantivy", None, Some(engine.sessions as u64), "unavailable",
                format!("{} sessões registradas, {} leitores de texto e {} preparações ativas. RAM interna e spill não são consultados por este coletor; seu consumo está nos processos do aplicativo.", engine.sessions, engine.text_readers, engine.building)));
        }
        None => { partial = true; components.push(unavailable("engine-readers", "Motor DuckDB + Tantivy", "Registro ocupado; a coleta não aguardou o lock.")); }
    }
    for (id, label, bytes, note) in [
        ("duckdb-budget", "Limite DuckDB por instância", crate::resources::duckdb_memory_mb().saturating_mul(1 << 20), "Limite configurado por instância DuckDB; não significa uso atual ou limite total de RSS."),
        ("text-budget", "Orçamento do escritor Tantivy", crate::resources::text_memory_bytes() as u64, "Orçamento configurado para cada escritor de texto; não representa memória ocupada pelos leitores."),
    ] {
        let mut entry = component(id, label, None, None, "configured", note.into());
        entry.budget_bytes = Some(bytes);
        components.push(entry);
    }
    match crate::global_scheduler::resource_snapshot() {
        Some(scheduler) => components.push(component("scheduler", "Agendador global", None, Some(scheduler.reserved as u64), "accounted",
            format!("{} de {} vagas de trabalho reservadas; {} aguardando admissão; pico {}. Vagas não são threads do SO nem uso de CPU. I/O pode ceder a vaga enquanto aguarda.", scheduler.reserved, scheduler.limit, scheduler.queued, scheduler.peak))),
        None => { partial = true; components.push(unavailable("scheduler", "Agendador global", "Agendador ocupado; a coleta não aguardou o lock.")); }
    }
    match crate::detections::resource_cache_metrics() {
        Some((entries, bytes)) => {
            components.push(component("security-cache", "Resultados de segurança", Some(bytes), Some(entries as u64), "estimated",
                "Subtotal conhecido de descritores/chaves. Resultados completos ficam em SQLite temporário; RAM interna e arquivo temporário sem contador rápido não estão incluídos. Não consulta conexões nem confunde orçamento com uso.".into()));
        }
        None => { partial = true; components.push(unavailable("security-cache", "Resultados de segurança", "Cache ocupado; coleta não aguardou o lock.")); }
    }
    partial |= components.iter().any(|component| component.basis == "unavailable");
    let memory_known_bytes = components.iter().filter_map(|component| component.memory_bytes).fold(0u64, u64::saturating_add);
    Inventory { memory_known_bytes, partial, components }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePath {
    pub path: String,
    pub bytes: u64,
    pub files: u64,
    pub partial: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSnapshot {
    pub root: String,
    pub bytes: u64,
    pub files: u64,
    pub entries: u64,
    pub sampled_at: u64,
    pub elapsed_ms: u64,
    pub partial: bool,
    pub stale: bool,
    pub paths: Vec<StoragePath>,
    pub note: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

impl StorageSnapshot {
    pub fn refresh_staleness(&mut self) {
        self.refresh_staleness_at(now_ms());
    }

    fn refresh_staleness_at(&mut self, now: u64) {
        self.stale = now < self.sampled_at || now.saturating_sub(self.sampled_at) > 60_000;
    }
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points must not escape the data root.
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

/// Separate, bounded background scan, intended for a 30-second sampler. It
/// measures logical file lengths under the app data root, not allocated blocks.
/// Never add this total to active-index storage: those are overlapping views.
pub fn storage_snapshot(root: &Path) -> StorageSnapshot {
    const MAX_ENTRIES: u64 = 20_000;
    const MAX_DEPTH: usize = 32;
    const MAX_TIME: Duration = Duration::from_millis(500);
    let started = Instant::now();
    let mut snapshot = StorageSnapshot { root: root.to_string_lossy().into_owned(), bytes: 0, files: 0, entries: 0,
        sampled_at: now_ms(), elapsed_ms: 0, partial: false, stale: false, paths: Vec::new(),
        note: "Tamanho lógico dos arquivos sob a pasta de dados do aplicativo; exclui fontes externas, temporários SQLite/SO fora da pasta e espaço alocado do volume. Limite de 20 mil entradas, 32 níveis e 500 ms; links e reparse points não são seguidos. Total sobrepõe os índices ativos e não deve ser somado a eles.".into() };
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        snapshot.partial = true;
        snapshot.note.push_str(" Pasta indisponível na coleta.");
        return snapshot;
    };
    if !metadata.is_dir() || is_link(&metadata) {
        snapshot.partial = true;
        snapshot
            .note
            .push_str(" A raiz não é um diretório regular.");
        return snapshot;
    }
    let mut groups: BTreeMap<PathBuf, StoragePath> = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), 0usize, None::<PathBuf>)];
    'scan: while let Some((directory, depth, group)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                snapshot.partial = true;
                if let Some(path) = &group {
                    if let Some(row) = groups.get_mut(path) {
                        row.partial = true;
                    }
                }
                continue;
            }
        };
        for entry in entries {
            if snapshot.entries >= MAX_ENTRIES || started.elapsed() >= MAX_TIME {
                snapshot.partial = true;
                break 'scan;
            }
            snapshot.entries += 1;
            let Ok(entry) = entry else {
                snapshot.partial = true;
                continue;
            };
            let path = entry.path();
            let top = group.clone().unwrap_or_else(|| path.clone());
            let row = groups.entry(top.clone()).or_insert_with(|| StoragePath {
                path: top.to_string_lossy().into_owned(),
                bytes: 0,
                files: 0,
                partial: false,
            });
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    row.partial = true;
                    snapshot.partial = true;
                    continue;
                }
            };
            if is_link(&metadata) {
                row.partial = true;
                snapshot.partial = true;
                continue;
            }
            if metadata.is_dir() {
                if depth + 1 >= MAX_DEPTH {
                    row.partial = true;
                    snapshot.partial = true;
                    continue;
                }
                pending.push((path, depth + 1, Some(top)));
            } else if metadata.is_file() {
                snapshot.bytes = snapshot.bytes.saturating_add(metadata.len());
                snapshot.files += 1;
                row.bytes = row.bytes.saturating_add(metadata.len());
                row.files += 1;
            }
        }
    }
    if snapshot.partial {
        for row in groups.values_mut() {
            row.partial = true;
        }
    }
    snapshot.paths = groups.into_values().collect();
    if snapshot.paths.len() > 256 {
        snapshot
            .paths
            .sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
        snapshot.paths.truncate(256);
        snapshot.partial = true;
        snapshot.note.push_str(" Lista de caminhos limitada aos 256 maiores; o total inclui os demais caminhos visitados.");
    }
    snapshot.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounted_credits_and_configured_limits_never_enter_heap_totals() {
        let inventory = snapshot(&state());
        let credits: Vec<_> = inventory.components.iter().filter(|entry| entry.accounted_bytes.is_some() || entry.budget_bytes.is_some()).collect();
        assert!(credits.len() >= 4);
        assert!(credits.iter().all(|entry| entry.memory_bytes.is_none()));
        assert_eq!(inventory.memory_known_bytes, inventory.components.iter().filter_map(|entry| entry.memory_bytes).sum::<u64>());
        assert!(!inventory.components.iter().any(|entry| matches!(entry.id.as_str(), "big-data" | "match-cache" | "case-records")));
    }

    fn state() -> AppState {
        AppState {
            source: parking_lot::RwLock::new(SourceData::None),
            source_publication: parking_lot::RwLock::new(Default::default()),
            source_names: parking_lot::RwLock::new(vec![]),
            codes: parking_lot::RwLock::new(Default::default()),
            system_codes: parking_lot::RwLock::new(Default::default()),
            derived: parking_lot::RwLock::new(vec![]),
            case_store_lock: parking_lot::Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        }
    }

    #[test]
    fn inventory_does_not_wait_for_busy_source_or_initialize_time_order() {
        let state = state();
        let guard = state.source.write();
        let busy = snapshot(&state);
        assert!(busy.partial);
        let source = busy
            .components
            .iter()
            .find(|component| component.id == "source")
            .unwrap();
        assert_eq!(source.basis, "unavailable");
        assert_eq!(source.memory_bytes, None);
        drop(guard);
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("events.jsonl");
        std::fs::write(&log, "{\"message\":\"one\"}\n").unwrap();
        *state.source.write() = SourceData::Indexed(
            crate::sources::index_file(log.to_str().unwrap(), "jsonl", None, None, None).unwrap(),
        );
        let inventory = snapshot(&state);
        assert!(inventory.memory_known_bytes > 0);
        let source = state.source.read();
        let SourceData::Indexed(index) = &*source else {
            panic!("indexed source expected")
        };
        assert!(index.time_order.get().is_none());
        let maps = inventory
            .components
            .iter()
            .find(|component| component.id == "source-maps")
            .unwrap();
        assert_eq!(
            maps.mapped_bytes,
            Some(std::fs::metadata(log).unwrap().len())
        );
        assert_eq!(maps.memory_bytes, None);
    }

    #[test]
    fn event_sampling_is_bounded_and_nested_fields_are_explicitly_partial() {
        let mut events = Vec::with_capacity(1024);
        events.resize_with(1024, Event::empty);
        let (bytes, samples, clipped) = memory_events_estimate(&events, events.capacity());
        assert_eq!(samples, 64);
        assert!(!clipped);
        assert!(bytes >= vec_bytes::<Event>(events.capacity()));
        let mut value = serde_json::json!("deep");
        for _ in 0..20 {
            value = serde_json::json!({"child": value});
        }
        events[0].fields.insert("nested".into(), value);
        assert!(memory_events_estimate(&events, events.capacity()).2);
    }

    #[test]
    fn storage_inventory_counts_logical_lengths_and_does_not_double_sum_views() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("index"), b"12345").unwrap();
        std::fs::write(directory.path().join("settings"), b"123").unwrap();
        let storage = storage_snapshot(directory.path());
        assert!(!storage.partial);
        assert!(!storage.stale);
        assert_eq!(storage.bytes, 8);
        assert_eq!(storage.files, 2);
        assert_eq!(storage.paths.iter().map(|path| path.bytes).sum::<u64>(), 8);
    }

    #[test]
    fn storage_staleness_handles_a_host_clock_correction() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = storage_snapshot(directory.path());
        storage.sampled_at = 100_000;
        storage.refresh_staleness_at(99_999);
        assert!(storage.stale);
        storage.refresh_staleness_at(100_000);
        assert!(!storage.stale);
        storage.refresh_staleness_at(160_001);
        assert!(storage.stale);
    }

    #[cfg(unix)]
    #[test]
    fn storage_inventory_never_follows_directory_links() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("external"), b"outside").unwrap();
        std::fs::write(directory.path().join("internal"), b"ok").unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.path().join("link")).unwrap();
        let storage = storage_snapshot(directory.path());
        assert!(storage.partial);
        assert_eq!(storage.bytes, 2);
        assert_eq!(storage.files, 1);
    }
}
