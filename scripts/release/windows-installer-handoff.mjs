// Real installed Windows checks, called only by the isolated signed E2E build.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex');

function completion(child, milliseconds = 90_000) {
  return new Promise(resolve => {
    const timer = setTimeout(() => { child.kill(); resolve({ error: 'owned fixture timed out' }); }, milliseconds);
    child.once('error', error => { clearTimeout(timer); resolve({ error: String(error) }); });
    child.once('exit', (code, signal) => { clearTimeout(timer); resolve({ code, signal }); });
  });
}

export async function holdInstalledExecutable(executable, work, label) {
  if (process.platform !== 'win32') throw Error('This fixture requires Windows.');
  const ready = join(work, `${label}.ready`), release = join(work, `${label}.release`);
  const child = spawn('powershell.exe', ['-NoProfile', '-File', fileURLToPath(new URL('./windows-update-lock.ps1', import.meta.url))], {
    env: { ...process.env, LOGINSIGHT_LOCK_FILE: executable, LOGINSIGHT_LOCK_READY: ready, LOGINSIGHT_LOCK_RELEASE: release },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '', failure;
  child.stdout.on('data', chunk => { output += chunk; });
  child.stderr.on('data', chunk => { output += chunk; });
  child.on('error', error => { failure = error; });
  const exited = new Promise(resolve => { child.once('close', code => resolve(code)); });
  try {
    const deadline = Date.now() + 15_000;
    while (!existsSync(ready) && child.exitCode === null && !failure && Date.now() < deadline) await sleep(100);
    if (!existsSync(ready)) throw Error(`The owned file-lock fixture did not start: ${failure || output}`);
  } catch (error) {
    if (child.exitCode === null) child.kill();
    await exited;
    throw error;
  }
  return {
    assertAlive() { assert.equal(child.exitCode, null, `Installer terminated a file holder: ${output}`); },
    async release() {
      writeFileSync(release, 'release');
      const outcome = await Promise.race([exited, sleep(5000).then(() => 'timeout')]);
      if (outcome === 'timeout') { child.kill(); await exited; throw Error('The owned lock fixture did not exit.'); }
      assert.equal(outcome, 0, `The owned file-lock fixture failed: ${output}`);
    },
  };
}

export async function checkLockedInstaller({ installer, executable, work, data, version, readVersion, report }) {
  const before = digest(executable), sentinel = join(data, 'update-preservation-fixture.txt');
  writeFileSync(sentinel, 'case-data-must-survive-failed-install');
  const lock = await holdInstalledExecutable(executable, work, 'blocked');
  try {
    const started = Date.now();
    const child = spawn(installer, ['/S', '/UPDATE'], { stdio: 'ignore' });
    const result = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { child.kill(); reject(Error('Locked installer exceeded 90 seconds.')); }, 90_000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', (code, signal) => { clearTimeout(timer); resolve({ code, signal }); });
    });
    lock.assertAlive();
    assert.equal(result.signal, null);
    assert.equal(result.code, 2, 'A silent locked installer must fail explicitly.');
    assert.ok(Date.now() - started >= 25_000, 'The installer must wait for graceful shutdown before failing.');
    assert.equal(readVersion(), version, 'A failed install must preserve the installed version.');
    assert.equal(digest(executable), before, 'A failed install must leave the current executable byte-identical.');
    assert.equal(readFileSync(sentinel, 'utf8'), 'case-data-must-survive-failed-install');
    report('PASS: held executable timed out with code 2; current version, executable and data preserved; unrelated holder survived.');
  } finally { await lock.release(); }
  return { sentinel, before };
}

export async function checkInstallerDialog({ installer, executable, work, version, readVersion, action, report }) {
  assert.ok(['cancel', 'retry'].includes(action));
  let lock = await holdInstalledExecutable(executable, work, `dialog-${action}-lock`);
  const before = digest(executable);
  const child = spawn(installer, ['/P', '/UPDATE'], { stdio: 'ignore' });
  const installed = completion(child);
  const ready = join(work, `dialog-${action}.ready`), proceed = join(work, `dialog-${action}.proceed`);
  const dialog = spawn('powershell.exe', ['-NoProfile', '-File', fileURLToPath(new URL('./windows-update-dialog.ps1', import.meta.url))], {
    env: { ...process.env, LOGINSIGHT_DIALOG_PROCESS: String(child.pid), LOGINSIGHT_DIALOG_ACTION: action, LOGINSIGHT_DIALOG_READY: ready, LOGINSIGHT_DIALOG_PROCEED: proceed },
    stdio: 'inherit',
  });
  const selected = completion(dialog);
  try {
    const deadline = Date.now() + 75_000;
    while (!existsSync(ready) && dialog.exitCode === null && child.exitCode === null && Date.now() < deadline) await sleep(200);
    assert.ok(existsSync(ready), 'The installed executable must produce a Retry/Cancel dialog while locked.');
    lock.assertAlive();
    assert.equal(readVersion(), version);
    assert.equal(digest(executable), before, 'The waiting installer must not modify the old executable.');
    if (action === 'retry') { await lock.release(); lock = null; }
    writeFileSync(proceed, action);
    const clicked = await selected;
    assert.equal(clicked.code, 0, `Dialog fixture failed: ${JSON.stringify(clicked)}`);
    const result = await installed;
    assert.equal(result.signal, null);
    assert.equal(result.code, action === 'cancel' ? 2 : 0, `Unexpected ${action} result: ${JSON.stringify(result)}`);
    assert.equal(readVersion(), version);
    if (action === 'cancel') { assert.equal(digest(executable), before); lock.assertAlive(); }
    report(`PASS: installed Windows ${action} dialog; version ${version} preserved; no unrelated process terminated.`);
  } finally {
    try { if (lock) await lock.release(); }
    finally {
      if (dialog.exitCode === null) dialog.kill();
      if (child.exitCode === null) child.kill();
      await Promise.all([selected, installed]);
    }
  }
}
