import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { parseBench, quantiles, identities, assertParity, assertStableVersion, RESIDUAL_WARNING, validatePageDiagnostics } from '../bench/compare-performance.mjs';


const diagnosticFixture = (iteration = 0, residual = false, count = 1) => ({
  responseSha256: 'd'.repeat(64),
  diagnostics: Array.from({ length: count }, () => ({ engine: 'columnar', warning: residual ? RESIDUAL_WARNING : null })),
  pathAudit: iteration === 0 ? Array.from({ length: count }, (_, index) => ({ pageIndex: index, offset: index * 100,
    inputCursor: index ? '{"version":1}' : null, plan: { analyzed: false, exactPredicate: !residual } })) : null,
});

test('native baseline parser accepts Rust harness prefixes and rejects malformed records', () => {
  assert.deepEqual(parseBench('test interactive_workload ... BENCH {"operation":"prepare","elapsedMs":1}\nignored noise'), [{ operation: 'prepare', elapsedMs: 1 }]);
  assert.throws(() => parseBench('BENCH {broken'), /Invalid BENCH/);
});
test('process-start distributions do not claim p95 from five samples', () => {
  assert.deepEqual(quantiles([5, 1, 4, 2, 3]), { samples: 5, minMs: 1, p50Ms: 3, p95Ms: null, maxMs: 5 });
  assert.equal(quantiles(Array.from({ length: 30 }, (_, i) => i)).p95Ms, 28);
  assert.throws(() => quantiles([])); assert.throws(() => quantiles([NaN]));
});
test('page parity requires valid digests, both phases and every A/B sample', () => {
  const record = (workload, sha256, phase, iteration = 0) => ({ operation: 'page_identity', identityVersion: 3, workload, sha256, fullSha256: sha256, phase, iteration, ...diagnosticFixture(iteration) });
  const before = record('residual', 'a'.repeat(64), 'before_exact_summary'), after = record('residual', 'a'.repeat(64), 'after_exact_summary');
  assert.equal(identities([before, after], 1).workloads.get('residual'), 'a'.repeat(64));
  assertParity([before, after], [after, before], 1);
  assert.throws(() => identities([]), /did not run/);
  assert.throws(() => identities([{ ...before, identityVersion: undefined }, after]), /identityVersion/);
  assert.throws(() => identities([{ ...before, identityVersion: 1 }, after]), /identityVersion/);
  assert.throws(() => identities([{ ...before, identityVersion: 2 }, after]), /identityVersion/);
  assert.throws(() => identities([before, { ...after, fullSha256: undefined }]), /Invalid native/);
  assert.throws(() => identities([before, { ...after, fullSha256: 'b'.repeat(64) }]), /Full page\/cursor parity changed within/);
  assert.throws(() => identities([before, { ...after, sha256: 'b'.repeat(64) }]), /within residual/);
  assert.throws(() => identities([before, { ...after, sha256: undefined }]), /Invalid native/);
  assert.throws(() => identities([before, after, after]), /Duplicate/);
  assert.throws(() => identities([before]), /Missing/);
  assert.throws(() => identities([before, after], 2), /Missing/);
  assert.throws(() => identities([{ ...before, iteration: 1, ...diagnosticFixture(1) }, { ...after, iteration: 1, ...diagnosticFixture(1) }]), /Missing/);
  assert.throws(() => assertParity([before], [before, after]), /Missing/);
  assert.throws(() => assertParity([before, after], [{ ...before, sha256: 'b'.repeat(64) }, { ...after, sha256: 'b'.repeat(64) }]), /A\/B page parity/);
  const extra = [record('extra', 'c'.repeat(64), 'before_exact_summary'), record('extra', 'c'.repeat(64), 'after_exact_summary')];
  assert.throws(() => assertParity([before, after], [before, after, ...extra]), /sets differ/);
});

