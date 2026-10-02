//! Build one ordered container from bounded immutable batches. The caller
//! publishes metadata only after every referenced file is sealed and verified.
use super::*;
use crate::case_work_budget::Lease;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
const LIMIT: &str = "CASE_EVIDENCE_CONTAINER_LIMIT: O conjunto excede o limite de preservação.";

pub(super) struct PreparedSet {
    root: PathBuf,
    pub reference: EvidenceRef,
    pub batches: Vec<BatchRef>,
    pub manifest: ContainerManifest,
    pub manifest_bytes: u64,
    pub container: Arc<VerifiedContainer>,
    _credit: Lease,
}
impl PreparedSet {
    pub(super) fn storage_bytes(&self) -> usize {
        self._credit.bytes()
    }
}

pub(super) fn assemble_envelopes(
    root: &Path,
    owner: &EvidenceOwner,
    records: &[RawJson<'_>],
) -> Result<PreparedSet, String> {
    if records.len() > MANIFEST_MEMBERS {
        return Err(LIMIT.into());
    }
    let credit = records
        .len()
        .checked_mul(512)
        .and_then(|n| n.checked_add(512 << 10))
        .ok_or(LIMIT)?;
    let mut credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), credit)?;
    let mut batches = Vec::new();
    let mut members = Vec::new();
    let mut index = 0;
    while index < records.len() {
        crate::operations::check()?;
        let first = index;
        let chunk = stage_records(
            root,
            owner,
            &serde_json::json!({"kind":"native_container_assembly","firstPosition":first}),
            |sink| {
                while index < records.len() && sink.fits_envelope(records[index].get().len()) {
                    sink.push_envelope(records[index].get())?;
                    index += 1;
                }
                if index == first {
                    return Err(LIMIT.into());
                }
                Ok(())
            },
        )?;
        chunk.publish_batch(root)?;
        for member in &chunk.manifest().members {
            let mut member = member.clone();
            member.origin_position = (first as u32)
                .checked_add(member.origin_position)
                .ok_or(LIMIT)?;
            members.push(member);
        }
        batches.push(chunk.batch.clone());
    }
    // The count and encoded-byte caps are independent. Charge the serialized
    // manifest before allocation; each fixed Member is bounded above here.
    let serialized_credit = members
        .len()
        .checked_mul(512)
        .and_then(|n| n.checked_add(64 << 10))
        .ok_or(LIMIT)?;
    credit.merge(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        serialized_credit,
    )?)?;
    seal_manifest(
        root,
        owner,
        uuid::Uuid::new_v4().to_string(),
        members,
        batches,
        credit,
    )
}

pub(super) fn seal_manifest(
    root: &Path,
    owner: &EvidenceOwner,
    container_id: String,
    members: Vec<Member>,
    batches: Vec<BatchRef>,
    mut credit: Lease,
) -> Result<PreparedSet, String> {
    if members.len() > MANIFEST_MEMBERS || batches.len() > 1024 {
        return Err(LIMIT.into());
    }
    uuid::Uuid::parse_str(&container_id).map_err(|_| LIMIT)?;
    let mut ids = HashSet::new();
    let mut positions = HashSet::new();
    for member in &members {
        if member.origin_position as usize >= MANIFEST_MEMBERS
            || !ids.insert(&member.occurrence_id)
            || !positions.insert(member.origin_position)
        {
            return Err(LIMIT.into());
        }
    }
    let manifest = ContainerManifest {
        schema_version: SCHEMA_VERSION,
        owner: owner.clone(),
        container_id,
        manifest_id: uuid::Uuid::new_v4().to_string(),
        members,
    };
    struct Encoded {
        bytes: Vec<u8>,
    }
    impl Write for Encoded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MANIFEST_BYTES as usize - self.bytes.len() {
                return Err(std::io::Error::other(LIMIT));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Encoded { bytes: Vec::new() };
    serde_json::to_writer(&mut output, &manifest).map_err(|e| e.to_string())?;
    let bytes = output.bytes;
    // The verified manifest reader temporarily owns both JSON and typed state.
    credit.merge(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes.len().checked_mul(4).ok_or(LIMIT)?,
    )?)?;
    let reference = EvidenceRef {
        kind: CommittedKind::NativeEvidence,
        schema_version: SCHEMA_VERSION,
        owner: owner.clone(),
        container_id: manifest.container_id.clone(),
        manifest_id: manifest.manifest_id.clone(),
        manifest_sha256: format!("{:x}", Sha256::digest(&bytes)),
        member_count: manifest.members.len() as u32,
    };
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let directory = root.join("evidence-v1");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    if !std::fs::symlink_metadata(&directory)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err(LIMIT.into());
    }
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist_noclobber(directory.join(format!("{}.manifest", reference.manifest_id)))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    crate::operations::check()?;
    let container = Arc::new(VerifiedContainer::open_sized(
        &root,
        &reference,
        batches.clone(),
        bytes.len() as u64,
    )?);
    Ok(PreparedSet {
        root,
        reference,
        batches,
        manifest,
        manifest_bytes: bytes.len() as u64,
        container,
        _credit: credit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_container_spans_batches_without_renumbering_occurrence_origins() {
        let dir = tempfile::tempdir().unwrap();
        let owner = EvidenceOwner {
            store_id: uuid::Uuid::new_v4().to_string(),
            case_id: "c".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
        };
        let raw = RawJson::checked(r#"{"id":0,"future":1.0}"#).unwrap();
        let records = vec![raw; CAPTURE_RECORDS + 1];
        let prepared = assemble_envelopes(dir.path(), &owner, &records).unwrap();
        assert_eq!(prepared.batches.len(), 2);
        assert_eq!(prepared.batches[0].records as usize, CAPTURE_RECORDS);
        assert_eq!(prepared.batches[1].records, 1);
        assert_eq!(
            prepared.manifest.members.last().unwrap().origin_position as usize,
            CAPTURE_RECORDS
        );
        let mut visited = 0;
        prepared
            .container
            .visit_envelopes_with_origin(
                0..prepared.reference.member_count as usize,
                |_, origin, value| {
                    assert_eq!(origin, visited);
                    assert_eq!(value, raw.get());
                    visited += 1;
                    Ok(Visit::Continue)
                },
            )
            .unwrap();
        assert_eq!(visited as usize, records.len());
    }
}
