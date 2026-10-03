/** Explicit, bounded native A/B runner. Never compiles, installs, or generates data. */
import { createHash } from 'node:crypto';
import { spawn, execFileSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { mkdir, readFile, writeFile, stat, lstat, readlink } from 'node:fs/promises';
import { availableParallelism, cpus, platform, release, totalmem } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const IDENTITY_VERSION = 3;
export const RESIDUAL_WARNING = 'Este filtro exige confirmação nos registros; consultas amplas podem demorar mais.';

export function validatePageDiagnostics(row) {
  if (!Array.isArray(row.diagnostics) || row.diagnostics.length < 1 || row.diagnostics.length > 2) throw new Error('Missing native page diagnostics');
  for (const diagnostic of row.diagnostics) {
    if (!diagnostic || Object.keys(diagnostic).sort().join(',') !== 'engine,warning' || diagnostic.engine !== 'columnar') throw new Error('Invalid native page engine/diagnostic fields');
    if (diagnostic.warning !== null && diagnostic.warning !== RESIDUAL_WARNING) throw new Error('Unknown native page warning');
  }
  if (row.iteration !== 0) {
    if (row.pathAudit !== null) throw new Error('Unexpected native page path audit iteration');
    return;
  }
  if (!Array.isArray(row.pathAudit) || row.pathAudit.length !== row.diagnostics.length) throw new Error('Missing native page path audit');
  row.pathAudit.forEach((audit, index) => {
    if (!audit || audit.pageIndex !== index || audit.offset !== index * 100 || (index === 0 ? audit.inputCursor !== null : typeof audit.inputCursor !== 'string' || !audit.inputCursor)) throw new Error('Invalid native page path audit bounds');
    const plan = audit.plan;
    if (!plan || plan.analyzed !== false || typeof plan.exactPredicate !== 'boolean' || (plan.mode !== undefined && (plan.mode !== 'verified_singleton' || !plan.exactPredicate))) throw new Error('Invalid native page path proof');
    const expectedWarning = plan.exactPredicate ? null : RESIDUAL_WARNING;
    if (row.diagnostics[index].warning !== expectedWarning) throw new Error('Native page warning/path mismatch');
  });
}

export function parseBench(output) {
  return output.split(/\r?\n/).flatMap(line => {
    const at = line.indexOf('BENCH ');
    if (at < 0) return [];
    try { return [JSON.parse(line.slice(at + 6))]; }
    catch { throw new Error(`Invalid BENCH record: ${line.slice(0, 160)}`); }
  });
}
export function quantiles(values) {
  if (!values.length || values.some(v => !Number.isFinite(v) || v < 0)) throw new Error('Expected nonnegative finite samples');
  const sorted = [...values].sort((a, b) => a - b);
  return { samples: sorted.length, minMs: sorted[0], p50Ms: sorted[Math.ceil(sorted.length * 0.5) - 1],
    // Five process starts support a median, not a robust p95 claim.
    p95Ms: sorted.length >= 30 ? sorted[Math.ceil(sorted.length * 0.95) - 1] : null,
    maxMs: sorted.at(-1) };
}
export function identities(records, expectedRepeats) {
  const found = new Map(), samples = new Map(), fullWorkloads = new Map(), fullSamples = new Map(), phaseDiagnostics = new Map();
  for (const row of records.filter(row => row.operation === 'page_identity')) {
    if (row.identityVersion !== IDENTITY_VERSION) throw new Error(`Unsupported native page identityVersion: ${row.identityVersion ?? 'missing'}; rebuild both binaries with the same v${IDENTITY_VERSION} harness`);
    if (typeof row.workload !== 'string' || !row.workload || !/^[a-f0-9]{64}$/.test(row.sha256 ?? '') || !/^[a-f0-9]{64}$/.test(row.fullSha256 ?? '') || !/^[a-f0-9]{64}$/.test(row.responseSha256 ?? '')) throw new Error('Invalid native page identity');
    if (!['before_exact_summary', 'after_exact_summary'].includes(row.phase) || !Number.isInteger(row.iteration) || row.iteration < 0) throw new Error('Invalid native page phase/iteration');
    validatePageDiagnostics(row);
    const phaseKey = `${row.workload}/${row.phase}`;
    const diagnosticIdentity = JSON.stringify(row.diagnostics);
    if (phaseDiagnostics.has(phaseKey) && phaseDiagnostics.get(phaseKey) !== diagnosticIdentity) throw new Error(`Native page diagnostic path changed within ${phaseKey}; rerun path audit`);
    phaseDiagnostics.set(phaseKey, diagnosticIdentity);
    const key = row.workload, sampleKey = `${key}/${row.phase}/${row.iteration}`;
    if (samples.has(sampleKey)) throw new Error(`Duplicate native page sample: ${sampleKey}`);
    samples.set(sampleKey, row.sha256);
    if (found.has(key) && found.get(key) !== row.sha256) throw new Error(`Page parity changed within ${key}`);
    found.set(key, row.sha256);
    if (fullWorkloads.has(key) && fullWorkloads.get(key) !== row.fullSha256) throw new Error(`Full page/cursor parity changed within ${key}`);
    fullWorkloads.set(key, row.fullSha256);
    fullSamples.set(sampleKey, row.fullSha256);
  }
  if (!found.size) throw new Error('No native page identity records; test did not run');
  for (const key of found.keys()) {
    const phases = ['before_exact_summary', 'after_exact_summary'].map(phase =>
      [...samples.keys()].filter(sample => sample.startsWith(`${key}/${phase}/`)).map(sample => Number(sample.split('/').at(-1))).sort((a, b) => a - b));
    if (!phases[0].length || phases[0].length !== phases[1].length || phases.some(list => list.some((iteration, i) => iteration !== i))
      || (expectedRepeats !== undefined && phases[0].length !== expectedRepeats)) throw new Error(`Missing native page phase/sample: ${key}`);
  }
  return { identityVersion: IDENTITY_VERSION, workloads: found, samples, fullWorkloads, fullSamples };
}
export function assertParity(left, right, expectedRepeats) {
  const a = identities(left, expectedRepeats), b = identities(right, expectedRepeats);
  if (a.samples.size !== b.samples.size) throw new Error('A/B workload/sample sets differ');
  for (const [key, hash] of a.samples) if (b.samples.get(key) !== hash) throw new Error(`Native A/B page parity failed: ${key}`);
}
export function assertStableVersion(left, right, expectedRepeats) {
  const a = identities(left, expectedRepeats), b = identities(right, expectedRepeats);
  if (a.fullSamples.size !== b.fullSamples.size) throw new Error('Same-version workload/sample sets differ');
  for (const [key, hash] of a.fullSamples) if (b.fullSamples.get(key) !== hash) throw new Error(`Full page/cursor parity changed across same-version runs: ${key}`);
}
async function digest(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}
async function diskBytes(path) {
  const { readdir } = await import('node:fs/promises');
  let total = 0, items;
  try { items = await readdir(path, { withFileTypes: true }); } catch (error) { if (error.code === 'ENOENT') return 0; throw error; }
  for (const item of items) {
    const next = join(path, item.name);
    if (item.isDirectory()) total += await diskBytes(next);
    else if (item.isFile()) total += (await stat(next)).size;
  }
  return total;
}
export async function nativeRun({ binary, fixture, runDir, cacheState, repeats, warmup, rows, sortDir, extraEnv = {} }) {
  await mkdir(runDir, { recursive: true });
  const started = performance.now();
  const env = { ...process.env, ...extraEnv };
  delete env.LOGINSIGHT_PAGE_TRACE; // Causal diagnostics run separately from latency samples.
  delete env.LOGINSIGHT_BENCH_DUMP_PAGES; // Never inherit an accidental data dump.
  delete env.LOGINSIGHT_BENCH_DUMP_PHASE;
  const child = spawn(binary, ['--ignored', '--exact', 'interactive_workload', '--nocapture', '--test-threads=1'], {
    env: { ...env, LOGINSIGHT_BENCH_FILE: fixture, LOGINSIGHT_BENCH_DIR: runDir,
      LOGINSIGHT_BENCH_CACHE_STATE: cacheState, LOGINSIGHT_BENCH_REPEATS: String(repeats),
      LOGINSIGHT_BENCH_WARMUP: String(warmup), LOGINSIGHT_BENCH_ROWS: String(rows), LOGINSIGHT_BENCH_SORT_DIR: sortDir },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stdout = '', stderr = '', maxSampledRssBytes = 0, rssSamples = 0;
  child.stdout.on('data', bytes => { stdout += bytes; });
  child.stderr.on('data', bytes => { stderr += bytes; });
  // Linux sampled RSS is a lower bound, not a claim of OS-recorded peak RSS.
  const sample = async () => {
    if (platform() !== 'linux' || !child.pid) return;
    try {
      const status = await readFile(`/proc/${child.pid}/status`, 'utf8');
      const kb = Number(status.match(/^VmRSS:\s+(\d+)\s+kB$/m)?.[1]);
      if (Number.isFinite(kb)) { maxSampledRssBytes = Math.max(maxSampledRssBytes, kb * 1024); rssSamples++; }
    } catch { /* A short process can exit between samples. */ }
  };
  const sampler = setInterval(sample, 100);
  let code, signal;
  try { [code, signal] = await new Promise((resolve, reject) => {
    child.once('error', reject); child.once('close', (code, signal) => resolve([code, signal]));
  }); } finally { clearInterval(sampler); }
  const result = { identityVersion: IDENTITY_VERSION, cacheState, code, signal, pageDumpEnabled: false, elapsedMs: performance.now() - started,
    sampledRss: { maxBytes: maxSampledRssBytes, samples: rssSamples, intervalMs: 100, isLowerBound: true },
    records: parseBench(stdout), cacheBytes: { data: await diskBytes(join(runDir, 'data')), engine: await diskBytes(join(runDir, 'engine')) } };
  await writeFile(join(runDir, `${cacheState}.stdout.log`), stdout, { flag: 'wx' });
  await writeFile(join(runDir, `${cacheState}.stderr.log`), stderr, { flag: 'wx' });
  await writeFile(join(runDir, `${cacheState}.json`), JSON.stringify(result, null, 2) + '\n', { flag: 'wx' });
  if (code !== 0) throw new Error(`Native ${cacheState} failed (${code ?? signal}); see ${runDir}`);
  identities(result.records, repeats);
  return result;
}
export async function sourceIdentity(source) {
  const git = (...args) => execFileSync('git', ['-C', source, ...args], { encoding: 'utf8' }).trim();
  // New parser/settings modules may be intentionally uncommitted during A/B.
  // git diff alone omits untracked contents, so retain their exact identity too.
  const untracked = [];
  const paths = execFileSync('git', ['-C', source, 'ls-files', '--others', '--exclude-standard', '-z'], { encoding: 'utf8' }).split('\0').filter(Boolean).sort();
  for (const path of paths) {
    const info = await lstat(join(source, path));
    if (info.isSymbolicLink()) untracked.push({ path, symlink: await readlink(join(source, path)) });
    else if (info.isFile()) untracked.push({ path, bytes: info.size, sha256: await digest(join(source, path)) });
    else throw new Error(`Unsupported untracked source entry: ${path}`);
  }
  return { directory: source, commit: git('rev-parse', 'HEAD'), tree: git('rev-parse', 'HEAD^{tree}'),
    untracked, untrackedSha256: createHash('sha256').update(JSON.stringify(untracked)).digest('hex'),
    diffSha256: createHash('sha256').update(git('diff', 'HEAD')).digest('hex'),
    status: git('status', '--short'), cargoLockSha256: await digest(join(source, 'src-tauri/Cargo.lock')) };
}
async function main() {
  const allowed = ['--baseline', '--candidate', '--baseline-source', '--candidate-source', '--fixture', '--output', '--processes', '--repeats', '--warmup', '--sort-dir'];
  const flags = new Map();
  for (let i = 2; i < process.argv.length; i += 2) {
    if (!allowed.includes(process.argv[i]) || process.argv[i + 1] === undefined || flags.has(process.argv[i])) throw new Error('Expected unique --flag value pairs');
    flags.set(process.argv[i], process.argv[i + 1]);
  }
  for (const key of allowed.slice(0, 6)) if (!flags.has(key)) throw new Error(`Required: ${key}`);
  const processes = Number(flags.get('--processes') ?? 5), repeats = Number(flags.get('--repeats') ?? 30), warmup = Number(flags.get('--warmup') ?? 3);
  if (!Number.isInteger(processes) || processes < 1 || processes > 10) throw new Error('processes must be 1..10');
  if (!Number.isInteger(repeats) || repeats < 1 || repeats > 100) throw new Error('repeats must be 1..100');
  if (!Number.isInteger(warmup) || warmup < 0 || warmup > 20) throw new Error('warmup must be 0..20');
  const sortDir = flags.get('--sort-dir') ?? 'asc';
  if (!['asc', 'desc'].includes(sortDir)) throw new Error('sort-dir must be asc or desc');
  const fixture = resolve(flags.get('--fixture')), output = resolve(flags.get('--output'));
  const manifest = JSON.parse(await readFile(`${fixture}.manifest.json`, 'utf8'));
  if (!Number.isInteger(manifest.rows) || manifest.rows < 1 || manifest.rows > 100_000) throw new Error('This initial runner allows at most 100,000 rows');
  if (await digest(fixture) !== manifest.sha256 || (await stat(fixture)).size !== manifest.bytes) throw new Error('Fixture manifest mismatch');
  // wx-style directory ownership: never reuse or erase a previous experiment.
  await mkdir(output);
  const binaries = { baseline: resolve(flags.get('--baseline')), candidate: resolve(flags.get('--candidate')) };
  const identity = { identityVersion: IDENTITY_VERSION, pageIdentityContract: { crossVersion: 'rows/order/total/hasMore/cursor, masking only validated 64hex fingerprint spans', sameVersion: 'full data and opaque cursors', operationalMetadata: 'columnar only; exact known warning checked against per-page path audit', fullResponse: 'recorded separately as responseSha256 and original diagnostic dumps' }, createdAt: new Date().toISOString(), limits: { processes, repeats, warmup, sortDir }, fixture: { path: fixture, ...manifest },
    machine: { platform: platform(), release: release(), cpu: cpus()[0]?.model, availableParallelism: availableParallelism(), totalMemoryBytes: totalmem(), node: process.version },
    environment: Object.fromEntries(Object.entries(process.env).filter(([key]) => ['LOGINSIGHT_MEMORY_LIMIT_MB', 'LOGINSIGHT_SELECTION_LIMIT_MB', 'LOGINSIGHT_SELECTION_CACHE_MB', 'LOGINSIGHT_COLLECTED_IDS_MB', 'LOGINSIGHT_ANALYTICS_LIMIT_MB', 'LOGINSIGHT_TEMP_LIMIT_MB', 'LOGINSIGHT_QUERY_SPILL_MB', 'LOGINSIGHT_BUILD_SPILL_MB', 'LOGINSIGHT_ENGINE', 'LOGINSIGHT_TIME_PRECOMPUTE', 'RAYON_NUM_THREADS'].includes(key))),
    pageTraceEnabled: false, pageDumpEnabled: false, binaries: {}, limitations: ['OS cache is uncontrolled and fixture hashing warms reads', 'Native backend only, no IPC/WebView/DOM/paint', 'RSS sampled every 100ms is a lower bound', 'Compilation excluded; use identical toolchain/profile/dependencies for both binaries'] };
  for (const label of ['baseline', 'candidate']) identity.binaries[label] = { path: binaries[label], sha256: await digest(binaries[label]),
    source: await sourceIdentity(resolve(flags.get(`--${label}-source`))) };
  await writeFile(join(output, 'identity.json'), JSON.stringify(identity, null, 2) + '\n', { flag: 'wx' });
  const results = [], firstVersionRun = new Map();
  for (let pair = 0; pair < processes; pair++) {
    const order = pair % 2 ? ['candidate', 'baseline'] : ['baseline', 'candidate'];
    for (const cacheState of ['initial_preparation', 'new_process_reopen']) {
      const compared = {};
      for (const label of cacheState === 'initial_preparation' ? order : [...order].reverse()) {
        console.log(`${pair + 1}/${processes}: ${label} ${cacheState}`);
        const result = await nativeRun({ binary: binaries[label], fixture, runDir: join(output, `${label}-${pair}`), cacheState, repeats, warmup, rows: manifest.rows, sortDir });
        if (firstVersionRun.has(label)) assertStableVersion(firstVersionRun.get(label), result.records, repeats);
        else firstVersionRun.set(label, result.records);
        results.push({ pair, label, ...result }); compared[label] = result;
      }
      assertParity(compared.baseline.records, compared.candidate.records, repeats);
    }
  }
  const summary = {};
  for (const result of results) for (const row of result.records) {
    if (!Number.isFinite(row.elapsedMs)) continue;
    const key = [result.label, result.cacheState, row.operation, row.detail?.workload ?? '', row.detail?.phase ?? ''].join('/');
    (summary[key] ??= []).push(row.elapsedMs);
  }
  await writeFile(join(output, 'summary.json'), JSON.stringify({ identityVersion: IDENTITY_VERSION, parity: 'passed', fullSameVersionParity: 'passed', distributions: Object.fromEntries(Object.entries(summary).map(([key, values]) => [key, quantiles(values)])) }, null, 2) + '\n', { flag: 'wx' });
  console.log(`Native A/B finished with page parity: ${output}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main().catch(error => { console.error(error); process.exitCode = 1; });
