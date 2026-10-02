//! Accounted Case work, including retained publications, evicted live payloads,
//! request-local clones and one-record scratch. This is not an RSS ceiling.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

pub(crate) const MATERIALIZATION_LIMIT: &str = "CASE_MATERIALIZATION_LIMIT";
pub(crate) const WORK_BUSY: &str = "CASE_WORK_BUSY: O trabalho contabilizado em uso ocupa a cota do Caso ou do aplicativo; tente novamente após concluir outra consulta.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub materialized: usize,
    pub retained: usize,
    pub live: usize,
}
impl Limits {
    pub(crate) fn for_effective_bytes(effective: u64) -> Self {
        let materialized = (effective / 8).min(64 << 20) as usize;
        Self {
            materialized,
            retained: materialized,
            live: (effective / 2).min(128 << 20) as usize,
        }
    }
}

/// Which existing allocations can actually relieve a rejected reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pressure { Owner, Aggregate }

#[derive(Debug)]
pub(crate) struct Pool {
    limits: Limits,
    used: AtomicUsize,
    peak: AtomicUsize,
    parent: Option<Arc<Pool>>,
    owner: Option<Arc<OwnerCounter>>,
    owner_limit: usize,
    application: bool,
}
impl Pool {
    pub(crate) fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            parent: None,
            owner: None,
            owner_limit: 0,
            application: false,
        })
    }
    pub(crate) fn child(limits: Limits, parent: Arc<Pool>, owner: Arc<OwnerCounter>, owner_limit: usize) -> Arc<Self> {
        Arc::new(Self { limits, used: AtomicUsize::new(0), peak: AtomicUsize::new(0), parent: Some(parent), owner: Some(owner), owner_limit, application: false })
    }
    pub(crate) fn limits(&self) -> Limits {
        if self.application {
            if let Some(policy) = crate::case_resources::current() { return policy.pool().limits; }
        }
        self.limits
    }
    pub(crate) fn base_limits(&self) -> Limits { self.limits }
    pub(crate) fn request_pool(self: &Arc<Self>) -> Arc<Self> {
        if let Some(policy) = crate::case_resources::current() {
            let requested = policy.pool();
            // Old retained evidence keeps its original lease. A NEW allocation
            // for that same owner follows the newly admitted policy instead.
            let same_evidence_owner = self.parent.as_ref().is_some_and(|parent| parent.application)
                && self.owner.as_ref().zip(requested.owner.as_ref()).is_some_and(|(old, new)| Arc::ptr_eq(old, new));
            if self.application || same_evidence_owner { return Arc::clone(requested); }
        }
        Arc::clone(self)
    }
    pub(crate) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Lease, String> {
        crate::operations::check()?;
        let selected = self.request_pool();
        if !Arc::ptr_eq(self, &selected) { return selected.reserve(bytes); }
        self.add(bytes)?;
        Ok(Lease {
            pool: Arc::clone(self),
            bytes,
        })
    }
    fn add(&self, bytes: usize) -> Result<(), String> {
        if let Some(owner) = &self.owner { owner.add(bytes, self.owner_limit)?; }
        if let Some(parent) = &self.parent {
            if let Err(error) = parent.add(bytes) {
                if let Some(owner) = &self.owner { owner.release(bytes); }
                return Err(error);
            }
        }
        let result = self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.limits.live)
            })
            .map(|previous| {
                self.peak.fetch_max(previous + bytes, Ordering::Relaxed);
            })
            .map_err(|_| WORK_BUSY.to_string());
        if result.is_err() {
            if let Some(parent) = &self.parent { parent.release(bytes); }
            if let Some(owner) = &self.owner { owner.release(bytes); }
        }
        result
    }
    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        if let Some(parent) = &self.parent { parent.release(bytes); }
        if let Some(owner) = &self.owner { owner.release(bytes); }
    }
    fn aggregate_root(&self) -> &Pool {
        self.parent.as_ref().map_or(self, |parent| parent.aggregate_root())
    }
    pub(crate) fn pressure(&self, bytes: usize) -> Option<Pressure> {
        let exceeds = |used: usize, limit: usize| used.checked_add(bytes).is_none_or(|next| next > limit);
        if self.owner.as_ref().is_some_and(|owner| exceeds(owner.used(), self.owner_limit))
            || self.parent.is_some() && exceeds(self.used(), self.limits.live) {
            return Some(Pressure::Owner);
        }
        let root = self.aggregate_root();
        exceeds(root.used(), root.limits.live).then_some(Pressure::Aggregate)
    }
    pub(crate) fn same_owner(&self, other: &Pool) -> bool {
        match (&self.owner, &other.owner) {
            (Some(owner), Some(other)) => Arc::ptr_eq(owner, other),
            (None, None) => std::ptr::eq(self.aggregate_root(), other.aggregate_root()),
            _ => false,
        }
    }
    pub(crate) fn releases_pressure(&self, candidate: &Pool, pressure: Pressure) -> bool {
        match pressure {
            Pressure::Owner => self.owner.as_ref().zip(candidate.owner.as_ref())
                .is_some_and(|(owner, other)| Arc::ptr_eq(owner, other)),
            Pressure::Aggregate => std::ptr::eq(self.aggregate_root(), candidate.aggregate_root()),
        }
    }
    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    #[cfg(test)]
    fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }
}

/// All revisions of the same Case incarnation share this counter. A new
/// policy cannot obtain a fresh allowance while an older admission is alive.
#[derive(Debug, Default)]
pub(crate) struct OwnerCounter { used: AtomicUsize }
impl OwnerCounter {
    fn add(&self, bytes: usize, limit: usize) -> Result<(), String> {
        self.used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= limit)
        }).map(|_| ()).map_err(|_| WORK_BUSY.to_string())
    }
    fn release(&self, bytes: usize) { self.used.fetch_sub(bytes, Ordering::AcqRel); }
    pub(crate) fn used(&self) -> usize { self.used.load(Ordering::Acquire) }
}

