//! Build time of the columnar store on a real file (manual benchmark):
//! LOGINSIGHT_BENCH_FILE=<log> cargo test --release --test engine_build -- --ignored --nocapture
use loginsight_lib::testkit::{self, Source};
use std::time::Instant;

#[test]
#[ignore]
fn build_time_on_a_large_file() {
    let Ok(file) = std::env::var("LOGINSIGHT_BENCH_FILE") else { return };
    let dir = std::env::var("LOGINSIGHT_BENCH_DIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    if std::env::var_os("LOGINSIGHT_DATA_DIR").is_none() {
        std::env::set_var("LOGINSIGHT_DATA_DIR", format!("{dir}/dados"));
    }
    std::env::set_var("LOGINSIGHT_ENGINE_TRACE", "1");
    let engine = format!("{dir}/motor-{}", std::process::id());
    testkit::set_engine_dir(&engine);
    let started = Instant::now();
    let src = Source::open(&[file.as_str()], "{}", "[]").unwrap();
    eprintln!("índice: {:?} ({} linhas)", started.elapsed(), src.len());
    let started = Instant::now();
    src.prepare().unwrap();
    let size: u64 = std::fs::read_dir(&engine)
        .unwrap()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    eprintln!("motor: {:?}, {} MB", started.elapsed(), size / 1_000_000);
    let _ = std::fs::remove_dir_all(&engine);
}
