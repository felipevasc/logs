//! Continuous, bounded telemetry. The UI reads snapshots; opening the modal
//! never starts another collector or scans the logs.
use crate::{resource_actions, resource_inventory, resource_system, AppState};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};

const INTERVAL_MS: u64 = 1000;
const HISTORY_LIMIT: usize = 900;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySample {
    pub timestamp_ms: u64,
    pub app_cpu_percent: Option<f64>,
    pub host_cpu_percent: Option<f64>,
    pub app_memory_bytes: Option<u64>,
    pub host_memory_used_bytes: u64,
    pub read_bytes_per_sec: Option<f64>,
    pub written_bytes_per_sec: Option<f64>,
    pub active_operations: usize,
}

#[derive(Default)]
struct Store {
    latest: Option<resource_system::SystemSnapshot>,
    inventory: Option<resource_inventory::Inventory>,
    storage: Option<resource_inventory::StorageSnapshot>,
    history: VecDeque<HistorySample>,
}

impl Store {
    fn push(&mut self, sample: HistorySample) {
        // A host clock correction must not leave future/out-of-order points
        // inside the current window. Rates use Instant and remain unaffected.
        if self.history.back().is_some_and(|last| last.timestamp_ms > sample.timestamp_ms) {
            self.history.clear();
        }
        // Time and count are both bounded: a delayed sample never turns the
        // "15 minutes" view into old, misleading session data.
        let cutoff = sample
            .timestamp_ms
            .saturating_sub(HISTORY_LIMIT as u64 * INTERVAL_MS);
        while self
            .history
            .front()
            .is_some_and(|old| old.timestamp_ms < cutoff)
            || self.history.len() >= HISTORY_LIMIT
        {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }
}

pub struct ResourceMonitor {
    store: Arc<Mutex<Store>>,
    stop: Arc<AtomicBool>,
    started: AtomicBool,
    started_at_ms: u64,
}

impl ResourceMonitor {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(Store::default())),
            stop: Arc::new(AtomicBool::new(false)),
            started: AtomicBool::new(false),
            started_at_ms: resource_system::unix_ms(),
        }
    }

    pub fn start(&self, app: AppHandle) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let store = self.store.clone();
        let stop = self.stop.clone();
        let _ = std::thread::Builder::new()
            .name("loginsight-resources".into())
            .spawn(move || {
                let _action = resource_actions::begin("Monitoramento de recursos");
                let mut sampler = resource_system::SystemSampler::new(crate::config_dir());
                while !stop.load(Ordering::Acquire) {
                    let started = Instant::now();
                    let sample = sampler.sample();
                    let inventory = resource_inventory::snapshot(app.state::<AppState>().inner());
                    let active_operations = resource_actions::active_count();
                    let point = HistorySample {
                        timestamp_ms: sample.sampled_at_ms,
                        app_cpu_percent: sample.app.cpu_percent,
                        host_cpu_percent: sample.host.cpu_percent,
                        app_memory_bytes: sample.app.resident_bytes,
                        host_memory_used_bytes: sample.host.used_memory_bytes,
                        read_bytes_per_sec: sample.app.read_bytes_per_sec,
                        written_bytes_per_sec: sample.app.written_bytes_per_sec,
                        active_operations,
                    };
                    {
                        let mut data = store.lock();
                        data.push(point);
                        data.latest = Some(sample);
                        data.inventory = Some(inventory);
                    }
                    std::thread::sleep(
                        Duration::from_millis(INTERVAL_MS).saturating_sub(started.elapsed()),
                    );
                }
            });
        // Directory walking is deliberately separate from the 1 Hz sampler.
        let store = self.store.clone();
        let stop = self.stop.clone();
        let _ = std::thread::Builder::new()
            .name("loginsight-storage-accounting".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    {
                        let _action =
                            resource_actions::begin("Inventário de arquivos do aplicativo");
                        let storage = resource_inventory::storage_snapshot(&crate::config_dir());
                        store.lock().storage = Some(storage);
                    }
                    for _ in 0..30 {
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            });
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn snapshot(&self) -> Result<MonitorSnapshot, String> {
        let (latest, inventory, mut storage, history) = {
            let store = self.store.lock();
            (
                store.latest.clone(),
                store.inventory.clone(),
                store.storage.clone(),
                store.history.iter().cloned().collect(),
            )
        };
        let latest = latest.ok_or("Primeira coleta de recursos em andamento.")?;
        if let Some(storage) = &mut storage {
            storage.refresh_staleness();
        }
        Ok(MonitorSnapshot {
            sample_interval_ms: INTERVAL_MS, history_limit: HISTORY_LIMIT, started_at_ms: self.started_at_ms,
            system: latest, inventory, storage, history, actions: resource_actions::snapshot(),
            notes: vec![
                "CPU do aplicativo inclui o backend e seus processos descendentes; 100% corresponde a todos os processadores lógicos da máquina.",
                "RAM residente é a soma dos processos próprios. Páginas compartilhadas podem aparecer em mais de um processo. Memória virtual e mapas de arquivos não equivalem a RAM ocupada.",
                "I/O do aplicativo é o contador fornecido pelo SO, em bytes por segundo entre amostras. Cache, mmap e diferenças entre Windows/Linux impedem tratar isso como velocidade física do disco ou largura de banda da RAM.",
                "Contadores acumulados de I/O pertencem aos processos atualmente vivos e podem diminuir quando um auxiliar termina. Processos muito curtos, entre duas amostras, podem não ser observados.",
                "As ações exibem CPU da thread que iniciou o trabalho; workers paralelos e WebView entram no total do aplicativo. Memória e I/O compartilhados não são atribuídos artificialmente a cada ação.",
                "Histórico local de até 15 minutos durante esta execução, com amostras a cada segundo. Fechar o modal mantém a coleta. Reiniciar o aplicativo inicia outro histórico.",
                "Frequência de CPU depende da informação fornecida pelo SO. Largura de banda real da RAM, uso do chipset, GPU e temperatura não estão disponíveis neste coletor.",
            ],
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    sample_interval_ms: u64,
    history_limit: usize,
    started_at_ms: u64,
    #[serde(flatten)]
    system: resource_system::SystemSnapshot,
    inventory: Option<resource_inventory::Inventory>,
    storage: Option<resource_inventory::StorageSnapshot>,
    history: Vec<HistorySample>,
    actions: resource_actions::ActionRegistrySnapshot,
    notes: Vec<&'static str>,
}

#[tauri::command]
pub fn resource_snapshot(monitor: State<ResourceMonitor>) -> Result<MonitorSnapshot, String> {
    monitor.snapshot()
}

/// The closure's Rust type contains its enclosing command even when the command
/// shares `offload` with other modules. Use only that static name, never arguments.
pub fn operation_label<F>() -> String {
    let name = std::any::type_name::<F>()
        .rsplit("::")
        .find(|part| !part.starts_with('{'))
        .unwrap_or("operação");
    let label = match name.trim_end_matches("_impl") {
        "load_file" | "load_files" | "load_bundle" => "Carregar logs",
        "load_event_log" => "Ler Windows Event Log",
        "set_big_data_mode" => "Preparar Big Data",
        "case_sync" => "Sincronizar e indexar eventos do Caso",
        "dataset_overview" => "Analisar visão geral",
        "explore_snapshot" => "Explorar registros e facetas",
        "query_events" => "Consultar registros",
        "aggregate_events" | "tree_aggs" => "Agregar campos",
        "stats_events" => "Calcular estatísticas",
        "count_filtered" => "Contar eventos filtrados",
        "triage" => "Analisar comprometimentos",
        "threat_scan" => "Pesquisar ameaças",
        "threat_events" => "Consultar evidências de ameaça",
        "discover_patterns" => "Descobrir padrões",
        "profile_fields" => "Analisar perfil de campos",
        "compute_series" => "Calcular séries",
        "pivot" => "Cruzar dados",
        "cases_load" => "Abrir Casos",
        "cases_save" => "Salvar Casos",
        "export_events" | "export_document" | "export_investigation" | "export_timeline" => {
            "Exportar dados"
        }
        "import_investigation" => "Importar investigação",
        "remote_import" => "Importar fonte remota",
        "remote_test" => "Testar conexão remota",
        "sigma_import" => "Importar regras Sigma",
        "threat_catalog_update" => "Atualizar regras de ameaça",
        "source_hashes" => "Verificar hashes das fontes",
        "harvest_codes" => "Catalogar eventos do sistema",
        _ => return name.trim_end_matches("_impl").replace('_', " "),
    };
    label.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(timestamp_ms: u64) -> HistorySample {
        HistorySample {
            timestamp_ms,
            app_cpu_percent: None,
            host_cpu_percent: None,
            app_memory_bytes: None,
            host_memory_used_bytes: 0,
            read_bytes_per_sec: None,
            written_bytes_per_sec: None,
            active_operations: 0,
        }
    }
    #[test]
    fn history_bounds_count_and_time_even_after_a_pause() {
        let mut store = Store::default();
        for i in 0..1000 {
            store.push(point(i * 1000));
        }
        assert_eq!(store.history.len(), HISTORY_LIMIT);
        assert_eq!(store.history.front().unwrap().timestamp_ms, 100_000);
        store.push(point(2_000_000));
        assert_eq!(store.history.len(), 1);
    }

    #[test]
    fn history_restarts_when_host_clock_moves_backwards() {
        let mut store = Store::default();
        store.push(point(100_000));
        store.push(point(101_000));
        store.push(point(90_000));
        assert_eq!(store.history.len(), 1);
        assert_eq!(store.history.front().unwrap().timestamp_ms, 90_000);
        store.push(point(91_000));
        assert_eq!(store.history.len(), 2);
    }
    #[test]
    fn operation_name_uses_static_command_not_closure_arguments() {
        fn label<F>(_: F) -> String {
            operation_label::<F>()
        }
        let secret_argument = "private log text";
        let value = label(|| secret_argument.len());
        assert!(!value.contains(secret_argument));
        assert!(!value.contains("{{closure}}"));
    }
}
