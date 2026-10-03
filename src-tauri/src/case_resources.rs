//! Immutable per-Case quotas for accounted analytical work and logical IDs.
//! These are not RSS, DuckDB memory, parser-thread or spill-disk reservations.
use crate::{analysis_context::Identity, case_work_budget::{Limits, OwnerCounter, Pool}};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{Arc, OnceLock, Weak}};

const MIB: u64 = 1 << 20;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Mode { Inherit, Custom }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Preferences {
    pub schema_version: u32,
    pub mode: Mode,
    pub work_limit_mib: Option<u64>,
}
impl Default for Preferences {
    fn default() -> Self { Self { schema_version: 1, mode: Mode::Inherit, work_limit_mib: None } }
}
impl Preferences {
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.validate_for_machine(crate::resources::MAX_MEMORY_PREFERENCE_MIB)
    }
    pub(crate) fn validate_for_machine(&self, maximum: u64) -> Result<(), String> {
        if self.schema_version != 1 { return Err("Versão de recursos do Caso não suportada.".into()); }
        match (&self.mode, self.work_limit_mib) {
            (Mode::Inherit, None) => Ok(()),
            (Mode::Custom, Some(value)) if (8..=maximum.min(crate::resources::MAX_MEMORY_PREFERENCE_MIB)).contains(&value) => Ok(()),
            _ => Err(format!("Recursos do Caso: use Herdar ou uma cota inteira entre 8 e {maximum} MiB.")),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Policy {
    pool: Arc<Pool>,
    selections: Arc<Pool>,
    owner: Option<Arc<OwnerCounter>>,
    owner_limit: usize,
    selection_bytes: u64,
    selection_cache_bytes: u64,
    collected_ids_bytes: usize,
    analytics_bytes: usize,
}
#[derive(Default)]
struct Registry {
    owners: HashMap<(std::path::PathBuf, String, String), Weak<OwnerCounter>>,
    policies: HashMap<(std::path::PathBuf, String, String, Preferences), Weak<Policy>>,
}
fn registry() -> &'static parking_lot::Mutex<Registry> {
    static REGISTRY: OnceLock<parking_lot::Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
fn selection_global() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let bytes = crate::resources::allocation_bytes(crate::resources::application_selection_bytes());
        Pool::new(Limits { materialized: bytes, retained: bytes, live: bytes })
    })
}

