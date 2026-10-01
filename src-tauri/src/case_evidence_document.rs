//! Borrowed preservation extraction and direct native serialization. Record
//! fragments never pass through Value/Event or a global serde feature switch.
use super::raw_json::RawJson;
use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::Write,
    sync::Arc,
};
const DOCUMENT_BYTES: usize = 64 << 20;
const METADATA_BYTES: usize = 16 << 20;
const MAX_ITEMS: usize = 10_000;
const MAX_CONTAINERS: usize = 10_000;
const INVALID: &str = "CASE_EVIDENCE_DOCUMENT: Estrutura inválida; os originais foram preservados.";
const LIMIT: &str = "CASE_EVIDENCE_DOCUMENT_LIMIT: A investigação excede o limite de preservação.";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ContainerLocation {
    CaseRows,
    CaseEvents,
    ItemRows { index: u32 },
    ItemEvents { index: u32 },
}
impl ContainerLocation {
    pub(crate) fn pointer(&self) -> String {
        match self {
            Self::CaseRows => "/rows".into(),
            Self::CaseEvents => "/events".into(),
            Self::ItemRows { index } => format!("/items/{index}/rows"),
            Self::ItemEvents { index } => format!("/items/{index}/events"),
        }
    }
}
pub(crate) struct RawContainer<'a> {
    pub(crate) location: ContainerLocation,
    pub(crate) records: Vec<RawJson<'a>>,
}
pub(crate) struct ExtractedCase<'a> {
    pub(crate) metadata: Value,
    pub(crate) containers: Vec<RawContainer<'a>>,
}
struct Extraction {
    metadata_bytes: usize,
    records: usize,
    decoder: super::decode::MetadataDecoder,
}
impl Extraction {
    fn metadata(&mut self, key: &str, raw: RawJson<'_>) -> Result<Value, String> {
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(key.len())
            .and_then(|n| n.checked_add(raw.get().len()))
            .filter(|n| *n <= METADATA_BYTES)
            .ok_or(LIMIT)?;
        self.decoder.value(raw)
    }
    fn object<'a>(
        &mut self,
        raw: RawJson<'a>,
        item: Option<u32>,
        containers: &mut Vec<RawContainer<'a>>,
    ) -> Result<Value, String> {
        let mut values = serde_json::Map::new();
        let mut fields = raw.members()?;
        while let Some(field) = fields.next()? {
            crate::operations::check()?;
            if field.key.len() > METADATA_BYTES.saturating_sub(self.metadata_bytes) {
                return Err(LIMIT.into());
            }
            let key = field.key()?;
            if values.contains_key(&key) {
                return Err(INVALID.into());
            }
            let raw = field.value;
            if matches!(key.as_str(), "rows" | "events") && raw.kind() == b'[' {
                if containers.len() >= MAX_CONTAINERS {
                    return Err(LIMIT.into());
                }
                let mut records = Vec::new();
                let mut rows = raw.elements()?;
                while let Some(row) = rows.next()? {
                    if records.len() >= MANIFEST_MEMBERS
                        || self.records >= MANIFEST_MEMBERS
                        || row.get().len() > ENVELOPE_BYTES
                        || row.kind() != b'{'
                    {
                        return Err(LIMIT.into());
                    }
                    self.records += 1;
                    if records.len() % 256 == 0 {
                        crate::operations::check()?;
                    }
                    records.push(row);
                }
                let location = match (item, key.as_str()) {
                    (None, "rows") => ContainerLocation::CaseRows,
                    (None, _) => ContainerLocation::CaseEvents,
                    (Some(index), "rows") => ContainerLocation::ItemRows { index },
                    (Some(index), _) => ContainerLocation::ItemEvents { index },
                };
                containers.push(RawContainer { location, records });
                values.insert(key, Value::Null);
            } else if item.is_none() && key == "items" && raw.kind() == b'[' {
                let mut items = Vec::new();
                let mut rows = raw.elements()?;
                while let Some(row) = rows.next()? {
                    if items.len() >= MAX_ITEMS {
                        return Err(LIMIT.into());
                    }
                    let value = if row.kind() == b'{' {
                        self.object(row, Some(items.len() as u32), containers)?
                    } else {
                        self.metadata("", row)?
                    };
                    items.push(value);
                }
                values.insert(key, Value::Array(items));
            } else {
                let value = self.metadata(&key, raw)?;
                values.insert(key, value);
            }
        }
        Ok(Value::Object(values))
    }
}
pub(crate) fn extract_case(text: &str) -> Result<ExtractedCase<'_>, String> {
    crate::operations::check()?;
    let raw = RawJson::checked_with_limit(text, DOCUMENT_BYTES)?;
    let mut state = Extraction {
        metadata_bytes: 0,
        records: 0,
        decoder: super::decode::MetadataDecoder::new(),
    };
    let mut containers = Vec::new();
    let metadata = state.object(raw, None, &mut containers)?;
    if metadata.get("id").and_then(Value::as_str).is_none() {
        return Err(INVALID.into());
    }
    Ok(ExtractedCase {
        metadata,
        containers,
    })
}
/// Authored metadata plus separately verified immutable container authority.
/// All bindings are server-resolved; a matching client reference is not proof.
pub(crate) struct VerifiedCase {
    metadata: Value,
    owner: EvidenceOwner,
    containers: BTreeMap<ContainerLocation, Arc<VerifiedContainer>>,
    references: BTreeMap<ContainerLocation, EvidenceRef>,
}
impl VerifiedCase {
    pub(crate) fn from_parts(
        metadata: Value,
        owner: EvidenceOwner,
        bindings: impl IntoIterator<Item = (ContainerLocation, Arc<VerifiedContainer>)>,
    ) -> Result<Self, String> {
        let bindings: Vec<_> = bindings.into_iter().collect();
        let references = bindings
            .iter()
            .map(|(location, container)| (location.clone(), container.reference().clone()))
            .collect::<Vec<_>>();
        Self::from_selected_parts(metadata, owner, references, bindings)
    }
    pub(crate) fn from_selected_parts(
        metadata: Value,
        owner: EvidenceOwner,
        references: impl IntoIterator<Item = (ContainerLocation, EvidenceRef)>,
        bindings: impl IntoIterator<Item = (ContainerLocation, Arc<VerifiedContainer>)>,
    ) -> Result<Self, String> {
        if metadata.get("id").and_then(Value::as_str) != Some(owner.case_id.as_str()) {
            return Err(INVALID.into());
        }
        let mut all = BTreeMap::new();
        let mut ids = HashSet::new();
        for (location, reference) in references {
            if all.len() >= MAX_CONTAINERS
                || reference.owner != owner
                || !metadata
                    .pointer(&location.pointer())
                    .is_some_and(Value::is_null)
                || !ids.insert(reference.container_id.clone())
                || all.insert(location, reference).is_some()
            {
                return Err(INVALID.into());
            }
        }
        // A management view cannot accidentally carry an inline evidence array
        // because its authority binding was omitted or replaced.
        let inline = |value: &Value| {
            ["rows", "events"]
                .iter()
                .any(|key| value.get(*key).is_some_and(Value::is_array))
        };
        if inline(&metadata)
            || metadata
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(inline))
        {
            return Err(INVALID.into());
        }
        let mut containers = BTreeMap::new();
        for (location, container) in bindings {
            if all.get(&location) != Some(container.reference())
                || containers.insert(location, container).is_some()
            {
                return Err(INVALID.into());
            }
        }
        Ok(Self {
            metadata,
            owner,
            containers,
            references: all,
        })
    }
    pub(crate) fn metadata(&self) -> &Value {
        &self.metadata
    }
    pub(crate) fn owner(&self) -> &EvidenceOwner {
        &self.owner
    }
    pub(crate) fn references(&self) -> impl Iterator<Item = (&ContainerLocation, &EvidenceRef)> {
        self.references.iter()
    }
    pub(crate) fn containers(
        &self,
    ) -> impl Iterator<Item = (&ContainerLocation, &Arc<VerifiedContainer>)> {
        self.containers.iter()
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        for container in self.containers.values() {
            container.validate()?;
        }
        Ok(())
    }
    pub(crate) fn view_metadata(&self) -> Result<Value, String> {
        let mut metadata = self.metadata.clone();
        for (location, reference) in &self.references {
            *metadata.pointer_mut(&location.pointer()).ok_or(INVALID)? =
                serde_json::to_value(ContainerView {
                    kind: "native_evidence_container",
                    reference: EvidenceReference::Committed(reference.clone()),
                    preserved_count: reference.member_count,
                    preview: None,
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(metadata)
    }
    pub(crate) fn evidence_state(&self) -> Result<CaseEvidenceState, String> {
        let mut hash = Sha256::new();
        hash.update(b"native-case-evidence-v1\0");
        hash.update(serde_json::to_vec(&self.owner).map_err(|e| e.to_string())?);
        let mut preserved_count = 0u32;
        if let Some(items) = self.metadata.get("items").and_then(Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                let Some(reference) = self.references.get(&ContainerLocation::ItemRows {
                    index: index as u32,
                }) else {
                    continue;
                };
                preserved_count = preserved_count
                    .checked_add(reference.member_count)
                    .ok_or(LIMIT)?;
                let tuple = (
                    reference,
                    item.get("stationId"),
                    item.get("artifactId"),
                    item.get("origin"),
                );
                hash.update(serde_json::to_vec(&tuple).map_err(|e| e.to_string())?);
                hash.update([0]);
            }
        }
        Ok(CaseEvidenceState::Ready(CaseEvidenceSummary {
            owner: self.owner.clone(),
            evidence_signature: format!("{:x}", hash.finalize()),
            preserved_count,
            legacy_item_aliases: Vec::new(),
        }))
    }
    pub(crate) fn station_signature(&self, station: Option<&str>) -> Result<(String, u32), String> {
        let CaseEvidenceState::Ready(full) = self.evidence_state()? else {
            return Err(INVALID.into());
        };
        let mut count = 0u32;
        if let Some(items) = self.metadata.get("items").and_then(Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                if station
                    .is_some_and(|id| item.get("stationId").and_then(Value::as_str) != Some(id))
                {
                    continue;
                }
                if let Some(reference) = self.references.get(&ContainerLocation::ItemRows {
                    index: index as u32,
                }) {
                    count = count.checked_add(reference.member_count).ok_or(LIMIT)?;
                }
            }
        }
        let encoded =
            serde_json::to_vec(&("native-case-station-v1", full.evidence_signature, station))
                .map_err(|e| e.to_string())?;
        Ok((format!("{:x}", Sha256::digest(encoded)), count))
    }
    /// Emit validated record fragments verbatim into the caller's bounded,
    /// atomically published Write sink. No full restored Value array exists.
    pub(crate) fn write_to(&self, writer: impl Write, strip_context: bool) -> Result<(), String> {
        self.write_to_with_envelopes(writer, strip_context, |raw, writer| {
            writer.write_all(raw.as_bytes()).map_err(|e| e.to_string())
        })
    }
    /// The callback is for an explicit bounded export transformation. Native
    /// authority and ordinary export continue to emit the original fragments.
    pub(crate) fn write_to_with_envelopes(
        &self,
        writer: impl Write,
        strip_context: bool,
        envelope: impl FnMut(&str, &mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        self.write_to_with_parts(writer, strip_context, envelope, |_, value, writer| {
            serde_json::to_writer(writer, value).map_err(|e| e.to_string())
        })
    }
    pub(crate) fn write_to_with_parts(
        &self,
        mut writer: impl Write,
        strip_context: bool,
        mut envelope: impl FnMut(&str, &mut dyn Write) -> Result<(), String>,
        mut metadata: impl FnMut(&str, &Value, &mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        self.validate()?;
        if self.containers.len() != self.references.len() {
            return Err("CASE_EVIDENCE_FULL_EXPORT_REQUIRED".into());
        }
        self.write_object(
            &mut writer,
            &self.metadata,
            None,
            strip_context,
            &mut envelope,
            &mut metadata,
        )?;
        self.validate()
    }
    fn write_object<W: Write>(
        &self,
        writer: &mut W,
        value: &Value,
        item: Option<u32>,
        strip_context: bool,
        envelope_writer: &mut impl FnMut(&str, &mut dyn Write) -> Result<(), String>,
        metadata_writer: &mut impl FnMut(&str, &Value, &mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(object) = value.as_object() else {
            return metadata_writer("", value, writer);
        };
        writer.write_all(b"{").map_err(|e| e.to_string())?;
        let mut first = true;
        for (key, value) in object {
            crate::operations::check()?;
            if item.is_none()
                && strip_context
                && (key == "analysisContext" || key == crate::case_archive::TOKEN_FIELD)
            {
                continue;
            }
            if !first {
                writer.write_all(b",").map_err(|e| e.to_string())?;
            }
            first = false;
            serde_json::to_writer(&mut *writer, key).map_err(|e| e.to_string())?;
            writer.write_all(b":").map_err(|e| e.to_string())?;
            let location = match (item, key.as_str()) {
                (None, "rows") => Some(ContainerLocation::CaseRows),
                (None, "events") => Some(ContainerLocation::CaseEvents),
                (Some(index), "rows") => Some(ContainerLocation::ItemRows { index }),
                (Some(index), "events") => Some(ContainerLocation::ItemEvents { index }),
                _ => None,
            };
            if let Some(container) = location
                .as_ref()
                .and_then(|location| self.containers.get(location))
            {
                writer.write_all(b"[").map_err(|e| e.to_string())?;
                let mut first = true;
                container.visit_envelopes(
                    0..container.reference().member_count as usize,
                    |_, envelope| {
                        if !first {
                            writer.write_all(b",").map_err(|e| e.to_string())?;
                        }
                        first = false;
                        // envelope() has checked the exact fragment and its size.
                        envelope_writer(envelope, writer)?;
                        Ok(Visit::Continue)
                    },
                )?;
                writer.write_all(b"]").map_err(|e| e.to_string())?;
            } else if item.is_none() && key == "items" && value.is_array() {
                writer.write_all(b"[").map_err(|e| e.to_string())?;
                for (index, item) in value.as_array().unwrap().iter().enumerate() {
                    if index > 0 {
                        writer.write_all(b",").map_err(|e| e.to_string())?;
                    }
                    self.write_object(
                        writer,
                        item,
                        Some(index as u32),
                        false,
                        envelope_writer,
                        metadata_writer,
                    )?;
                }
                writer.write_all(b"]").map_err(|e| e.to_string())?;
            } else {
                metadata_writer(key, value, writer)?;
            }
        }
        writer.write_all(b"}").map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extraction_preserves_raw_values_before_any_event_or_number_parse() {
        let raw = r#"{"id":"c","note":"kept","rows":[{"top":1.0}],"items":[{"id":"i","note":"hello","rows":[{"x":18446744073709551615,"future":{"n":-0.0}},{"x":1.0}],"events":[]}] }"#;
        let extracted = extract_case(raw).unwrap();
        assert_eq!(extracted.containers.len(), 3);
        assert_eq!(extracted.metadata["note"], "kept");
        assert_eq!(
            extracted.containers[1].records[0].get(),
            r#"{"x":18446744073709551615,"future":{"n":-0.0}}"#
        );
        assert!(extracted.metadata["items"][0]["rows"].is_null());
    }
    #[test]
    fn duplicate_case_or_item_keys_are_not_silently_normalized() {
        for raw in [
            r#"{"id":"c","rows":[],"rows":[]}"#,
            r#"{"id":"c","items":[{"rows":[],"rows":[]}]}"#,
        ] {
            assert!(extract_case(raw).is_err());
        }
    }
    #[test]
    fn native_serializer_preserves_number_lexemes_unknown_members_and_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let owner = EvidenceOwner {
            store_id: uuid::Uuid::new_v4().to_string(),
            case_id: "c".into(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
        };
        let raw = "{\n \"id\": 0, \"unknown\": [1.000, -0.0, 18446744073709551615]\n}";
        let staged = stage_records(root.path(), &owner, &Value::Null, |sink| {
            sink.push_envelope(raw)?;
            sink.push_envelope(raw)
        })
        .unwrap();
        staged.publish_files(root.path()).unwrap();
        let container = Arc::new(
            VerifiedContainer::open(root.path(), &staged.reference, [staged.batch.clone()])
                .unwrap(),
        );
        let metadata = serde_json::json!({"id":"c","items":[{"id":"i","rows":null,"note":"edited"}],"analysisContext":{"test":true}});
        let case = VerifiedCase::from_parts(
            metadata,
            owner,
            [(ContainerLocation::ItemRows { index: 0 }, container)],
        )
        .unwrap();
        let mut bytes = Vec::new();
        case.write_to(&mut bytes, false).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.matches(raw).count(), 2);
        assert!(text.contains("edited"));
        assert!(text.contains("analysisContext"));
        let mut bytes = Vec::new();
        case.write_to(&mut bytes, true).unwrap();
        assert!(!String::from_utf8(bytes)
            .unwrap()
            .contains("analysisContext"));
        let view = case.view_metadata().unwrap();
        assert_eq!(
            view["items"][0]["rows"]["kind"],
            "native_evidence_container"
        );
    }
}

/// Preflight only metadata and borrowed envelope spans. Large record bodies do
/// not become Value trees merely because they occur inside a Case document.
pub(super) fn preflight_case(text: &str) -> Result<usize, String> {
    struct Plan {
        owned: usize,
        metadata: usize,
        nodes: usize,
        records: usize,
        containers: usize,
        items: usize,
    }
    impl Plan {
        fn add(&mut self, bytes: usize) -> Result<(), String> {
            self.owned = self
                .owned
                .checked_add(bytes)
                .filter(|n| *n <= 64 << 20)
                .ok_or(LIMIT)?;
            Ok(())
        }
        fn value(&mut self, key_bytes: usize, raw: RawJson<'_>) -> Result<(), String> {
            self.metadata = self
                .metadata
                .checked_add(key_bytes)
                .and_then(|n| n.checked_add(raw.get().len()))
                .filter(|n| *n <= METADATA_BYTES)
                .ok_or(LIMIT)?;
            let plan = super::preflight_value(raw)?;
            self.nodes = self
                .nodes
                .checked_add(plan.nodes)
                .filter(|n| *n <= 131_072)
                .ok_or(LIMIT)?;
            self.add(
                plan.materialization_credit
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(key_bytes * 4 + 512))
                    .ok_or(LIMIT)?,
            )
        }
        fn object(&mut self, raw: RawJson<'_>, item: bool) -> Result<(), String> {
            self.add(512)?;
            let mut fields = raw.members()?;
            while let Some(field) = fields.next()? {
                crate::operations::check()?;
                let key = if field.key.len() <= 128 {
                    Some(field.key()?)
                } else {
                    None
                };
                if key
                    .as_deref()
                    .is_some_and(|key| matches!(key, "rows" | "events"))
                    && field.value.kind() == b'['
                {
                    self.containers += 1;
                    if self.containers > MAX_CONTAINERS {
                        return Err(LIMIT.into());
                    }
                    self.add(512)?;
                    let mut records = field.value.elements()?;
                    while let Some(record) = records.next()? {
                        self.records += 1;
                        if self.records > MANIFEST_MEMBERS
                            || record.get().len() > ENVELOPE_BYTES
                            || record.kind() != b'{'
                        {
                            return Err(LIMIT.into());
                        }
                        self.add(64)?;
                    }
                } else if !item && key.as_deref() == Some("items") && field.value.kind() == b'[' {
                    let mut items = field.value.elements()?;
                    while let Some(value) = items.next()? {
                        self.items += 1;
                        if self.items > MAX_ITEMS {
                            return Err(LIMIT.into());
                        }
                        self.add(64)?;
                        if value.kind() == b'{' {
                            self.object(value, true)?;
                        } else {
                            self.value(0, value)?;
                        }
                    }
                } else {
                    self.value(field.key.len(), field.value)?;
                }
            }
            Ok(())
        }
    }
    let raw = RawJson::checked_with_limit(text, DOCUMENT_BYTES)?;
    let mut plan = Plan {
        owned: 128 << 10,
        metadata: 0,
        nodes: 0,
        records: 0,
        containers: 0,
        items: 0,
    };
    plan.object(raw, false)?;
    Ok(plan.owned)
}
