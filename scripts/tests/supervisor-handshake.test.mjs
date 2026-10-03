import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { waitForSupervisorRun } from '../preview/supervisor-handshake.mjs';

const frames = ['run\n', 'run\r\n', '\uFEFFrun\n', '\uFEFFrun\r\n'].map(value => Buffer.from(value, 'utf8'));

test('supervisor release accepts exactly run with LF/CRLF and one optional UTF-8 BOM, across every byte boundary', async () => {
  for (const frame of frames) {
    for (let split = 1; split < frame.length; split++) {
      const input = new PassThrough();
      let released = false;
      const result = waitForSupervisorRun(input).then(() => { released = true; });
      input.write(frame.subarray(0, split));
      await delay(0);
      assert.equal(released, false, `partial frame ${frame.subarray(0, split).toString('hex')} must keep the gate closed`);
      input.end(frame.subarray(split));
      await result;
      assert.equal(input.listenerCount('data'), 0);
      assert.equal(input.listenerCount('close'), 0);
    }
  }
  const input = new PassThrough();
  const result = waitForSupervisorRun(input);
  for (const byte of frames.at(-1)) input.write(Buffer.from([byte]));
  input.end();
  await result;
});

test('EOF never releases an empty or incomplete supervisor command', async () => {
  for (const frame of frames) {
    for (let length = 0; length < frame.length; length++) {
      const input = new PassThrough();
      const result = waitForSupervisorRun(input);
      input.end(frame.subarray(0, length));
      await assert.rejects(result, /Supervisor closed before assigning its job/);
    }
  }
  const closed = new PassThrough();
  closed.destroy();
  await assert.rejects(waitForSupervisorRun(closed), /Supervisor closed/);
  const interrupted = new PassThrough();
  const result = waitForSupervisorRun(interrupted);
  interrupted.destroy();
  await assert.rejects(result, /Supervisor closed/);
});

test('invalid commands, extra whitespace, duplicate BOM and other encodings fail closed', async () => {
  for (const frame of [
    '\n', 'stop\n', 'RUN\n', ' run\n', 'run \n', 'run\0\n', '\uFEFF\uFEFFrun\n', 'run\nextra',
    Buffer.from('\uFEFFrun\n', 'utf16le'), Buffer.alloc(1024, 0x72),
  ]) {
    const input = new PassThrough();
    const result = waitForSupervisorRun(input);
    input.end(frame);
    await assert.rejects(result, /Unexpected supervisor command/);
    assert.equal(input.listenerCount('data'), 0);
  }
  const input = new PassThrough();
  const result = waitForSupervisorRun(input);
  input.destroy(Error('pipe failed'));
  await assert.rejects(result, /pipe failed/);
});

test('managed child imports its target only after a complete supervisor release and preserves arguments', { timeout: 20_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'supervisor-handshake-'));
  const fixture = join(directory, 'target fixture.mjs');
  const managedChild = fileURLToPath(new URL('../preview/managed-child.mjs', import.meta.url));
  const argument = 'quotes" and trailing\\';
  await writeFile(fixture, 'console.log(JSON.stringify(process.argv.slice(1)));\n');
  let child;
  try {
    for (const frame of frames) {
      const result = spawnSync(process.execPath, [managedChild, fixture, argument], {
        input: frame, encoding: 'utf8', timeout: 5000, windowsHide: true,
      });
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout), [fixture, argument]);
    }
    for (const frame of ['', 'run', '\uFEFF', 'stop\n', '\uFEFF\uFEFFrun\n']) {
      const result = spawnSync(process.execPath, [managedChild, fixture, argument], {
        input: frame, encoding: 'utf8', timeout: 5000, windowsHide: true,
      });
      assert.ifError(result.error);
      assert.equal(result.status, 1, result.stderr);
      assert.equal(result.stdout, '', 'invalid/partial commands must never import the target');
      assert.match(result.stderr, /Supervisor closed|Unexpected supervisor command/);
    }
    child = spawn(process.execPath, [managedChild, fixture, argument], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    let output = '', errors = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    const closed = once(child, 'close');
    child.stdin.write(Buffer.from([0xef]));
    await delay(50);
    assert.equal(output, '', 'a fragmented preamble must not import the target');
    child.stdin.write(Buffer.from([0xbb, 0xbf, 0x72, 0x75, 0x6e]));
    await delay(50);
    assert.equal(output, '', 'run without its delimiter must not import the target');
    child.stdin.end('\r\n');
    assert.deepEqual(await closed, [0, null], errors);
    assert.deepEqual(JSON.parse(output), [fixture, argument]);
  } finally {
    if (child && child.exitCode === null) {
      const closed = once(child, 'close');
      child.kill();
      await closed;
    }
    await rm(directory, { recursive: true, force: true });
  }
});
