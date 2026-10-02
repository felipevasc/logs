//! Reserve aggregate metadata ownership before the storage reader allocates.
//! The returned lease is retained by its authority Arc; publications report
//! that same allocation for retention without obtaining a second reservation.
use crate::case_work_budget::{self, Lease, Pool, MATERIALIZATION_LIMIT};
use std::sync::Arc;
const PLANNING_SCRATCH: usize = 256 << 10;
const CONTAINER_OVERHEAD: usize = 4 << 10;
const MEMBER_METADATA: usize = 512;

pub(crate) struct StorageBudget {
    credit: Lease,
}
impl StorageBudget {
    pub(crate) fn begin(encoded_metadata: usize, encoded_bindings: usize) -> Result<Self, String> {
        Self::begin_in(
            case_work_budget::global(),
            encoded_metadata,
            encoded_bindings,
        )
    }
    fn begin_in(
        pool: &Arc<Pool>,
        encoded_metadata: usize,
        encoded_bindings: usize,
    ) -> Result<Self, String> {
        if encoded_metadata > crate::case_evidence::VIEW_DOCUMENT_BYTES
            || encoded_bindings > crate::case_evidence::MANIFEST_BYTES as usize
        {
            return Err(MATERIALIZATION_LIMIT.into());
        }
        // SQLite TEXT and the returned Rust String can coexist, so metadata
        // input needs two copies before typed preflight.
        // Bindings have a fixed record shape. Four encoded copies cover the
        // input, owned strings/structs and the ordered lookup representation.
        let bytes = encoded_bindings
            .checked_mul(4)
            .and_then(|n| {
                encoded_metadata
                    .checked_mul(2)
                    .and_then(|metadata| n.checked_add(metadata))
            })
            .and_then(|n| n.checked_add(PLANNING_SCRATCH))
            .ok_or(MATERIALIZATION_LIMIT)?;
        Ok(Self {
            credit: crate::case_cache::reserve_work(pool, bytes)?,
        })
    }
    pub(crate) fn admit_metadata(&mut self, materialization_credit: usize) -> Result<(), String> {
        self.add(materialization_credit)
    }
    pub(crate) fn admit_manifest(
        &mut self,
        encoded_bytes: usize,
        member_count: usize,
        batch_metadata_bytes: usize,
    ) -> Result<(), String> {
        if encoded_bytes > crate::case_evidence::MANIFEST_BYTES as usize
            || member_count > crate::case_evidence::MANIFEST_MEMBERS
        {
            return Err(MATERIALIZATION_LIMIT.into());
        }
        let bytes = encoded_bytes
            .checked_mul(4)
            .and_then(|n| {
                member_count
                    .checked_mul(MEMBER_METADATA)
                    .and_then(|members| n.checked_add(members))
            })
            .and_then(|n| {
                batch_metadata_bytes
                    .checked_mul(4)
                    .and_then(|batches| n.checked_add(batches))
            })
            .and_then(|n| n.checked_add(CONTAINER_OVERHEAD))
            .ok_or(MATERIALIZATION_LIMIT)?;
        self.add(bytes)
    }
    fn add(&mut self, bytes: usize) -> Result<(), String> {
        self.credit
            .bytes()
            .checked_add(bytes)
            .ok_or(MATERIALIZATION_LIMIT)?;
        let additional = crate::case_cache::reserve_work(self.credit.pool(), bytes)?;
        self.credit.merge(additional)
    }
    pub(crate) fn bytes(&self) -> usize {
        self.credit.bytes()
    }
    pub(crate) fn finish(self) -> Lease {
        self.credit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case_work_budget::Limits;
    fn pool(live: usize) -> Arc<Pool> {
        Pool::new(Limits {
            materialized: live,
            retained: live,
            live,
        })
    }
    #[test]
    fn metadata_ownership_moves_without_a_second_reservation() {
        let pool = pool(1 << 20);
        let mut budget = StorageBudget::begin_in(&pool, 100, 200).unwrap();
        assert_eq!(budget.bytes(), PLANNING_SCRATCH + 1000);
        budget.admit_metadata(2048).unwrap();
        budget.admit_manifest(1000, 3, 400).unwrap();
        let bytes = budget.bytes();
        assert_eq!(pool.used(), bytes);
        let lease = Arc::new(budget.finish());
        let same = Arc::clone(&lease);
        assert_eq!(pool.used(), bytes);
        drop(lease);
        assert_eq!(pool.used(), bytes);
        drop(same);
        assert_eq!(pool.used(), 0);
    }
    #[test]
    fn aggregate_refusal_keeps_existing_metadata_credit_intact() {
        let pool = pool(600 << 10);
        let mut budget = StorageBudget::begin_in(&pool, 10, 20).unwrap();
        let admitted = budget.bytes();
        assert!(budget.admit_metadata(400 << 10).is_err());
        assert_eq!(budget.bytes(), admitted);
        assert_eq!(pool.used(), admitted);
        assert!(budget.admit_manifest(0, 0, usize::MAX).is_err());
        assert_eq!(pool.used(), admitted);
        drop(budget);
        assert_eq!(pool.used(), 0);
    }
    #[test]
    fn descriptor_limits_are_checked_before_any_reservation_growth() {
        let pool = pool(1 << 20);
        assert!(StorageBudget::begin_in(&pool, usize::MAX, 0).is_err());
        assert_eq!(pool.used(), 0);
        let mut budget = StorageBudget::begin_in(&pool, 0, 0).unwrap();
        let before = budget.bytes();
        assert!(budget
            .admit_manifest(crate::case_evidence::MANIFEST_BYTES as usize + 1, 0, 0)
            .is_err());
        assert!(budget
            .admit_manifest(0, crate::case_evidence::MANIFEST_MEMBERS + 1, 0)
            .is_err());
        assert_eq!(budget.bytes(), before);
    }
}
