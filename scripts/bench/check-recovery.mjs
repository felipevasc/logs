// Destructive only to the child benchmark process. Always uses a new, isolated output directory.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createReadStream, createWriteStream } from 'node:fs';
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

export function checkpointRows(markers) {
  return markers.reduce((sum, marker) => {
    assert.ok(Number.isSafeInteger(marker.rows) && marker.rows > 0, 'Invalid checkpoint row count');
    return sum + marker.rows;
  }, 0);
}
export function benchmarkRecords(log) {
  return log.split('\n').filter(line => line.startsWith('BENCH ')).map(line => JSON.parse(line.slice(6)));
}
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
async function markers(engine) {
  const names = await readdir(engine).catch(error => { if (error.code === 'ENOENT') return []; throw error; });
  const result = [];
  for (const name of names.filter(name => name.endsWith('.complete.json')).sort()) {
    const bytes = await readFile(join(engine, name));
    result.push({ name, digest: digest(bytes), ...JSON.parse(bytes) });
  }
  return result;
}
async function run(binary, file, output, rows, interrupted) {
  const engine = join(output, 'engine');
  const log = createWriteStream(join(output, interrupted ? 'interrupted.log' : 'resumed.log'), { flags: 'wx' });
  let tail = '', killed = false, before = [], timer, logError;
  const child = spawn(binary, ['interactive_workload', '--exact', '--ignored', '--nocapture', '--test-threads=1'], {
    env: { ...process.env, LOGINSIGHT_BENCH_FILE: file, LOGINSIGHT_BENCH_DIR: output,
      LOGINSIGHT_BENCH_ROWS: String(rows), LOGINSIGHT_BENCH_REPEATS: '1', LOGINSIGHT_BENCH_CACHE_STATE: interrupted ? 'crash-probe' : 'crash-resume' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const stopChild = () => child.kill('SIGKILL');
  process.once('SIGINT', stopChild); process.once('SIGTERM', stopChild);
  log.on('error', error => { logError = error; stopChild(); });
  const capture = chunk => { if (!log.destroyed) log.write(chunk); tail += chunk; };
  child.stdout.on('data', capture); child.stderr.on('data', capture);
  const exited = new Promise((done, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => done({ code, signal }));
  });
  const deadline = setTimeout(() => child.kill('SIGKILL'), 15 * 60_000);
  const watch = interrupted ? (async () => {
    let ended = false; exited.finally(() => { ended = true; }).catch(() => {});
    while (!ended) {
      before = await markers(engine);
      const complete = checkpointRows(before);
      if (complete > 0 && complete < rows) {
        killed = child.kill('SIGKILL');
        break;
      }
      await new Promise(done => { timer = setTimeout(done, 20); });
    }
  })() : Promise.resolve();
  try {
    await watch;
    const result = await exited;
    if (logError) throw logError;
    if (interrupted) {
      assert.ok(killed && result.signal === 'SIGKILL', 'Child completed before a partial checkpoint boundary was observed; recovery was not exercised');
      before = await markers(engine);
      assert.ok(checkpointRows(before) > 0 && checkpointRows(before) < rows, 'All checkpoints completed before the child stopped; recovery was not exercised');
    }
    else assert.equal(result.code, 0, `Resumed benchmark failed: ${tail.slice(-4000)}`);
    return { ...result, before, records: benchmarkRecords(tail) };
  } finally {
    clearTimeout(deadline); clearTimeout(timer);
    process.removeListener('SIGINT', stopChild); process.removeListener('SIGTERM', stopChild);
    if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await exited.catch(() => {}); }
    if (!log.destroyed) await new Promise(done => log.end(done));
  }
}
export async function main(args) {
  const options = Object.fromEntries(args.reduce((pairs, arg, i) => {
    if (i % 2 === 0) { assert.ok(['--binary', '--file', '--output'].includes(arg), `Unknown option: ${arg}`); pairs.push([arg.slice(2), args[i + 1]]); }
    return pairs;
  }, []));
  assert.ok(options.binary && options.file && options.output, 'Required: --binary path --file generated.jsonl --output NEW-directory');
  const binary = resolve(options.binary), file = resolve(options.file), output = resolve(options.output);
  assert.ok((await stat(binary)).isFile() && (await stat(file)).isFile());
  const fixture = JSON.parse(await readFile(`${file}.manifest.json`, 'utf8'));
  assert.ok(Number.isSafeInteger(fixture.rows) && fixture.rows > 0);
  assert.ok(fixture.rows > 1_000_000 || fixture.bytes > 256 * 1024 * 1024, 'Fixture must span multiple checkpoints');
  assert.equal((await stat(file)).size, fixture.bytes, 'Fixture byte count matches its manifest');
  const sourceHash = createHash('sha256');
  for await (const chunk of createReadStream(file)) sourceHash.update(chunk);
  assert.equal(sourceHash.digest('hex'), fixture.sha256, 'Fixture SHA-256 matches its manifest');
  await mkdir(output); // Deliberately no recursive/exist-ok: never reuse or remove somebody else's cache.
  const first = await run(binary, file, output, fixture.rows, true);
  const resumed = await run(binary, file, output, fixture.rows, false);
  const after = await markers(join(output, 'engine'));
  assert.equal(checkpointRows(after), fixture.rows, 'Every source row has a published checkpoint after resume');
  for (const old of first.before) assert.equal(after.find(marker => marker.name === old.name)?.digest, old.digest, 'Completed checkpoint manifest is reused unchanged');
  const count = resumed.records.find(record => record.operation === 'exact_count' && record.detail?.workload === 'all');
  assert.equal(count?.detail?.count, fixture.rows, 'Resumed indexed query returns the complete source count');
  const report = { fixtureSha256: fixture.sha256, rows: fixture.rows, completedRowsBeforeKill: checkpointRows(first.before),
    completedSegmentsBeforeKill: first.before.length, finalSegments: after.length, signal: first.signal,
    manifestReuseVerified: true, exactCountAfterResume: count.detail.count,
    preparationAfterResumeMs: resumed.records.find(record => record.operation === 'prepare')?.elapsedMs,
    scope: 'Actual process SIGKILL during engine preparation. Initial line-metadata pass was already complete. No desktop or 50M latency claim.' };
  await writeFile(join(output, 'recovery.json'), `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx' });
  console.log(JSON.stringify(report, null, 2));
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main(process.argv.slice(2)).catch(error => { console.error(error); process.exitCode = 1; });
