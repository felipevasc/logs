//! Portable reference descriptors and exact JSONL bytes. Rebuilds use the same
//! bounded validator as local imports; only committed Case config admits them.
use crate::{
    analysis_context::{Diagnostic, ReferenceDescriptor, Snapshot},
    case_archive_format::{self as format, Entry, EntryKind},
    reference_store::{self as store, Owner, PortableSource},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReferenceSet {
    pub owner: Owner,
    pub references: Vec<PortableReference>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortableReference {
    pub descriptor: ReferenceDescriptor,
    pub state: ReferenceState,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum ReferenceState {
    Available {
        #[serde(rename = "assetId")]
        asset_id: String,
        #[serde(rename = "sourceBytes")]
        source_bytes: u64,
        #[serde(rename = "schemaSha256")]
        schema_sha256: String,
    },
    Unavailable,
}
pub(crate) struct Captured {
    pub manifest: ReferenceSet,
    pub files: Vec<(String, PortableSource)>,
}
fn owner(snapshot: &Snapshot) -> Owner {
    Owner {
        case_id: snapshot.case_id.clone(),
        analysis_id: snapshot.analysis_id.clone(),
    }
}
pub(crate) fn capture(root: &Path, snapshot: &Snapshot) -> Result<Captured, String> {
    let owner = owner(snapshot);
    let mut captured = Captured {
        manifest: ReferenceSet {
            owner: owner.clone(),
            references: Vec::new(),
        },
        files: Vec::new(),
    };
    for descriptor in &snapshot.config.references {
        crate::operations::check()?;
        let state =
            match store::portable_source(root, &owner, descriptor, &crate::operations::cancelled) {
                Ok(source) => {
                    let asset_id = uuid::Uuid::new_v4().to_string();
                    let state = ReferenceState::Available {
                        asset_id: asset_id.clone(),
                        source_bytes: source.prepared().source_bytes,
                        schema_sha256: source.prepared().version.schema_sha256.clone(),
                    };
                    captured.files.push((asset_id, source));
                    state
                }
                Err(store::Error::Unavailable | store::Error::UnsupportedFormat) => {
                    ReferenceState::Unavailable
                }
                Err(error) => return Err(error.to_string()),
            };
        captured.manifest.references.push(PortableReference {
            descriptor: descriptor.clone(),
            state,
        });
    }
    Ok(captured)
}

/// Bind every descriptor and asset to the exact foreign config. An older v1
/// archive has no reference bytes; retain those declarations as unavailable.
pub(crate) fn validate(
    schema_version: u32,
    sets: &[ReferenceSet],
    snapshots: &mut [Snapshot],
    entries: &[Entry],
) -> Result<BTreeMap<String, Vec<PortableReference>>, String> {
    if (schema_version == 1 && !sets.is_empty())
        || (schema_version == 2 && sets.len() != snapshots.len())
    {
        return Err("Documento e manifesto de referências não correspondem.".into());
    }
    let mut indexed = BTreeMap::new();
    for set in sets {
        if indexed.insert(set.owner.case_id.as_str(), set).is_some() {
            return Err("Proprietário de referências duplicado.".into());
        }
    }
    let assets: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Reference(id) => Some((id.as_str(), entry)),
            _ => None,
        })
        .collect();
    let mut used = HashSet::new();
    let mut reference_count = 0usize;
    let mut result = BTreeMap::new();
    for snapshot in snapshots {
        reference_count = reference_count
            .checked_add(snapshot.config.references.len())
            .ok_or("Quantidade de referências excessiva.")?;
        if reference_count > format::MAX_REFERENCES {
            return Err("O arquivo portátil excede 4.096 referências.".into());
        }
        let references = if schema_version == 1 {
            snapshot
                .config
                .references
                .iter()
                .map(|descriptor| PortableReference {
                    descriptor: descriptor.clone(),
                    state: ReferenceState::Unavailable,
                })
                .collect::<Vec<_>>()
        } else {
            let set = indexed
                .get(snapshot.case_id.as_str())
                .ok_or("Caso sem manifesto de referências.")?;
            if set.owner != owner(snapshot)
                || set.references.len() != snapshot.config.references.len()
            {
                return Err("Proprietário ou configuração das referências não confere.".into());
            }
            set.references.clone()
        };
        let mut missing = HashSet::new();
        let mut seen = HashSet::new();
        for reference in &references {
            if !seen.insert(&reference.descriptor.id)
                || !snapshot.config.references.contains(&reference.descriptor)
            {
                return Err(
                    "O esquema de uma referência não corresponde à configuração do Caso.".into(),
                );
            }
            match &reference.state {
                ReferenceState::Available {
                    asset_id,
                    source_bytes,
                    schema_sha256,
                } => {
                    let entry = assets
                        .get(asset_id.as_str())
                        .ok_or("Bytes de referência ausentes do arquivo portátil.")?;
                    if !used.insert(asset_id.clone())
                        || reference.descriptor.format != "jsonl"
                        || *source_bytes != entry.bytes
                        || entry.sha256 != reference.descriptor.content_sha256.to_ascii_lowercase()
                        || schema_sha256.len() != 64
                        || !schema_sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    {
                        return Err("Os metadados de uma referência portátil não conferem.".into());
                    }
                }
                ReferenceState::Unavailable => {
                    missing.insert(reference.descriptor.id.clone());
                }
            }
        }
        snapshot.migration_diagnostics.retain(|diagnostic| {
            ![
                "portable_references_unavailable",
                "portable_reference_unavailable",
                "portable_reference_runtime_budget",
            ]
            .contains(&diagnostic.code.as_str())
        });
        if !missing.is_empty() {
            snapshot.migration_diagnostics.push(Diagnostic {
                definition_index: None,
                code: "portable_references_unavailable".into(),
                message: format!("{} referência(s) mantêm a configuração, mas seus bytes não estão disponíveis neste arquivo. Importe a versão original para reproduzir essas consultas.", missing.len()),
            });
            for (index, definition) in snapshot.config.derived_fields.iter().enumerate() {
                if definition
                    .get("lookup")
                    .and_then(|lookup| lookup.get("referenceId"))
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| missing.contains(id))
                {
                    snapshot.migration_diagnostics.push(Diagnostic {
                        definition_index: Some(index),
                        code: "portable_reference_unavailable".into(),
                        message: "Consulta à referência desativada: os bytes não estão disponíveis no arquivo portátil. A definição original foi preservada para reparo.".into(),
                    });
                }
            }
        }
        result.insert(snapshot.case_id.clone(), references);
    }
    if used.len() != assets.len() {
        return Err("O arquivo portátil contém referências sem proprietário.".into());
    }
    Ok(result)
}

