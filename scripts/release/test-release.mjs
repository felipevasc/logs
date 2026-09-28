import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync, unlinkSync } from 'node:fs';
import { resolve, join, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';

const outputRoot = resolve('output'); mkdirSync(outputRoot, { recursive: true });
const temp = mkdtempSync(join(outputRoot, 'release-check-'));
const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
const input = join(temp, 'input'), ready = join(temp, 'ready'); mkdirSync(input);
const fixtures = { '-setup.exe': '4d5a', '.msi': 'd0cf11e0a1b11ae1', '.deb': '213c617263683e0a', '.rpm': 'edabeedb', '.AppImage': '7f454c46' };
const originalFetch = global.fetch, originalArgv = process.argv, originalEnv = { ...process.env }, originalLog = console.log;

// A throwaway key signs the fixtures in the minisign format written by `tauri build`.
const { publicKey, privateKey } = generateKeyPairSync('ed25519');
const keyId = Buffer.from('0102030405060708', 'hex');
const b64 = value => Buffer.from(value).toString('base64');
const pubkey = b64(`untrusted comment: minisign public key: 0807060504030201\n${b64(Buffer.concat([Buffer.from('Ed'), keyId, publicKey.export({ format: 'der', type: 'spki' }).subarray(-32)]))}\n`);
function signature(data, name, signedVersion = version) {
  const signed = sign(null, createHash('blake2b512').update(data).digest(), privateKey);
  const trusted = `timestamp:1790000000\tfile:${name}\tversion:${signedVersion}`;
  const global = sign(null, Buffer.concat([signed, Buffer.from(trusted)]), privateKey);
  return b64(`untrusted comment: signature from tauri secret key\n${b64(Buffer.concat([Buffer.from('ED'), keyId, signed]))}\ntrusted comment: ${trusted}\n${b64(global)}\n`);
}
const env = { ...process.env, LOGINSIGHT_UPDATER_PUBKEY: pubkey, GITHUB_REPOSITORY: 'fixture/repo' };
const prepare = (from, to) => execFileSync(process.execPath, ['scripts/release/prepare-assets.mjs', from, to], { stdio: 'pipe', env });
function writeFixtures(directory, change = {}) {
  mkdirSync(directory, { recursive: true });
  for (const [suffix, header] of Object.entries(fixtures)) {
    const name = `LogInsight_${version}${suffix}`, bytes = Buffer.alloc(100_100);
    Buffer.from(header, 'hex').copy(bytes);
    writeFileSync(join(directory, name), bytes);
    if (change.skipSignature !== suffix) writeFileSync(join(directory, `${name}.sig`), signature(bytes, name, change.signedVersion?.[suffix]));
  }
}
try {
  writeFixtures(input);
  prepare(input, ready);
  assert.equal(readdirSync(ready).length, 12);
  const manifest = JSON.parse(readFileSync(join(ready, 'latest.json'), 'utf8'));
  assert.equal(manifest.version, version);
  assert.deepEqual(Object.keys(manifest.platforms).sort(), ['linux-x86_64-appimage', 'linux-x86_64-deb', 'linux-x86_64-rpm', 'windows-x86_64-msi', 'windows-x86_64-nsis']);
  assert.equal(manifest.platforms['windows-x86_64-nsis'].url, `https://github.com/fixture/repo/releases/download/v${version}/LogInsight_${version}-setup.exe`);
  assert.equal(manifest.platforms['windows-x86_64-nsis'].signature, readFileSync(join(ready, `LogInsight_${version}-setup.exe.sig`), 'utf8').trim());
  assert.equal(readFileSync(join(ready, 'SHA256SUMS.txt'), 'utf8').trim().split('\n').length, 11);

  const rejects = (label, change, tamper) => {
    const directory = join(temp, label);
    writeFixtures(directory, change);
    tamper?.(directory);
    assert.throws(() => prepare(directory, join(temp, `${label}-out`)), undefined, `${label} must be rejected`);
  };
  rejects('invalid-binary', {}, directory => writeFileSync(join(directory, `LogInsight_${version}-setup.exe`), Buffer.alloc(100_100)));
  rejects('tampered-after-signing', {}, directory => { const path = join(directory, `LogInsight_${version}.deb`), bytes = readFileSync(path); bytes[5000] ^= 1; writeFileSync(path, bytes); });
  rejects('signed-for-other-version', { signedVersion: { '.AppImage': '0.0.1' } });
  rejects('missing-signature', { skipSignature: '.rpm' });
  rejects('signature-of-other-file', {}, directory => writeFileSync(join(directory, `LogInsight_${version}.msi.sig`), readFileSync(join(directory, `LogInsight_${version}.deb.sig`))));
  rejects('unknown-key', {}, directory => { const name = `LogInsight_${version}.rpm`; unlinkSync(join(directory, `${name}.sig`)); writeFileSync(join(directory, `${name}.sig`), b64('untrusted comment: x\n' + b64(Buffer.alloc(74)) + '\ntrusted comment: t\n' + b64(Buffer.alloc(64)) + '\n')); });

  Object.assign(process.env, { GITHUB_TOKEN: 'test-only-not-a-credential', GITHUB_REPOSITORY: 'fixture/repo', GITHUB_SHA: 'a'.repeat(40), GITHUB_RUN_ID: '1', RELEASE_CHECK_ATTEMPTS: '1' });
  process.argv = [process.execPath, 'publish.mjs', ready];
  const moduleUrl = pathToFileURL(resolve('scripts/release/publish.mjs')).href;
  console.log = () => {};
  for (const scenario of ['wrong-tag', 'already-published', 'not-newer', 'digest-mismatch', 'manifest-not-served', 'success']) {
    const calls = [], uploaded = [];
    global.fetch = async (value, options = {}) => {
      const url = new URL(value), method = options.method || 'GET'; calls.push([method, url.hostname, url.pathname]);
      const respond = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
      if (url.hostname === 'github.com') return scenario === 'manifest-not-served' ? respond({}, 404) : respond({ version });
      if (url.pathname.endsWith('/releases/latest')) return scenario === 'not-newer' ? respond({ tag_name: 'v999.0.0' }) : respond({ tag_name: 'v0.0.1' });
      if (url.pathname.includes('/git/ref/')) return scenario === 'wrong-tag' ? respond({ object: { type: 'commit', sha: 'b'.repeat(40) } }) : respond({}, 404);
      if (url.pathname.includes('/releases/tags/')) return scenario === 'already-published' ? respond({ draft: false }) : respond({}, 404);
      if (method === 'POST' && url.hostname === 'api.github.com') return respond({ id: 1, draft: true, assets: [], upload_url: 'https://uploads.github.com/repos/fixture/repo/releases/1/assets{?name,label}' }, 201);
      if (url.hostname === 'uploads.github.com') {
        const hash = createHash('sha256'); let size = 0;
        for await (const chunk of options.body) { size += chunk.length; hash.update(chunk); }
        uploaded.push({ name: url.searchParams.get('name'), state: 'uploaded', size, digest: `sha256:${hash.digest('hex')}` }); return respond({}, 201);
      }
      if (method === 'GET' && url.pathname.endsWith('/assets')) return respond(scenario === 'digest-mismatch' ? uploaded.map(asset => ({ ...asset, digest: 'sha256:wrong' })) : uploaded);
      if (method === 'PATCH') { assert.equal(uploaded.length, 12); assert.equal(JSON.parse(options.body).draft, false); return respond({ html_url: 'https://github.com/fixture/repo/releases/tag/test' }); }
      throw Error(`Unexpected mocked request ${method} ${url.pathname}`);
    };
    const published = () => calls.some(([method]) => method === 'PATCH');
    if (scenario === 'success') { await import(`${moduleUrl}?case=${scenario}`); assert(published()); assert(calls.some(([, host]) => host === 'github.com'), 'the served manifest is checked'); }
    else if (scenario === 'manifest-not-served') { await assert.rejects(import(`${moduleUrl}?case=${scenario}`)); assert(published(), 'the check runs after publication'); }
    else { await assert.rejects(import(`${moduleUrl}?case=${scenario}`)); assert(!published(), `${scenario} must not publish`); }
  }
  originalLog('PASS: signed assets and update manifest, rejection of invalid, tampered, mismatched or unsigned packages, tag conflict, published release protection, older version protection, digest verification, publication only after all twelve uploads and check of the served manifest. No network requests were made.');
} finally {
  global.fetch = originalFetch; process.argv = originalArgv; console.log = originalLog;
  for (const key of ['GITHUB_TOKEN', 'GITHUB_REPOSITORY', 'GITHUB_SHA', 'GITHUB_RUN_ID', 'RELEASE_CHECK_ATTEMPTS']) { if (originalEnv[key] === undefined) delete process.env[key]; else process.env[key] = originalEnv[key]; }
  if (!resolve(temp).startsWith(outputRoot + sep)) throw Error('Unexpected cleanup path.');
  rmSync(temp, { recursive: true, force: true });
}
