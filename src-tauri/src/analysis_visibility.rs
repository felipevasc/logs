//! Immutable, source-bound visibility masks. SQLite is used only while compiling
//! a verified ledger generation. Query hot paths read bounded bits and retain
//! source/metadata/payload leases; they never probe SQLite for each event.
use crate::{
    analysis_context::Identity,
    exclusion_store::{
        InputMember, Locator, Member, MembershipLease, Scope, SourceDescriptor, VerifiedView, Work,
    },
    sources::{CustomParse, FileIndex, FilePart},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

/// Bump whenever sources' newline, multiline, JSON-envelope or tabular framing
/// changes in a way that can give the same offset a different record extent.
const FRAMING_VERSION: u32 = 1;
pub const UNPROVEN_SNAPSHOT_ORIGIN: &str = "VISIBILITY_UNPROVEN_SNAPSHOT_ORIGIN";
/// Exact dense u64-word payload, excluding the small mask object and leases.
pub fn required_mask_bytes(rows: usize) -> Result<usize, String> {
    rows.checked_add(63)
        .and_then(|n| (n / 64).checked_mul(8))
        .ok_or_else(|| "Máscara de visibilidade excessiva.".into())
}
fn encoded_ref(value: &str) -> Option<(&str, u64)> {
    let (prefix, offset) = value.rsplit_once(':')?;
    if prefix.len() != 64 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let number = offset.parse::<u64>().ok()?;
    (number.to_string() == offset).then_some((prefix, number))
}
fn foreign_snapshot_reference(part: &FilePart, value: &str) -> bool {
    encoded_ref(value).is_some_and(|(version, _)| {
        version != part.event_identity.as_deref().unwrap_or(&part.identity)
    })
}
fn unproven_snapshot() -> String {
    format!("{UNPROVEN_SNAPSHOT_ORIGIN}: snapshot preserva referência de outra fonte sem seu enquadramento original; a exclusão não pode ser ignorada.")
}
#[derive(Clone, Debug)]
pub struct MaskBudget {
    pub mask_bytes: usize,
    pub cache_bytes: usize,
    pub max_snapshot_bytes: usize,
    pub alias_bytes: usize,
    pub evidence_bytes: usize,
}
impl Default for MaskBudget {
    fn default() -> Self {
        Self {
            mask_bytes: 8 << 20,
            cache_bytes: 8 << 20,
            max_snapshot_bytes: 1 << 20,
            alias_bytes: 8 << 20,
            evidence_bytes: 8 << 20,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceProvenance {
    pub source: SourceDescriptor,
    pub locator: Locator,
}
impl EvidenceProvenance {
    fn key(&self, event_ref: &str) -> Result<crate::exclusion_store::Key, String> {
        if event_ref.is_empty() || event_ref.len() > 4096 {
            return Err("Referência estável de evidência inválida.".into());
        }
        match &self.locator {
            Locator::ByteOffset(offset) => {
                if let Some(prefix) = &self.source.event_ref_prefix {
                    if format!("{prefix}:{offset}") != event_ref {
                        return Err("Proveniência da evidência diverge da referência.".into());
                    }
                }
            }
            Locator::StableRecord(value) => {
                if value.is_empty() || value.len() > 512 {
                    return Err("Identidade persistida da evidência inválida.".into());
                }
            }
        }
        Ok(crate::exclusion_store::Key {
            source_key: self.source.key()?,
            locator: self.locator.clone(),
        })
    }
}
#[derive(Clone)]
struct Alias {
    source: usize,
    locator: Locator,
    row: usize,
}
#[derive(Clone)]
struct PartBinding {
    source: usize,
    start: usize,
    end: usize,
}
pub struct SourceSet {
    descriptors: Vec<SourceDescriptor>,
    part_bindings: Vec<PartBinding>,
    fingerprint: String,
    aliases: Vec<Alias>,
    unproven_refs: Vec<(String, usize)>,
    prepared: bool,
}
fn check(work: &Work<'_>) -> Result<(), String> {
    if (work.cancelled)() {
        Err("Operação cancelada.".into())
    } else {
        Ok(())
    }
}
fn custom(part: &FilePart) -> Value {
    match &part.custom {
        Some(CustomParse::Regex(regex)) => serde_json::json!(["regex", regex.as_str()]),
        Some(CustomParse::Delimited { sep, fields }) => {
            serde_json::json!(["delimited", sep, fields])
        }
        None => Value::Null,
    }
}
fn descriptor(part: &FilePart) -> Result<SourceDescriptor, String> {
    let calendar = match part.format.as_str() {
        "apache" | "jsonl" | "snapshot" | "text" | "csv" | "tsv" | "csv:semicolon" | "csv:pipe"
        | "custom" | "syslog5424" | "cef" | "leef" | "logfmt" | "w3c" | "zeek" | "auditd" => {
            Value::Null
        }
        // Calendar-sensitive/unknown framing remains deliberately conservative.
        _ => serde_json::to_value(&part.calendar).map_err(|e| e.to_string())?,
    };
    let framing = serde_json::to_vec(&(
        FRAMING_VERSION,
        &part.format,
        custom(part),
        &part.header,
        calendar,
    ))
    .map_err(|e| e.to_string())?;
    let version = part
        .event_identity
        .as_deref()
        .unwrap_or(&part.identity)
        .to_string();
    if version.is_empty() {
        return Err("Fonte sem identidade estável para exclusões.".into());
    }
    Ok(SourceDescriptor {
        version: version.clone(),
        record_space: format!("framing-v1:{:x}", Sha256::digest(framing)),
        label: part.file_name.clone(),
        event_ref_prefix: (part.format != "snapshot").then_some(version),
    })
}
impl SourceSet {
    pub fn new(index: &FileIndex) -> Result<Self, String> {
        let mut descriptors = Vec::new();
        let mut by_key = HashMap::new();
        let mut part_bindings = Vec::new();
        let mut digest = Sha256::new();
        digest.update(b"visibility-source-v1");
        digest.update(index.lines.len().to_le_bytes());
        // This cache-generation identity is process local. Retaining the Arc
        // prevents address reuse while a cached mask still holds its lease.
        digest.update((Arc::as_ptr(&index.lines) as usize).to_le_bytes());
        let mut previous_end = 0;
        for part in &index.parts {
            let source = descriptor(part)?;
            let key = source.key()?;
            let source_index = match by_key.get(&key) {
                Some(index) => *index,
                None => {
                    let id = descriptors.len();
                    descriptors.push(source);
                    by_key.insert(key.clone(), id);
                    id
                }
            };
            let end_offset = part
                .base
                .checked_add(part.mmap.len() as u64)
                .ok_or("Intervalo da fonte excede o limite.")?;
            let start = index.lines.partition_point(|line| line.offset < part.base);
            let end = index.lines.partition_point(|line| line.offset < end_offset);
            if start != previous_end || end < start {
                return Err("Intervalos da fonte são incompatíveis com a visibilidade.".into());
            }
            previous_end = end;
            part_bindings.push(PartBinding {
                source: source_index,
                start,
                end,
            });
            let binding = serde_json::to_vec(&(
                &key,
                &part.identity,
                &part.metadata_identity,
                part.physical_file_id,
                part.base,
                part.mmap.len(),
                start,
                end,
                part.ts_config.as_ref().map(|ts| ts.signature()),
            ))
            .map_err(|e| e.to_string())?;
            digest.update(binding);
            digest.update((Arc::as_ptr(&part.mmap) as usize).to_le_bytes());
        }
        if previous_end != index.lines.len() {
            return Err("Metadados contêm registros sem fonte verificável.".into());
        }
        Ok(Self {
            descriptors,
            part_bindings,
            fingerprint: format!("{:x}", digest.finalize()),
            aliases: Vec::new(),
            unproven_refs: Vec::new(),
            prepared: false,
        })
    }
    pub fn prepare(
        index: &FileIndex,
        budget: &MaskBudget,
        work: &Work<'_>,
    ) -> Result<Self, String> {
        Self::new(index)?.prepare_aliases(index, budget, work)
    }
    fn prepare_aliases(
        mut self,
        index: &FileIndex,
        budget: &MaskBudget,
        work: &Work<'_>,
    ) -> Result<Self, String> {
        if self.prepared {
            return Ok(self);
        }
        let mut keys: HashMap<String, usize> = self
            .descriptors
            .iter()
            .enumerate()
            .map(|(id, source)| Ok((source.key()?, id)))
            .collect::<Result<_, String>>()?;
        let mut charged = 0usize;
        let mut examined = 0u64;
        let mut reported = Instant::now();
        for (part_index, part) in index
            .parts
            .iter()
            .enumerate()
            .filter(|(_, part)| part.format == "snapshot")
        {
            for row in self.part_bindings[part_index].start..self.part_bindings[part_index].end {
                check(work)?;
                examined += 1;
                if reported.elapsed() >= Duration::from_millis(150) {
                    (work.progress)("snapshot_provenance", examined, None);
                    reported = Instant::now();
                }
                let bytes = record_bytes(index, row, part)?;
                let (raw_reference, proof) = snapshot_identity(bytes, budget)?;
                let Some(proof) = proof else {
                    if raw_reference
                        .as_deref()
                        .is_some_and(|value| foreign_snapshot_reference(part, value))
                    {
                        return Err(unproven_snapshot());
                    }
                    if let Some(value) = raw_reference {
                        charged = charged
                            .checked_add(value.len() + std::mem::size_of::<(String, usize)>())
                            .ok_or("Índice de referências excessivo.")?;
                        if charged > budget.alias_bytes {
                            return Err(
                                "Referências de snapshot excedem o orçamento de proveniência."
                                    .into(),
                            );
                        }
                        self.unproven_refs.try_reserve_exact(1).map_err(|_| {
                            "Não há memória para verificar referências do snapshot."
                        })?;
                        self.unproven_refs
                            .push((value, self.part_bindings[part_index].source));
                    }
                    continue;
                };
                let reference = reference(index, row, part, budget)?;
                let key = proof.key(&reference)?.source_key;
                let source = match keys.get(&key) {
                    Some(index) => *index,
                    None => {
                        let size = serde_json::to_vec(&proof.source)
                            .map_err(|e| e.to_string())?
                            .len()
                            + key.len()
                            + std::mem::size_of::<SourceDescriptor>();
                        charged = charged
                            .checked_add(size)
                            .ok_or("Proveniência de snapshot excessiva.")?;
                        if charged > budget.alias_bytes {
                            return Err("Proveniência preservada do snapshot excede o orçamento; a exclusão não pode ser ignorada.".into());
                        }
                        let id = self.descriptors.len();
                        self.descriptors.push(proof.source);
                        keys.insert(key, id);
                        id
                    }
                };
                let extra = std::mem::size_of::<Alias>()
                    + match &proof.locator {
                        Locator::StableRecord(value) => value.len(),
                        _ => 0,
                    };
                charged = charged
                    .checked_add(extra)
                    .ok_or("Índice de proveniência excessivo.")?;
                if charged > budget.alias_bytes {
                    return Err("Índice de identidades do snapshot excede o orçamento; use uma fonte com identidades preparadas.".into());
                }
                if self.aliases.len() == self.aliases.capacity() {
                    let remaining = (budget.alias_bytes - charged) / std::mem::size_of::<Alias>();
                    self.aliases
                        .try_reserve_exact(remaining.min(1024).max(1))
                        .map_err(|_| "Não há memória para preparar identidades do snapshot.")?;
                }
                self.aliases.push(Alias {
                    source,
                    locator: proof.locator,
                    row,
                });
            }
        }
        self.unproven_refs.sort_unstable();
        self.aliases.sort_unstable_by(|a, b| {
            (a.source, &a.locator, a.row).cmp(&(b.source, &b.locator, b.row))
        });
        self.prepared = true;
        Ok(self)
    }
    pub fn descriptors(&self) -> &[SourceDescriptor] {
        &self.descriptors
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    /// A foreign explicit snapshot reference without original framing remains
    /// readable, but cannot be promoted into fabricated exclusion provenance.
    pub fn provenance(
        &self,
        index: &FileIndex,
        row: usize,
        event_ref: &str,
        budget: &MaskBudget,
    ) -> Result<Option<EvidenceProvenance>, String> {
        let (part_index, offset) = self.locate(index, row)?;
        let part = &index.parts[part_index];
        if reference(index, row, part, budget)? != event_ref {
            return Err("Referência do registro diverge da fonte admitida.".into());
        }
        if part.format == "snapshot" {
            if let Some(proof) = snapshot_identity(record_bytes(index, row, part)?, budget)?.1 {
                proof.key(event_ref)?;
                return Ok(Some(proof));
            }
            if foreign_snapshot_reference(part, event_ref) {
                return Ok(None);
            }
        }
        Ok(Some(EvidenceProvenance {
            source: self.descriptors[self.part_bindings[part_index].source].clone(),
            locator: Locator::ByteOffset(offset),
        }))
    }
    fn locate(&self, index: &FileIndex, row: usize) -> Result<(usize, u64), String> {
        let line = index
            .lines
            .get(row)
            .ok_or("Registro fora da fonte admitida.")?;
        let part = index
            .parts
            .partition_point(|part| part.base <= line.offset)
            .checked_sub(1)
            .ok_or("Registro sem fonte.")?;
        let bound = self
            .part_bindings
            .get(part)
            .ok_or("Fonte mudou durante a seleção.")?;
        if !(bound.start..bound.end).contains(&row) {
            return Err("Registro fora da proveniência admitida.".into());
        }
        Ok((
            part,
            line.offset
                .checked_sub(index.parts[part].base)
                .ok_or("Posição da fonte inválida.")?,
        ))
    }
    pub fn member_row(
        &self,
        index: &FileIndex,
        row: usize,
        budget: &MaskBudget,
    ) -> Result<InputMember, String> {
        let (part_index, offset) = self.locate(index, row)?;
        let part = &index.parts[part_index];
        let event_ref = reference(index, row, part, budget)?;
        if part.format == "snapshot" {
            if let Some(proof) = snapshot_identity(record_bytes(index, row, part)?, budget)?.1 {
                let key = proof.key(&event_ref)?.source_key;
                let source=self.descriptors.iter().position(|source|source.key().ok().as_deref()==Some(&key)).ok_or("Prepare as identidades preservadas do snapshot antes de excluir registros.")?;
                return Ok(InputMember {
                    source_index: source,
                    locator: proof.locator,
                    event_ref,
                });
            }
        }
        if part.format == "snapshot" && foreign_snapshot_reference(part, &event_ref) {
            return Err(unproven_snapshot());
        }
        Ok(InputMember {
            source_index: self.part_bindings[part_index].source,
            locator: Locator::ByteOffset(offset),
            event_ref,
        })
    }
    pub fn member(
        &self,
        index: &FileIndex,
        row: usize,
        event_ref: &str,
        budget: &MaskBudget,
    ) -> Result<InputMember, String> {
        let member = self.member_row(index, row, budget)?;
        if member.event_ref != event_ref {
            return Err("Referência do registro diverge da fonte admitida.".into());
        }
        Ok(member)
    }
    pub fn resolve_member(
        &self,
        index: &FileIndex,
        member: &Member,
        budget: &MaskBudget,
    ) -> Result<Option<usize>, String> {
        let Some(source) = self
            .descriptors
            .iter()
            .position(|source| source.key().ok().as_deref() == Some(&member.key.source_key))
        else {
            return Ok(None);
        };
        let low = self.aliases.partition_point(|alias| {
            (alias.source, &alias.locator) < (source, &member.key.locator)
        });
        if let Some(alias) = self
            .aliases
            .get(low)
            .filter(|alias| alias.source == source && alias.locator == member.key.locator)
        {
            let (part, _) = self.locate(index, alias.row)?;
            if reference(index, alias.row, &index.parts[part], budget)? != member.event_ref {
                return Err("Referência arquivada diverge do snapshot.".into());
            }
            return Ok(Some(alias.row));
        }
        let Locator::ByteOffset(offset) = member.key.locator else {
            return Err("Localizador da evidência ainda não possui índice persistido.".into());
        };
        for (part_id, binding) in self
            .part_bindings
            .iter()
            .enumerate()
            .filter(|(_, part)| part.source == source)
        {
            let target = index.parts[part_id]
                .base
                .checked_add(offset)
                .ok_or("Localizador de arquivo inválido.")?;
            let row = binding.start
                + index
                    .lines
                    .range(binding.start..binding.end)
                    .partition_point(|line| line.offset < target);
            if row < binding.end && index.lines.at(row).offset == target {
                if reference(index, row, &index.parts[part_id], budget)? != member.event_ref {
                    return Err("Referência arquivada diverge da fonte.".into());
                }
                return Ok(Some(row));
            }
        }
        Err("Registro arquivado não corresponde ao enquadramento atual da fonte.".into())
    }
}

// Borrow large canonical strings. Only fields/derived provenance require typed
// JSON validation, so an invalid Event cannot falsely acquire an explicit ref.
// The input ceiling bounds this one-record verification; no Event vector exists.
#[allow(dead_code)]
#[derive(Deserialize)]
struct SnapshotProof<'a> {
    id: usize,
    #[serde(default, borrow)]
    event_ref: Cow<'a, str>,
    #[serde(default, borrow)]
    parse_status: Cow<'a, str>,
    timestamp: Option<i64>,
    #[serde(borrow)]
    source: Cow<'a, str>,
    #[serde(borrow)]
    level: Cow<'a, str>,
    #[serde(borrow)]
    code: Cow<'a, str>,
    #[serde(borrow)]
    name: Cow<'a, str>,
    #[serde(borrow)]
    description: Cow<'a, str>,
    #[serde(borrow)]
    message: Cow<'a, str>,
    #[serde(borrow)]
    raw: Cow<'a, str>,
    #[serde(default)]
    fields: serde_json::Map<String, Value>,
    #[serde(default, borrow)]
    derived_diagnostics: Vec<DiagnosticProof<'a>>,
    #[serde(default)]
    derived_originals: BTreeMap<String, OriginalProof>,
    #[serde(default)]
    evidence_provenance: Option<EvidenceProvenance>,
}
#[allow(dead_code)]
#[derive(Deserialize)]
struct DiagnosticProof<'a> {
    #[serde(borrow)]
    field: Cow<'a, str>,
    #[serde(borrow)]
    code: Cow<'a, str>,
    #[serde(borrow)]
    message: Cow<'a, str>,
    #[serde(default)]
    warning: bool,
}
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
enum OriginalProof {
    Missing,
    Present(Value),
}
fn snapshot_identity(
    bytes: &[u8],
    budget: &MaskBudget,
) -> Result<(Option<String>, Option<EvidenceProvenance>), String> {
    if bytes.len() > budget.max_snapshot_bytes {
        return Err("Registro snapshot excede o limite de verificação de identidade; a exclusão não foi aplicada.".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    match serde_json::from_str::<SnapshotProof<'_>>(&text) {
        Ok(proof) if !proof.event_ref.is_empty() => {
            if proof.event_ref.len() > 4096 {
                return Err("Referência de snapshot excede o limite de identidade.".into());
            }
            Ok((
                Some(proof.event_ref.into_owned()),
                proof.evidence_provenance,
            ))
        }
        // sources::parse_line(snapshot) falls back to text for an invalid Event,
        // and event_at then assigns the ordinary prefix + byte-offset ref.
        Ok(proof) if proof.evidence_provenance.is_some() => {
            Err("Snapshot preserva proveniência sem referência explícita verificável.".into())
        }
        _ => Ok((None, None)),
    }
}
fn record_bytes<'a>(
    index: &'a FileIndex,
    row: usize,
    part: &'a FilePart,
) -> Result<&'a [u8], String> {
    let line = index
        .lines
        .get(row)
        .ok_or("Registro fora da fonte admitida.")?;
    let offset = line
        .offset
        .checked_sub(part.base)
        .ok_or("Posição inválida.")?;
    let end = offset
        .checked_add(u64::from(line.len))
        .ok_or("Tamanho do registro inválido.")?;
    if end > part.mmap.len() as u64 {
        return Err("Registro fora da fonte mapeada.".into());
    }
    Ok(&part.mmap[offset as usize..end as usize])
}
fn reference(
    index: &FileIndex,
    row: usize,
    part: &FilePart,
    budget: &MaskBudget,
) -> Result<String, String> {
    let line = index
        .lines
        .get(row)
        .ok_or("Registro fora da fonte admitida.")?;
    let offset = line
        .offset
        .checked_sub(part.base)
        .ok_or("Posição da fonte inválida.")?;
    let end = offset
        .checked_add(u64::from(line.len))
        .ok_or("Tamanho do registro inválido.")?;
    if end > part.mmap.len() as u64 {
        return Err("Registro está fora da fonte mapeada.".into());
    }
    if part.format == "snapshot" {
        let bytes = &part.mmap[offset as usize..end as usize];
        if let Some(reference) = snapshot_identity(bytes, budget)?.0 {
            return Ok(reference);
        }
    }
    Ok(format!(
        "{}:{offset}",
        part.event_identity.as_deref().unwrap_or(&part.identity)
    ))
}

/// Work performed by a positional mask visit; no row IDs are retained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RowVisit {
    pub bitmap_words: usize,
    pub rows: usize,
}

/// Iterate one side of a verified dense mask without inspecting source metadata.
/// Keep the domain/cardinality checks here so optional consumers cannot silently
/// accept a truncated mask, trailing bits, or a mismatched stored cardinality.
fn visit_mask_rows(
    bits: &[u64],
    rows: usize,
    cardinality: usize,
    excluded: bool,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
    mut visit: impl FnMut(usize) -> Result<(), String>,
) -> Result<Option<RowVisit>, String> {
    let check = || {
        if cancelled() { Err("Operação cancelada.".to_string()) } else { Ok(()) }
    };
    check()?;
    let visible = rows.checked_sub(cardinality).ok_or("Máscara excede a fonte admitida.")?;
    let expected = if excluded { cardinality } else { visible };
    // Decide density before touching bitmap words or invoking the metadata reader.
    if expected > limit { return Ok(None); }
    let words = rows.div_ceil(64);
    if !(bits.len() == words || bits.is_empty() && cardinality == 0) {
        return Err("Tamanho da máscara diverge da fonte admitida.".into());
    }
    let mut work = RowVisit::default();
    for word_index in 0..words {
        if word_index % 256 == 0 { check()?; }
        let remaining = rows - word_index * 64;
        let valid = if remaining >= 64 { u64::MAX } else { (1u64 << remaining) - 1 };
        let original = bits.get(word_index).copied().unwrap_or(0);
        work.bitmap_words += usize::from(!bits.is_empty());
        if original & !valid != 0 {
            return Err("Máscara contém registros fora da fonte admitida.".into());
        }
        let mut selected = if excluded { original } else { !original & valid };
        while selected != 0 {
            if work.rows % 256 == 0 { check()?; }
            if work.rows >= expected {
                return Err("Contagem da máscara diverge da fonte admitida.".into());
            }
            let bit = selected.trailing_zeros() as usize;
            selected &= selected - 1;
            visit(word_index * 64 + bit)?;
            work.rows += 1;
        }
    }
    check()?;
    if work.rows != expected {
        return Err("Contagem da máscara diverge da fonte admitida.".into());
    }
    Ok(Some(work))
}

pub struct Mask {
    identity: Identity,
    source_generation: u64,
    scope: Scope,
    source_fingerprint: String,
    rows: usize,
    bits: Box<[u64]>,
    cardinality: usize,
    ignored_members: u64,
    parts: Vec<FilePart>,
    _metadata: Arc<crate::metadata_store::LineStore>,
    leases: Vec<MembershipLease>,
}
impl Mask {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cardinality(&self) -> usize {
        self.cardinality
    }
    pub fn is_empty(&self) -> bool {
        self.cardinality == 0
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn source_fingerprint(&self) -> &str {
        &self.source_fingerprint
    }
    pub fn source_generation(&self) -> u64 {
        self.source_generation
    }
    pub fn ignored_members(&self) -> u64 {
        self.ignored_members
    }
    pub fn contains_row(&self, row: usize) -> bool {
        row < self.rows
            && self
                .bits
                .get(row / 64)
                .is_some_and(|word| word & (1u64 << (row % 64)) != 0)
    }
    pub fn checked_contains_row(&self, row: usize) -> Result<bool, String> {
        if row >= self.rows {
            Err("Registro fora da geração de visibilidade admitida.".into())
        } else {
            Ok(self.contains_row(row))
        }
    }
    /// Visit at most `limit` selected positional rows, reading only mask words
    /// to locate them. The caller retains this mask and validates its leases.
    pub(crate) fn visit_rows(
        &self,
        excluded: bool,
        limit: usize,
        cancelled: &dyn Fn() -> bool,
        visit: impl FnMut(usize) -> Result<(), String>,
    ) -> Result<Option<RowVisit>, String> {
        visit_mask_rows(&self.bits, self.rows, self.cardinality, excluded, limit, cancelled, visit)
    }
    /// Validate once at query admission/cache reuse, never inside the row UDF.
    pub fn validate(&self) -> Result<(), String> {
        for part in &self.parts {
            crate::sources::validate_source(part)?;
        }
        for lease in &self.leases {
            lease.validate()?;
        }
        Ok(())
    }
    fn bytes(&self) -> usize {
        self.bits.len() * std::mem::size_of::<u64>()
    }
}
fn set_bit(
    bits: &mut Vec<u64>,
    row: usize,
    words: usize,
    bytes: usize,
    budget: &MaskBudget,
    cardinality: &mut usize,
) -> Result<(), String> {
    if bits.is_empty() {
        if bytes > budget.mask_bytes {
            return Err(format!(
                "A máscara exata exige {bytes} bytes e excede o orçamento de {} bytes.",
                budget.mask_bytes
            ));
        }
        bits.try_reserve_exact(words)
            .map_err(|_| "Não há memória para a máscara exata.")?;
        bits.resize(words, 0);
    }
    let bit = 1u64 << (row % 64);
    let word = bits
        .get_mut(row / 64)
        .ok_or("Registro fora da máscara exata.")?;
    if *word & bit == 0 {
        *word |= bit;
        *cardinality += 1;
    }
    Ok(())
}
fn compile(
    index: &FileIndex,
    generation: u64,
    identity: &Identity,
    view: &VerifiedView,
    binding: SourceSet,
    budget: &MaskBudget,
    work: &Work<'_>,
) -> Result<Arc<Mask>, String> {
    check(work)?;
    if view.identity != *identity {
        return Err("O ledger pertence a outra revisão de análise.".into());
    }
    for part in &index.parts {
        crate::sources::validate_source(part)?;
    }
    let source_fingerprint = binding.fingerprint.clone();
    let mut pending = Some(binding);
    let mut prepared: Option<SourceSet> = None;
    let sources = view.sources();
    let mut by_version: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut source_versions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let bytes = required_mask_bytes(index.lines.len())?;
    let words = bytes / 8;
    let mut bits = Vec::new();
    let mut cardinality = 0usize;
    let mut ignored = 0u64;
    let mut examined = 0u64;
    let mut next_rows = Vec::new();
    let mut reported = Instant::now();
    (work.progress)("compiling_visibility", 0, None);
    view.visit(work,|member|{
        check(work)?;examined=examined.checked_add(1).ok_or("Contagem excessiva.")?;
        if reported.elapsed()>=Duration::from_millis(150){(work.progress)("compiling_visibility",examined,None);reported=Instant::now();}
        if let Some(binding)=pending.take(){
            let binding=binding.prepare_aliases(index,budget,work)?;
            for(source,descriptor)in binding.descriptors.iter().enumerate(){source_versions.entry(descriptor.version.clone()).or_default().push(source);}
            for(part_index,part)in binding.part_bindings.iter().enumerate(){by_version.entry(binding.descriptors[part.source].version.clone()).or_default().push(part_index);}
            next_rows=binding.part_bindings.iter().map(|part|part.start).collect();prepared=Some(binding);
        }
        let binding=prepared.as_ref().unwrap();
        let archived=sources.get(&member.key.source_key).ok_or("Membro de exclusão sem descritor verificável.")?;
        let unresolved=binding.unproven_refs.partition_point(|(reference,_)|reference.as_str()<member.event_ref.as_str());
        for (_,source) in binding.unproven_refs[unresolved..].iter().take_while(|(reference,_)|reference==&member.event_ref){
            if binding.descriptors[*source].key()?!=member.key.source_key{return Err(unproven_snapshot());}
        }
        let matches=source_versions.get(&archived.version).map(Vec::as_slice).unwrap_or(&[]);
        if matches.is_empty(){ignored=ignored.checked_add(1).ok_or("Contagem excessiva.")?;return Ok(true)}
        for &source in matches{
            let current=&binding.descriptors[source];
            if current.record_space!=archived.record_space||current.event_ref_prefix!=archived.event_ref_prefix{return Err("A mesma versão da fonte usa enquadramento incompatível com as exclusões; recupere a configuração original.".into());}
        }
        let mut resolved=false;
        for &source in matches{
            let low=binding.aliases.partition_point(|alias|(alias.source,&alias.locator)<(source,&member.key.locator));
            for alias in binding.aliases[low..].iter().take_while(|alias|alias.source==source&&alias.locator==member.key.locator){
                let(part,_)=binding.locate(index,alias.row)?;
                if reference(index,alias.row,&index.parts[part],budget)?!=member.event_ref{return Err("Referência preservada do snapshot diverge da exclusão.".into());}
                resolved=true;
                set_bit(&mut bits,alias.row,words,bytes,budget,&mut cardinality)?;
            }
        }
        if let Some(parts)=by_version.get(&archived.version){
            let Locator::ByteOffset(offset)=member.key.locator else{return Err("Localizador sem mapeamento persistido para a fonte indexada.".into())};
            for &part_index in parts{
                let target=&binding.part_bindings[part_index];let part=&index.parts[part_index];
                let target_offset=part.base.checked_add(offset).ok_or("Localizador de exclusão inválido.")?;
                let mut row=next_rows[part_index];
                if row>=target.end||index.lines.at(row).offset!=target_offset{
                    if row<target.end&&index.lines.at(row).offset>target_offset{row=target.start;}
                    row+=index.lines.range(row..target.end).partition_point(|line|line.offset<target_offset);
                }
                if row>=target.end||index.lines.at(row).offset!=target_offset{return Err("Registro excluído não existe no enquadramento atual da mesma fonte.".into());}
                if reference(index,row,part,budget)?!=member.event_ref{return Err("Referência excluída diverge do registro admitido; a análise permanece bloqueada.".into());}
                next_rows[part_index]=row+1;
                resolved=true;
                set_bit(&mut bits,row,words,bytes,budget,&mut cardinality)?;
            }
        }
        if !resolved{ignored=ignored.checked_add(1).ok_or("Contagem excessiva.")?;}
        Ok(true)
    })?;
    check(work)?;
    view.validate()?;
    let mask = Arc::new(Mask {
        identity: identity.clone(),
        source_generation: generation,
        scope: view.scope().clone(),
        source_fingerprint,
        rows: index.lines.len(),
        bits: bits.into_boxed_slice(),
        cardinality,
        ignored_members: ignored,
        parts: index.parts.clone(),
        _metadata: Arc::clone(&index.lines),
        leases: view.leases(),
    });
    mask.validate()?;
    (work.progress)(
        "visibility_ready",
        cardinality as u64,
        Some(index.lines.len() as u64),
    );
    Ok(mask)
}

#[derive(Default)]
struct CacheState {
    entries: Vec<Arc<Mask>>,
    building: bool,
}
#[derive(Default)]
pub struct Cache {
    state: Mutex<CacheState>,
    wake: Condvar,
}
struct BuildGuard<'a>(&'a Cache);
impl Drop for BuildGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.building = false;
        self.0.wake.notify_all();
    }
}
impl Cache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get_or_compile(
        &self,
        index: &FileIndex,
        generation: u64,
        identity: &Identity,
        view: &VerifiedView,
        budget: &MaskBudget,
        work: &Work<'_>,
    ) -> Result<Arc<Mask>, String> {
        self.prepare(
            index,
            generation,
            identity,
            view.scope(),
            budget,
            work,
            |binding| compile(index, generation, identity, view, binding, budget, work),
        )
    }
    pub fn get_or_prepare(
        &self,
        index: &FileIndex,
        generation: u64,
        identity: &Identity,
        scope: &Scope,
        budget: &MaskBudget,
        work: &Work<'_>,
        load_view: impl FnOnce() -> Result<VerifiedView, String>,
    ) -> Result<Arc<Mask>, String> {
        self.prepare(
            index,
            generation,
            identity,
            scope,
            budget,
            work,
            |binding| {
                let view = load_view()?;
                if view.scope() != scope {
                    return Err("Escopo de visibilidade divergente.".into());
                }
                compile(index, generation, identity, &view, binding, budget, work)
            },
        )
    }
    fn prepare(
        &self,
        index: &FileIndex,
        generation: u64,
        identity: &Identity,
        scope: &Scope,
        budget: &MaskBudget,
        work: &Work<'_>,
        build: impl FnOnce(SourceSet) -> Result<Arc<Mask>, String>,
    ) -> Result<Arc<Mask>, String> {
        let binding = SourceSet::new(index)?;
        loop {
            check(work)?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Cache de visibilidade indisponível.")?;
            if let Some(position) = state.entries.iter().position(|mask| {
                mask.identity == *identity
                    && mask.source_generation == generation
                    && mask.scope == *scope
                    && mask.source_fingerprint == binding.fingerprint
            }) {
                let mask = state.entries.remove(position);
                if mask.bytes() > budget.mask_bytes {
                    return Err("Máscara em cache excede o orçamento atual.".into());
                }
                state.entries.push(Arc::clone(&mask));
                drop(state);
                mask.validate()?;
                return Ok(mask);
            }
            if state.building {
                let (guard, _) = self
                    .wake
                    .wait_timeout(state, Duration::from_millis(50))
                    .map_err(|_| "Cache de visibilidade indisponível.")?;
                drop(guard);
                continue;
            }
            state.building = true;
            drop(state);
            break;
        }
        let guard = BuildGuard(self);
        let result = build(binding);
        if let Ok(mask) = &result {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Cache de visibilidade indisponível.")?;
            if mask.bytes() <= budget.cache_bytes {
                while !state.entries.is_empty()
                    && (state.entries.len() >= 2
                        || state
                            .entries
                            .iter()
                            .map(|entry| entry.bytes())
                            .sum::<usize>()
                            + mask.bytes()
                            > budget.cache_bytes)
                {
                    state.entries.remove(0);
                }
                state.entries.push(Arc::clone(mask));
            }
        }
        drop(guard);
        result
    }
}

