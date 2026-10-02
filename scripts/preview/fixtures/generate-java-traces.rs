#![allow(dead_code)]
#[path="../../../src-tauri/src/java_stacktrace.rs"]
mod java_stacktrace;
use sha2::{Digest, Sha256};

fn main() {
    let prefix = "2026-10-01 12:00:00,001 ERROR [worker] com.example.Service - ";
    let mut complete = format!("{prefix}com.example.RequestException: falha café 日\n");
    for frame in 0..25 {
        complete.push_str(&format!("\tat app/module@1/com.example.Service.step{frame}(Serviço.java:{})\n", frame + 1));
    }
    complete.push_str("\tat java.base/java.lang.Thread.start(Native Method)\n\tat com.example.Helper.dispatch(Unknown Source)\n\tSuppressed: com.example.CloseException: recurso indisponível\n\t\tat com.example.Resource.close(Resource.java:90)\n\tCaused by: com.example.SuppressedCause: somente a causa suprimida\n\t\tat com.example.Resource.flush(Resource.java)\n\t\t... 1 more\nCaused by: com.example.RootProblem: causa final observada\n\tat com.example.Storage.read(Storage.java:42)\n\t... 3 more\n");
    let incomplete = format!("{prefix}com.example.RequestException: tentativa incompleta\n\tat com.example.Service.request(Service.java:3)\nCaused by: ???\n\tat com.example.Storage.after(Storage.java:8)\n\tat malformed frame\n");
    let cases: Vec<_> = [("complete", complete), ("incomplete", incomplete)].into_iter().enumerate().map(|(id, (name, raw))| {
        let trace = java_stacktrace::parse(&raw, prefix.len(), Default::default()).unwrap();
        let fields = trace.scalar_fields();
        serde_json::json!({"name":name,"raw":raw,"fields":fields,"detail":{"state":"available","trace":trace,"reason":null,"row":{"id":id,"eventRef":format!("java-preview:{id}")}}})
    }).collect();
    assert_eq!(cases[0]["fields"]["java.trace.complete"], true);
    assert!(cases[0]["detail"]["trace"]["frames"].as_array().unwrap().len() > 20);
    assert_eq!(cases[1]["fields"]["java.trace.complete"], false);
    let source = std::fs::read("src-tauri/src/java_stacktrace.rs").unwrap();
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "parserCommit":"d0f8621cc047e028637aae2b442b40a5c0c07b5e",
        "parserSourceSha256":format!("{:x}", Sha256::digest(source)),
        "generatedBy":"actual java_stacktrace::parse + Trace::scalar_fields; exact Trace serialization, coordinator-defined detail envelope",
        "spanBasis":"utf8_bytes_in_raw_block",
        "cases":cases
    })).unwrap());
}
