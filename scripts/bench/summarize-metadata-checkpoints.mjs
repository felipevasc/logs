// Reads an explicit native report. Never runs a benchmark or changes checkpoint policy.
// Policy factors are recorded explicitly; legacy reports remain readable.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const integer = (n, field) => assert.ok(Number.isSafeInteger(n) && n >= 0, `Invalid ${field}`);
const digest = value => assert.match(value ?? '', /^[a-f0-9]{64}$/);

export function summarizeCheckpointReport(report) {
  assert.equal(report.schemaVersion, 1, 'Unsupported checkpoint report schema');
  assert.ok(['cold', 'warm', 'interrupt', 'resume'].includes(report.mode), 'Unknown mode');
  const { context, progress, mode } = report;
  assert.ok(context && Array.isArray(progress), 'Missing native context/progress');
  digest(context.sourceSha256); digest(context.binarySha256);
  integer(context.sourceRows, 'sourceRows'); integer(context.sourceBytes, 'sourceBytes');
  if (context.checkpointPolicy !== undefined || context.checkpointThresholds !== undefined) {
    assert.ok(['wave', 'batched'].includes(context.checkpointPolicy), 'Unknown checkpoint policy');
    const thresholds = context.checkpointThresholds;
    assert.ok(thresholds && thresholds.safeBoundaryOnly === true, 'Missing checkpoint boundary contract');
    for (const field of ['rows', 'sourceBytes', 'elapsedMs']) integer(thresholds[field], `checkpoint ${field}`);
    assert.ok(thresholds.rows > 0 && thresholds.sourceBytes > 0, 'Empty checkpoint threshold');
    if (!context.smallWaves) {
      assert.deepEqual(thresholds, context.checkpointPolicy === 'wave'
        ? { rows: 1, sourceBytes: 1, elapsedMs: 0, safeBoundaryOnly: true }
        : { rows: 65_536, sourceBytes: 16 * 1024 * 1024, elapsedMs: 2000, safeBoundaryOnly: true },
      'Production sample changed checkpoint thresholds');
    }
  }
  assert.ok(context.sourceRows > 0 && context.sourceRows <= 100_000);
  assert.ok(context.sourceBytes > 0 && context.sourceBytes <= 64 * 1024 * 1024);
  assert.equal(context.smallWaves, mode === 'interrupt' || mode === 'resume');
  assert.ok(Number.isFinite(report.probeElapsedMs) && report.probeElapsedMs >= 0);
  if (context.smallWaves) assert.ok(context.sourceRows <= 1_000 && context.sourceBytes <= 1024 * 1024);
  let active = null, previousTime = -1, callsReachingWrite = 0;
  let lastCommittedCursor = 0, maxObservedUncommittedRows = 0, maxObservedUncommittedBytes = 0;
  const calls = [];
  for (const p of progress) {
    for (const field of ['atUs', 'completed', 'total', 'resumedRows', 'checkpointRows', 'parsedRows']) integer(p[field], field);
    assert.ok(p.atUs >= previousTime, 'Progress clock moved backwards'); previousTime = p.atUs;
    assert.notEqual(p.phaseId, 'metadata-unavailable', 'Persistence degraded; do not report a successful timing sample');
    assert.ok(p.checkpointRows <= context.sourceRows && p.parsedRows <= context.sourceRows && p.resumedRows <= context.sourceRows);
    maxObservedUncommittedRows = Math.max(maxObservedUncommittedRows, p.resumedRows + p.parsedRows - p.checkpointRows);
    if (p.phaseId === 'metadata-scan' && (mode === 'cold' || mode === 'interrupt')) {
      assert.ok(p.completed <= context.sourceBytes);
      maxObservedUncommittedBytes = Math.max(maxObservedUncommittedBytes, p.completed - lastCommittedCursor);
    }
    if (p.phaseId === 'metadata-checkpoint-write') {
      if (active === null) {
        assert.equal(p.completed, 0, 'Checkpoint write must begin at zero');
        active = { startedUs: p.atUs, oldRows: p.checkpointRows, appendedRows: p.total, writeReports: 1, stage: 'write' };
        callsReachingWrite++;
      } else {
        assert.equal(active.stage, 'write', 'Overlapping checkpoint calls');
        assert.equal(active.writeReports, 1, 'Duplicate write completion report');
        assert.equal(p.completed, p.total, 'Incomplete checkpoint write');
        assert.equal(p.total, active.appendedRows); assert.equal(p.checkpointRows, active.oldRows);
        active.writeReports++;
      }
    } else if (p.phaseId === 'metadata-checkpoint-sync') {
      assert.equal(active?.stage, 'write', 'Sync without a write');
      assert.equal(active.writeReports, 2, 'Missing write completion report');
      assert.equal(p.checkpointRows, active.oldRows, 'Sync changed the committed prefix');
      active.syncUs = p.atUs; active.stage = 'sync';
    } else if (p.phaseId === 'metadata-checkpoint-publish') {
      assert.equal(active?.stage, 'sync', 'Publish without sync');
      assert.equal(p.checkpointRows, active.oldRows, 'Publication began with a different committed prefix');
      active.publishUs = p.atUs; active.stage = 'publish';
    } else if (p.phaseId === 'metadata-checkpoint-manifest-ready') {
      assert.equal(active?.stage, 'publish', 'Manifest sync without publication');
      assert.equal(p.checkpointRows, active.oldRows, 'Unpublished manifest changed the committed prefix');
      active.manifestReadyUs = p.atUs; active.stage = 'manifest-ready';
    } else if (p.phaseId === 'metadata-checkpoint-committed') {
      assert.ok(active?.stage === 'publish' || active?.stage === 'manifest-ready', 'Commit without publication');
      assert.equal(p.checkpointRows, active.oldRows + active.appendedRows, 'Committed row delta mismatch');
      assert.ok(p.completed >= lastCommittedCursor && p.completed <= context.sourceBytes, 'Committed cursor regressed');
      lastCommittedCursor = p.completed;
      calls.push({
        appendedRows: active.appendedRows, committedRows: p.checkpointRows, committedCursor: p.completed,
        writeReporterIntervalMs: (active.syncUs - active.startedUs) / 1000,
        syncAndValidationReporterIntervalMs: (active.publishUs - active.syncUs) / 1000,
        publicationReporterIntervalMs: (p.atUs - active.publishUs) / 1000,
        // New phase is optional for previously captured baseline reports.
        manifestPreparationReporterIntervalMs: active.manifestReadyUs === undefined ? null : (active.manifestReadyUs - active.publishUs) / 1000,
        renameAndDirectorySyncReporterIntervalMs: active.manifestReadyUs === undefined ? null : (p.atUs - active.manifestReadyUs) / 1000,
        observedCheckpointIntervalMs: (p.atUs - active.startedUs) / 1000,
      });
      active = null;
    }
  }
  if (mode === 'interrupt') {
    assert.equal(active?.stage, 'publish', 'Interruption did not reach the pre-publication gate');
    assert.ok(calls.length > 0 && active.appendedRows > 0, 'Need a durable prefix and new uncommitted work');
    assert.equal(typeof report.expectedCancellationError, 'string');
    assert.ok(report.expectedCancellationError.length > 0);
    assert.equal(report.result, undefined, 'Interrupted probe must not return an index');
  } else {
    assert.equal(active, null, 'Incomplete checkpoint timing; never impute zero');
    assert.equal(report.result?.rows, context.sourceRows);
    digest(report.result?.semanticSha256);
    for (const field of ['parsedRows', 'resumedRows', 'checkpointRows']) integer(report.result[field], field);
    assert.equal(report.result.checkpointRows, context.sourceRows);
    assert.equal(report.result.parsedRows + report.result.resumedRows, context.sourceRows);
    if (mode === 'warm') {
      assert.equal(callsReachingWrite, 0, 'Warm reopen wrote a checkpoint');
      assert.equal(report.result.parsedRows, 0, 'Warm reopen parsed metadata');
    } else {
      assert.equal(report.uncachedParityVerified, true, 'Missing uncached oracle parity');
      assert.ok(calls.length > 0);
      assert.equal(calls.at(-1).committedRows, context.sourceRows);
      assert.equal(calls.at(-1).committedCursor, context.sourceBytes);
    }
    if (mode === 'cold') assert.equal(report.result.resumedRows, 0);
    if (mode === 'resume') {
      assert.equal(report.recovery?.committedPrefixNotReparsed, true);
      assert.equal(report.recovery.durableRowsBeforeInterrupt, report.result.resumedRows);
    }
  }
  const sum = field => calls.reduce((total, call) => total + call[field], 0);
  return {
    schemaVersion: 1, mode, context, probeElapsedMs: report.probeElapsedMs,
    callsReachingWrite, committedCalls: calls.length,
    metadataOnlyCommittedCalls: calls.filter(call => call.appendedRows === 0).length,
    uncommittedCall: active ? { stage: active.stage, appendedRows: active.appendedRows } : null,
    committedIntervalsMs: {
      write: sum('writeReporterIntervalMs'), syncAndValidation: sum('syncAndValidationReporterIntervalMs'),
      publication: sum('publicationReporterIntervalMs'), total: sum('observedCheckpointIntervalMs'),
    },
    maxObservedUncommittedMetadataRows: maxObservedUncommittedRows,
    // Restore progress does not expose its byte cursor. Never infer zero loss on resume/warm.
    maxObservedUncommittedSourceBytes: mode === 'cold' || mode === 'interrupt' ? maxObservedUncommittedBytes : null,
    result: report.result ?? null, recovery: report.recovery ?? null, calls,
    limitations: 'Phase-entry counters and reporter intervals, not exact checkpoint attempts or pure syscalls. Observed uncommitted work is a sampled lower bound, not a worst-case guarantee. Interrupted intervals are excluded from committed totals. Cooperative cancellation is not a process crash or power loss.',
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  assert.equal(process.argv.length, 3, 'Usage: node scripts/bench/summarize-metadata-checkpoints.mjs REPORT.json');
  console.log(JSON.stringify(summarizeCheckpointReport(JSON.parse(await readFile(process.argv[2], 'utf8'))), null, 2));
}
