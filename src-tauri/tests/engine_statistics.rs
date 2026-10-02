//! Exact statistics must agree with recovery for missing times and all levels.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::io::Write;

#[test]
fn fused_statistics_preserve_time_sentinels_level_counts_and_filtered_selections() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
    testkit::set_engine_dir(dir.path().join("engine").to_str().unwrap());
    let records = [
        json!({"timestamp":"1969-12-31T23:59:59Z","level":"info","source":"timed"}),
        json!({"timestamp":"1970-01-01T00:00:01Z","level":"unfamiliar","source":"timed"}),
        json!({"timestamp":null,"level":"error","source":"untimed"}),
        json!({"timestamp":"1970-01-01T00:00:00Z","level":"warn","source":"untimed"}),
        json!({"timestamp":"1970-01-01T00:01:00Z","level":"critical","source":"timed"}),
        json!({"timestamp":"1970-01-01T00:02:00Z","level":"debug","source":"timed"}),
        json!({"timestamp":"1970-01-01T00:03:00Z","level":"trace","source":"timed"}),
        json!({"level":"info","source":"untimed"}),
    ];
    let paths: Vec<_> = records
        .chunks(4)
        .enumerate()
        .map(|(part, rows)| {
            let path = dir.path().join(format!("stats-{part}.jsonl"));
            let mut file = std::fs::File::create(&path).unwrap();
            for row in rows {
                let mut row = row.clone();
                row["message"] = json!("ordinary record");
                row["raw_marker"] = json!("only a raw key");
                writeln!(file, "{row}").unwrap();
            }
            path
        })
        .collect();
    let names: Vec<_> = paths.iter().map(|path| path.to_str().unwrap()).collect();
    let source = Source::open(&names, "{}", "[]").unwrap();
    source.prepare().unwrap();
    let filters = [
        json!([]),
        json!([{"column":"source","op":"equals_exact","value":"absent"}]),
        json!([{"column":"source","op":"equals_exact","value":"untimed"}]),
        json!([{"column":"source","op":"equals_exact","value":"timed"}]),
        json!([{"column":"id","op":"equals_exact","value":"0"}]),
        json!([{"column":"timestamp","op":"between","value":"-1000","value2":"1000"}]),
        json!([{"column":"timestamp","op":"gte","value":"0"}]),
        json!([{"column":"timestamp","op":"lte","value":"0"}]),
        json!([{"column":"timestamp","op":"lt","value":"9007199254740992"}]),
        json!([{"column":"_all","op":"query","value":"timestamp:-1000..1000"}]),
        json!([{"column":"_all","op":"query","value":"NOT timestamp:-1000..1000"}]),
        json!([{"column":"_all","op":"query","value":"NOT (timestamp>=0 OR NOT timestamp<1001)"}]),
        json!([{"column":"_all","op":"query","value":"NOT timestamp<9007199254740992"}]),
        json!([{"column":"_all","op":"contains","value":"raw_marker"}]),
    ];
    for filters in filters {
        let filters = filters.to_string();
        let expected = source.stats(Engine::Lines, &filters);
        let actual = source.stats(Engine::Columnar, &filters);
        assert_eq!(actual, expected, "statistics for {filters}");
        assert_eq!(source.count(Engine::Columnar, &filters), source.count(Engine::Lines, &filters), "count for {filters}");
        assert_eq!(source.query(Engine::Columnar, &filters, "id", "asc", 0, 20),
            source.query(Engine::Lines, &filters, "id", "asc", 0, 20), "rows for {filters}");
        let level_total: u64 = actual["levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry[1].as_u64().unwrap())
            .sum();
        assert_eq!(level_total, source.count(Engine::Lines, &filters) as u64);
    }
    let all = source.stats(Engine::Columnar, "[]");
    let bucket_total: u64 = all["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry[1].as_u64().unwrap())
        .sum();
    assert_eq!(
        bucket_total, 5,
        "null, missing and epoch-zero timestamps are excluded"
    );
    let untimed: Value = source.stats(
        Engine::Columnar,
        &json!([
            {"column":"source","op":"equals_exact","value":"untimed"}
        ])
        .to_string(),
    );
    assert_eq!(untimed["buckets"], json!([]));
    assert_eq!(untimed["bucketMs"], 0);
}