test('v3 cross-version gate permits scoped fingerprints but rejects changed rows, ordering and cursor bounds', () => {
  const hash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
  // Contract fixtures carry independently specified semantic/full payloads.
  // Native identity_tests exercise the actual byte-preserving normalizer.
  const pages = (fingerprint, overrides = {}) => [{ rows: [{ id: 1 }, { id: 2 }], total: 300, hasMore: true,
    nextCursor: JSON.stringify({ version: 1, fingerprint, position: 100, keys: [{ Integer: 99 }], future: true }), ...overrides }];
  const records = (semantic, original) => ['before_exact_summary', 'after_exact_summary'].flatMap(phase =>
    [0, 1].map(iteration => ({ operation: 'page_identity', identityVersion: 3, workload: 'all', phase, iteration,
      sha256: hash(semantic), fullSha256: hash(original), ...diagnosticFixture(iteration) })));
  const canonical = pages('0'.repeat(64));
  const baseline = records(canonical, pages('a'.repeat(64)));
  const candidate = records(canonical, pages('b'.repeat(64)));
  assertParity(baseline, candidate, 2);
  assertStableVersion(baseline, baseline.toReversed(), 2);
  assert.throws(() => assertStableVersion(baseline, candidate, 2), /same-version runs/);
  assert.throws(() => identities(candidate.map((row, i) => i === 1 ? { ...row, fullSha256: hash(pages('c'.repeat(64))) } : row), 2), /Full page\/cursor parity changed within/);
  const changed = [
    pages('0'.repeat(64), { rows: [{ id: 1 }, { id: 3 }] }),
    pages('0'.repeat(64), { rows: [{ id: 2 }, { id: 1 }] }),
    pages('0'.repeat(64), { total: 301 }),
    pages('0'.repeat(64), { hasMore: false, nextCursor: null }),
    ...['version', 'position', 'keys', 'future'].map(field => {
      const cursor = JSON.parse(canonical[0].nextCursor);
      cursor[field] = { version: 2, position: 101, keys: [{ Text: '99' }], future: false }[field];
      return pages('0'.repeat(64), { nextCursor: JSON.stringify(cursor) });
    }),
  ];
  for (const semantic of changed) assert.throws(() => assertParity(baseline, records(semantic, semantic), 2), /A\/B page parity/);
});

test('v3 rejects unsafe diagnostics and requires warning to match every audited page path', () => {
  const exact = { iteration: 0, ...diagnosticFixture(0, false, 2) };
  const residual = { iteration: 0, ...diagnosticFixture(0, true, 2) };
  validatePageDiagnostics(exact);
  validatePageDiagnostics(residual);
  const singleton = structuredClone(exact);
  singleton.pathAudit[0].plan.mode = 'verified_singleton';
  validatePageDiagnostics(singleton);
  for (const warning of ['truncated', 'incomplete', 'error', 'cancelled', 'resource limit', '', false, {}]) {
    const invalid = structuredClone(exact); invalid.diagnostics[1].warning = warning;
    assert.throws(() => validatePageDiagnostics(invalid), /Unknown native page warning/);
  }
  for (const field of ['truncated', 'incomplete', 'error', 'cancelled', 'limitExceeded']) {
    const invalid = structuredClone(exact); invalid.diagnostics[0][field] = true;
    assert.throws(() => validatePageDiagnostics(invalid), /diagnostic fields/);
  }
  const fallback = structuredClone(exact); fallback.diagnostics[0].engine = 'lines';
  assert.throws(() => validatePageDiagnostics(fallback), /engine/);
  const wrongExact = structuredClone(residual); wrongExact.pathAudit[1].plan.exactPredicate = true;
  assert.throws(() => validatePageDiagnostics(wrongExact), /warning\/path mismatch/);
  const wrongResidual = structuredClone(exact); wrongResidual.pathAudit[0].plan.exactPredicate = false;
  assert.throws(() => validatePageDiagnostics(wrongResidual), /warning\/path mismatch/);
  const falseSingleton = structuredClone(residual); falseSingleton.pathAudit[0].plan.mode = 'verified_singleton';
  assert.throws(() => validatePageDiagnostics(falseSingleton), /path proof/);
  const bounds = structuredClone(exact); bounds.pathAudit[1].offset = 0;
  assert.throws(() => validatePageDiagnostics(bounds), /audit bounds/);
  const noCursor = structuredClone(exact); noCursor.pathAudit[1].inputCursor = null;
  assert.throws(() => validatePageDiagnostics(noCursor), /audit bounds/);
  assert.throws(() => validatePageDiagnostics({ ...exact, pathAudit: undefined }), /Missing native page path audit/);
  assert.throws(() => validatePageDiagnostics({ ...exact, diagnostics: undefined }), /Missing native page diagnostics/);
});

test('v3 retains operational differences across phases without weakening data/full cursor parity', () => {
  const record = (phase, iteration, residual) => ({ operation: 'page_identity', identityVersion: 3, workload: 'cached_raw_regex',
    sha256: 'a'.repeat(64), fullSha256: 'b'.repeat(64), phase, iteration, ...diagnosticFixture(iteration, residual) });
  const records = [record('before_exact_summary', 0, true), record('before_exact_summary', 1, true),
    record('after_exact_summary', 0, false), record('after_exact_summary', 1, false)];
  identities(records, 2);
  const baseline = records.map(row => ({ ...row, ...diagnosticFixture(row.iteration, true) }));
  assertParity(baseline, records, 2);
  const changed = structuredClone(records); changed[1].diagnostics[0].warning = null;
  assert.throws(() => identities(changed, 2), /diagnostic path changed within/);
  const droppedAudit = structuredClone(records); droppedAudit[2].pathAudit = null;
  assert.throws(() => identities(droppedAudit, 2), /Missing native page path audit/);
  const changedData = structuredClone(records); changedData[2].fullSha256 = 'c'.repeat(64);
  assert.throws(() => identities(changedData, 2), /Full page\/cursor parity changed within/);
});