pub struct EvidenceMask {
    empty_union: bool,
    identity: Identity,
    scope: Scope,
    fingerprint: String,
    rows: usize,
    bits: Box<[u64]>,
    cardinality: usize,
    leases: Vec<MembershipLease>,
}
impl EvidenceMask {
    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cardinality(&self) -> usize {
        self.cardinality
    }
    pub fn is_empty(&self) -> bool {
        self.cardinality == 0
    }
    pub fn contains_row(&self, row: usize) -> bool {
        row < self.rows
            && self
                .bits
                .get(row / 64)
                .is_some_and(|word| word & (1u64 << (row % 64)) != 0)
    }
    pub fn checked_contains_row(&self, row: usize) -> Result<bool, String> {
        if row >= self.rows {
            Err("Evidência fora do conjunto admitido.".into())
        } else {
            Ok(self.contains_row(row))
        }
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn validate(&self) -> Result<(), String> {
        for lease in &self.leases {
            lease.validate()?;
        }
        Ok(())
    }
    fn bytes(&self) -> usize {
        self.bits.len() * 8
    }
}
fn evidence_proof<'a>(
    events: &'a [crate::model::Event],
    proofs: Option<&'a [Option<EvidenceProvenance>]>,
    row: usize,
) -> Option<&'a EvidenceProvenance> {
    proofs
        .and_then(|proofs| proofs[row].as_ref())
        .or_else(|| events[row].evidence_provenance.as_ref())
}
fn evidence_fingerprint(
    events: &[crate::model::Event],
    proofs: Option<&[Option<EvidenceProvenance>]>,
    budget: &MaskBudget,
    work: &Work<'_>,
) -> Result<String, String> {
    if proofs.is_some_and(|proofs| proofs.len() != events.len()) {
        return Err("Proveniências não correspondem aos registros admitidos.".into());
    }
    let mut hash = Sha256::new();
    hash.update(b"evidence-membership-v1");
    hash.update(events.len().to_le_bytes());
    let mut bytes = 0usize;
    for (row, event) in events.iter().enumerate() {
        if row % 256 == 0 {
            check(work)?;
        }
        let source_proof = evidence_proof(events, proofs, row);
        if let Some(proof) = source_proof {
            proof.key(&event.event_ref)?;
        }
        if event.event_ref.len() > 4096 {
            return Err("Referência estável de evidência excessiva.".into());
        }
        let proof = serde_json::to_vec(&source_proof).map_err(|e| e.to_string())?;
        bytes = bytes
            .checked_add(event.event_ref.len() + proof.len() + 16)
            .ok_or("Identidades de evidência excessivas.")?;
        if bytes > budget.evidence_bytes {
            return Err("As identidades de evidência excedem o orçamento de preparação.".into());
        }
        hash.update(event.event_ref.len().to_le_bytes());
        hash.update(event.event_ref.as_bytes());
        hash.update(proof.len().to_le_bytes());
        hash.update(proof);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn union_is_empty(view: &VerifiedView, work: &Work<'_>) -> Result<bool, String> {
    let mut empty = true;
    view.visit(work, |_| {
        empty = false;
        Ok(false)
    })?;
    view.validate()?;
    Ok(empty)
}
fn empty_evidence(
    rows: usize,
    identity: &Identity,
    scope: &Scope,
    leases: Vec<MembershipLease>,
) -> EvidenceMask {
    EvidenceMask {
        empty_union: true,
        identity: identity.clone(),
        scope: scope.clone(),
        fingerprint: String::new(),
        rows,
        bits: Box::default(),
        cardinality: 0,
        leases,
    }
}
pub fn compile_evidence(
    events: &[crate::model::Event],
    identity: &Identity,
    view: &VerifiedView,
    proofs: Option<&[Option<EvidenceProvenance>]>,
    budget: &MaskBudget,
    work: &Work<'_>,
) -> Result<EvidenceMask, String> {
    check(work)?;
    if view.identity != *identity {
        return Err("Visibilidade pertence a outra análise.".into());
    }
    if proofs.is_some_and(|proofs| proofs.len() != events.len()) {
        return Err("Proveniências não correspondem aos registros admitidos.".into());
    }
    // An actually empty union needs neither a reference map nor row-sized bits.
    if union_is_empty(view, work)? {
        return Ok(empty_evidence(
            events.len(),
            identity,
            view.scope(),
            view.leases(),
        ));
    }
    let fingerprint = evidence_fingerprint(events, proofs, budget, work)?;
    compile_evidence_known(events, identity, view, proofs, budget, work, fingerprint)
}
fn compile_evidence_known(
    events: &[crate::model::Event],
    identity: &Identity,
    view: &VerifiedView,
    proofs: Option<&[Option<EvidenceProvenance>]>,
    budget: &MaskBudget,
    work: &Work<'_>,
    fingerprint: String,
) -> Result<EvidenceMask, String> {
    if view.identity != *identity {
        return Err("Visibilidade pertence a outra análise.".into());
    }
    let bytes = required_mask_bytes(events.len())?;
    let words = bytes / 8;
    let sources = view.sources();
    let mut candidates: Option<HashMap<&str, Vec<usize>>> = None;
    let mut bits = Vec::new();
    let mut cardinality = 0;
    view.visit(work,|member|{
        check(work)?;
        if candidates.is_none(){
            let mut map:HashMap<&str,Vec<usize>>=HashMap::new();let mut charged=0usize;
            for(row,event)in events.iter().enumerate(){
                if row%256==0{check(work)?;}
                if event.event_ref.is_empty(){return Err("Evidência legada sem referência estável; preserve uma identidade de origem antes de aplicar exclusões.".into());}
                if let Some(proof)=evidence_proof(events,proofs,row){
                    proof.key(&event.event_ref)?;
                    if !sources.values().any(|source|source.version==proof.source.version){continue;}
                }else if let Some((prefix,_))=encoded_ref(&event.event_ref){
                    if !sources.values().any(|source|source.version==prefix){continue;}
                }
                charged=charged.checked_add(160).ok_or("Índice de evidências excessivo.")?;
                if charged>budget.evidence_bytes{return Err("Índice exato de evidências excede o orçamento de preparação.".into());}
                map.try_reserve(1).map_err(|_|"Não há memória para verificar a proveniência das evidências.")?;
                let rows=map.entry(event.event_ref.as_str()).or_default();rows.try_reserve(1).map_err(|_|"Não há memória para a evidência.")?;rows.push(row);
            }
            candidates=Some(map);
        }
        let Some(rows)=candidates.as_ref().unwrap().get(member.event_ref.as_str())else{return Ok(true)};
        let archived=sources.get(&member.key.source_key).ok_or("Exclusão sem fonte verificável.")?;
        for &row in rows{
            if let Some(proof)=evidence_proof(events,proofs,row){
                let key=proof.key(&events[row].event_ref)?;
                if proof.source.version!=archived.version{continue;}
                if proof.source.record_space!=archived.record_space||proof.source.event_ref_prefix!=archived.event_ref_prefix{return Err("Evidência da mesma fonte usa enquadramento incompatível com a exclusão.".into());}
                if key!=member.key{continue;}
            }else{
                let Some((prefix,offset))=events[row].event_ref.rsplit_once(':')else{return Err("Referência de snapshot sem proveniência persistida verificável.".into())};
                let Ok(offset)=offset.parse::<u64>()else{return Err("Referência de evidência não possui localizador persistido verificável.".into())};
                if prefix!=archived.version||member.key.locator!=Locator::ByteOffset(offset)||format!("{prefix}:{offset}")!=events[row].event_ref{return Err("Não é possível provar a origem desta evidência sem metadados persistidos.".into());}
                if sources.values().any(|source|source.version==prefix&&(source.record_space!=archived.record_space||source.event_ref_prefix!=archived.event_ref_prefix)){return Err("Referência legada ambígua entre enquadramentos da mesma fonte.".into());}
            }
            set_bit(&mut bits,row,words,bytes,budget,&mut cardinality)?;
        }
        Ok(true)
    })?;
    check(work)?;
    view.validate()?;
    Ok(EvidenceMask {
        empty_union: false,
        identity: identity.clone(),
        scope: view.scope().clone(),
        fingerprint,
        rows: events.len(),
        bits: bits.into_boxed_slice(),
        cardinality,
        leases: view.leases(),
    })
}
#[derive(Default)]
struct EvidenceCacheState {
    entries: Vec<Arc<EvidenceMask>>,
    building: bool,
}
#[derive(Default)]
pub struct EvidenceCache {
    state: Mutex<EvidenceCacheState>,
    wake: Condvar,
}
struct EvidenceBuildGuard<'a>(&'a EvidenceCache);
impl Drop for EvidenceBuildGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.building = false;
        self.0.wake.notify_all();
    }
}
impl EvidenceCache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get_or_prepare(
        &self,
        events: &[crate::model::Event],
        identity: &Identity,
        scope: &Scope,
        budget: &MaskBudget,
        work: &Work<'_>,
        load_view: impl FnOnce() -> Result<VerifiedView, String>,
    ) -> Result<Arc<EvidenceMask>, String> {
        let mut fingerprint = None;
        loop {
            check(work)?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Cache de evidência indisponível.")?;
            if let Some(mask) = state
                .entries
                .iter()
                .find(|mask| mask.identity == *identity && mask.scope == *scope && mask.empty_union)
                .cloned()
            {
                drop(state);
                mask.validate()?;
                return Ok(if mask.rows == events.len() {
                    mask
                } else {
                    Arc::new(empty_evidence(
                        events.len(),
                        identity,
                        scope,
                        mask.leases.clone(),
                    ))
                });
            }
            let known_nonempty = state
                .entries
                .iter()
                .any(|mask| mask.identity == *identity && mask.scope == *scope);
            if known_nonempty && fingerprint.is_none() {
                drop(state);
                fingerprint = Some(evidence_fingerprint(events, None, budget, work)?);
                continue;
            }
            if let Some(position) = state.entries.iter().position(|mask| {
                mask.identity == *identity
                    && mask.scope == *scope
                    && fingerprint.as_ref() == Some(&mask.fingerprint)
            }) {
                let mask = state.entries.remove(position);
                if mask.bytes() > budget.mask_bytes {
                    return Err("Máscara de evidência excede o orçamento atual.".into());
                }
                state.entries.push(Arc::clone(&mask));
                drop(state);
                mask.validate()?;
                return Ok(mask);
            }
            if state.building {
                let (guard, _) = self
                    .wake
                    .wait_timeout(state, Duration::from_millis(50))
                    .map_err(|_| "Cache de evidência indisponível.")?;
                drop(guard);
                continue;
            }
            state.building = true;
            drop(state);
            break;
        }
        let guard = EvidenceBuildGuard(self);
        let result = (|| {
            let view = load_view()?;
            if view.scope() != scope {
                return Err("Escopo de evidência divergente.".into());
            }
            compile_evidence(events, identity, &view, None, budget, work).map(Arc::new)
        })();
        if let Ok(mask) = &result {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "Cache de evidência indisponível.")?;
            if mask.bytes() <= budget.cache_bytes {
                while !state.entries.is_empty()
                    && (state.entries.len() >= 2
                        || state.entries.iter().map(|mask| mask.bytes()).sum::<usize>()
                            + mask.bytes()
                            > budget.cache_bytes)
                {
                    state.entries.remove(0);
                }
                state.entries.push(Arc::clone(mask));
            }
        }
        drop(guard);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusion_store::{self, Admission, Budget as LedgerBudget, Purpose};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn work() -> Work<'static> {
        Work {
            cancelled: &|| false,
            progress: &|_, _, _| {},
        }
    }
    struct Fixture {
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::case_store::save_at(dir.path(), json!({"cases":[{"id":"a"}]})).unwrap();
            Self { dir }
        }
        fn identity(&self) -> Identity {
            let data = crate::case_store::load_at(self.dir.path()).unwrap();
            serde_json::from_value::<crate::analysis_context::Snapshot>(
                data["cases"][0]["analysisContext"].clone(),
            )
            .unwrap()
            .identity()
        }
        fn index(&self, name: &str, format: &str, lines: &[String]) -> FileIndex {
            let path = self.dir.path().join(name);
            std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
            crate::sources::index_file(path.to_str().unwrap(), format, None, None, None).unwrap()
        }
        fn exclude(&self, index: &FileIndex, rows: &[usize]) -> String {
            let budget = MaskBudget::default();
            let sources = SourceSet::prepare(index, &budget, &work()).unwrap();
            let admission = Admission {
                analysis: self.identity(),
                source_receipt: json!({"generation":1}),
            };
            let staged = exclusion_store::stage(
                self.dir.path(),
                admission,
                Purpose::Exclude,
                sources.descriptors(),
                rows.iter()
                    .map(|row| sources.member_row(index, *row, &budget)),
                &LedgerBudget::default(),
                &work(),
            )
            .unwrap();
            exclusion_store::publish(
                self.dir.path(),
                staged,
                "fixture",
                "",
                json!({}),
                &LedgerBudget::default(),
                &work(),
            )
            .unwrap()
            .batch_id
        }
        fn view(&self) -> VerifiedView {
            exclusion_store::visibility(
                self.dir.path(),
                &self.identity(),
                &LedgerBudget::default(),
                &work(),
            )
            .unwrap()
        }
        fn mask(&self, index: &FileIndex) -> Arc<Mask> {
            Cache::new()
                .get_or_compile(
                    index,
                    1,
                    &self.identity(),
                    &self.view(),
                    &MaskBudget::default(),
                    &work(),
                )
                .unwrap()
        }
    }
    fn lines(n: usize) -> Vec<String> {
        (0..n)
            .map(|id| json!({"message":format!("row{id}"),"status":200}).to_string())
            .collect()
    }
    fn event(reference: &str) -> crate::model::Event {
        serde_json::from_value(json!({"id":99,"event_ref":reference,"timestamp":null,"source":"s","level":"Informação","code":"200","name":"","description":"","message":"record","raw":"","fields":{}})).unwrap()
    }
    #[test]
    fn sparse_mask_visits_complements_tail_bits_and_stored_cardinality() {
        for rows in [0usize, 1, 63, 64, 65, 131] {
            let mut bits = vec![0u64; rows.div_ceil(64)];
            let hidden: Vec<_> = (0..rows).filter(|row| row % 3 == 0).collect();
            for &row in &hidden { bits[row / 64] |= 1 << (row % 64); }
            for excluded in [false, true] {
                let expected: Vec<_> = (0..rows).filter(|row| hidden.contains(row) == excluded).collect();
                let mut actual = Vec::new();
                let work = visit_mask_rows(&bits, rows, hidden.len(), excluded, 256, &|| false, |row| {
                    actual.push(row); Ok(())
                }).unwrap().unwrap();
                assert_eq!(actual, expected);
                assert_eq!(work.rows, expected.len());
                assert_eq!(work.bitmap_words, bits.len());
            }
        }
        let never = |_| -> Result<(), String> { panic!("dense decline read metadata") };
        assert!(visit_mask_rows(&[u64::MAX], 10_000, 5_000, true, 4_096, &|| false, never).unwrap().is_none());
        for (bits, rows, cardinality) in [(vec![1], 65, 1), (vec![1 << 63], 1, 1), (vec![1], 64, 2), (vec![3], 64, 1)] {
            assert!(visit_mask_rows(&bits, rows, cardinality, true, 256, &|| false, |_| Ok(())).is_err());
        }
        let mut visible = Vec::new();
        assert_eq!(visit_mask_rows(&[], 3, 0, false, 3, &|| false, |row| {
            visible.push(row); Ok(())
        }).unwrap().unwrap(), RowVisit { bitmap_words: 0, rows: 3 });
        assert_eq!(visible, [0, 1, 2]);
        assert!(visit_mask_rows(&[1], 64, 1, true, 1, &|| true, |_| Ok(())).unwrap_err().contains("cancelad"));
        let stopped = std::cell::Cell::new(false);
        assert!(visit_mask_rows(&[u64::MAX; 8], 512, 512, true, 512, &|| stopped.get(), |_| {
            stopped.set(true); Ok(())
        }).unwrap_err().contains("cancelad"));
    }

    #[test]
    fn sparse_mask_visits_active_union_once_per_mapped_appearance() {
        let f = Fixture::new();
        let mut index = f.index("repeated.jsonl", "jsonl", &lines(5));
        let duplicate = crate::sources::index_file(
            f.dir.path().join("repeated.jsonl").to_str().unwrap(), "jsonl", None, None, None,
        ).unwrap();
        index.append(duplicate).unwrap();
        f.exclude(&index, &[1, 3]);
        f.exclude(&index, &[3, 4]);
        let mask = f.mask(&index);
        mask.validate().unwrap();
        for (excluded, expected) in [(true, vec![1, 3, 4, 6, 8, 9]), (false, vec![0, 2, 5, 7])] {
            let mut actual = Vec::new();
            let work = mask.visit_rows(excluded, 10, &|| false, |row| { actual.push(row); Ok(()) }).unwrap().unwrap();
            assert_eq!(actual, expected);
            assert_eq!(work.rows, expected.len());
            assert_eq!(work.bitmap_words, 1);
        }
    }

    #[test]
    fn indexed_mask_counts_duplicates_and_uses_positional_rows_not_numeric_identity() {
        let f = Fixture::new();
        let mut index = f.index("a.jsonl", "jsonl", &lines(5));
        let duplicate = crate::sources::index_file(
            f.dir.path().join("a.jsonl").to_str().unwrap(),
            "jsonl",
            None,
            None,
            None,
        )
        .unwrap();
        index.append(duplicate).unwrap();
        f.exclude(&index, &[1, 3]);
        let mask = f.mask(&index);
        assert_eq!(mask.rows(), 10);
        assert_eq!(mask.cardinality(), 4);
        for row in 0..10 {
            assert_eq!(mask.contains_row(row), [1, 3, 6, 8].contains(&row));
        }
        assert!(mask.checked_contains_row(10).is_err());
        fn shared<T: Send + Sync>() {}
        shared::<Mask>();
        shared::<EvidenceMask>();
    }
    #[test]
    fn canonical_reopen_keeps_exclusions_and_changed_framing_fails_closed() {
        let f = Fixture::new();
        let mut a = f.index("a.jsonl", "jsonl", &lines(3));
        a.parts[0].event_identity = Some("logical-source".into());
        f.exclude(&a, &[1]);
        let mut reopened = f.index("regenerated.jsonl", "jsonl", &lines(3));
        reopened.parts[0].event_identity = Some("logical-source".into());
        assert_eq!(f.mask(&reopened).cardinality(), 1);
        reopened.parts[0].format = "custom".into();
        let result = Cache::new().get_or_compile(
            &reopened,
            2,
            &f.identity(),
            &f.view(),
            &MaskBudget::default(),
            &work(),
        );
        assert!(result.err().unwrap().contains("enquadramento"));
    }
    #[test]
    fn unrelated_versions_have_zero_applicable_bits_but_nonzero_revision() {
        let f = Fixture::new();
        let a = f.index("a.jsonl", "jsonl", &lines(2));
        f.exclude(&a, &[0]);
        let b = f.index("b.jsonl", "jsonl", &lines(3));
        let mask = f.mask(&b);
        assert!(mask.is_empty());
        assert_eq!(mask.ignored_members(), 1);
        assert!(f.identity().visibility_revision > 0);
        assert!(mask.bits.is_empty());
    }
    #[test]
    fn lazy_mask_cache_hashes_ledger_once_and_keeps_scope_separate() {
        let f = Fixture::new();
        let index = f.index("a.jsonl", "jsonl", &lines(4));
        let a = f.exclude(&index, &[0]);
        f.exclude(&index, &[2]);
        let identity = f.identity();
        let cache = Cache::new();
        let loads = AtomicUsize::new(0);
        let first = cache
            .get_or_prepare(
                &index,
                1,
                &identity,
                &Scope::ActiveUnion,
                &MaskBudget::default(),
                &work(),
                || {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Ok(f.view())
                },
            )
            .unwrap();
        let second = cache
            .get_or_prepare(
                &index,
                1,
                &identity,
                &Scope::ActiveUnion,
                &MaskBudget::default(),
                &work(),
                || panic!("cache hit reopened ledger"),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(first.cardinality(), 2);
        let scope = Scope::ExcludedInBatch(a.clone());
        let selected = cache
            .get_or_prepare(
                &index,
                1,
                &identity,
                &scope,
                &MaskBudget::default(),
                &work(),
                || {
                    exclusion_store::batch_visibility(
                        f.dir.path(),
                        &identity,
                        &a,
                        &LedgerBudget::default(),
                        &work(),
                    )
                },
            )
            .unwrap();
        assert_eq!(selected.cardinality(), 1);
        assert!(selected.contains_row(0));
        assert!(!selected.contains_row(2));
    }
    #[test]
    fn empty_restored_union_never_parses_unrelated_large_snapshots() {
        let f = Fixture::new();
        let original = f.index("a.jsonl", "jsonl", &lines(1));
        let batch = f.exclude(&original, &[0]);
        let sources = SourceSet::prepare(&original, &MaskBudget::default(), &work()).unwrap();
        let admission = Admission {
            analysis: f.identity(),
            source_receipt: json!({}),
        };
        let selected = exclusion_store::stage(
            f.dir.path(),
            admission,
            Purpose::RestoreSelection { batch_id: batch },
            sources.descriptors(),
            [sources.member_row(&original, 0, &MaskBudget::default())],
            &LedgerBudget::default(),
            &work(),
        )
        .unwrap();
        exclusion_store::publish(
            f.dir.path(),
            selected,
            "",
            "",
            json!({}),
            &LedgerBudget::default(),
            &work(),
        )
        .unwrap();
        let snapshot = f.index(
            "snapshot.jsonl",
            "snapshot",
            &[serde_json::to_string(&event("explicit")).unwrap()],
        );
        let budget = MaskBudget {
            max_snapshot_bytes: 1,
            alias_bytes: 0,
            ..MaskBudget::default()
        };
        let mask = Cache::new()
            .get_or_compile(&snapshot, 2, &f.identity(), &f.view(), &budget, &work())
            .unwrap();
        assert!(mask.is_empty());
    }
    #[test]
    fn memory_and_snapshot_carried_provenance_match_the_same_original_record() {
        let f = Fixture::new();
        let original = f.index("source.jsonl", "jsonl", &lines(3));
        let sources = SourceSet::new(&original).unwrap();
        let member = sources
            .member_row(&original, 1, &MaskBudget::default())
            .unwrap();
        let proof = sources
            .provenance(&original, 1, &member.event_ref, &MaskBudget::default())
            .unwrap()
            .unwrap();
        f.exclude(&original, &[1]);
        let mut saved = event(&member.event_ref);
        saved.evidence_provenance = Some(proof.clone());
        let evidence = compile_evidence(
            &[saved.clone(), saved.clone()],
            &f.identity(),
            &f.view(),
            None,
            &MaskBudget::default(),
            &work(),
        )
        .unwrap();
        assert_eq!(evidence.cardinality(), 2);
        let snapshot = f.index(
            "saved.snapshot",
            "snapshot",
            &[serde_json::to_string(&saved).unwrap()],
        );
        assert_eq!(f.mask(&snapshot).cardinality(), 1);
        let aliases = SourceSet::prepare(&snapshot, &MaskBudget::default(), &work()).unwrap();
        let actual = aliases
            .member_row(&snapshot, 0, &MaskBudget::default())
            .unwrap();
        assert_eq!(actual.event_ref, member.event_ref);
        assert_eq!(aliases.descriptors()[actual.source_index], proof.source);
        let wanted = Member {
            key: proof.key(&member.event_ref).unwrap(),
            event_ref: member.event_ref,
        };
        assert_eq!(
            aliases
                .resolve_member(&snapshot, &wanted, &MaskBudget::default())
                .unwrap(),
            Some(0)
        );
    }
    #[test]
    fn explicit_snapshot_refs_are_checked_without_hydrating_an_event_vector() {
        let f = Fixture::new();
        let events = [event("evtx:channel:42"), event("")];
        let rows = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>();
        let index = f.index("events.snapshot", "snapshot", &rows);
        f.exclude(&index, &[0, 1]);
        let mask = f.mask(&index);
        assert_eq!(mask.cardinality(), 2);
        assert_eq!(
            SourceSet::new(&index)
                .unwrap()
                .member_row(&index, 0, &MaskBudget::default())
                .unwrap()
                .event_ref,
            "evtx:channel:42"
        );
        let identity = f.identity();
        assert!(compile_evidence(
            &[events[0].clone()],
            &identity,
            &f.view(),
            None,
            &MaskBudget::default(),
            &work()
        )
        .is_err());
    }
    #[test]
    fn snapshot_probe_matches_actual_event_deserialization_boundaries() {
        let value = serde_json::to_value(event("explicit")).unwrap();
        let budget = MaskBudget::default();
        for key in value.as_object().unwrap().keys() {
            for replacement in [Value::Null, json!(7), json!([]), json!({}), json!("text")] {
                let mut changed = value.clone();
                changed[key] = replacement;
                let bytes = serde_json::to_vec(&changed).unwrap();
                let expected = serde_json::from_slice::<crate::model::Event>(&bytes)
                    .ok()
                    .and_then(|event| (!event.event_ref.is_empty()).then_some(event.event_ref));
                assert_eq!(
                    snapshot_identity(&bytes, &budget).unwrap().0,
                    expected,
                    "field {key}"
                );
            }
        }
        let invalid = br#"{"id":0,"event_ref":"wrong","source":"s"}"#;
        assert_eq!(snapshot_identity(invalid, &budget).unwrap().0, None);
    }
    #[test]
    fn budgets_and_cancellation_never_return_an_unrestricted_partial_mask() {
        let f = Fixture::new();
        let index = f.index("a.jsonl", "jsonl", &lines(100));
        f.exclude(&index, &[99]);
        let budget = MaskBudget {
            mask_bytes: 1,
            ..MaskBudget::default()
        };
        assert!(Cache::new()
            .get_or_compile(&index, 1, &f.identity(), &f.view(), &budget, &work())
            .is_err());
        let cancelled = Work {
            cancelled: &|| true,
            progress: &|_, _, _| {},
        };
        assert!(Cache::new()
            .get_or_compile(
                &index,
                1,
                &f.identity(),
                &f.view(),
                &MaskBudget::default(),
                &cancelled
            )
            .is_err());
        let legacy = event("");
        assert!(compile_evidence(
            &[legacy],
            &f.identity(),
            &f.view(),
            None,
            &MaskBudget::default(),
            &work()
        )
        .is_err());
    }
    #[test]
    fn evidence_cache_uses_refs_proofs_and_scope_but_not_mutable_body_text() {
        let f = Fixture::new();
        let index = f.index("a.jsonl", "jsonl", &lines(2));
        let set = SourceSet::new(&index).unwrap();
        let member = set.member_row(&index, 0, &MaskBudget::default()).unwrap();
        f.exclude(&index, &[0]);
        let identity = f.identity();
        let cache = EvidenceCache::new();
        let mut records = vec![event(&member.event_ref)];
        let first = cache
            .get_or_prepare(
                &records,
                &identity,
                &Scope::ActiveUnion,
                &MaskBudget::default(),
                &work(),
                || Ok(f.view()),
            )
            .unwrap();
        records[0].message = "changed overlay only".into();
        let second = cache
            .get_or_prepare(
                &records,
                &identity,
                &Scope::ActiveUnion,
                &MaskBudget::default(),
                &work(),
                || panic!("body edit reopened membership"),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.cardinality(), 1);
    }
    #[test]
    fn shared_preparation_loads_once_for_simultaneous_callers() {
        let f = Fixture::new();
        let index = f.index("a.jsonl", "jsonl", &lines(20));
        f.exclude(&index, &[5]);
        let identity = f.identity();
        let cache = Cache::new();
        let loads = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..3)
                .map(|_| {
                    let cache = &cache;
                    let index = &index;
                    let identity = &identity;
                    let loads = &loads;
                    let barrier = &barrier;
                    let f = &f;
                    scope.spawn(move || {
                        barrier.wait();
                        cache
                            .get_or_prepare(
                                index,
                                1,
                                identity,
                                &Scope::ActiveUnion,
                                &MaskBudget::default(),
                                &work(),
                                || {
                                    loads.fetch_add(1, Ordering::SeqCst);
                                    Ok(f.view())
                                },
                            )
                            .unwrap()
                            .cardinality()
                    })
                })
                .collect();
            for job in jobs {
                assert_eq!(job.join().unwrap(), 1);
            }
        });
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn dense_budget_is_checked_and_word_rounded() {
        assert_eq!(required_mask_bytes(0).unwrap(), 0);
        assert_eq!(required_mask_bytes(1).unwrap(), 8);
        assert_eq!(required_mask_bytes(64).unwrap(), 8);
        assert_eq!(required_mask_bytes(65).unwrap(), 16);
        assert_eq!(required_mask_bytes(50_000_000).unwrap(), 6_250_000);
        assert!(required_mask_bytes(usize::MAX).is_err());
    }
    #[test]
    fn restored_empty_evidence_union_needs_no_reference_or_bitmap_budget() {
        let f = Fixture::new();
        let index = f.index("a.jsonl", "jsonl", &lines(2));
        let batch = f.exclude(&index, &[0]);
        exclusion_store::restore_batch(
            f.dir.path(),
            &Admission {
                analysis: f.identity(),
                source_receipt: json!({}),
            },
            &batch,
            &work(),
        )
        .unwrap();
        let identity = f.identity();
        assert!(identity.visibility_revision > 0);
        let records = vec![event(""); 3];
        let budget = MaskBudget {
            evidence_bytes: 0,
            mask_bytes: 0,
            ..Default::default()
        };
        assert!(
            compile_evidence(&records, &identity, &f.view(), None, &budget, &work())
                .unwrap()
                .is_empty()
        );
        let cache = EvidenceCache::new();
        let first = cache
            .get_or_prepare(
                &records,
                &identity,
                &Scope::ActiveUnion,
                &budget,
                &work(),
                || Ok(f.view()),
            )
            .unwrap();
        assert_eq!(first.rows(), 3);
        assert!(first.is_empty());
        let second = cache
            .get_or_prepare(
                &records[..1],
                &identity,
                &Scope::ActiveUnion,
                &budget,
                &work(),
                || panic!("empty union was reopened"),
            )
            .unwrap();
        assert_eq!(second.rows(), 1);
        assert!(second.is_empty());
    }
    #[test]
    fn foreign_legacy_snapshot_ref_keeps_raw_access_but_required_visibility_fails_closed() {
        let f = Fixture::new();
        let original = f.index("a.jsonl", "jsonl", &lines(2));
        let source = SourceSet::new(&original).unwrap();
        let reference = source
            .member_row(&original, 0, &MaskBudget::default())
            .unwrap()
            .event_ref;
        assert!(encoded_ref(&reference).is_some());
        let snapshot = f.index(
            "old.snapshot",
            "snapshot",
            &[serde_json::to_string(&event(&reference)).unwrap()],
        );
        let set = SourceSet::new(&snapshot).unwrap();
        assert!(set
            .provenance(&snapshot, 0, &reference, &MaskBudget::default())
            .unwrap()
            .is_none());
        assert!(set
            .member_row(&snapshot, 0, &MaskBudget::default())
            .unwrap_err()
            .contains(UNPROVEN_SNAPSHOT_ORIGIN));
        assert!(f.mask(&snapshot).is_empty());
        f.exclude(&original, &[0]);
        let error = Cache::new()
            .get_or_compile(
                &snapshot,
                1,
                &f.identity(),
                &f.view(),
                &MaskBudget::default(),
                &work(),
            )
            .err()
            .unwrap();
        assert!(error.contains(UNPROVEN_SNAPSHOT_ORIGIN), "{error}");
    }
    #[test]
    fn copied_opaque_snapshot_ref_without_origin_proof_cannot_reveal_excluded_row() {
        let f = Fixture::new();
        let record = serde_json::to_string(&event("evtx:channel:42")).unwrap();
        let original = f.index("a.snapshot", "snapshot", &[record.clone()]);
        let copied = f.index("b.snapshot", "snapshot", &[record]);
        f.exclude(&original, &[0]);
        assert_eq!(f.mask(&original).cardinality(), 1);
        let error = Cache::new()
            .get_or_compile(
                &copied,
                2,
                &f.identity(),
                &f.view(),
                &MaskBudget::default(),
                &work(),
            )
            .err()
            .unwrap();
        assert!(error.contains(UNPROVEN_SNAPSHOT_ORIGIN), "{error}");
    }
    #[test]
    fn two_valid_provenance_keys_for_one_snapshot_row_are_both_applicable() {
        let f = Fixture::new();
        let original = f.index("origin.jsonl", "jsonl", &lines(1));
        let source = SourceSet::new(&original).unwrap();
        let member = source
            .member_row(&original, 0, &MaskBudget::default())
            .unwrap();
        let mut saved = event(&member.event_ref);
        saved.evidence_provenance = source
            .provenance(&original, 0, &member.event_ref, &MaskBudget::default())
            .unwrap();
        let snapshot = f.index(
            "snapshot.jsonl",
            "snapshot",
            &[serde_json::to_string(&saved).unwrap()],
        );
        f.exclude(&original, &[0]);
        // A historical container-bound member and the carried original member
        // both identify this row; deduplicating the bit must not mark either
        // valid provenance key as unavailable.
        let container = SourceSet::new(&snapshot).unwrap();
        let staged = exclusion_store::stage(
            f.dir.path(),
            Admission {
                analysis: f.identity(),
                source_receipt: json!({}),
            },
            Purpose::Exclude,
            container.descriptors(),
            [Ok(InputMember {
                source_index: 0,
                locator: Locator::ByteOffset(0),
                event_ref: member.event_ref,
            })],
            &LedgerBudget::default(),
            &work(),
        )
        .unwrap();
        exclusion_store::publish(
            f.dir.path(),
            staged,
            "historical",
            "",
            json!({}),
            &LedgerBudget::default(),
            &work(),
        )
        .unwrap();
        let mask = f.mask(&snapshot);
        assert_eq!(mask.cardinality(), 1);
        assert_eq!(mask.ignored_members(), 0);
    }
}
