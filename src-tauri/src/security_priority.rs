//! A transparent investigation ordering heuristic, never an evidence grade.
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const VERSION: &str = "priority-1";
#[derive(Clone)]
pub struct Signal {
    pub id: String,
    pub family: String,
    pub priority: u8,
    pub members: BTreeSet<String>,
}
pub fn rank(signals: &[Signal]) -> Value {
    let mut groups: Vec<(BTreeSet<String>, BTreeSet<String>, u8, Vec<String>)> = Vec::new();
    for signal in signals {
        let mut group = (
            BTreeSet::from([signal.family.clone()]),
            signal.members.clone(),
            signal.priority,
            vec![signal.id.clone()],
        );
        let mut i = 0;
        while i < groups.len() {
            if !group.0.is_disjoint(&groups[i].0) || !group.1.is_disjoint(&groups[i].1) {
                let old = groups.remove(i);
                group.0.extend(old.0);
                group.1.extend(old.1);
                group.2 = group.2.max(old.2);
                group.3.extend(old.3);
                i = 0;
            } else {
                i += 1;
            }
        }
        groups.push(group);
    }
    groups.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let maximum = groups.iter().map(|g| g.2).max().unwrap_or(0);
    let score = (usize::from(maximum) + groups.len().saturating_sub(1) * 3).min(85);
    json!({"version":VERSION,"score":score,"independent_groups":groups.len(),"signals":signals.len(),
        "groups":groups.iter().map(|g|json!({"families":g.0,"signal_ids":g.3,"maximum_priority":g.2})).collect::<Vec<_>>(),
        "meaning":"Ordem heurística de investigação; não é probabilidade de ataque ou nível de evidência.",
        "formula":"max(priority) + 3 * max(0, independent_groups - 1), capped at 85",
        "dependence":"Sinais da mesma família ou com eventos em comum contam como um único grupo."})
}
pub fn family(kind: &str) -> &str {
    match kind {
        "new_relationship" | "contextual_rarity" => "relationship",
        "password_spraying" | "authentication_chain" => "authentication",
        "beaconing" | "dns_tunneling_context" | "volume_change" => "network",
        "process_parent_rarity" | "lolbin_context" => "process",
        k if k.starts_with("telemetry_") => "coverage",
        k => k,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn s(id: &str, family: &str, p: u8, refs: &[&str]) -> Signal {
        Signal {
            id: id.into(),
            family: family.into(),
            priority: p,
            members: refs.iter().map(|s| s.to_string()).collect(),
        }
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_repeated_and_dependent_signals_do_not_inflate_priority() {
        let a = s("a", "network", 40, &["event-a"]);
        let b = s("b", "network", 20, &["event-b"]);
        assert_eq!(
            rank(&[a.clone()])["score"],
            rank(&[a.clone(), a.clone(), b])["score"]
        );
        let c = s("c", "relationship", 30, &["event-a"]);
        assert_eq!(rank(&[a, c])["independent_groups"], 1);
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_independent_support_changes_order_without_claiming_confidence() {
        assert_eq!(
            rank(&[
                s("a", "process", 40, &["1"]),
                s("b", "identity", 30, &["2"])
            ])["score"],
            43
        );
    }
}
