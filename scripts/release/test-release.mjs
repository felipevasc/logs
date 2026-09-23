import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { resolve, join, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const outputRoot = resolve('output'); mkdirSync(outputRoot, { recursive: true });
const temp = mkdtempSync(join(outputRoot, 'release-check-'));
const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
const input = join(temp, 'input'), ready = join(temp, 'ready'); mkdirSync(input);
const fixtures = { '.exe': '4d5a', '.msi': 'd0cf11e0a1b11ae1', '.deb': '213c617263683e0a', '.rpm': 'edabeedb', '.AppImage': '7f454c46' };
const originalFetch = global.fetch, originalArgv = process.argv, originalEnv = { ...process.env }, originalLog = console.log;
try {
  for (const [ext, header] of Object.entries(fixtures)) { const bytes = Buffer.alloc(100_100); Buffer.from(header, 'hex').copy(bytes); writeFileSync(join(input, `LogInsight_${version}${ext}`), bytes); }
  execFileSync(process.execPath, ['scripts/release/prepare-assets.mjs', input, ready], { stdio: 'pipe' });
  assert.equal(readdirSync(ready).length, 6);
  assert.equal(readFileSync(join(ready, 'SHA256SUMS.txt'), 'utf8').trim().split('\n').length, 5);
  writeFileSync(join(input, `LogInsight_${version}.exe`), Buffer.alloc(100_100));
  assert.throws(() => execFileSync(process.execPath, ['scripts/release/prepare-assets.mjs', input, join(temp, 'bad')], { stdio: 'pipe' }));

  Object.assign(process.env, { GITHUB_TOKEN: 'test-only-not-a-credential', GITHUB_REPOSITORY: 'fixture/repo', GITHUB_SHA: 'a'.repeat(40), GITHUB_RUN_ID: '1' });
  process.argv = [process.execPath, 'publish.mjs', ready];
  const moduleUrl = pathToFileURL(resolve('scripts/release/publish.mjs')).href;
  console.log = () => {};
  for (const scenario of ['wrong-tag', 'already-published', 'digest-mismatch', 'success']) {
    const calls = [], uploaded = [];
    global.fetch = async (value, options = {}) => {
      const url = new URL(value), method = options.method || 'GET'; calls.push([method, url.pathname]);
      const respond = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
      if (url.pathname.includes('/git/ref/')) return scenario === 'wrong-tag' ? respond({ object: { type: 'commit', sha: 'b'.repeat(40) } }) : respond({}, 404);
      if (url.pathname.includes('/releases/tags/')) return scenario === 'already-published' ? respond({ draft: false }) : respond({}, 404);
      if (method === 'POST' && url.hostname === 'api.github.com') return respond({ id: 1, draft: true, assets: [], upload_url: 'https://uploads.github.com/repos/fixture/repo/releases/1/assets{?name,label}' }, 201);
      if (url.hostname === 'uploads.github.com') {
        const hash = createHash('sha256'); let size = 0;
        for await (const chunk of options.body) { size += chunk.length; hash.update(chunk); }
        uploaded.push({ name: url.searchParams.get('name'), state: 'uploaded', size, digest: `sha256:${hash.digest('hex')}` }); return respond({}, 201);
      }
      if (method === 'GET' && url.pathname.endsWith('/assets')) return respond(scenario === 'digest-mismatch' ? uploaded.map(asset => ({ ...asset, digest: 'sha256:wrong' })) : uploaded);
      if (method === 'PATCH') { assert.equal(uploaded.length, 6); assert.equal(JSON.parse(options.body).draft, false); return respond({ html_url: 'https://github.com/fixture/repo/releases/tag/test' }); }
      throw Error(`Unexpected mocked request ${method} ${url.pathname}`);
    };
    if (scenario === 'success') { await import(`${moduleUrl}?case=${scenario}`); assert(calls.some(([method]) => method === 'PATCH')); }
    else { await assert.rejects(import(`${moduleUrl}?case=${scenario}`)); assert(!calls.some(([method]) => method === 'PATCH'), `${scenario} must not publish`); }
  }
  originalLog('PASS: complete assets, invalid binary rejection, tag conflict, published release protection, digest verification and publication only after all six uploads. No network requests were made.');
} finally {
  global.fetch = originalFetch; process.argv = originalArgv; console.log = originalLog;
  for (const key of ['GITHUB_TOKEN', 'GITHUB_REPOSITORY', 'GITHUB_SHA', 'GITHUB_RUN_ID']) { if (originalEnv[key] === undefined) delete process.env[key]; else process.env[key] = originalEnv[key]; }
  if (!resolve(temp).startsWith(outputRoot + sep)) throw Error('Unexpected cleanup path.');
  rmSync(temp, { recursive: true, force: true });
}
