//! Presentation groups, never correlations or additional evidence.
use crate::{
    detections::{Detection, Episode},
    evidence,
    model::Event,
};
use serde::Serialize;
use std::collections::BTreeMap;

pub const VERSION: &str = "pattern-groups-1";

#[derive(Serialize)]
pub struct Grouping {
    pub kind: &'static str,
    pub occurrence_count: usize,
    pub version: &'static str,
}

/// Keep imported files and producers separate even when neither has a namespace.
pub fn source(ev: &Event) -> String {
    let file = ev
        .event_ref
        .rsplit_once(':')
        .filter(|(digest, offset)| {
            digest.len() == 64
                && digest.bytes().all(|b| b.is_ascii_hexdigit())
                && offset.parse::<u64>().is_ok()
        })
        .map(|(digest, _)| digest)
        .unwrap_or("");
    evidence::stable_id(
        "source",
        [
            ev.source.as_str(),
            file,
            ev.fields
                .get("caminho")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        ],
    )
}

/// Ignore surrounding log noise, not the matched payload, its context or result.
pub fn pattern(d: &Detection) -> Option<String> {
    let e = &d.evidence;
    if !matches!(d.kind.as_str(), "single" | "signal")
        || d.count != 1
        || e.event_refs.len() != 1
        || e.evidence_members.len() != 1
        || d.participants.limited
        || !e.relationships.is_empty()
        || !(1..=5).contains(&e.evidence_level)
        || e.excerpts.is_empty()
        || e.excerpts
            .iter()
            .any(|x| x.match_truncated || x.matched.trim().is_empty())
    {
        return None;
    }
    let source = d.pattern_source.as_ref()?;
    let mut excerpts: Vec<_> = e
        .excerpts
        .iter()
        .map(|x| (&x.field, &x.transformation, x.matched.trim()))
        .collect();
    excerpts.sort_unstable();
    excerpts.dedup();
    let mut entities: Vec<_> = d.entities.iter().map(|x| (&x.column, &x.value)).collect();
    entities.sort_unstable();
    entities.dedup();
    let mut signals: Vec<_> = d.signal_rules.iter().collect();
    signals.sort_unstable();
    let provenance = &e.evidence_members[0].provenance;
    let scope: Vec<_> = [
        "namespace",
        "host",
        "service",
        "actor",
        "target",
        "identity",
        "source_address",
        "destination",
        "resource",
        "file",
        "application",
    ]
    .iter()
    .map(|key| (*key, provenance.get(*key).map(|p| &p.value)))
    .collect();
    let signature = serde_json::json!([
        VERSION,
        source,
        d.namespace,
        d.participants.signature(),
        d.rule,
        d.kind,
        d.origin,
        signals,
        d.name,
        d.severity,
        e.evidence_level,
        e.claim,
        e.outcome,
        e.evaluation,
        e.rule_version,
        e.policy_version,
        e.normalization_version,
        scope,
        entities,
        excerpts
    ]);
    Some(evidence::stable_id("pattern", [signature.to_string()]))
}

/// Multiple rules on the same original event can share a display card, but an
/// actual multi-event correlation is never absorbed into a repetition group.
pub fn episode_pattern(patterns: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let mut keys = patterns.into_iter().collect::<Option<Vec<_>>>()?;
    if keys.is_empty() {
        return None;
    }
    keys.sort_unstable();
    Some(evidence::stable_id("pattern-group", keys))
}

pub fn group(episodes: Vec<Episode>, detections: &[Detection]) -> Vec<Episode> {
    let mut positions = BTreeMap::new();
    let mut output: Vec<Episode> = Vec::new();
    for mut episode in episodes {
        let key = (episode.event_refs.len() == 1)
            .then(|| episode_pattern(episode.detections.iter().map(|&i| pattern(&detections[i]))))
            .flatten();
        if let Some(key) = key {
            if let Some(&index) = positions.get(&key) {
                let group: &mut Episode = &mut output[index];
                group.id = key;
                let count = group.grouping.as_ref().map_or(1, |g| g.occurrence_count) + 1;
                group.grouping = Some(Grouping {
                    kind: "pattern",
                    occurrence_count: count,
                    version: VERSION,
                });
                group.start = group.start.into_iter().chain(episode.start).min();
                group.end = group.end.into_iter().chain(episode.end).max();
                group.detections.append(&mut episode.detections);
                group.participants.merge(&episode.participants);
                group.event_refs.append(&mut episode.event_refs);
                continue;
            }
            positions.insert(key, output.len());
        }
        output.push(episode);
    }
    for group in &mut output {
        if group.grouping.is_some() {
            group
                .detections
                .sort_by_key(|&i| (detections[i].start, &detections[i].id));
            group.event_refs.sort_unstable();
            group.event_refs.dedup();
        }
    }
    output
}
