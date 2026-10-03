import test from 'node:test';
import assert from 'node:assert/strict';
import { summarizeCheckpointReport } from '../bench/summarize-metadata-checkpoints.mjs';

function fixture() {
  const progress = [];
  let atUs = 0;
  const p = (phaseId, completed, total, checkpointRows, parsedRows = 10) => progress.push({
    phaseId, completed, total, checkpointRows, parsedRows, resumedRows: 0, atUs: atUs += 1000,
  });
  p('metadata-scan', 100, 100, 0);
  for (const [old, appended] of [[0, 10], [10, 0]]) {
    p('metadata-checkpoint-write', 0, appended, old);
    p('metadata-checkpoint-write', appended, appended, old);
    p('metadata-checkpoint-sync', 0, 0, old);
    p('metadata-checkpoint-publish', 0, 0, old);
    p('metadata-checkpoint-committed', 100, 100, 10);
  }
  return { schemaVersion: 1, mode: 'cold', probeElapsedMs: 15,
    context: { sourceRows: 10, sourceBytes: 100, sourceSha256: 'a'.repeat(64), binarySha256: 'b'.repeat(64), smallWaves: false },
    progress, result: { rows: 10, parsedRows: 10, resumedRows: 0, checkpointRows: 10, semanticSha256: 'c'.repeat(64) },
    uncachedParityVerified: true,
  };
}

test('counts checkpoint calls once, retains zero-row publication and labels inclusive stage intervals', () => {
  const actual = summarizeCheckpointReport(fixture());
  assert.equal(actual.callsReachingWrite, 2);
  assert.equal(actual.committedCalls, 2);
  assert.equal(actual.metadataOnlyCommittedCalls, 1);
  assert.deepEqual(actual.committedIntervalsMs, { write: 4, syncAndValidation: 2, publication: 2, total: 8 });
  assert.equal(actual.maxObservedUncommittedMetadataRows, 10);
  assert.equal(actual.maxObservedUncommittedSourceBytes, 100);
  assert.equal(actual.uncommittedCall, null);
});

test('never treats incomplete timing or degraded persistence as a zero-duration success', () => {
  const report = fixture(); report.progress.pop();
  assert.throws(() => summarizeCheckpointReport(report), /Incomplete checkpoint/);
  report.progress.push({ ...report.progress.at(-1), phaseId: 'metadata-unavailable' });
  assert.throws(() => summarizeCheckpointReport(report), /Persistence degraded/);
});

test('rejects reordered, duplicate, missing and inconsistent checkpoint stages', () => {
  for (const mutate of [
    report => { report.progress[3].phaseId = 'metadata-checkpoint-publish'; },
    report => { report.progress[3] = { ...report.progress[2] }; },
    report => { report.progress.splice(2, 1); },
    report => { report.progress[5].checkpointRows = 9; },
    report => { report.progress[2].atUs = 0; },
  ]) {
    const report = fixture(); mutate(report);
    assert.throws(() => summarizeCheckpointReport(report));
  }
});

test('warm reports require zero parser calls and zero checkpoint writes', () => {
  const report = fixture(); report.mode = 'warm'; report.progress = [];
  report.result.parsedRows = 0; report.result.resumedRows = 10;
  const actual = summarizeCheckpointReport(report);
  assert.equal(actual.callsReachingWrite, 0);
  assert.equal(actual.maxObservedUncommittedSourceBytes, null);
  report.progress = fixture().progress;
  assert.throws(() => summarizeCheckpointReport(report), /Warm reopen wrote/);
});

