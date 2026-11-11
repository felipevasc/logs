/// The release binary carries only what `attack::reference` reads from the
/// STIX bundle: technique names, ATT&CK tactics and revocations. The bundle
/// itself stays in the repository, verbatim, as the reviewed source.
fn attack_index() {
    use serde_json::{json, Map, Value};
    let source = "resources/attack-enterprise-v19.2.json";
    println!("cargo:rerun-if-changed={source}");
    let bundle: Value = serde_json::from_str(&std::fs::read_to_string(source).expect("ATT&CK bundle"))
        .expect("versioned ATT&CK STIX bundle");
    let all = bundle["objects"].as_array().expect("STIX objects");
    let mut ids = std::collections::HashMap::new();
    let mut techniques = Map::new();
    for object in all {
        if object["type"] != "attack-pattern" {
            continue;
        }
        let Some(id) = object["external_references"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["source_name"] == "mitre-attack")
            .and_then(|r| r["external_id"].as_str())
        else {
            continue;
        };
        ids.insert(object["id"].as_str().unwrap_or_default().to_string(), id.to_string());
        let tactics: Vec<&str> = object["kill_chain_phases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["kill_chain_name"] == "mitre-attack")
            .filter_map(|p| p["phase_name"].as_str())
            .collect();
        techniques.insert(id.to_string(), json!({"name": object["name"].as_str().unwrap_or(id), "tactics": tactics}));
    }
    let mut replacements = Map::new();
    for object in all {
        if object["relationship_type"] == "revoked-by" {
            if let (Some(a), Some(b)) = (
                object["source_ref"].as_str().and_then(|v| ids.get(v)),
                object["target_ref"].as_str().and_then(|v| ids.get(v)),
            ) {
                replacements.insert(a.clone(), Value::from(b.clone()));
            }
        }
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("attack-index.json");
    std::fs::write(out, json!({"techniques": techniques, "replacements": replacements}).to_string())
        .expect("ATT&CK index");
}

fn main() {
    attack_index();
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        // Tauri's resource embedding targets binaries only. Supply its unchanged
        // 2.7 manifest through the linker so the library test harness gets it too.
        // Keep icons/version resources, without embedding a second app manifest.
        let attributes = tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new_without_app_manifest(),
        );
        tauri_build::try_build(attributes).expect("failed to run tauri-build");
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo package directory"),
        )
        .join("windows/tauri-default.manifest.xml");
        println!("cargo:rerun-if-changed=windows/tauri-default.manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // The original Tauri manifest has no UAC or DPI overrides. Prevent the
        // linker from adding its default UAC section while embedding this XML.
        println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
    } else {
        tauri_build::build()
    }
}