/// Non-cloneable ownership of a reservation. Sharing the payload's Arc shares
/// this one charge; only allocating another payload obtains another lease.
#[derive(Debug)]
pub(crate) struct Lease {
    pool: Arc<Pool>,
    bytes: usize,
}
impl Lease {
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(crate) fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }
    pub(crate) fn resize(&mut self, bytes: usize) -> Result<(), String> {
        if bytes > self.bytes {
            crate::operations::check()?;
            self.pool.add(bytes - self.bytes)?;
        } else {
            self.pool.release(self.bytes - bytes);
        }
        self.bytes = bytes;
        Ok(())
    }
    pub(crate) fn merge(&mut self, mut other: Lease) -> Result<(), String> {
        if !Arc::ptr_eq(&self.pool, &other.pool) {
            return Err("CASE_WORK_POOL_CHANGED".into());
        }
        self.bytes = self.bytes.checked_add(other.bytes).ok_or(WORK_BUSY)?;
        other.bytes = 0;
        Ok(())
    }
    pub(crate) fn split(&mut self, bytes: usize) -> Result<Lease, String> {
        if bytes > self.bytes {
            return Err(WORK_BUSY.into());
        }
        self.bytes -= bytes;
        Ok(Lease {
            pool: Arc::clone(&self.pool),
            bytes,
        })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.pool.release(self.bytes);
    }
}

pub(crate) fn global() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let mut pool = Pool::new(Limits::for_effective_bytes(crate::resources::effective_bytes()));
        Arc::get_mut(&mut pool).expect("new pool").application = true;
        pool
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tiny() -> Arc<Pool> {
        Pool::new(Limits {
            materialized: 16,
            retained: 16,
            live: 64,
        })
    }

    #[test]
    fn low_profile_separates_preserved_disk_bytes_from_owned_case_work() {
        let low = Limits::for_effective_bytes(128 << 20);
        assert_eq!(
            low,
            Limits {
                materialized: 16 << 20,
                retained: 16 << 20,
                live: 64 << 20
            }
        );
        assert!(
            32 << 20 > low.materialized,
            "a preserved capture need not fit an analytical view"
        );
        assert_eq!(
            Limits::for_effective_bytes(8 << 30),
            Limits {
                materialized: 64 << 20,
                retained: 64 << 20,
                live: 128 << 20
            }
        );
    }

    #[test]
    fn simultaneous_leases_and_live_arcs_keep_credit_until_the_last_owner() {
        let pool = tiny();
        let payload = Arc::new(pool.reserve(16).unwrap());
        let active = Arc::clone(&payload);
        let copy = pool.reserve(40).unwrap();
        drop(payload);
        assert_eq!(
            pool.used(),
            56,
            "eviction does not release an active publication"
        );
        assert!(pool.reserve(9).is_err());
        drop(active);
        assert_eq!(pool.used(), 40);
        let scratch = pool.reserve(24).unwrap();
        assert_eq!(pool.peak(), 64);
        drop((scratch, copy));
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn failed_growth_overflow_and_cancellation_leave_accounting_unchanged() {
        let pool = tiny();
        let mut lease = pool.reserve(20).unwrap();
        assert!(lease.resize(65).is_err());
        assert_eq!((lease.bytes(), pool.used()), (20, 20));
        assert!(pool.reserve(usize::MAX).is_err());
        lease.resize(8).unwrap();
        let other = pool.reserve(12).unwrap();
        lease.merge(other).unwrap();
        assert_eq!((lease.bytes(), pool.used()), (20, 20));
        let id = format!("case-work-cancel-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id(&id);
            pool.reserve(1)
        })
        .is_err());
        assert_eq!(pool.used(), 20);
        drop(lease);
        assert_eq!(pool.used(), 0);
    }

    #[test]
    fn case_categories_and_revisions_share_owner_and_application_counters() {
        let aggregate = tiny();
        let owner = Arc::new(OwnerCounter::default());
        let limits = Limits { materialized: 64, retained: 64, live: 64 };
        let a = Pool::child(limits, Arc::clone(&aggregate), Arc::clone(&owner), 40);
        let a_revision = Pool::child(limits, Arc::clone(&aggregate), Arc::clone(&owner), 24);
        let b = Pool::child(limits, Arc::clone(&aggregate), Arc::new(OwnerCounter::default()), 40);
        let retained = Arc::new(a.reserve(24).unwrap());
        assert!(a_revision.reserve(1).is_err());
        let other_case = b.reserve(40).unwrap();
        assert_eq!(aggregate.used(), 64);
        assert!(a.reserve(1).is_err());
        assert_eq!(owner.used(), 24, "failed aggregate admission rolls back owner credit");
        let live = Arc::clone(&retained); drop(retained);
        assert_eq!(aggregate.used(), 64);
        drop(other_case);
        let id = format!("hierarchical-cancel-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id(&id); a.reserve(1)
        }).is_err());
        assert_eq!(owner.used(), 24);
        drop(live);
        assert_eq!((owner.used(), aggregate.used()), (0, 0));
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_live_limit() {
        let pool = tiny();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let pool = Arc::clone(&pool);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    let lease = pool.reserve(16);
                    barrier.wait();
                    assert!(pool.used() <= 64);
                    drop(lease);
                });
            }
        });
        assert_eq!(pool.used(), 0);
        assert!(pool.peak() <= 64);
    }
}
