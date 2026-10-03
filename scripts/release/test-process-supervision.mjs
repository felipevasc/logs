// Real short-lived Node descendants, including a detached/orphaned grandchild;
// no application build, browser installation or network service is required.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, copyFile, rm } from 'node:fs/promises';
import { resolve, join, sep } from 'node:path';
import { spawnManaged, waitManaged, previewEnvironment } from '../preview/managed-process.mjs';

assert.equal(previewEnvironment({}).PLAYWRIGHT_CHANNEL, 'chromium');
assert.equal(previewEnvironment({ PLAYWRIGHT_CHANNEL: 'chrome', KEEP: 'yes' }).PLAYWRIGHT_CHANNEL, 'chrome');
assert.equal(previewEnvironment({ KEEP: 'yes' }).KEEP, 'yes');
const output = resolve('output');
await mkdir(output, { recursive: true });
const directory = await mkdtemp(join(output, 'process supervision-'));
const fixture = join(directory, 'child fixture.mjs');
await writeFile(fixture, `
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
const mode = process.argv[2];
assert.equal(process.argv[3], 'quotes" and trailing\\\\');
console.log(JSON.stringify({ role: mode, pid: process.pid }));
if (mode === 'leaf') {
  process.send?.('ready');
} else if (mode === 'middle') {
  const leaf = spawn(process.execPath, [process.argv[1], 'leaf', process.argv[3]], { detached: true, stdio: ['ignore', 'inherit', 'inherit', 'ipc'] });
  leaf.once('message', () => { console.log(JSON.stringify({ ready: true })); process.exit(0); });
} else {
  const nested = mode === 'orphan' ? 'middle' : 'leaf';
  const child = spawn(process.execPath, [process.argv[1], nested, process.argv[3]], { detached: true, stdio: ['ignore', 'inherit', 'inherit', 'ipc'] });
  child.once('message', () => {
    console.log(JSON.stringify({ ready: true }));
    if (mode === 'exit-zero' || mode === 'exit-error') process.exit(mode === 'exit-error' ? 7 : 0);
  });
}
setInterval(() => {}, 1000);
`);
const sentinel = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore', windowsHide: true });
const sentinelClosed = new Promise(resolve => sentinel.once('close', resolve));
let active;
const alive = pid => { try { process.kill(pid, 0); return true; } catch (error) { if (error.code === 'ESRCH') return false; throw error; } };
try {
  // Exercise the real runner's startup failure and report without overwriting
  // another run's evidence in the repository's output/playwright directory.
  const preview = join(directory, 'scripts', 'preview');
  await mkdir(preview, { recursive: true });
  await mkdir(join(directory, 'scripts', 'ci'), { recursive: true });
  await copyFile(resolve('scripts/ci/validation-plan.mjs'), join(directory, 'scripts', 'ci', 'validation-plan.mjs'));
  for (const name of ['run-smoke.mjs', 'managed-process.mjs', 'windows-job.ps1']) {
    await copyFile(resolve('scripts/preview', name), join(preview, name));
  }
  await writeFile(join(preview, 'test-startup.mjs'), "throw Error('Must not execute when the supervisor is missing');\n");
  assert.throws(() => execFileSync(process.execPath, [join(preview, 'run-smoke.mjs'), 'test-startup.mjs'], {
    cwd: directory, env: { ...process.env, PATH: directory }, stdio: 'pipe', timeout: 10_000,
  }), error => error.status === 1);
  const failedStartup = JSON.parse(await readFile(join(directory, 'output', 'playwright', 'smoke-summary.json'), 'utf8'));
  assert.equal(failedStartup.passed, false);
  assert.equal(failedStartup.tests.length, 0);
  assert.match(failedStartup.runError, /ENOENT|requires Python/);
  console.log('PASS: runner startup failure exits nonzero and records a global error.');
  if (process.platform === 'linux') {
    active = spawnManaged([fixture, 'timeout', 'quotes" and trailing\\'], {
      cwd: resolve('.'), env: { ...process.env, PATH: directory }, stdout: 'pipe',
    });
    await assert.rejects(waitManaged(active, 1000), /requires Python 3\.9\+/);
    active = null;
    console.log('PASS: missing Python fails with an actionable prerequisite error.');
  }
  for (const mode of ['timeout', 'orphan', 'exit-zero', 'exit-error', 'stop']) {
    active = spawnManaged([fixture, mode, 'quotes" and trailing\\'], { cwd: resolve('.'), stdout: 'pipe' });
    const pids = [];
    await new Promise((done, reject) => {
      const timer = setTimeout(() => reject(Error(`Fixture startup timed out: ${mode}`)), 15_000);
      let buffer = '';
      active.closed.then(({ code }) => { clearTimeout(timer); if (!pids.length) reject(Error(`Fixture failed to start (${code})`)); }, reject);
      active.stdout.on('data', data => {
        buffer += data;
        while (buffer.includes('\n')) {
          const end = buffer.indexOf('\n'), line = buffer.slice(0, end).trim(); buffer = buffer.slice(end + 1);
          if (!line) continue;
          try {
            const message = JSON.parse(line);
            if (message.pid) pids.push(message.pid);
            if (message.ready) { clearTimeout(timer); done(); }
          } catch (error) { clearTimeout(timer); reject(error); }
        }
      });
    });
    if (mode === 'stop') {
      const first = active.stop(); assert.equal(active.stop(), first, 'cleanup is shared and idempotent'); await first;
    } else if (mode === 'exit-zero') await waitManaged(active, 5000);
    else if (mode === 'exit-error') await assert.rejects(waitManaged(active, 5000), /Exited with 7/);
    else await assert.rejects(waitManaged(active, 100), /Timed out/);
    assert(pids.length >= (mode === 'orphan' ? 3 : 2), 'fixture really created the nested processes');
    assert(pids.every(pid => !alive(pid)), `${mode}: all owned descendants must be gone before the next test`);
    assert(alive(sentinel.pid), 'cleanup must preserve unrelated Node processes');
    active = null;
    console.log(`PASS: ${mode} waits for owned process tree cleanup, unrelated process preserved.`);
  }
} finally {
  await active?.stop();
  if (sentinel.exitCode === null) sentinel.kill();
  await sentinelClosed;
  if (!resolve(directory).startsWith(output + sep)) throw Error('Unexpected fixture cleanup path');
  await rm(directory, { recursive: true, force: true });
}
