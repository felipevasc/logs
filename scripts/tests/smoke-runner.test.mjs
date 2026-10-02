import test from 'node:test';
import assert from 'node:assert/strict';
import { writeFile, rm } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';

test('preview runner continues independent checks and still fails overall', async () => {
  const id = randomUUID().replaceAll('-', '');
  const fail = `test-runner-${id}-fail.mjs`, pass = `test-runner-${id}-pass.mjs`;
  const paths = [new URL(`../preview/${fail}`, import.meta.url), new URL(`../preview/${pass}`, import.meta.url)];
  try {
    await writeFile(paths[0], "console.log('expected fixture failure'); process.exitCode=1;", { flag: 'wx' });
    await writeFile(paths[1], "console.log('subsequent fixture completed');", { flag: 'wx' });
    const result = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, ['scripts/preview/run-smoke.mjs', fail, pass], {
        cwd: fileURLToPath(new URL('../../', import.meta.url)), stdio: ['ignore', 'pipe', 'pipe'],
      });
      let output = '';
      child.stdout.on('data', chunk => { output += chunk; });
      child.stderr.on('data', chunk => { output += chunk; });
      child.on('error', reject);
      child.on('exit', code => resolve({ code, output }));
    });
    assert.equal(result.code, 1);
    assert.match(result.output, /subsequent fixture completed/);
    assert.match(result.output, /1\/2 preview regressions failed/);
    assert.match(result.output, /Preview duration: .*\(failed\)/);
    assert.match(result.output, /Preview duration: .*\(passed\)/);
  } finally { await Promise.all(paths.map(path => rm(path, { force: true }))); }
});
