// End-to-end update test: installs a build of the current code, announces a newer build from a local server and
// waits until the installed app has updated itself and reopened (NSIS on Windows, AppImage on Linux). Test builds
// use their own product name, identifier and data folder, so a real installation is never touched.
// Usage: node scripts/release/update-e2e.mjs   (after npm ci; runs two release builds)
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { chmodSync, copyFileSync, createReadStream, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { buildManifest, updaterPubkey, verifyManifest } from './manifest.mjs';

const windows = process.platform === 'win32';
if (!windows && process.platform !== 'linux') throw Error('The update test runs on Windows and Linux.');
const PRODUCT = 'LogInsightE2E', IDENTIFIER = 'com.loginsight.e2e', NEXT = '99.0.0', PORT = 47831;
const bundle = windows ? 'nsis' : 'appimage', key = windows ? 'windows-x86_64-nsis' : 'linux-x86_64-appimage', kind = windows ? 'Nsis' : 'Appimage';
const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
const work = mkdtempSync(join(tmpdir(), 'loginsight-update-e2e-'));
const served = join(work, 'serve'), data = join(work, 'data'), report = join(work, 'report.txt');
const bundleDir = resolve(process.env.CARGO_TARGET_DIR || 'src-tauri/target', 'release', 'bundle', bundle);
const baseUrl = `http://127.0.0.1:${PORT}/`;
const sleep = ms => new Promise(done => setTimeout(done, ms));
const say = message => console.log(`[update-e2e] ${message}`);
mkdirSync(served); mkdirSync(data);

function build(appVersion) {
  const config = join(work, `config-${appVersion}.json`);
  writeFileSync(config, JSON.stringify({
    productName: PRODUCT, version: appVersion, identifier: IDENTIFIER,
    plugins: { updater: { endpoints: [`${baseUrl}latest.json`], dangerousInsecureTransportProtocol: true } },
  }));
  say(`building ${PRODUCT} ${appVersion}`);
  const result = spawnSync(process.execPath, ['scripts/tauri-build.mjs', '--ci', '--bundles', bundle, '--features', 'update-e2e', '--config', config, '--', '--locked'], { stdio: 'inherit' });
  if (result.status !== 0) throw Error(`The build of ${appVersion} failed.`);
  const name = readdirSync(bundleDir).find(file => file.startsWith(`${PRODUCT}_${appVersion}_`) && file.endsWith(windows ? '-setup.exe' : '.AppImage'));
  if (!name || !existsSync(join(bundleDir, `${name}.sig`))) throw Error(`No signed installer for ${appVersion} in ${bundleDir}.`);
  return { name, path: join(bundleDir, name) };
}
function registry(value) {
  try {
    const output = execFileSync('reg', ['query', `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${PRODUCT}`, '/v', value], { encoding: 'utf8' });
    return output.match(/REG_\w+\s+(.*)$/m)?.[1].trim().replace(/^"|"$/g, '') || null;
  } catch { return null; }
}
const reportLines = () => (existsSync(report) ? readFileSync(report, 'utf8').split(/\r?\n/).filter(Boolean) : []);

const requests = [];
let server = null, display = null, installDir = null;
try {
  const current = build(version), next = build(NEXT);
  copyFileSync(next.path, join(served, next.name));
  const manifest = buildManifest({
    version: NEXT, notes: '# Teste\n\n- Atualização de teste.', pubDate: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    baseUrl, files: [{ name: next.name, signature: readFileSync(`${next.path}.sig`, 'utf8') }], keys: [key],
  });
  // The same checks as a real release, against signatures made by `tauri build`.
  verifyManifest(manifest, served, { version: NEXT, baseUrl, pubkey: updaterPubkey(), keys: [key] });
  writeFileSync(join(served, 'latest.json'), JSON.stringify(manifest));

  server = createServer((request, response) => {
    const name = decodeURIComponent(new URL(request.url, baseUrl).pathname.slice(1));
    requests.push(name);
    const path = join(served, name);
    if (!name || /[\\/]/.test(name) || !existsSync(path)) { response.writeHead(404).end(); return; }
    response.writeHead(200, { 'Content-Length': statSync(path).size, 'Content-Type': name.endsWith('.json') ? 'application/json' : 'application/octet-stream' });
    createReadStream(path).pipe(response);
  });
  await new Promise((done, fail) => server.once('error', fail).listen(PORT, '127.0.0.1', done));

  const env = { ...process.env, LOGINSIGHT_E2E_REPORT: report, LOGINSIGHT_DATA_DIR: data };
  let executable;
  if (windows) {
    say(`installing ${current.name}`);
    if (spawnSync(current.path, ['/S'], { stdio: 'inherit' }).status !== 0) throw Error('The silent installation failed.');
    installDir = registry('InstallLocation');
    executable = installDir && join(installDir, registry('MainBinaryName') || 'loginsight.exe');
    if (!executable || !existsSync(executable)) throw Error('The installed executable was not found.');
  } else {
    executable = join(work, 'app', `${PRODUCT}.AppImage`);
    mkdirSync(join(work, 'app'));
    copyFileSync(current.path, executable);
    chmodSync(executable, 0o755);
    env.APPIMAGE_EXTRACT_AND_RUN = '1';
    // A display of our own: the restarted app must outlive the first process.
    if (!env.DISPLAY) {
      display = spawn('Xvfb', [':97', '-screen', '0', '1280x800x24'], { stdio: 'ignore' });
      env.DISPLAY = ':97';
      await sleep(2000);
    }
  }
  say(`starting ${executable}`);
  spawn(executable, [], { env, detached: true, stdio: 'ignore' }).unref();

  // The first build ends by installing; only a build without a newer version reports `current`.
  const finished = lines => lines.some(line => /^(error|unavailable)/.test(line)) || /^current/.test(lines.at(-1) ?? '');
  const deadline = Date.now() + 8 * 60_000;
  while (!finished(reportLines()) && Date.now() < deadline) await sleep(1000);
  const lines = reportLines();
  say(`report:\n  ${lines.join('\n  ')}`);
  const expect = (condition, message) => { if (!condition) throw Error(message); };
  expect(lines[0]?.startsWith(`started ${version} kind=${kind} notice=none`), 'The installed test build did not start as expected.');
  expect(lines.includes(`available ${NEXT}`), 'The update was not announced.');
  expect(lines.includes('installing'), 'The download did not reach the installation.');
  expect(lines.some(line => line.startsWith(`started ${NEXT} kind=${kind} notice=updated`)), 'The updated app did not reopen with the new version.');
  expect(/^current/.test(lines.at(-1)), 'The updated app still announced an update.');
  expect(requests.includes('latest.json') && requests.includes(next.name), `Unexpected requests: ${requests.join(', ')}`);
  if (windows) expect(registry('DisplayVersion') === NEXT, `The installation reports version ${registry('DisplayVersion')}.`);
  say(`PASS: ${version} updated itself to ${NEXT} (${bundle}), reopened and reported the installation.`);
} catch (error) {
  say(`requests: ${requests.join(', ') || 'none'}`);
  throw error;
} finally {
  server?.close();
  display?.kill();
  if (windows && installDir && existsSync(join(installDir, 'uninstall.exe'))) spawnSync(join(installDir, 'uninstall.exe'), ['/S'], { stdio: 'ignore' });
  for (const file of existsSync(bundleDir) ? readdirSync(bundleDir) : []) if (file.startsWith(`${PRODUCT}_`)) rmSync(join(bundleDir, file), { force: true });
  const webviewData = windows ? [join(process.env.LOCALAPPDATA || '', IDENTIFIER)] : [join(homedir(), '.local', 'share', IDENTIFIER), join(homedir(), '.cache', IDENTIFIER)];
  await sleep(1500);
  for (const path of [...webviewData, work]) if (path.includes(IDENTIFIER) || path === work) rmSync(path, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
}