test('runner preserves logs, excludes them from cache bytes and disables causal trace', { skip: process.platform === 'win32' ? 'POSIX fake executable; real native runner supports .exe' : false }, async () => {
  const { nativeRun } = await import('../bench/compare-performance.mjs');
  const { mkdtemp, writeFile, readFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const root = await mkdtemp(join(tmpdir(), 'performance-runner-'));
  try {
    const binary = join(root, 'fake-native.mjs');
    await writeFile(binary, `#!/usr/bin/env node
import { mkdirSync, writeFileSync } from 'node:fs';
import assert from 'node:assert/strict';
assert.equal(process.env.LOGINSIGHT_PAGE_TRACE, undefined);
assert.equal(process.env.LOGINSIGHT_BENCH_DUMP_PAGES, undefined);
assert.equal(process.env.LOGINSIGHT_BENCH_DUMP_PHASE, undefined);
assert.deepEqual(process.argv.slice(2), ['--ignored','--exact','interactive_workload','--nocapture','--test-threads=1']);
for (const [name, bytes] of [['data', 7], ['engine', 11]]) {
  mkdirSync(process.env.LOGINSIGHT_BENCH_DIR + '/' + name, { recursive: true });
  writeFileSync(process.env.LOGINSIGHT_BENCH_DIR + '/' + name + '/cache', 'x'.repeat(bytes));
}
for (const phase of ['before_exact_summary','after_exact_summary']) console.log('BENCH ' + JSON.stringify({ operation:'page_identity', identityVersion:3, workload:'test', sha256:'a'.repeat(64), fullSha256:'b'.repeat(64), responseSha256:'c'.repeat(64), phase, iteration:0, diagnostics:[{engine:'columnar',warning:null}], pathAudit:[{pageIndex:0,offset:0,inputCursor:null,plan:{analyzed:false,exactPredicate:true}}] }));
console.error('fake diagnostic: tests orchestration only');
`, { mode: 0o700 });
    const options = { binary, fixture: join(root, 'not-read-by-fake.jsonl'), runDir: join(root, 'run'), repeats: 1, warmup: 0, rows: 1, sortDir: 'asc', extraEnv: { LOGINSIGHT_PAGE_TRACE: '1', LOGINSIGHT_BENCH_DUMP_PAGES: '*', LOGINSIGHT_BENCH_DUMP_PHASE: 'both' } };
    const first = await nativeRun({ ...options, cacheState: 'initial_preparation' });
    const second = await nativeRun({ ...options, cacheState: 'new_process_reopen' });
    assert.deepEqual(first.cacheBytes, { data: 7, engine: 11 });
    assert.deepEqual(second.cacheBytes, first.cacheBytes);
    assert.match(await readFile(join(root, 'run/new_process_reopen.stderr.log'), 'utf8'), /fake diagnostic/);
    assert.equal(second.code, 0);
  } finally { await rm(root, { recursive: true, force: true }); }
});


test('source identity includes new untracked module bytes', async () => {
  const { sourceIdentity } = await import('../bench/compare-performance.mjs');
  const { mkdtemp, mkdir, writeFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { execFileSync } = await import('node:child_process');
  const root = await mkdtemp(join(tmpdir(), 'performance-source-'));
  const git = (...args) => execFileSync('git', ['-C', root, ...args], { stdio: 'pipe' });
  try {
    git('init'); await mkdir(join(root, 'src-tauri'));
    await writeFile(join(root, 'src-tauri/Cargo.lock'), 'lock fixture');
    await writeFile(join(root, '.gitignore'), 'ignored.txt\n');
    git('add', '.');
    git('-c', 'user.name=Test fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-m', 'fixture');
    await writeFile(join(root, 'new-module.rs'), 'first');
    await writeFile(join(root, 'ignored.txt'), 'ignored');
    const first = await sourceIdentity(root);
    assert.deepEqual(first.untracked.map(row => row.path), ['new-module.rs']);
    await writeFile(join(root, 'new-module.rs'), 'second');
    const second = await sourceIdentity(root);
    assert.equal(first.diffSha256, second.diffSha256);
    assert.notEqual(first.untrackedSha256, second.untrackedSha256);
    assert.notEqual(first.untracked[0].sha256, second.untracked[0].sha256);
    git('add', 'new-module.rs');
    const staged = await sourceIdentity(root);
    assert.deepEqual(staged.untracked, []);
    assert.notEqual(second.diffSha256, staged.diffSha256);
  } finally { await rm(root, { recursive: true, force: true }); }
});
