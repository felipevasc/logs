//! Exact fixed-range Timeline on the smaller admitted positional row domain.
//! One bitmap pass locates at most 4,096 metadata records; no per-row values or
//! IDs survive the call. This is a conservative direct-read cap, not an RSS or
//! cold-latency guarantee for scattered mapped records.
use super::time_index::{self, Histogram, HistogramBucket, ReadSet};
use crate::{
    analysis_visibility::{Mask, RowVisit},
    metadata_store::LineStore,
    sources::FileIndex,
};

type Result<T> = std::result::Result<T, String>;
pub(crate) const MAX_DIRECT_ROWS: usize = 4_096;
/// Tiny sources lose to the fixed verification/rank overhead. The first focused
/// mapped comparison establishes this conservative dispatch floor only.
pub(crate) const MIN_SOURCE_ROWS: usize = 100_000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Plan {
    excluded: bool,
}
impl Plan {
    pub(crate) fn requires_base(self) -> bool {
        self.excluded
    }

    /// Read stored cardinalities before metadata, bitmap words, or Session setup.
    pub(crate) fn new(index: &FileIndex, mask: &Mask) -> Result<Option<Self>> {
        if mask.rows() != index.lines.len() {
            return Err("Máscara temporal pertence a outra geração da fonte.".into());
        }
        if mask.scope() != &crate::exclusion_store::Scope::ActiveUnion {
            return Ok(None);
        }
        let hidden = mask.cardinality();
        let visible = mask
            .rows()
            .checked_sub(hidden)
            .ok_or("Máscara excede a fonte admitida.")?;
        let excluded = hidden <= visible;
        if hidden.min(visible) > MAX_DIRECT_ROWS {
            return Ok(None);
        }
        if excluded && !metadata_matches_store(index) {
            return Ok(None);
        }
        Ok(Some(Self { excluded }))
    }
}

fn metadata_matches_store(index: &FileIndex) -> bool {
    index.parts.iter().all(|part| {
        // Multiline metadata may parse only the first physical line, while the
        // store parses the whole logical record. Unknown parsers need a proof.
        matches!(part.format.as_str(), "apache" | "jsonl" | "snapshot" | "text"
            | "csv" | "tsv" | "csv:semicolon" | "csv:pipe" | "custom"
            | "syslog3164" | "syslog5424" | "firewall" | "cef" | "leef"
            | "logfmt" | "w3c" | "zeek" | "auditd")
        // event_at assigns identity and enriches names before timestamp rules;
        // retimestamp_index applies the rules to the original parsed event.
        && part.ts_config.as_ref().is_none_or(|config| config.sources.iter().all(|source|
            !matches!(source.as_str(), "id" | "event_ref" | "name" | "description")))
    })
}

#[derive(Clone, Copy)]
struct Layout {
    start: i64,
    end: i64,
    width: i64,
    buckets: usize,
}
impl Layout {
    fn new(start: i64, end: i64, width: i64, buckets: usize) -> Result<Self> {
        if start > end || width < 1 || !(1..=240).contains(&buckets) {
            return Err("Intervalo ou tamanho de faixa temporal inválido.".into());
        }
        Ok(Self {
            start,
            end,
            width,
            buckets,
        })
    }
    fn empty(self) -> Histogram {
        Histogram {
            buckets: vec![HistogramBucket::default(); self.buckets],
            ..Histogram::default()
        }
    }
    fn add(self, histogram: &mut Histogram, timestamp: i64, level: u8) {
        // The indexed Timeline's zero/missing sentinel is deliberately omitted.
        if timestamp == 0 || timestamp < self.start || timestamp > self.end {
            return;
        }
        let at = (timestamp.saturating_sub(self.start) / self.width) as usize;
        let bucket = &mut histogram.buckets[at.min(self.buckets - 1)];
        bucket.count += 1;
        histogram.total += 1;
        if matches!(level, crate::model::LV_ERR | crate::model::LV_CRIT) {
            bucket.errors += 1;
            histogram.errors += 1;
        } else if level == crate::model::LV_WARN {
            bucket.warnings += 1;
            histogram.warnings += 1;
        }
    }
}

