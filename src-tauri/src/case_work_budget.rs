//! Accounted Case work, including retained publications, evicted live payloads,
//! request-local clones and one-record scratch. This is not an RSS ceiling.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};

pub(crate) const MATERIALIZATION_LIMIT: &str = "CASE_MATERIALIZATION_LIMIT";
pub(crate) const WORK_BUSY: &str = "CASE_WORK_BUSY: As evidências em uso ocupam o orçamento; tente novamente após concluir outra consulta.";

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

#[derive(Debug)]
pub(crate) struct Pool {
    limits: Limits,
    used: AtomicUsize,
    peak: AtomicUsize,
}
impl Pool {
    pub(crate) fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }
    pub(crate) fn limits(&self) -> Limits {
        self.limits
    }
    pub(crate) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Lease, String> {
        crate::operations::check()?;
        self.add(bytes)?;
        Ok(Lease {
            pool: Arc::clone(self),
            bytes,
        })
    }
    fn add(&self, bytes: usize) -> Result<(), String> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.limits.live)
            })
            .map(|previous| {
                self.peak.fetch_max(previous + bytes, Ordering::Relaxed);
            })
            .map_err(|_| WORK_BUSY.to_string())
    }
    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    #[cfg(test)]
    fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }
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
            self.pool
                .used
                .fetch_sub(self.bytes - bytes, Ordering::AcqRel);
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
        self.pool.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub(crate) fn global() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        Pool::new(Limits::for_effective_bytes(
            crate::resources::effective_bytes(),
        ))
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