test('interrupted publication is kept separate and does not invent a commit or syscall duration', () => {
  const report = fixture(); report.mode = 'interrupt'; report.context.smallWaves = true;
  report.context.sourceRows = 20; report.context.sourceBytes = 200;
  report.progress = report.progress.slice(0, 10);
  for (const p of report.progress.slice(6)) { p.total = p.phaseId.endsWith('write') ? 10 : 0; p.parsedRows = 20; }
  report.progress[7].completed = 10;
  delete report.result; report.expectedCancellationError = 'Operação cancelada.';
  const actual = summarizeCheckpointReport(report);
  assert.equal(actual.callsReachingWrite, 2); assert.equal(actual.committedCalls, 1);
  assert.deepEqual(actual.uncommittedCall, { stage: 'publish', appendedRows: 10 });
  assert.equal(actual.committedIntervalsMs.total, 4);
  report.progress.pop();
  assert.throws(() => summarizeCheckpointReport(report), /pre-publication gate/);
});

test('requires bounded input, provenance, complete metadata count and oracle parity', () => {
  for (const mutate of [
    report => { report.context.sourceRows = 100_001; },
    report => { report.context.sourceBytes = 65 * 1024 * 1024; },
    report => { report.context.binarySha256 = 'unknown'; },
    report => { report.result.parsedRows = 9; },
    report => { report.uncachedParityVerified = false; },
    report => { report.context.smallWaves = true; },
  ]) {
    const report = fixture(); mutate(report);
    assert.throws(() => summarizeCheckpointReport(report));
  }
});

test('resume keeps byte-risk unknown and requires verified reuse of the committed prefix', () => {
  const report = fixture(); report.mode = 'resume'; report.context.smallWaves = true;
  report.result.parsedRows = 6; report.result.resumedRows = 4;
  report.progress = report.progress.slice(1);
  for (const p of report.progress) { p.resumedRows = 4; p.parsedRows = 6; if (p.checkpointRows === 0) p.checkpointRows = 4; }
  report.progress[0].checkpointRows = 4; report.progress[0].total = 6;
  report.progress[1].checkpointRows = 4; report.progress[1].completed = 6; report.progress[1].total = 6;
  report.recovery = { durableRowsBeforeInterrupt: 4, committedPrefixNotReparsed: true };
  const actual = summarizeCheckpointReport(report);
  assert.equal(actual.maxObservedUncommittedSourceBytes, null);
  assert.equal(actual.maxObservedUncommittedMetadataRows, 6);
  report.recovery.durableRowsBeforeInterrupt = 3;
  assert.throws(() => summarizeCheckpointReport(report));
});


test('explicit production policy factors are validated and legacy reports remain readable', () => {
  const report = fixture();
  report.context.checkpointPolicy = 'batched';
  report.context.checkpointThresholds = { rows: 65_536, sourceBytes: 16 * 1024 * 1024, elapsedMs: 2000, safeBoundaryOnly: true };
  assert.equal(summarizeCheckpointReport(report).context.checkpointPolicy, 'batched');
  report.context.checkpointThresholds.rows = 8;
  assert.throws(() => summarizeCheckpointReport(report), /Production sample changed/);
  report.context.checkpointPolicy = 'wave';
  report.context.checkpointThresholds = { rows: 1, sourceBytes: 1, elapsedMs: 0, safeBoundaryOnly: true };
  assert.equal(summarizeCheckpointReport(report).context.checkpointPolicy, 'wave');
});

test('manifest sync is still uncommitted and its optional interval never invents legacy timing', () => {
  const report = fixture();
  assert.equal(summarizeCheckpointReport(report).calls[0].manifestPreparationReporterIntervalMs, null);
  const ready = { ...report.progress[4], phaseId: 'metadata-checkpoint-manifest-ready' };
  report.progress.splice(5, 0, ready);
  for (const [i, p] of report.progress.entries()) p.atUs = (i + 1) * 1000;
  const actual = summarizeCheckpointReport(report);
  assert.equal(actual.calls[0].manifestPreparationReporterIntervalMs, 1);
  assert.equal(actual.calls[0].renameAndDirectorySyncReporterIntervalMs, 1);
  assert.equal(actual.calls[0].publicationReporterIntervalMs, 2);
  report.progress[5].checkpointRows = 10;
  assert.throws(() => summarizeCheckpointReport(report), /Unpublished manifest changed/);
});