/// None declines only this acceleration; the caller retains its exact gated
/// metadata scan. Mask/source integrity and cancellation failures remain errors.
pub(crate) fn histogram(
    plan: Plan,
    lines: &LineStore,
    mask: &Mask,
    readers: Option<&ReadSet>,
    start: i64,
    end: i64,
    width: i64,
    buckets: usize,
) -> Result<Option<(Histogram, RowVisit)>> {
    crate::operations::check()?;
    mask.validate()?;
    let layout = Layout::new(start, end, width, buckets)?;
    let base = if plan.excluded {
        let Some(readers) = readers else {
            return Ok(None);
        };
        // The complete Session capability counts repeated source appearances,
        // including untimed rows. A partial or differently sized set declines.
        let all = time_index::predicate(&[]).expect("empty temporal predicate");
        if time_index::count(readers, &all)? != Some(lines.len()) {
            return Ok(None);
        }
        let Some(base) = time_index::histogram(readers, start, end, width, buckets)? else {
            return Ok(None);
        };
        Some(base)
    } else {
        None
    };
    let mut selected = layout.empty();
    let token = crate::operations::current_token();
    let work = mask.visit_rows(
        plan.excluded,
        MAX_DIRECT_ROWS,
        &|| token.cancelled(),
        |row| {
            let meta = lines
                .get(row)
                .ok_or("Registro temporal fora da fonte admitida.")?;
            layout.add(&mut selected, meta.ts, meta.level);
            Ok(())
        },
    )?;
    let Some(work) = work else {
        return Ok(None);
    };
    crate::operations::check()?;
    mask.validate()?;
    // Readers and their checkpoint/sidecar leases remain pinned through the
    // metadata visit. Never return a result from a generation changed mid-call.
    if plan.excluded && !time_index::changed_readers(readers.expect("excluded readers"))?.is_empty()
    {
        return Ok(None);
    }
    let output = match base {
        Some(base) => subtract(base, selected),
        None => Some(selected),
    };
    crate::operations::check()?;
    Ok(output.map(|histogram| (histogram, work)))
}

fn subtract(mut base: Histogram, selected: Histogram) -> Option<Histogram> {
    if base.buckets.len() != selected.buckets.len() {
        return None;
    }
    base.total = base.total.checked_sub(selected.total)?;
    base.errors = base.errors.checked_sub(selected.errors)?;
    base.warnings = base.warnings.checked_sub(selected.warnings)?;
    for (bucket, remove) in base.buckets.iter_mut().zip(selected.buckets) {
        bucket.count = bucket.count.checked_sub(remove.count)?;
        bucket.errors = bucket.errors.checked_sub(remove.errors)?;
        bucket.warnings = bucket.warnings.checked_sub(remove.warnings)?;
        if bucket.errors.checked_add(bucket.warnings)? > bucket.count {
            return None;
        }
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_ranges_keep_duplicates_zero_extremes_and_level_counts() {
        let rows: Vec<_> = (0..7u8)
            .flat_map(|level| [i64::MIN, -5, -1, 0, 1, 1, 5, 5, i64::MAX].map(|ts| (ts, level)))
            .collect();
        for selection in 0..80 {
            for (start, end, width, buckets) in [
                (i64::MIN, i64::MAX, i64::MAX / 239 + 1, 240),
                (-5, 5, 2, 6),
                (0, 0, 1, 1),
                (1, 1, 1, 1),
                (-1, 1, 1, 3),
                (-100, 100, 1, 3),
            ] {
                let layout = Layout::new(start, end, width, buckets).unwrap();
                let (mut base, mut removed, mut visible) =
                    (layout.empty(), layout.empty(), layout.empty());
                for (id, &(timestamp, level)) in rows.iter().enumerate() {
                    layout.add(&mut base, timestamp, level);
                    if (id * 17 + selection * 31) % 29 < selection % 29 {
                        layout.add(&mut removed, timestamp, level);
                    } else {
                        layout.add(&mut visible, timestamp, level);
                    }
                }
                assert_eq!(subtract(base, removed).unwrap(), visible);
            }
        }
    }

    #[test]
    fn invalid_layout_and_underflow_decline_without_partial_results() {
        for (start, end, width, buckets) in
            [(2, 1, 1, 1), (1, 2, 0, 1), (1, 2, 1, 0), (1, 2, 1, 241)]
        {
            assert!(Layout::new(start, end, width, buckets).is_err());
        }
        let layout = Layout::new(-1, 1, 1, 3).unwrap();
        let mut removed = layout.empty();
        layout.add(&mut removed, 1, crate::model::LV_ERR);
        assert!(subtract(layout.empty(), removed).is_none());
    }

    #[test]
    fn subtraction_declines_identity_timestamp_rules_and_unproved_multiline_parsers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("timestamp-rule.jsonl");
        std::fs::write(&path, "{\"timevalue\":100}\n{\"timevalue\":101}\n").unwrap();
        let mut index =
            crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = crate::model::CodesConfig::default();
        for source in ["timevalue", "id"] {
            index.parts[0].ts_config = Some(
                crate::sources::TsConfig {
                    sources: vec![source.into()],
                    format: "epoch_ms".into(),
                    ..Default::default()
                }
                .compile()
                .unwrap(),
            );
            crate::sources::retimestamp_index(&mut index, None).unwrap();
            let hydrated = crate::sources::event_at(&index, 1, &codes, &codes, &[]).timestamp;
            if source == "id" {
                // Existing behavior: metadata rules see Event.id=0, hydration
                // sees the assigned row ID. Keep this explicit until that
                // independent timestamp-config contract is corrected.
                assert_eq!(index.lines.at(1).ts, 0);
                assert_eq!(hydrated, Some(1));
                assert!(!metadata_matches_store(&index));
            } else {
                assert_eq!(index.lines.at(1).ts, 101);
                assert_eq!(hydrated, Some(101));
                assert!(metadata_matches_store(&index));
            }
        }
        index.parts[0].ts_config = None;
        for format in ["log4j", "wildfly", "future-parser"] {
            index.parts[0].format = format.into();
            assert!(!metadata_matches_store(&index));
        }
    }
}