pub(crate) struct OwnerUsage { pub case_id: String, pub analysis_id: String, pub accounted_bytes: u64 }
pub(crate) struct ResourceSnapshot {
    pub selection_bytes: u64,
    pub selection_limit: u64,
    pub owners: Vec<OwnerUsage>,
    pub partial: bool,
}
/// Credits are logical work ownership, not allocations or process RSS. Owners
/// overlap the global work/selection counters and must not be added to them.
pub(crate) fn resource_snapshot() -> ResourceSnapshot {
    const MAX_OWNERS: usize = 128;
    let pool = selection_global();
    let mut snapshot = ResourceSnapshot { selection_bytes: pool.used() as u64,
        selection_limit: pool.base_limits().live as u64, owners: Vec::new(), partial: false };
    let Some(registry) = registry().try_lock() else { snapshot.partial = true; return snapshot; };
    snapshot.partial = registry.owners.len() > MAX_OWNERS;
    for ((_, case_id, analysis_id), owner) in registry.owners.iter().take(MAX_OWNERS) {
        if let Some(owner) = owner.upgrade() {
            snapshot.owners.push(OwnerUsage { case_id: case_id.chars().take(128).collect(),
                analysis_id: analysis_id.chars().take(128).collect(), accounted_bytes: owner.used() as u64 });
        }
    }
    snapshot.owners.sort_unstable_by(|a, b| (&a.case_id, &a.analysis_id).cmp(&(&b.case_id, &b.analysis_id)));
    snapshot
}
/// A Case only narrows the application's actual component pools. Logical IDs
/// can live on disk; this limit neither enlarges DuckDB nor reserves RAM.
fn maximum_work_bytes(work: usize, selection: usize, memory: u64) -> usize {
    work.saturating_add(selection).min(crate::resources::allocation_bytes(memory))
}
pub(crate) fn maximum_work_mib() -> u64 {
    crate::resources::maximum_memory_mib(crate::resources::total_memory())
}
pub(crate) fn effective_maximum_work_mib() -> u64 {
    maximum_work_bytes(crate::case_work_budget::global().base_limits().live,
        selection_global().base_limits().live, crate::resources::total_memory()) as u64 / MIB
}
fn requested_work_bytes(preferences: &Preferences, maximum: usize) -> usize {
    preferences.work_limit_mib.map(|mib| crate::resources::allocation_bytes(mib.saturating_mul(MIB)))
        .unwrap_or(maximum).min(maximum)
}
impl Policy {
    /// Called before offloading, using the same committed identity/settings.
    /// Registry access never reserves memory, waits for permits or evicts caches.
    pub(crate) fn capture(identity: Option<&Identity>, preferences: &Preferences) -> Result<Arc<Self>, String> {
        preferences.validate()?;
        let aggregate = crate::case_work_budget::global();
        let work = aggregate.base_limits();
        let selection = selection_global();
        let inherited = work.live.saturating_add(selection.base_limits().live);
        let maximum = maximum_work_bytes(work.live, selection.base_limits().live, crate::resources::total_memory());
        // Inherit preserves the existing per-component behavior. Custom only
        // restricts it, including when a Case is opened on a smaller machine.
        let limit = if preferences.mode == Mode::Inherit { inherited } else { requested_work_bytes(preferences, maximum) };
        let build = |owner: Option<Arc<OwnerCounter>>| {
            let pool = owner.as_ref().map(|owner| Pool::child(Limits {
                materialized: work.materialized.min(limit), retained: work.retained.min(limit), live: work.live.min(limit),
            }, Arc::clone(aggregate), Arc::clone(owner), limit)).unwrap_or_else(|| Arc::clone(aggregate));
            let selections = owner.as_ref().map(|owner| Pool::child(Limits {
                materialized: selection.base_limits().materialized.min(limit), retained: selection.base_limits().retained.min(limit), live: selection.base_limits().live.min(limit),
            }, Arc::clone(selection), Arc::clone(owner), limit)).unwrap_or_else(|| Arc::clone(selection));
            Arc::new(Self { pool, selections, owner, owner_limit: limit,
                selection_bytes: (selection.base_limits().live.min(limit)) as u64,
                selection_cache_bytes: crate::resources::application_selection_cache_bytes().min(selection.base_limits().live.min(limit) as u64),
                collected_ids_bytes: crate::resources::application_collected_ids_bytes().min(limit),
                analytics_bytes: crate::resources::application_analytics_bytes().min(limit),
            })
        };
        let Some(identity) = identity else { return Ok(build(None)); };
        let root = crate::config_dir();
        let key = (root.clone(), identity.case_id.clone(), identity.analysis_id.clone(), preferences.clone());
        let mut registry = registry().lock();
        if let Some(policy) = registry.policies.get(&key).and_then(Weak::upgrade) { return Ok(policy); }
        registry.policies.retain(|_, value| value.strong_count() > 0);
        registry.owners.retain(|_, value| value.strong_count() > 0);
        let owner_key = (root, identity.case_id.clone(), identity.analysis_id.clone());
        let owner = registry.owners.get(&owner_key).and_then(Weak::upgrade).unwrap_or_else(|| {
            let owner = Arc::new(OwnerCounter::default());
            registry.owners.insert(owner_key, Arc::downgrade(&owner));
            owner
        });
        let policy = build(Some(owner));
        registry.policies.insert(key, Arc::downgrade(&policy));
        Ok(policy)
    }
    pub(crate) fn pool(&self) -> &Arc<Pool> { &self.pool }
    pub(crate) fn selection_pool(&self) -> &Arc<Pool> { &self.selections }
    pub(crate) fn selection_bytes(&self) -> u64 { self.selection_bytes }
    pub(crate) fn selection_cache_bytes(&self) -> u64 { self.selection_cache_bytes }
    pub(crate) fn collected_ids_bytes(&self) -> usize { self.collected_ids_bytes }
    pub(crate) fn analytics_bytes(&self) -> usize { self.analytics_bytes }
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let work = self.pool.base_limits();
        serde_json::json!({ "accountedLimitMib": self.owner_limit as u64 / MIB,
            "accountedUsedBytes": self.owner.as_ref().map_or(0, |owner| owner.used()),
            "materializedMib": work.materialized as u64 / MIB,
            "workLiveMib": work.live as u64 / MIB,
            "selectionMib": self.selection_bytes / MIB,
            "selectionCacheMib": self.selection_cache_bytes / MIB,
            "collectedIdsMib": self.collected_ids_bytes as u64 / MIB,
            "analyticsMib": self.analytics_bytes as u64 / MIB,
            "applicationWorkMib": crate::case_work_budget::global().base_limits().live as u64 / MIB,
            "applicationSelectionMib": selection_global().base_limits().live as u64 / MIB })
    }
}
thread_local! { static CURRENT: std::cell::RefCell<Option<Arc<Policy>>> = const { std::cell::RefCell::new(None) }; }
pub(crate) fn with<T>(policy: Arc<Policy>, f: impl FnOnce() -> T) -> T { with_optional(Some(policy), f) }
/// A worker entering an unowned scope must clear a previous explicit policy
/// too; Rayon may execute another root while waiting for nested work.
pub(crate) fn with_optional<T>(policy: Option<Arc<Policy>>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<Policy>>);
    impl Drop for Restore { fn drop(&mut self) { CURRENT.with(|current| *current.borrow_mut() = self.0.take()); } }
    let _restore = Restore(CURRENT.with(|current| current.replace(policy)));
    f()
}
/// Capture only an explicit override. A Case-derived fallback must remain
/// dynamic when a nested scope deliberately selects another admitted Case.
pub(crate) fn captured_override() -> Option<Arc<Policy>> { CURRENT.with(|current| current.borrow().clone()) }
pub(crate) fn current() -> Option<Arc<Policy>> {
    captured_override().or_else(||
        crate::analysis_runtime::current().map(|admitted| Arc::clone(&admitted.resource_policy)))
}
/// Callers allocating in parallel keep this captured Policy, rather than
/// looking up a mutable active Case or depending on Rayon worker TLS.
pub(crate) fn current_selection_pool() -> Arc<Pool> {
    current().map(|policy| Arc::clone(policy.selection_pool())).unwrap_or_else(|| Arc::clone(selection_global()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_observation_keeps_global_totals_when_owner_registry_is_busy() {
        let held = registry().lock();
        let sample = resource_snapshot();
        assert!(sample.partial);
        assert!(sample.owners.is_empty());
        assert_eq!(sample.selection_limit, selection_global().base_limits().live as u64);
        drop(held);
    }

    fn identity(case: &str) -> Identity { Identity { case_id: case.into(), analysis_id: uuid::Uuid::new_v4().to_string(), config_revision: 0, visibility_revision: 0 } }
    fn custom(mib: u64) -> Preferences { Preferences { schema_version: 1, mode: Mode::Custom, work_limit_mib: Some(mib) } }
    #[test]
    fn preferences_are_explicit_bounded_and_round_trip() {
        assert!(Preferences::default().validate().is_ok());
        for value in [0, 7, crate::resources::MAX_MEMORY_PREFERENCE_MIB + 1, u64::MAX] { assert!(custom(value).validate().is_err()); }
        let value = custom(32);
        assert_eq!(serde_json::from_str::<Preferences>(&serde_json::to_string(&value).unwrap()).unwrap(), value);
        assert!(serde_json::from_str::<Preferences>(r#"{"schemaVersion":1,"mode":"custom","workLimitMib":8.5}"#).is_err());
    }
    #[test]
    fn large_preferences_survive_machine_changes_without_enlarging_components() {
        for mib in [8193, 65536, 131072] {
            let preferences = custom(mib);
            assert!(preferences.validate().is_ok());
            assert!(preferences.validate_for_machine(mib).is_ok());
            assert!(preferences.validate_for_machine(mib - 1).is_err());
            let loaded: Preferences = serde_json::from_str(&serde_json::to_string(&preferences).unwrap()).unwrap();
            assert_eq!(loaded, preferences);
            assert_eq!(requested_work_bytes(&loaded, 128 << 20), 128 << 20);
        }
        let work = 128 << 20;
        let selection = crate::resources::allocation_bytes(8 << 30);
        let maximum = maximum_work_bytes(work, selection, 128 << 30);
        assert_eq!(maximum, work.saturating_add(selection).min(isize::MAX as usize));
        assert_eq!(maximum_work_bytes(work, selection, 256 << 20), 256 << 20);
        assert_eq!(maximum_work_bytes(usize::MAX, usize::MAX, u64::MAX), isize::MAX as usize);
        let id = identity("moved-large-quota");
        let policy = Policy::capture(Some(&id), &custom(131072)).unwrap();
        assert_eq!(policy.owner_limit as u64 / MIB, effective_maximum_work_mib());
        assert!(policy.pool().base_limits().live <= crate::case_work_budget::global().base_limits().live);
        assert!(policy.selection_bytes() <= crate::resources::application_selection_bytes());
        assert!(policy.analytics_bytes() <= crate::resources::application_analytics_bytes());
        assert!(policy.collected_ids_bytes() <= crate::resources::application_collected_ids_bytes());
    }
    #[test]
    fn revisions_share_owner_credit_but_cases_do_not() {
        let mut a = identity("a"); let b = identity("b");
        let first = Policy::capture(Some(&a), &custom(16)).unwrap();
        let held = Arc::new(first.selection_pool().reserve(10 << 20).unwrap());
        a.config_revision += 1;
        let smaller = Policy::capture(Some(&a), &custom(8)).unwrap();
        assert!(smaller.selection_pool().reserve(1).is_err());
        let other = Policy::capture(Some(&b), &custom(8)).unwrap();
        let b_credit = other.selection_pool().reserve(1 << 20).unwrap();
        let live = Arc::clone(&held); drop(held); drop(first);
        assert!(smaller.pool().reserve(1).is_err(), "eviction cannot release active ownership");
        drop(live);
        let allowed = smaller.selection_pool().reserve(8 << 20).unwrap();
        assert!(smaller.pool().reserve(1).is_err(), "categories share the same Case total");
        drop((allowed, b_credit));
        assert_eq!(smaller.owner.as_ref().unwrap().used(), 0);
    }
    #[test]
    fn new_admission_restricts_new_copies_from_old_publication_pool() {
        let id = identity("lowered");
        let old = Policy::capture(Some(&id), &custom(16)).unwrap();
        let held = old.pool().reserve(6 << 20).unwrap();
        let next = Policy::capture(Some(&id), &custom(8)).unwrap();
        with(Arc::clone(&next), || {
            assert!(Arc::ptr_eq(&old.pool().request_pool(), next.pool()));
            assert!(old.pool().reserve(3 << 20).is_err());
            let allowed = old.pool().reserve(2 << 20).unwrap();
            assert!(Arc::ptr_eq(allowed.pool(), next.pool()));
        });
        assert_eq!(old.owner.as_ref().unwrap().used(), 6 << 20);
        drop(held);
    }
    #[test]
    fn captured_policy_enforces_owner_on_worker_without_tls() {
        let policy = Policy::capture(Some(&identity("thread")), &custom(8)).unwrap();
        let kept = policy.pool().reserve(6 << 20).unwrap();
        let worker = Arc::clone(&policy);
        std::thread::spawn(move || {
            assert!(current().is_none());
            assert!(worker.selection_pool().reserve(3 << 20).is_err());
            let permitted = worker.selection_pool().reserve(2 << 20).unwrap();
            assert_eq!(worker.owner.as_ref().unwrap().used(), 8 << 20);
            drop(permitted);
        }).join().unwrap();
        drop(kept);
        assert_eq!(policy.owner.as_ref().unwrap().used(), 0);
    }
    #[test]
    fn test_operation_limit_does_not_poison_application_pool() {
        let configured = crate::resources::application_selection_bytes();
        let addressable = crate::resources::allocation_bytes(configured) as u64;
        crate::resources::with_selection_limit(16, || {
            assert_eq!(crate::resources::selection_bytes(), 16);
            assert_eq!(current_selection_pool().base_limits().live as u64, addressable);
        });
        assert_eq!(crate::resources::selection_bytes(), configured);
        let credit = current_selection_pool().reserve(24).unwrap();
        assert_eq!(credit.bytes(), 24);
    }
    #[test]
    fn inherited_type_limits_are_not_reduced_to_evidence_work_limit() {
        let policy = Policy::capture(Some(&identity("inherit")), &Preferences::default()).unwrap();
        assert_eq!(policy.selection_bytes(), crate::resources::allocation_bytes(crate::resources::application_selection_bytes()) as u64);
        assert_eq!(policy.collected_ids_bytes(), crate::resources::application_collected_ids_bytes());
        assert_eq!(policy.analytics_bytes(), crate::resources::application_analytics_bytes());
    }
}
