//! One-time migration of authored Case anchors. Only references present in
//! metadata are indexed; record payload bytes are never rewritten.
use crate::{
    case_evidence::{self, MemberHandle, RawJson},
    case_work_budget::{self, Lease},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

const LIMIT: &str = "CASE_ANCHOR_LIMIT";
const INVALID: &str = "CASE_ANCHOR_IDENTITY_CHANGED";
const MAX_ENTRIES: usize = 10_000;
const MAX_BYTES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockedState {
    Missing,
    Ambiguous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockedReason {
    KeyCollision,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BlockedAlias {
    pub alias: String,
    pub state: BlockedState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<BlockedReason>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AnchorMap {
    pub schema_version: u32,
    pub event_aliases: Vec<BlockedAlias>,
    pub item_aliases: Vec<BlockedAlias>,
}
impl Default for AnchorMap {
    fn default() -> Self {
        Self {
            schema_version: 1,
            event_aliases: vec![],
            item_aliases: vec![],
        }
    }
}
impl AnchorMap {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || self
                .event_aliases
                .len()
                .checked_add(self.item_aliases.len())
                .ok_or(LIMIT)?
                > MAX_ENTRIES
        {
            return Err(LIMIT.into());
        }
        for list in [&self.event_aliases, &self.item_aliases] {
            let mut previous = None;
            for entry in list {
                if entry.alias.len() > 4096
                    || previous.is_some_and(|p: &str| p >= entry.alias.as_str())
                    || (entry.reason.is_some() && entry.state != BlockedState::Ambiguous)
                {
                    return Err(INVALID.into());
                }
                previous = Some(entry.alias.as_str());
            }
        }
        encoded_len(self, MAX_BYTES)?;
        Ok(())
    }
    /// The automatic a:e: alias has the same original ambiguity as e:.
    pub(crate) fn blocked_event(&self, alias: &str) -> Option<&BlockedAlias> {
        let original = canonical_alias(alias);
        self.event_aliases
            .iter()
            .find(|entry| canonical_alias(&entry.alias) == original)
    }
}
pub(crate) struct FrozenAnchors {
    pub anchors: AnchorMap,
    pub credit: Lease,
}
#[derive(Clone, Debug)]
pub(crate) struct ItemBinding {
    pub item_index: u32,
    pub container_id: String,
    pub member_count: u32,
}

struct Count {
    bytes: usize,
    limit: usize,
}
impl Write for Count {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(b.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other(LIMIT))?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encoded_len(value: &impl Serialize, limit: usize) -> Result<usize, String> {
    let mut w = Count { bytes: 0, limit };
    serde_json::to_writer(&mut w, value).map_err(|_| LIMIT)?;
    Ok(w.bytes)
}
fn reserve(bytes: usize) -> Result<Lease, String> {
    crate::case_cache::reserve_work(case_work_budget::global(), bytes)
}
fn admit(credit: &mut Lease, bytes: usize) -> Result<(), String> {
    credit.merge(reserve(bytes)?)
}
fn canonical_alias(value: &str) -> &str {
    value
        .strip_prefix("a:")
        .filter(|v| v.starts_with("e:"))
        .unwrap_or(value)
}
fn legacy(value: &str) -> bool {
    canonical_alias(value).starts_with("e:")
}
fn native(member: &MemberHandle) -> String {
    format!("n:{}:{}", member.container_id, member.occurrence_id)
}
fn valid_uuid(value: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| INVALID.into())
}
fn array(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[derive(Clone, Debug)]
enum Resolution {
    Pending { count: u32, target: Option<String> },
    Blocked(BlockedState, Option<BlockedReason>),
}
impl Resolution {
    fn add(&mut self, target: String) {
        if let Self::Pending {
            count,
            target: found,
        } = self
        {
            *count = count.saturating_add(1);
            *found = if *count == 1 { Some(target) } else { None };
        }
    }
    fn target(&self) -> Option<&str> {
        match self {
            Self::Pending {
                count: 1,
                target: Some(v),
            } => Some(v),
            _ => None,
        }
    }
    fn blocked(&self) -> Option<(BlockedState, Option<BlockedReason>)> {
        match self {
            Self::Blocked(s, r) => Some((*s, *r)),
            Self::Pending { count: 0, .. } => Some((BlockedState::Missing, None)),
            Self::Pending { count, .. } if *count > 1 => Some((BlockedState::Ambiguous, None)),
            _ => None,
        }
    }
}
fn insert_candidate(
    map: &mut BTreeMap<String, Resolution>,
    alias: &str,
    prior: Option<&BlockedAlias>,
    credit: &mut Lease,
    bytes: &mut usize,
    count: &mut usize,
) -> Result<(), String> {
    if map.contains_key(alias) {
        return Ok(());
    }
    if alias.len() > 4096 || *count >= MAX_ENTRIES {
        return Err(LIMIT.into());
    }
    *bytes = bytes
        .checked_add(encoded_len(&alias, MAX_BYTES)? + 96)
        .filter(|n| *n <= MAX_BYTES)
        .ok_or(LIMIT)?;
    admit(credit, 512 + alias.len().checked_mul(4).ok_or(LIMIT)?)?;
    *count += 1;
    map.insert(
        alias.into(),
        prior
            .map(|p| Resolution::Blocked(p.state, p.reason))
            .unwrap_or(Resolution::Pending {
                count: 0,
                target: None,
            }),
    );
    Ok(())
}
fn visit_aliases(
    metadata: &Value,
    mut visit: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let Some(timeline) = metadata.get("timeline") else {
        return Ok(());
    };
    for field in ["edits", "layout"] {
        if let Some(map) = timeline.get(field).and_then(Value::as_object) {
            for key in map.keys() {
                visit(key)?;
            }
        }
    }
    for group in array(timeline.get("groups")) {
        for value in array(group.get("ids")) {
            if let Some(value) = value.as_str() {
                visit(value)?;
            }
        }
    }
    for note in array(timeline.get("annotations")) {
        if let Some(anchor) = note.get("anchor").and_then(Value::as_str) {
            visit(anchor)?;
        }
        for link in array(note.get("links")) {
            if let Some(target) = link.get("targetId").and_then(Value::as_str) {
                visit(target)?;
            }
        }
    }
    Ok(())
}
fn authored_ids(metadata: &Value, credit: &mut Lease) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    if let Some(timeline) = metadata.get("timeline") {
        for field in ["groups", "annotations"] {
            for value in array(timeline.get(field)) {
                if let Some(id) = value.get("id").and_then(Value::as_str) {
                    if id.len() > 4096 || ids.len() >= MAX_ENTRIES {
                        return Err(LIMIT.into());
                    }
                    admit(credit, 128 + id.len() * 2)?;
                    ids.insert(id.into());
                }
            }
        }
    }
    Ok(ids)
}
fn visit_item_lists(
    metadata: &Value,
    mut visit: impl FnMut(usize, usize, &Value) -> Result<(), String>,
) -> Result<(), String> {
    for (kind, (list, field)) in [
        (array(metadata.get("caseTrails")), "itemIds"),
        (
            array(metadata.get("intel").and_then(|v| v.get("hypotheses"))),
            "items",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        for (index, entry) in list.iter().enumerate() {
            if entry.get("itemRefs").is_none() {
                if let Some(values) = entry.get(field).filter(|v| v.is_array()) {
                    visit(kind, index, values)?;
                }
            }
        }
    }
    Ok(())
}
fn suffix(envelope: &str, origin: u32) -> Result<String, String> {
    let raw = RawJson::checked(envelope)?;
    if raw.kind() != b'{' {
        return Err("CASE_ANCHOR_ALIAS_UNAVAILABLE".into());
    }
    let (mut reference, mut id) = (None, None);
    let mut fields = raw.members()?;
    while let Some(field) = fields.next()? {
        match field.key()?.as_str() {
            "event_ref" => {
                if reference.replace(field.value).is_some() {
                    return Err(INVALID.into());
                }
            }
            "id" => {
                if id.replace(field.value).is_some() {
                    return Err(INVALID.into());
                }
            }
            _ => {}
        }
    }
    if let Some(reference) = reference {
        if reference.kind() == b'"' {
            let value: String = serde_json::from_str(reference.get()).map_err(|_| INVALID)?;
            if !value.is_empty() {
                return Ok(value);
            }
        } else if !matches!(reference.kind(), b'n' | b'f') && reference.get() != "0" {
            return Err("CASE_ANCHOR_ALIAS_UNAVAILABLE".into());
        }
    }
    if let Some(id) = id {
        if id.kind() == b'"' {
            return serde_json::from_str(id.get()).map_err(|_| INVALID.into());
        }
        if id.kind() != b'n' {
            return id
                .get()
                .parse::<u64>()
                .map(|v| v.to_string())
                .map_err(|_| "CASE_ANCHOR_ALIAS_UNAVAILABLE".into());
        }
    }
    Ok(origin.to_string())
}
fn alias_digest(item_id: &str, suffix: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"e:");
    h.update(item_id.as_bytes());
    h.update(b":");
    h.update(suffix.as_bytes());
    h.finalize().into()
}
fn block_key_collisions(metadata: &Value, aliases: &mut BTreeMap<String, Resolution>) {
    let mut blocked = BTreeSet::new();
    for field in ["edits", "layout"] {
        let Some(map) = metadata
            .get("timeline")
            .and_then(|v| v.get(field))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let mut destinations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for key in map.keys() {
            if let Some(target) = aliases.get(key).and_then(Resolution::target) {
                destinations
                    .entry(target.into())
                    .or_default()
                    .push(key.clone());
            }
        }
        for (target, keys) in destinations {
            if (map.contains_key(&target) && keys.iter().any(|key| key != &target))
                || keys.len() > 1
            {
                blocked.extend(keys);
            }
        }
    }
    for key in blocked {
        aliases.insert(
            key,
            Resolution::Blocked(BlockedState::Ambiguous, Some(BlockedReason::KeyCollision)),
        );
    }
}

fn rewrite_value(value: &mut Value, aliases: &BTreeMap<String, Resolution>) {
    if let Some(target) = value
        .as_str()
        .and_then(|id| aliases.get(id))
        .and_then(Resolution::target)
    {
        *value = Value::String(target.into());
    }
}
fn rewrite_aliases(metadata: &mut Value, aliases: &BTreeMap<String, Resolution>) {
    let Some(timeline) = metadata.get_mut("timeline") else {
        return;
    };
    for field in ["edits", "layout"] {
        if let Some(map) = timeline.get_mut(field).and_then(Value::as_object_mut) {
            for (alias, resolution) in aliases {
                if let Some(target) = resolution.target() {
                    if let Some(value) = map.remove(alias) {
                        map.insert(target.into(), value);
                    }
                }
            }
        }
    }
    if let Some(groups) = timeline.get_mut("groups").and_then(Value::as_array_mut) {
        for group in groups {
            if let Some(ids) = group.get_mut("ids").and_then(Value::as_array_mut) {
                for id in ids {
                    rewrite_value(id, aliases);
                }
            }
        }
    }
    if let Some(notes) = timeline
        .get_mut("annotations")
        .and_then(Value::as_array_mut)
    {
        for note in notes {
            if let Some(anchor) = note.get_mut("anchor") {
                rewrite_value(anchor, aliases);
            }
            if let Some(links) = note.get_mut("links").and_then(Value::as_array_mut) {
                for link in links {
                    if let Some(target) = link.get_mut("targetId") {
                        rewrite_value(target, aliases);
                    }
                }
            }
        }
    }
}
fn blocked_entries(map: &BTreeMap<String, Resolution>) -> Vec<BlockedAlias> {
    map.iter()
        .filter_map(|(alias, value)| {
            value.blocked().map(|(state, reason)| BlockedAlias {
                alias: alias.clone(),
                state,
                reason,
            })
        })
        .collect()
}

/// Only invoke during first adoption/import over complete original ItemRows.
/// `prior` is transported authoritative ambiguity, never inferred from a later
/// membership. All fallible preparation finishes before metadata is changed.
pub(crate) fn freeze(
    metadata: &mut Value,
    bindings: &[ItemBinding],
    prior: &AnchorMap,
    visit: impl FnOnce(
        &mut dyn FnMut(u32, u32, MemberHandle, &str) -> Result<(), String>,
    ) -> Result<(), String>,
) -> Result<FrozenAnchors, String> {
    crate::operations::check()?;
    prior.validate()?;
    let original_bytes = encoded_len(metadata, case_evidence::VIEW_DOCUMENT_BYTES)?;
    let mut credit = reserve(256 << 10)?;
    let ids = authored_ids(metadata, &mut credit)?;
    let (mut aliases, mut items) = (BTreeMap::new(), BTreeMap::new());
    let (mut bytes, mut entries) = (0usize, 0usize);
    for entry in &prior.event_aliases {
        insert_candidate(
            &mut aliases,
            &entry.alias,
            Some(entry),
            &mut credit,
            &mut bytes,
            &mut entries,
        )?;
    }
    for entry in &prior.item_aliases {
        insert_candidate(
            &mut items,
            &entry.alias,
            Some(entry),
            &mut credit,
            &mut bytes,
            &mut entries,
        )?;
    }
    visit_aliases(metadata, |alias| {
        crate::operations::check()?;
        if legacy(alias) && !ids.contains(alias) {
            insert_candidate(
                &mut aliases,
                alias,
                prior.blocked_event(alias),
                &mut credit,
                &mut bytes,
                &mut entries,
            )?;
        }
        Ok(())
    })?;
    visit_item_lists(metadata, |_, _, values| {
        for value in array(Some(values)) {
            crate::operations::check()?;
            if let Some(id) = value.as_str() {
                let old = prior.item_aliases.iter().find(|p| p.alias == id);
                insert_candidate(&mut items, id, old, &mut credit, &mut bytes, &mut entries)?;
            }
        }
        Ok(())
    })?;
    let original_items = array(metadata.get("items"));
    admit(&mut credit, bindings.len().checked_mul(256).ok_or(LIMIT)?)?;
    let mut by_index = BTreeMap::new();
    let mut expected = 0u32;
    for binding in bindings {
        valid_uuid(&binding.container_id)?;
        if binding.item_index as usize >= original_items.len()
            || by_index.insert(binding.item_index, binding).is_some()
        {
            return Err(INVALID.into());
        }
        expected = expected.checked_add(binding.member_count).ok_or(LIMIT)?;
    }
    for (index, item) in original_items.iter().enumerate() {
        crate::operations::check()?;
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            if let Some(resolution) = items.get_mut(id) {
                resolution.add(
                    by_index
                        .get(&(index as u32))
                        .map(|b| format!("container:{}", b.container_id))
                        .unwrap_or_else(|| format!("item:{id}")),
                );
            }
        }
    }
    let mut digests: BTreeMap<[u8; 32], Vec<String>> = BTreeMap::new();
    for (alias, resolution) in &aliases {
        if matches!(resolution, Resolution::Pending { .. }) {
            digests
                .entry(Sha256::digest(canonical_alias(alias).as_bytes()).into())
                .or_default()
                .push(alias.clone());
        }
    }
    if !digests.is_empty() {
        let _encoded = reserve(case_evidence::ENVELOPE_BYTES)?;
        let _scratch = reserve(2 * case_evidence::ENVELOPE_BYTES)?;
        let mut seen = 0u32;
        let mut per_item: BTreeMap<u32, u32> = BTreeMap::new();
        let mut unavailable_items = BTreeSet::new();
        let mut previous = None;
        visit(&mut |item_index, origin, member, envelope| {
            crate::operations::check()?;
            seen = seen
                .checked_add(1)
                .filter(|n| *n <= expected)
                .ok_or(INVALID)?;
            let position = (item_index, origin);
            if previous.is_some_and(|p| p >= position) {
                return Err(INVALID.into());
            }
            previous = Some(position);
            let binding = by_index.get(&item_index).ok_or(INVALID)?;
            let observed = per_item.entry(item_index).or_default();
            *observed = observed
                .checked_add(1)
                .filter(|n| *n <= binding.member_count)
                .ok_or(INVALID)?;
            if member.container_id != binding.container_id {
                return Err(INVALID.into());
            }
            valid_uuid(&member.manifest_id)?;
            valid_uuid(&member.occurrence_id)?;
            let item_id = original_items[item_index as usize]
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("undefined");
            if unavailable_items.contains(&item_index) {
                return Ok(());
            }
            let suffix = match suffix(envelope, origin) {
                Ok(suffix) => suffix,
                Err(_) => {
                    crate::operations::check()?;
                    // An unsupported identity cannot prove any authored alias
                    // in this item unique. Keep those original notes unresolved.
                    if item_id.len() > 4096 {
                        return Ok(());
                    }
                    let prefix = format!("e:{item_id}:");
                    for (alias, resolution) in &mut aliases {
                        if canonical_alias(alias).starts_with(&prefix)
                            && matches!(resolution, Resolution::Pending { .. })
                        {
                            *resolution = Resolution::Blocked(BlockedState::Ambiguous, None);
                        }
                    }
                    unavailable_items.insert(item_index);
                    return Ok(());
                }
            };
            if let Some(keys) = digests.get(&alias_digest(item_id, &suffix)) {
                for key in keys {
                    aliases.get_mut(key).ok_or(INVALID)?.add(native(&member));
                }
            }
            Ok(())
        })?;
        if seen != expected
            || bindings
                .iter()
                .any(|b| per_item.get(&b.item_index).copied().unwrap_or(0) != b.member_count)
        {
            return Err(INVALID.into());
        }
    }
    drop(digests);
    block_key_collisions(metadata, &mut aliases);
    // Keep seed decisions even when old notes were temporarily removed.
    let anchors = AnchorMap {
        schema_version: 1,
        event_aliases: blocked_entries(&aliases),
        item_aliases: blocked_entries(&items),
    };
    anchors.validate()?;
    admit(
        &mut credit,
        encoded_len(&anchors, MAX_BYTES)?
            .checked_mul(4)
            .ok_or(LIMIT)?,
    )?;
    let mut planned = Vec::new();
    let (mut growth, mut positions) = (0usize, 0usize);
    visit_item_lists(metadata, |kind, index, values| {
        let mut refs = Vec::new();
        for value in array(Some(values)) {
            crate::operations::check()?;
            positions = positions
                .checked_add(1)
                .filter(|n| *n <= MAX_ENTRIES)
                .ok_or(LIMIT)?;
            let item_id = value.as_str();
            admit(&mut credit, 1024 + item_id.map_or(0, str::len) * 4)?;
            let reference = if let Some(id) = item_id {
                match items.get(id).ok_or(INVALID)? {
                    resolution if resolution.target().is_some() => {
                        let target = resolution.target().unwrap();
                        if let Some(container) = target.strip_prefix("container:") {
                            serde_json::json!({"kind":"container","containerId":container})
                        } else {
                            serde_json::json!({"kind":"item","itemId":id})
                        }
                    }
                    resolution => {
                        let (state, _) = resolution.blocked().ok_or(INVALID)?;
                        serde_json::json!({"kind":"unresolved_legacy_item","itemId":id,"state":state})
                    }
                }
            } else {
                serde_json::json!({"kind":"unresolved_legacy_item","itemId":null,"state":"unsupported"})
            };
            refs.push(reference);
        }
        let refs = Value::Array(refs);
        let size = encoded_len(&refs, MAX_BYTES)?;
        growth = growth
            .checked_add(size + 16)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or(LIMIT)?;
        planned.push((kind, index, refs));
        Ok(())
    })?;
    visit_aliases(metadata, |alias| {
        if let Some(target) = aliases.get(alias).and_then(Resolution::target) {
            let delta =
                encoded_len(&target, MAX_BYTES)?.saturating_sub(encoded_len(&alias, MAX_BYTES)?);
            growth = growth
                .checked_add(delta)
                .filter(|n| *n <= MAX_BYTES)
                .ok_or(LIMIT)?;
        }
        Ok(())
    })?;
    if original_bytes
        .checked_add(growth)
        .filter(|n| *n <= case_evidence::VIEW_DOCUMENT_BYTES)
        .is_none()
    {
        return Err(LIMIT.into());
    }
    admit(&mut credit, growth.checked_mul(4).ok_or(LIMIT)?)?;
    crate::operations::check()?;
    rewrite_aliases(metadata, &aliases);
    for (kind, index, refs) in planned {
        let list = if kind == 0 {
            metadata.get_mut("caseTrails")
        } else {
            metadata
                .get_mut("intel")
                .and_then(|v| v.get_mut("hypotheses"))
        };
        // Paths and object shape were collected from this same unmodified tree.
        if let Some(entry) = list
            .and_then(Value::as_array_mut)
            .and_then(|a| a.get_mut(index))
            .and_then(Value::as_object_mut)
        {
            entry.insert("itemRefs".into(), refs);
        }
    }
    Ok(FrozenAnchors { anchors, credit })
}

fn native_parts(alias: &str) -> Result<(&str, &str), String> {
    let rest = alias.strip_prefix("n:").ok_or(INVALID)?;
    let (container, occurrence) = rest.split_once(':').ok_or(INVALID)?;
    valid_uuid(container)?;
    valid_uuid(occurrence)?;
    Ok((container, occurrence))
}
/// Sparse native identities actually named by authored metadata. Frozen missing
/// aliases stay in AnchorMap and never become candidates for later retargeting.
pub(crate) struct NativeTargets {
    pub members: BTreeSet<(String, String)>,
    pub containers: BTreeSet<String>,
    pub credit: Lease,
}
pub(crate) fn requested_native_targets(
    metadata: &Value,
    prior: &AnchorMap,
) -> Result<NativeTargets, String> {
    prior.validate()?;
    let mut credit = reserve(64 << 10)?;
    let ids = authored_ids(metadata, &mut credit)?;
    let mut members = BTreeSet::new();
    let mut containers = BTreeSet::new();
    visit_aliases(metadata, |alias| {
        crate::operations::check()?;
        if !alias.starts_with("n:") || ids.contains(alias) || prior.blocked_event(alias).is_some() {
            return Ok(());
        }
        let (container, occurrence) = native_parts(alias)?;
        let target = (container.to_owned(), occurrence.to_owned());
        if !members.contains(&target) {
            if members.len() + containers.len() >= MAX_ENTRIES {
                return Err(LIMIT.into());
            }
            admit(&mut credit, 512)?;
            members.insert(target);
        }
        Ok(())
    })?;
    for list in [
        array(metadata.get("caseTrails")),
        array(metadata.get("intel").and_then(|v| v.get("hypotheses"))),
    ] {
        for entry in list {
            for reference in array(entry.get("itemRefs")) {
                crate::operations::check()?;
                if reference.get("kind").and_then(Value::as_str) != Some("container") {
                    continue;
                }
                let id = reference
                    .get("containerId")
                    .and_then(Value::as_str)
                    .ok_or(INVALID)?;
                valid_uuid(id)?;
                if !containers.contains(id) {
                    if members.len() + containers.len() >= MAX_ENTRIES {
                        return Err(LIMIT.into());
                    }
                    admit(&mut credit, 256)?;
                    containers.insert(id.to_owned());
                }
            }
        }
    }
    Ok(NativeTargets {
        members,
        containers,
        credit,
    })
}
/// Portable callers provide verified old-to-new identities for only the authored
/// native anchors and typed item refs. Missing targets remain frozen unresolved.
/// No record-content comparison and no all-row mapping is built here.
pub(crate) fn remap_native(
    metadata: &mut Value,
    prior: &AnchorMap,
    occurrence: &mut dyn FnMut(&str, &str) -> Result<Option<(String, String)>, String>,
    container: &mut dyn FnMut(&str) -> Result<Option<String>, String>,
) -> Result<FrozenAnchors, String> {
    crate::operations::check()?;
    prior.validate()?;
    let original_bytes = encoded_len(metadata, case_evidence::VIEW_DOCUMENT_BYTES)?;
    let mut credit = reserve(256 << 10)?;
    let ids = authored_ids(metadata, &mut credit)?;
    let (mut aliases, mut item_aliases) = (BTreeMap::new(), BTreeMap::new());
    let (mut bytes, mut count) = (0usize, 0usize);
    for entry in &prior.event_aliases {
        insert_candidate(
            &mut aliases,
            &entry.alias,
            Some(entry),
            &mut credit,
            &mut bytes,
            &mut count,
        )?;
    }
    for entry in &prior.item_aliases {
        insert_candidate(
            &mut item_aliases,
            &entry.alias,
            Some(entry),
            &mut credit,
            &mut bytes,
            &mut count,
        )?;
    }
    visit_aliases(metadata, |alias| {
        if alias.starts_with("n:") && !ids.contains(alias) {
            insert_candidate(
                &mut aliases,
                alias,
                prior.blocked_event(alias),
                &mut credit,
                &mut bytes,
                &mut count,
            )?;
        }
        Ok(())
    })?;
    for (alias, resolution) in &mut aliases {
        crate::operations::check()?;
        if !matches!(resolution, Resolution::Pending { .. }) {
            continue;
        }
        let (old_container, old_occurrence) = native_parts(alias)?;
        match occurrence(old_container, old_occurrence)? {
            Some((container, occurrence)) => {
                valid_uuid(&container)?;
                valid_uuid(&occurrence)?;
                resolution.add(format!("n:{container}:{occurrence}"));
            }
            None => *resolution = Resolution::Blocked(BlockedState::Missing, None),
        }
    }
    block_key_collisions(metadata, &mut aliases);
    let anchors = AnchorMap {
        schema_version: 1,
        event_aliases: blocked_entries(&aliases),
        item_aliases: blocked_entries(&item_aliases),
    };
    anchors.validate()?;
    admit(
        &mut credit,
        encoded_len(&anchors, MAX_BYTES)?
            .checked_mul(4)
            .ok_or(LIMIT)?,
    )?;
    let mut mapped: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut planned = Vec::new();
    let (mut growth, mut positions) = (0usize, 0usize);
    for (kind, list) in [
        array(metadata.get("caseTrails")),
        array(metadata.get("intel").and_then(|v| v.get("hypotheses"))),
    ]
    .into_iter()
    .enumerate()
    {
        for (index, entry) in list.iter().enumerate() {
            for (position, reference) in array(entry.get("itemRefs")).iter().enumerate() {
                crate::operations::check()?;
                // Frozen unresolved variants are deliberately never reclassified.
                if reference.get("kind").and_then(Value::as_str) != Some("container") {
                    continue;
                }
                let old = reference
                    .get("containerId")
                    .and_then(Value::as_str)
                    .ok_or(INVALID)?;
                valid_uuid(old)?;
                positions = positions
                    .checked_add(1)
                    .filter(|n| *n <= MAX_ENTRIES)
                    .ok_or(LIMIT)?;
                admit(&mut credit, 1024 + old.len() * 4)?;
                if !mapped.contains_key(old) {
                    let new = container(old)?;
                    if let Some(new) = &new {
                        valid_uuid(new)?;
                    }
                    mapped.insert(old.into(), new);
                }
                let new = mapped.get(old).ok_or(INVALID)?.clone();
                if new.is_none() {
                    growth = growth
                        .checked_add(128)
                        .filter(|n| *n <= MAX_BYTES)
                        .ok_or(LIMIT)?;
                }
                planned.push((kind, index, position, new));
            }
        }
    }
    visit_aliases(metadata, |alias| {
        if let Some(target) = aliases.get(alias).and_then(Resolution::target) {
            growth = growth
                .checked_add(
                    encoded_len(&target, MAX_BYTES)?
                        .saturating_sub(encoded_len(&alias, MAX_BYTES)?),
                )
                .filter(|n| *n <= MAX_BYTES)
                .ok_or(LIMIT)?;
        }
        Ok(())
    })?;
    if original_bytes
        .checked_add(growth)
        .filter(|n| *n <= case_evidence::VIEW_DOCUMENT_BYTES)
        .is_none()
    {
        return Err(LIMIT.into());
    }
    admit(&mut credit, growth.checked_mul(4).ok_or(LIMIT)?)?;
    crate::operations::check()?;
    rewrite_aliases(metadata, &aliases);
    for (kind, index, position, new) in planned {
        let list = if kind == 0 {
            metadata.get_mut("caseTrails")
        } else {
            metadata
                .get_mut("intel")
                .and_then(|v| v.get_mut("hypotheses"))
        };
        if let Some(reference) = list
            .and_then(Value::as_array_mut)
            .and_then(|a| a.get_mut(index))
            .and_then(|v| v.get_mut("itemRefs"))
            .and_then(Value::as_array_mut)
            .and_then(|a| a.get_mut(position))
            .and_then(Value::as_object_mut)
        {
            if let Some(new) = new {
                reference.insert("containerId".into(), Value::String(new));
            } else {
                reference.insert(
                    "kind".into(),
                    Value::String("unresolved_native_container".into()),
                );
                reference.insert("state".into(), Value::String("missing".into()));
            }
        }
    }
    Ok(FrozenAnchors { anchors, credit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    struct Fixture {
        metadata: Value,
        bindings: Vec<ItemBinding>,
        rows: Vec<(u32, u32, MemberHandle, String)>,
    }
    impl Fixture {
        fn new(items: Value) -> Self {
            Self {
                metadata: json!({"items":items}),
                bindings: vec![],
                rows: vec![],
            }
        }
        fn push(&mut self, index: u32, reference: &str) -> String {
            let binding =
                if let Some(index) = self.bindings.iter().position(|b| b.item_index == index) {
                    &mut self.bindings[index]
                } else {
                    self.bindings.push(ItemBinding {
                        item_index: index,
                        container_id: id(),
                        member_count: 0,
                    });
                    self.bindings.last_mut().unwrap()
                };
            let origin = binding.member_count;
            binding.member_count += 1;
            let member = MemberHandle {
                container_id: binding.container_id.clone(),
                manifest_id: id(),
                occurrence_id: id(),
            };
            let native = native(&member);
            self.rows.push((
                index,
                origin,
                member,
                json!({"event_ref":reference,"id":origin,"unknown":18446744073709551615u64})
                    .to_string(),
            ));
            native
        }
        fn freeze(&mut self, prior: &AnchorMap) -> FrozenAnchors {
            freeze(&mut self.metadata, &self.bindings, prior, |visit| {
                for (index, origin, member, envelope) in &self.rows {
                    visit(*index, *origin, member.clone(), envelope)?;
                }
                Ok(())
            })
            .unwrap()
        }
    }
    #[test]
    fn unique_authored_strings_rewrite_without_splitting_colons_or_touching_authored_ids() {
        let mut f = Fixture::new(json!([{"id":"item:with:colons"}]));
        let target = f.push(0, "record:with:colons");
        let alias = "e:item:with:colons:record:with:colons";
        f.metadata["timeline"] = json!({"edits":{alias:{"title":"note"}},"layout":{format!("a:{alias}"):{"x":5}},"groups":[{"id":"group","ids":[alias,"m:manual","group","note"]}],"annotations":[{"id":"note","anchor":alias,"links":[{"targetId":format!("a:{alias}")},{"targetId":"group"},{"targetId":"note"}]}]});
        let frozen = f.freeze(&AnchorMap::default());
        assert!(frozen.anchors.event_aliases.is_empty());
        let t = &f.metadata["timeline"];
        assert_eq!(t["edits"][&target]["title"], "note");
        assert_eq!(t["layout"][&target]["x"], 5);
        assert_eq!(
            t["groups"][0]["ids"],
            json!([target, "m:manual", "group", "note"])
        );
        assert_eq!(t["annotations"][0]["anchor"], target);
        assert_eq!(t["annotations"][0]["links"][0]["targetId"], target);
        assert_eq!(t["groups"][0]["id"], "group");
        assert_eq!(t["annotations"][0]["id"], "note");
        assert_eq!(t["annotations"][0]["links"][1]["targetId"], "group");
    }
    #[test]
    fn duplicate_item_deletion_never_reclassifies_a_frozen_ambiguous_anchor() {
        let mut f = Fixture::new(json!([{"id":"duplicate"},{"id":"duplicate"}]));
        f.push(0, "same");
        f.push(1, "same");
        f.metadata["timeline"] = json!({"annotations":[{"id":"note","anchor":"e:duplicate:same"}]});
        let original = f.freeze(&AnchorMap::default());
        assert_eq!(
            original.anchors.event_aliases[0].state,
            BlockedState::Ambiguous
        );
        f.metadata["items"].as_array_mut().unwrap().pop();
        f.bindings.pop();
        f.rows.pop();
        let current = freeze(&mut f.metadata, &f.bindings, &original.anchors, |_| {
            panic!("frozen ambiguity must not re-scan a later membership")
        })
        .unwrap();
        assert_eq!(current.anchors, original.anchors);
        assert_eq!(
            f.metadata["timeline"]["annotations"][0]["anchor"],
            "e:duplicate:same"
        );
        assert_eq!(
            current
                .anchors
                .blocked_event("a:e:duplicate:same")
                .unwrap()
                .state,
            BlockedState::Ambiguous
        );
    }
    #[test]
    fn duplicate_reference_removal_retains_ambiguity_and_missing_origins_do_not_retarget() {
        let mut f = Fixture::new(json!([{"id":"item"}]));
        f.push(0, "duplicate");
        f.push(0, "duplicate");
        f.metadata["timeline"] = json!({"edits":{"e:item:duplicate":{"title":"ambiguous"},"e:item:missing":{"title":"missing"}}});
        let prior = f.freeze(&AnchorMap::default());
        assert_eq!(prior.anchors.event_aliases.len(), 2);
        f.rows.remove(0);
        f.bindings[0].member_count = 1;
        let current = f.freeze(&prior.anchors);
        assert_eq!(current.anchors, prior.anchors);
        assert_eq!(
            f.metadata["timeline"]["edits"]["e:item:duplicate"]["title"],
            "ambiguous"
        );
    }
    #[test]
    fn key_collisions_preserve_both_values_and_remain_explicitly_blocked() {
        let mut f = Fixture::new(json!([{"id":"item"}]));
        let target = f.push(0, "record");
        f.metadata["timeline"] = json!({"edits":{"e:item:record":{"title":"legacy"},target.clone():{"title":"native"}},"layout":{"e:item:record":{"x":1},"a:e:item:record":{"x":2}}});
        let before = f.metadata.clone();
        let frozen = f.freeze(&AnchorMap::default());
        assert_eq!(f.metadata, before);
        assert_eq!(
            frozen.anchors.blocked_event("e:item:record").unwrap().state,
            BlockedState::Ambiguous
        );
        assert_eq!(
            frozen
                .anchors
                .blocked_event("e:item:record")
                .unwrap()
                .reason,
            Some(BlockedReason::KeyCollision)
        );
    }
    #[test]
    fn item_associations_keep_mixed_order_and_original_malformed_values() {
        let mut f = Fixture::new(
            json!([{"id":"evidence"},{"id":"duplicate"},{"id":"duplicate"},{"id":"note"}]),
        );
        f.push(0, "r");
        let originals = json!(["evidence","missing","duplicate","note",42,null,{"bad":"original"}]);
        f.metadata["caseTrails"] = json!([{"itemIds":originals.clone()}]);
        f.metadata["intel"] = json!({"hypotheses":[{"items":["note","evidence"]}]});
        let frozen = f.freeze(&AnchorMap::default());
        let refs = &f.metadata["caseTrails"][0]["itemRefs"];
        assert_eq!(refs.as_array().unwrap().len(), 7);
        assert_eq!(
            refs[0],
            json!({"kind":"container","containerId":f.bindings[0].container_id})
        );
        assert_eq!(
            refs[1],
            json!({"kind":"unresolved_legacy_item","itemId":"missing","state":"missing"})
        );
        assert_eq!(refs[2]["state"], "ambiguous");
        assert_eq!(refs[3], json!({"kind":"item","itemId":"note"}));
        for i in 4..7 {
            assert_eq!(
                refs[i],
                json!({"kind":"unresolved_legacy_item","itemId":null,"state":"unsupported"})
            );
        }
        assert_eq!(f.metadata["caseTrails"][0]["itemIds"], originals);
        assert_eq!(
            f.metadata["intel"]["hypotheses"][0]["items"],
            json!(["note", "evidence"])
        );
        assert_eq!(
            f.metadata["intel"]["hypotheses"][0]["itemRefs"][0]["kind"],
            "item"
        );
        assert_eq!(frozen.anchors.item_aliases.len(), 2);
    }
    #[test]
    fn many_authored_aliases_freeze_in_one_bounded_original_walk() {
        let mut f = Fixture::new(json!([{"id":"item"}]));
        for i in 0..700 {
            f.push(0, &format!("row:{i}"));
        }
        f.metadata["timeline"] = json!({"groups":[{"id":"group","ids":(0..520).map(|i|format!("e:item:row:{i}")).collect::<Vec<_>>()}]});
        let visits = Cell::new(0);
        let before = case_work_budget::global().used();
        let frozen = freeze(
            &mut f.metadata,
            &f.bindings,
            &AnchorMap::default(),
            |visitor| {
                for (i, o, m, e) in &f.rows {
                    visits.set(visits.get() + 1);
                    visitor(*i, *o, m.clone(), e)?;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(visits.get(), 700);
        assert!(frozen.anchors.event_aliases.is_empty());
        assert!(f.metadata["timeline"]["groups"][0]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .all(|id| id.as_str().unwrap().starts_with("n:")));
        assert!(frozen.credit.bytes() > 0);
        assert!(case_work_budget::global().used() > before);
        drop(frozen);
        assert_eq!(case_work_budget::global().used(), before);
    }
    #[test]
    fn refused_incomplete_or_foreign_scope_leaves_metadata_untouched() {
        let mut f = Fixture::new(json!([{"id":"item"}]));
        f.push(0, "r");
        f.metadata["timeline"] = json!({"edits":{"e:item:r":{"title":"keep"}}});
        let before = f.metadata.clone();
        assert!(
            freeze(&mut f.metadata, &f.bindings, &AnchorMap::default(), |_| Ok(
                ()
            ))
            .is_err()
        );
        assert_eq!(f.metadata, before);
        let mut member = f.rows[0].2.clone();
        member.container_id = id();
        assert!(freeze(
            &mut f.metadata,
            &f.bindings,
            &AnchorMap::default(),
            |visit| visit(0, 0, member, &f.rows[0].3)
        )
        .is_err());
        assert_eq!(f.metadata, before);
    }
    #[test]
    fn bounded_authoring_limit_precedes_scan_and_cancellation_releases_credit() {
        let mut f = Fixture::new(json!([{"id":"item"}]));
        f.push(0, "r");
        f.metadata["timeline"] = json!({"groups":[{"id":"g","ids":(0..10_001).map(|i|format!("e:item:{i}")).collect::<Vec<_>>()}]});
        let before = f.metadata.clone();
        assert!(freeze(
            &mut f.metadata,
            &f.bindings,
            &AnchorMap::default(),
            |_| panic!("oversized authored set must not scan")
        )
        .is_err());
        assert_eq!(f.metadata, before);
        f.metadata["timeline"] = json!({"edits":{"e:item:r":{}}});
        let before = f.metadata.clone();
        let credit_before = case_work_budget::global().used();
        let name = format!("anchors-{}", id());
        let token = crate::operations::token(Some(name.clone())).unwrap();
        let result = crate::operations::run_with_token(token, || {
            let result = freeze(
                &mut f.metadata,
                &f.bindings,
                &AnchorMap::default(),
                |visit| {
                    crate::operations::cancel_id(&name);
                    visit(0, 0, f.rows[0].2.clone(), &f.rows[0].3)
                },
            );
            assert!(result.is_err());
        });
        assert!(result.is_err());
        assert_eq!(f.metadata, before);
        assert_eq!(case_work_budget::global().used(), credit_before);
    }
    #[test]
    fn portable_remap_uses_only_exact_requested_identities_and_preserves_removed_targets() {
        let old_container = id();
        let old_occurrence = id();
        let removed_occurrence = id();
        let new_container = id();
        let new_occurrence = id();
        let missing_container = id();
        let old = format!("n:{old_container}:{old_occurrence}");
        let missing = format!("n:{old_container}:{removed_occurrence}");
        let new = format!("n:{new_container}:{new_occurrence}");
        let mut metadata = json!({"timeline":{"edits":{old.clone():{"title":"exact"}},"groups":[{"id":"group","ids":[old.clone(),missing.clone()]}]},"caseTrails":[{"itemRefs":[{"kind":"container","containerId":old_container.clone()},{"kind":"container","containerId":missing_container.clone()},{"kind":"unresolved_native_container","containerId":missing_container.clone(),"state":"missing"},{"kind":"unresolved_legacy_item","itemId":"original","state":"ambiguous"}]}]});
        let occurrences = Cell::new(0);
        let containers = Cell::new(0);
        let frozen = remap_native(
            &mut metadata,
            &AnchorMap::default(),
            &mut |container, occurrence| {
                occurrences.set(occurrences.get() + 1);
                assert_eq!(container, old_container);
                Ok((occurrence == old_occurrence)
                    .then(|| (new_container.clone(), new_occurrence.clone())))
            },
            &mut |container| {
                containers.set(containers.get() + 1);
                Ok((container == old_container).then(|| new_container.clone()))
            },
        )
        .unwrap();
        assert_eq!(occurrences.get(), 2);
        assert_eq!(containers.get(), 2);
        assert_eq!(metadata["timeline"]["edits"][&new]["title"], "exact");
        assert_eq!(
            metadata["timeline"]["groups"][0]["ids"],
            json!([new, missing])
        );
        assert_eq!(
            frozen.anchors.blocked_event(&missing).unwrap().state,
            BlockedState::Missing
        );
        assert_eq!(
            metadata["caseTrails"][0]["itemRefs"][0]["containerId"],
            new_container
        );
        assert_eq!(
            metadata["caseTrails"][0]["itemRefs"][1],
            json!({"kind":"unresolved_native_container","containerId":missing_container,"state":"missing"})
        );
        assert_eq!(
            metadata["caseTrails"][0]["itemRefs"][3]["state"],
            "ambiguous"
        );
        let prior = AnchorMap {
            schema_version: 1,
            event_aliases: vec![BlockedAlias {
                alias: missing.clone(),
                state: BlockedState::Missing,
                reason: None,
            }],
            item_aliases: vec![],
        };
        let mut removed = json!({"timeline":{"annotations":[{"anchor":missing}]}});
        remap_native(
            &mut removed,
            &prior,
            &mut |_, _| panic!("frozen missing must not be remapped later"),
            &mut |_| panic!(),
        )
        .unwrap();
    }
    #[test]
    fn portable_native_key_collision_keeps_both_authored_values() {
        let one = format!("n:{}:{}", id(), id());
        let two = format!("n:{}:{}", id(), id());
        let target_container = id();
        let target_occurrence = id();
        let mut metadata = json!({"timeline":{"layout":{one:{"x":1},two:{"x":2}}}});
        let original = metadata.clone();
        let frozen = remap_native(
            &mut metadata,
            &AnchorMap::default(),
            &mut |_, _| Ok(Some((target_container.clone(), target_occurrence.clone()))),
            &mut |_| panic!(),
        )
        .unwrap();
        assert_eq!(metadata, original);
        assert_eq!(frozen.anchors.event_aliases.len(), 2);
        assert!(frozen
            .anchors
            .event_aliases
            .iter()
            .all(|e| e.reason == Some(BlockedReason::KeyCollision)));
    }
    #[test]
    fn portable_callback_failure_is_atomic_and_retains_original_typed_associations() {
        let original =
            json!({"caseTrails":[{"itemRefs":[{"kind":"container","containerId":id()}]}]});
        let mut metadata = original.clone();
        assert!(remap_native(
            &mut metadata,
            &AnchorMap::default(),
            &mut |_, _| panic!(),
            &mut |_| Err("verified mapping unavailable".into())
        )
        .is_err());
        assert_eq!(metadata, original);
    }
    #[test]
    fn unsupported_original_identity_preserves_authored_notes_as_ambiguous() {
        let mut f = Fixture::new(json!([{"id":"item"}]));
        f.push(0, "good");
        f.push(0, "ignored");
        f.rows[1].3 = r#"{"event_ref":true,"future":18446744073709551615}"#.into();
        f.metadata["timeline"] = json!({"edits":{"e:item:good":{"title":"keep"}}});
        let original = f.metadata.clone();
        let frozen = f.freeze(&AnchorMap::default());
        assert_eq!(f.metadata, original);
        assert_eq!(
            frozen.anchors.blocked_event("e:item:good").unwrap().state,
            BlockedState::Ambiguous
        );
    }
}
