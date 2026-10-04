//! Content/configuration-addressed results reused across desktop sessions.
//! Source generation counters are request guards, never persistent cache keys.
use crate::{cache_validation::Generation, AppState, SourceData};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 32 << 20;

pub(crate) fn source_signature(state: &AppState) -> Option<String> {
    let source = crate::analysis_runtime::source(state);
    let SourceData::Indexed(index) = &*source else {
        return None;
    };
    let owner = crate::analysis_runtime::current().map(|admitted| admitted.identity.clone());
    let parts = index
        .parts
        .iter()
        .map(|part| {
            (
                &part.identity,
                &part.metadata_identity,
                &part.event_identity,
                &part.path,
                &part.file_name,
                &part.format,
                &part.header,
                &part.calendar.timezone,
                crate::sources::uses_inferred_calendar_year(&part.format)
                    .then_some(part.calendar.year),
                part.custom.as_ref().map(|custom| match custom {
                    crate::sources::CustomParse::Regex(regex) => {
                        format!("regex:{}", regex.as_str())
                    }
                    crate::sources::CustomParse::Delimited { sep, fields } => {
                        format!("delimited:{sep}:{fields:?}")
                    }
                }),
                part.base,
                part.mmap.len(),
                part.physical_file_id,
                part.ts_config.as_ref().map(|config| config.signature()),
                crate::sources::parser_semantics_signature(&part.format),
                crate::java_stacktrace::enrichment_signature(&part.format),
            )
        })
        .collect::<Vec<_>>();
    let fields = crate::analysis_runtime::derived(state);
    let derived = fields
        .iter()
        .map(|field| {
            (
                &field.name,
                &field.source,
                &field.steps,
                field
                    .lookup
                    .as_ref()
                    .map(|lookup| (&lookup.definition, lookup.version())),
                field
                    .rules
                    .iter()
                    .map(|rule| {
                        (
                            rule.re.as_str(),
                            &rule.template,
                            &rule.filter,
                            &rule.filter_security_signature,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    let codes = crate::analysis_runtime::codes(state);
    let system = crate::analysis_runtime::system_codes(state);
    let mut interpretation = serde_json::to_value((
        owner,
        index.lines.len(),
        parts,
        derived,
        &*codes,
        &*system,
        crate::analysis_runtime::fact_interpretation_signature(),
    ))
    .ok()?;
    interpretation.sort_all_objects();
    Some(format!("{:x}", Sha256::digest(interpretation.to_string())))
}

#[derive(Serialize, Deserialize)]
struct Saved {
    key: String,
    generation: Generation,
    value: serde_json::Value,
    checksum: String,
}
fn checksum(key: &str, generation: &Generation, value: &serde_json::Value) -> Option<String> {
    let bytes = serde_json::to_vec(&(key, generation, value)).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}
fn read<T: DeserializeOwned>(path: &Path, key: &str, generation: &Generation) -> Option<T> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    let saved: Saved = serde_json::from_reader(file.take(MAX_BYTES + 1)).ok()?;
    if saved.key != key
        || saved.generation != *generation
        || checksum(key, generation, &saved.value).as_deref() != Some(saved.checksum.as_str())
    {
        return None;
    }
    serde_json::from_value(saved.value).ok()
}
fn write<T: Serialize>(path: &Path, key: &str, generation: &Generation, value: &T) {
    let result = (|| -> Option<()> {
        let value = serde_json::to_value(value).ok()?;
        let saved = Saved {
            key: key.into(),
            generation: generation.clone(),
            checksum: checksum(key, generation, &value)?,
            value,
        };
        let bytes = serde_json::to_vec(&saved).ok()?;
        if bytes.len() as u64 > MAX_BYTES {
            return None;
        }
        let parent = path.parent()?;
        std::fs::create_dir_all(parent).ok()?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).ok()?;
        temp.write_all(&bytes).ok()?;
        temp.as_file().sync_all().ok()?;
        if !generation.is_current() {
            return None;
        }
        temp.persist(path).ok()?;
        Some(())
    })();
    let _ = result;
}
fn cached<T: Serialize + DeserializeOwned>(
    key: &str,
    generation: Option<Generation>,
    compute: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let Some(generation) = generation else {
        return compute();
    };
    let path = crate::config_dir()
        .join("computed-v1")
        .join(format!("{key}.json"));
    crate::operations::check()?;
    if let Some(value) = read(&path, key, &generation) {
        if !generation.is_current() {
            return Err("A fonte mudou durante a leitura do resultado salvo.".into());
        }
        return Ok(value);
    }
    let value = compute()?;
    crate::operations::check()?;
    if !generation.is_current() {
        return Err("A fonte mudou durante o cálculo; reabra a fonte.".into());
    }
    write(&path, key, &generation, &value);
    Ok(value)
}

pub(crate) fn dataset<T: Serialize + DeserializeOwned>(
    state: &AppState,
    filters: &[crate::query::Filter],
    kind: &str,
    compute: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let Some(signature) = source_signature(state) else {
        return compute();
    };
    let source = crate::analysis_runtime::source(state);
    let SourceData::Indexed(index) = &*source else {
        return compute();
    };
    let mut paths = Vec::new();
    for part in &index.parts {
        crate::sources::validate_source(part)?;
        paths.push(PathBuf::from(&part.physical_path));
        if let Some((original, _)) =
            crate::workspace::canonical_original(Path::new(&part.physical_path))?
        {
            paths.push(original);
        }
    }
    paths.sort();
    paths.dedup();
    drop(source);
    let key = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "computed-v1",
                env!("CARGO_PKG_VERSION"),
                signature,
                kind,
                filters
            ))
            .map_err(|e| e.to_string())?
        )
    );
    cached(&key, Generation::capture(&paths), compute)
}

pub(crate) fn file<T: Serialize + DeserializeOwned>(
    path: &Path,
    kind: &str,
    compute: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let generation = Generation::capture(&[path]);
    let key = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&("file-computed-v1", kind, &generation))
                .map_err(|e| e.to_string())?
        )
    );
    cached(&key, generation, compute)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_result_reuses_without_computation_and_invalidates_mutation_or_damage() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let cache = root.path().join("saved.json");
        std::fs::write(&source, b"original").unwrap();
        let generation = Generation::capture(&[source.clone()]).unwrap();
        write(&cache, "context", &generation, &vec!["value"]);
        assert_eq!(
            read::<Vec<String>>(
                &cache,
                "context",
                &Generation::capture(&[source.clone()]).unwrap()
            )
            .unwrap(),
            ["value"]
        );
        assert!(read::<Vec<String>>(&cache, "different context", &generation).is_none());
        let original_modified = std::fs::metadata(&source).unwrap().modified().unwrap();
        std::fs::write(&source, b"modified").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(original_modified)
            .unwrap();
        assert!(
            read::<Vec<String>>(&cache, "context", &Generation::capture(&[source]).unwrap())
                .is_none()
        );
        std::fs::write(&cache, b"broken").unwrap();
        assert!(read::<Vec<String>>(&cache, "context", &generation).is_none());
    }
}
