//! Standalone OS-counter diagnostic using the same collector as the app.
//! cargo run --release --no-default-features --example resource_probe -- --self-test
//! Use --benchmark for 20 steady-state sampler rounds, without an auxiliary load.
#![allow(dead_code)]
#[path = "../src/resource_actions.rs"]
mod resource_actions;
#[path = "../src/resource_io.rs"]
mod resource_io;
#[path = "../src/resource_system.rs"]
mod resource_system;

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    if std::env::args().any(|value| value == "--child") {
        let mut buffer = vec![0x5au8; 16 * 1024 * 1024];
        let mut file = tempfile::tempfile().expect("temporary probe file");
        file.write_all(&buffer).unwrap();
        file.sync_all().unwrap();
        let started = Instant::now();
        let mut checksum = 1u64;
        while started.elapsed() < Duration::from_secs(4) {
            for (i, byte) in buffer.iter_mut().enumerate().step_by(4096) {
                *byte = byte.wrapping_add(1);
                checksum = checksum
                    .wrapping_mul(31)
                    .wrapping_add(i as u64 + *byte as u64);
            }
            std::hint::black_box(checksum);
        }
        return;
    }
    if std::env::args().any(|value| value == "--benchmark") {
        benchmark();
        return;
    }
    let _action = resource_actions::begin("Diagnóstico do coletor de recursos");
    let mut sampler = resource_system::SystemSampler::new(std::env::temp_dir());
    let first = sampler.sample();
    assert!(first.warming_up && first.app.cpu_percent.is_none());
    let self_test = std::env::args().any(|value| value == "--self-test");
    let mut child = self_test.then(|| {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--child")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        command.spawn().expect("spawn owned probe child")
    });
    println!("{}", serde_json::to_string(&first).unwrap());
    let mut saw_child = false;
    let mut saw_child_cpu = false;
    for _ in 0..3 {
        std::thread::sleep(Duration::from_secs(1));
        let snapshot = sampler.sample();
        assert!(snapshot.host.total_memory_bytes > 0);
        assert!(snapshot.app.resident_bytes.is_some_and(|v| v > 0));
        if let Some(child) = &child {
            if let Some(process) = snapshot.processes.iter().find(|p| p.pid == child.id()) {
                saw_child = true;
                saw_child_cpu |= process.cpu_percent.is_some_and(|v| v > 0.0);
            }
        }
        println!("{}", serde_json::to_string(&snapshot).unwrap());
    }
    if let Some(child) = &mut child {
        assert!(child.wait().unwrap().success());
        assert!(
            saw_child && saw_child_cpu,
            "own child and CPU were not observed"
        );
        let final_sample = sampler.sample();
        assert!(
            !final_sample.processes.iter().any(|p| p.pid == child.id()),
            "exited child still included"
        );
    }
    println!(
        "{}",
        serde_json::to_string(&resource_actions::snapshot()).unwrap()
    );
}

fn benchmark() {
    const SAMPLES: usize = 20;
    let initialized = Instant::now();
    let mut sampler = resource_system::SystemSampler::new(std::env::temp_dir());
    let first = sampler.sample();
    let initialization_ms = initialized.elapsed().as_secs_f64() * 1000.0;
    let pid = std::process::id();
    let baseline = resource_io::process_counters(pid);
    let started = Instant::now();
    let mut sample_ms = Vec::with_capacity(SAMPLES);
    let mut peak_resident = baseline.resident_bytes;
    let mut previous_duration = Duration::ZERO;
    for _ in 0..SAMPLES {
        std::thread::sleep(Duration::from_secs(1).saturating_sub(previous_duration));
        let round = Instant::now();
        let snapshot = sampler.sample();
        previous_duration = round.elapsed();
        sample_ms.push(previous_duration.as_secs_f64() * 1000.0);
        peak_resident = peak_resident.max(snapshot.app.resident_bytes);
        std::hint::black_box(snapshot);
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let final_counters = resource_io::process_counters(pid);
    let cpu_ms = final_counters.cpu_time_ns.and_then(|now| {
        now.checked_sub(baseline.cpu_time_ns?)
            .map(|ns| ns as f64 / 1_000_000.0)
    });
    sample_ms.sort_by(f64::total_cmp);
    println!("{}", serde_json::json!({
        "benchmark": "resource_system_sampler",
        "samples": SAMPLES,
        "initializationMs": initialization_ms,
        "elapsedMs": elapsed_ms,
        "sampleMeanMs": sample_ms.iter().sum::<f64>() / SAMPLES as f64,
        "sampleMedianMs": (sample_ms[SAMPLES / 2 - 1] + sample_ms[SAMPLES / 2]) / 2.0,
        "sampleP95Ms": sample_ms[(SAMPLES * 95).div_ceil(100) - 1],
        "sampleMaxMs": sample_ms[SAMPLES - 1],
        "steadyStateProcessCpuMs": cpu_ms,
        "oneCoreCpuPercent": cpu_ms.map(|cpu| cpu / elapsed_ms * 100.0),
        "hostCpuPercent": cpu_ms.map(|cpu| cpu / elapsed_ms * 100.0 / first.host.logical_cpus.max(1) as f64),
        "peakResidentBytes": peak_resident,
        "logicalCpus": first.host.logical_cpus,
        "notes": "Sampler isolado, sem UI, inventário de arquivos/componentes ou operações de análise. Inicialização fora do intervalo estável; CPU inclui somente o processo do diagnóstico."
    }));
}