pub(crate) fn prepare(
    root: &Path,
    local: &Snapshot,
    references: &[PortableReference],
    paths: &BTreeMap<String, PathBuf>,
) -> Result<Vec<PortableSource>, String> {
    let mut prepared = Vec::new();
    for reference in references {
        let ReferenceState::Available {
            asset_id,
            source_bytes,
            schema_sha256,
        } = &reference.state
        else {
            continue;
        };
        let input = File::open(paths.get(asset_id).ok_or("Referência portátil ausente.")?)
            .map_err(|e| e.to_string())?;
        let expected = store::prepare_jsonl(
            root,
            &owner(local),
            &reference.descriptor,
            input,
            store::Limits::default(),
            &crate::operations::cancelled,
        )
        .map_err(|e| e.to_string())?;
        if expected.source_bytes != *source_bytes
            || expected.version.schema_sha256 != *schema_sha256
        {
            return Err(
                "O esquema da referência preparada não corresponde ao arquivo portátil.".into(),
            );
        }
        let source = store::portable_source(
            root,
            &owner(local),
            &reference.descriptor,
            &crate::operations::cancelled,
        )
        .map_err(|e| e.to_string())?;
        if source.prepared() != &expected {
            return Err("A referência mudou durante a preparação portátil.".into());
        }
        prepared.push(source);
    }
    Ok(prepared)
}

/// Validate executable projections before publication without retaining up to
/// 8 MiB per pending Case. File-only leases keep the verified inputs unchanged.
pub(crate) fn preflight(
    root: &Path,
    prepared: &mut crate::exclusion_store::PreparedPortable,
) -> Result<(), String> {
    use crate::reference_lookup::PreparationError;
    match crate::analysis_runtime::prepare_portable_snapshot(
        root,
        prepared.snapshot(),
        &crate::operations::cancelled,
    ) {
        Ok(_) => return Ok(()),
        Err(error @ (PreparationError::RetainedBytes | PreparationError::RetainedRows)) => {
            let message = format!("Consulta à referência desativada: {error} Os bytes e a definição original foram preservados para reparo.");
            let disabled: HashSet<_> = prepared
                .snapshot()
                .migration_diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic.definition_index)
                .collect();
            let indices = prepared
                .snapshot()
                .config
                .derived_fields
                .iter()
                .enumerate()
                .filter_map(|(index, definition)| {
                    (definition.get("lookup").is_some() && !disabled.contains(&index))
                        .then_some(index)
                })
                .collect::<Vec<_>>();
            if indices.is_empty() {
                return Err(error.to_string());
            }
            for index in indices {
                prepared.add_import_diagnostic(Diagnostic {
                    definition_index: Some(index),
                    code: "portable_reference_runtime_budget".into(),
                    message: message.clone(),
                })?;
            }
        }
        Err(error) => return Err(error.to_string()),
    }
    // The compiler applies the same deterministic dependency disabling used by
    // ordinary reads. A second failure is an import error, never a readiness claim.
    crate::analysis_runtime::prepare_portable_snapshot(
        root,
        prepared.snapshot(),
        &crate::operations::cancelled,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}
