import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, writeFileSync, rmSync, copyFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { REPOSITORY, WORKFLOW, PLATFORMS, COMMON_STEPS, inventory, sha256, validateMetadata, validateProvenance } from '../release/promotion-guards.mjs';
import { stageInstallers, recordProvenance } from '../release/provenance.mjs';
import { githubClient, promote, verifyGitSource, validateSource } from '../release/promote.mjs';
const sourceSha = 'a'.repeat(40), tree = 'b'.repeat(40), workflowSha256 = 'c'.repeat(64);
const now = Date.now();
const time = seconds => new Date(now - 3600_000 + seconds * 1000).toISOString();
const temporary = () => mkdtempSync(join(tmpdir(), 'promotion-test-'));
const source = { tree, workflowSha256 };
function fixture() {
  const repository = { id: 123, full_name: REPOSITORY, default_branch: 'main', fork: false };
  const run = { id: 456, run_attempt: 1, repository, head_repository: { ...repository }, workflow_id: 789, path: WORKFLOW, head_sha: sourceSha, head_branch: 'main', event: 'push', pull_requests: [], status: 'completed', conclusion: 'success' };
  const workflow = { id: 789, path: WORKFLOW, state: 'active' };
  const jobs = Object.values(PLATFORMS).map((platform, index) => ({
    id: 100 + index, run_id: run.id, head_sha: sourceSha, run_attempt: 1, name: platform.job, status: 'completed', conclusion: 'success',
    steps: [...platform.native, ...COMMON_STEPS].map((name, number) => ({ name, number: number + 1, status: 'completed', conclusion: 'success', started_at: time(number * 2), completed_at: time(number * 2 + 1) })),
  }));
  const artifacts = Object.keys(PLATFORMS).map((platform, index) => ({
    id: 200 + index, name: `LogInsight-${platform}`, size_in_bytes: 400, digest: `sha256:${'d'.repeat(64)}`, expired: false, created_at: jobs[index].steps.at(-1).started_at, expires_at: new Date(now + 86400_000).toISOString(),
    workflow_run: { id: run.id, repository_id: repository.id, head_repository_id: repository.id, head_sha: sourceSha, head_branch: 'main' },
  }));
  return { repository, run, workflow, jobs, artifacts, runId: '456', attempt: '1', now };
}
function provenance(f, platform, files) {
  return { schema: 1, platform, repository: REPOSITORY, repositoryId: f.repository.id, runId: f.run.id, runAttempt: f.run.run_attempt, job: 'build', version: '0.10.0', updateE2E: 'success',
    source: { sha: sourceSha, tree, ref: 'refs/heads/main', workflowPath: WORKFLOW, workflowRef: `${REPOSITORY}/${WORKFLOW}@refs/heads/main`, workflowSha: sourceSha, workflowSha256 }, files };
}
test('successful original workflow and complete platform gates select exact artifacts', () => {
  assert.deepEqual(Object.keys(validateMetadata(fixture())), Object.keys(PLATFORMS));
});
const badMetadata = {
  'wrong repository': f => { f.repository.full_name = 'attacker/logs'; },
  'fork source': f => { f.run.head_repository.fork = true; },
  'foreign head repository': f => { f.run.head_repository.id = 9; },
  'arbitrary workflow': f => { f.run.path = '.github/workflows/spoof.yml'; },
  'workflow ID spoof': f => { f.run.workflow_id = 10; },
  'disabled workflow': f => { f.workflow.state = 'disabled_manually'; },
  'pull request event': f => { f.run.event = 'pull_request'; },
  'PR association': f => { f.run.pull_requests = [{}]; },
  'unapproved branch': f => { f.run.head_branch = 'feature'; },
  'tag-like manual branch': f => { f.run.head_branch = 'v0.10.0'; f.run.event = 'workflow_dispatch'; },
  'failed run': f => { f.run.conclusion = 'failure'; },
  'unfinished run': f => { f.run.status = 'in_progress'; },
  'stale attempt': f => { f.run.run_attempt = 2; },
  'wrong run ID': f => { f.run.id = 457; },
  'source SHA injection': f => { f.run.head_sha = '--help'; },
  'failed Linux job': f => { f.jobs[1].conclusion = 'failure'; },
  'missing platform': f => { f.jobs.pop(); },
  'duplicate platform': f => { f.jobs.push({ ...f.jobs[0], id: 999 }); },
  'job from another run': f => { f.jobs[0].run_id = 999; },
  'job from another attempt': f => { f.jobs[0].run_attempt = 2; },
  'job from another commit': f => { f.jobs[0].head_sha = 'e'.repeat(40); },
  'missing required updater': f => { f.jobs[0].steps = f.jobs[0].steps.filter(step => step.name !== 'Update end to end'); },
  'skipped updater': f => { f.jobs[0].steps.find(step => step.name === 'Update end to end').conclusion = 'skipped'; },
  'failed Windows manifest': f => { f.jobs[0].steps.find(step => step.name === 'Verify Windows app manifest').conclusion = 'failure'; },
  'missing native harness': f => { f.jobs[0].steps = f.jobs[0].steps.filter(step => step.name !== 'Verify Windows unit harness manifest'); },
  'duplicate required step': f => { f.jobs[0].steps.push(f.jobs[0].steps[0]); },
  'upload before E2E': f => { f.jobs[0].steps.at(-1).number = 1; },
  'provenance before actual E2E success': f => { f.jobs[0].steps.find(step => step.name === 'Record validated provenance').started_at = time(0); },
  'missing artifact': f => { f.artifacts.pop(); },
  'duplicate artifact name': f => { f.artifacts.push({ ...f.artifacts[0], id: 900 }); },
  'duplicate artifact ID': f => { f.artifacts[1].id = f.artifacts[0].id; },
  'unexpected release artifact': f => { f.artifacts.push({ id: 999, name: 'LogInsight-injected' }); },
  'expired artifact': f => { f.artifacts[0].expired = true; },
  'expired timestamp': f => { f.artifacts[0].expires_at = time(0); },
  'missing archive digest': f => { delete f.artifacts[0].digest; },
  'missing run binding': f => { delete f.artifacts[0].workflow_run; },
  'wrong artifact run binding': f => { f.artifacts[0].workflow_run.id = 88; },
  'wrong artifact repo binding': f => { f.artifacts[0].workflow_run.head_repository_id = 88; },
  'wrong artifact commit binding': f => { f.artifacts[0].workflow_run.head_sha = 'e'.repeat(40); },
  'prior-attempt artifact': f => { f.artifacts[0].created_at = time(-100); },
  'artifact outside upload': f => { f.artifacts[0].created_at = time(200); },
};
for (const [name, tamper] of Object.entries(badMetadata)) test(`refuse ${name}`, () => { const f = fixture(); tamper(f); assert.throws(() => validateMetadata(f)); });
test('every mandatory build/native/manifest step must independently succeed', () => {
  for (let platform = 0; platform < 2; platform++) for (let index = 0; index < fixture().jobs[platform].steps.length; index++) {
    const f = fixture(); f.jobs[platform].steps[index].conclusion = 'skipped'; assert.throws(() => validateMetadata(f));
  }
});
test('provenance binds every identity and every installer byte, not just its self-declared success', () => {
  const f = fixture(), files = [{ name: 'a.exe', size: 2, sha256: 'a'.repeat(64) }];
  const record = provenance(f, 'windows-x64', files), context = { ...f, platform: 'windows-x64', files, source, version: '0.10.0' };
  validateProvenance(record, context);
  for (const field of ['schema', 'platform', 'repository', 'repositoryId', 'runId', 'runAttempt', 'job', 'version', 'updateE2E']) assert.throws(() => validateProvenance({ ...record, [field]: 'spoof' }, context), field);
  for (const field of Object.keys(record.source)) assert.throws(() => validateProvenance({ ...record, source: { ...record.source, [field]: 'spoof' } }, context), field);
  for (const field of ['name', 'size', 'sha256']) assert.throws(() => validateProvenance({ ...record, files: [{ ...files[0], [field]: 'spoof' }] }, context), field);
  assert.throws(() => validateProvenance({ ...record, files: [...files, ...files] }, context));
});
test('staging saves production bytes before updater builds clear output, rejects missing/extra formats', () => {
  const work = temporary();
  try {
    const bundle = join(work, 'bundle'), output = join(work, 'staged');
    for (const [folder, name] of [['nsis', 'LogInsight_0.10.0-setup.exe'], ['msi', 'LogInsight_0.10.0.msi']]) {
      mkdirSync(join(bundle, folder), { recursive: true });
      writeFileSync(join(bundle, folder, name), `original ${name}`); writeFileSync(join(bundle, folder, `${name}.sig`), 'signature');
    }
    stageInstallers('windows-x64', output, bundle); rmSync(bundle, { recursive: true });
    assert.equal(inventory(output, 'windows-x64', ['.staged.json']).length, 4);
    assert.equal(readFileSync(join(output, 'LogInsight_0.10.0-setup.exe'), 'utf8'), 'original LogInsight_0.10.0-setup.exe');
    assert.throws(() => stageInstallers('windows-x64', output, bundle));
    writeFileSync(join(output, 'unexpected.exe'), 'injected'); assert.throws(() => inventory(output, 'windows-x64', ['.staged.json']));
    assert.throws(() => recordProvenance('windows-x64', output, { UPDATE_E2E_OUTCOME: 'success' }));
  } finally { rmSync(work, { recursive: true, force: true }); }
});
test('Git proof accepts identical full tree only when source is contained in current main', () => {
  const work = temporary(), previous = process.cwd();
  const run = args => execFileSync('git', args, { cwd: work, encoding: 'utf8', stdio: 'pipe' }).trim();
  const commit = message => { run(['add', '.']); run(['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '--allow-empty', '-m', message]); return run(['rev-parse', 'HEAD']); };
  try {
    run(['init']); mkdirSync(join(work, '.github/workflows'), { recursive: true }); writeFileSync(join(work, WORKFLOW), 'trusted workflow');
    const first = commit('source'), second = commit('same complete tree'), treeSha = run(['rev-parse', 'HEAD^{tree}']);
    process.chdir(work);
    const args = { mainSha: second, sourceSha: first, checkoutSha: second, sourceCommit: { sha: first, tree: { sha: treeSha } } };
    assert.equal(verifyGitSource(args).tree, treeSha);
    assert.throws(() => verifyGitSource({ ...args, sourceCommit: { sha: first, tree: { sha: 'f'.repeat(40) } } }));
    assert.throws(() => verifyGitSource({ ...args, checkoutSha: first }));
    writeFileSync(join(work, 'extra-build-input'), 'changed'); const changed = commit('different tree');
    assert.throws(() => verifyGitSource({ ...args, mainSha: changed, checkoutSha: changed }));
    run(['checkout', '--orphan', 'unrelated']); run(['rm', '-rf', '.']); mkdirSync(join(work, '.github/workflows'), { recursive: true }); writeFileSync(join(work, WORKFLOW), 'trusted workflow');
    const unrelated = commit('unrelated same tree');
    assert.throws(() => verifyGitSource({ ...args, mainSha: unrelated, checkoutSha: unrelated }));
  } finally { process.chdir(previous); rmSync(work, { recursive: true, force: true }); }
});
test('download verifies GitHub ZIP hash/size and never forwards token to redirected storage', async () => {
  const work = temporary(), bytes = Buffer.from('fixture zip'), artifact = { id: 200, size_in_bytes: bytes.length, digest: `sha256:${sha256(bytes)}` };
  let requests = 0;
  const fakeFetch = async (url, options) => {
    requests++;
    if (String(url).startsWith('https://api.github.com/')) { assert.equal(options.headers.Authorization, 'Bearer fixture-token'); return new Response(null, { status: 302, headers: { location: 'https://storage.example.test/fixture' } }); }
    assert.equal(options.headers, undefined); assert.equal(options.redirect, 'error'); return new Response(bytes);
  };
  try {
    const api = githubClient('fixture-token', fakeFetch);
    await api.download(artifact, join(work, 'valid.zip')); assert.equal(requests, 2);
    await assert.rejects(api.download({ ...artifact, digest: `sha256:${'f'.repeat(64)}` }, join(work, 'bad.zip')));
    await assert.rejects(api.download({ ...artifact, size_in_bytes: 1 }, join(work, 'size.zip')));
  } finally { rmSync(work, { recursive: true, force: true }); }
});
test('metadata pagination rejects truncation, count changes, and HTTP failure', async () => {
  for (const body of [{ total_count: 101, jobs: [] }, { total_count: 1001, jobs: [] }, { total_count: 1, jobs: [{}, {}] }]) {
    const api = githubClient('fixture', async () => Response.json(body)); await assert.rejects(api.list('/jobs', 'jobs'));
  }
  await assert.rejects(githubClient('fixture', async () => new Response(null, { status: 403 })).get('/actions/runs/1'));
});
test('untrusted promotion origins stop before network or publication', async () => {
  for (const env of [{}, { GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: REPOSITORY, GITHUB_REF: 'refs/heads/evil', GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_TOKEN: 'fixture' }]) await assert.rejects(promote(env));
});
test('archive extraction blocks zip slip, nested paths, duplicate names, symlinks, directories and oversized expansion', () => {
  const work = temporary(), python = process.platform === 'win32' ? 'python' : 'python3';
  try {
    const unpack = resolve('scripts/release/unpack-artifact.py');
    const generator = `import sys, zipfile, stat\nfrom pathlib import Path\nroot=Path(sys.argv[1])\nfor kind in ['good','traversal','nested','duplicate','symlink','directory','oversize']:\n with zipfile.ZipFile(root / (kind+'.zip'),'w') as z:\n  for i in range(4): z.writestr('file'+str(i),b'x')\n  name={'traversal':'../escape','nested':'nested/file','duplicate':'file0','directory':'folder/'}.get(kind,'last')\n  info=zipfile.ZipInfo(name)\n  if kind=='symlink': info.create_system=3; info.external_attr=(stat.S_IFLNK|0o777)<<16\n  z.writestr(info,b'target')\n if kind=='oversize':\n  p=root / (kind+'.zip'); data=bytearray(p.read_bytes()); start=data.index(b'PK\\x01\\x02'); data[start+24:start+28]=(2**31).to_bytes(4,'little'); p.write_bytes(data)\n`;
    execFileSync(python, ['-c', generator, work], { stdio: 'pipe' });
    execFileSync(python, [unpack, join(work, 'good.zip'), join(work, 'good')]); assert.equal(readdirSync(join(work, 'good')).length, 5);
    for (const kind of ['traversal', 'nested', 'duplicate', 'symlink', 'directory', 'oversize']) assert.throws(() => execFileSync(python, [unpack, join(work, `${kind}.zip`), join(work, kind)], { stdio: 'pipe' }), kind);
  } finally { rmSync(work, { recursive: true, force: true }); }
});
test('workflow uploads only after installed E2E and promotion never rebuilds or bypasses the guarded publisher', () => {
  const build = readFileSync('.github/workflows/build.yml', 'utf8'), workflow = readFileSync('.github/workflows/promote.yml', 'utf8'), driver = readFileSync('scripts/release/promote.mjs', 'utf8');
  const names = ['Stage release installers', 'Update end to end', 'Record validated provenance', 'Upload installers'];
  for (let i = 1; i < names.length; i++) assert(build.indexOf(`name: ${names[i - 1]}`) < build.indexOf(`name: ${names[i]}`));
  assert.match(build, /UPDATE_E2E_OUTCOME: \$\{\{ steps.updater_e2e.outcome \}\}/);
  assert.match(workflow, /actions: read\n      contents: write/); assert.doesNotMatch(workflow, /actions: write|id-token:|secrets\.(?!GITHUB_TOKEN)/);
  const release = build.split('\n  release:')[1];
  for (const publisher of [release, workflow]) assert.match(publisher, /concurrency:\n      group: release-publication\n      cancel-in-progress: false/);
  assert.doesNotMatch(build, /^concurrency:/m, 'workflow cancellation must not interrupt a running publisher');
  assert.match(build, /group: build-\$\{\{ github.ref \}\}-\$\{\{ matrix.artifact \}\}\n      cancel-in-progress: true/);
  assert.match(workflow, /persist-credentials: false/); assert.match(workflow, /if: github.ref == 'refs\/heads\/main'/);
  assert.doesNotMatch(workflow, /npm ci|npm run build|cargo |workflow_run:/);
  assert.match(driver, /scripts\/release\/prepare-assets.mjs/); assert.match(driver, /scripts\/release\/publish.mjs/);
  assert.doesNotMatch(build.split('\n  release:')[0], /contents: write|actions: read/);
});
test('driver verifies API-bound archives and real signed assets end to end, and rejects forged success before publication', async () => {
  const work = temporary(), previous = process.cwd(), originalFetch = global.fetch;
  const checkout = join(work, 'checkout'); mkdirSync(checkout);
  const python = process.platform === 'win32' ? 'python' : 'python3';
  const command = args => execFileSync('git', args, { cwd: checkout, encoding: 'utf8', stdio: 'pipe' }).trim();
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const b64 = value => Buffer.from(value).toString('base64'), keyId = Buffer.from('0102030405060708', 'hex');
  const pubkey = b64(`untrusted comment: fixture\n${b64(Buffer.concat([Buffer.from('Ed'), keyId, publicKey.export({ format: 'der', type: 'spki' }).subarray(-32)]))}\n`);
  const signFile = (bytes, name, version) => {
    const signed = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey), trusted = `timestamp:1790000000\tfile:${name}\tversion:${version}`;
    return b64(`untrusted comment: fixture\n${b64(Buffer.concat([Buffer.from('ED'), keyId, signed]))}\ntrusted comment: ${trusted}\n${b64(sign(null, Buffer.concat([signed, Buffer.from(trusted)]), privateKey))}\n`);
  };
  try {
    for (const directory of ['scripts/release', '.github/workflows', 'src-tauri', 'docs/releases']) mkdirSync(join(checkout, directory), { recursive: true });
    for (const name of ['prepare-assets.mjs', 'manifest.mjs', 'verify-version.mjs', 'unpack-artifact.py']) copyFileSync(join(previous, 'scripts/release', name), join(checkout, 'scripts/release', name));
    copyFileSync(join(previous, WORKFLOW), join(checkout, WORKFLOW));
    writeFileSync(join(checkout, 'scripts/release/publish.mjs'), "throw Error('A validation-only request must never execute publication');\n");
    writeFileSync(join(checkout, 'package.json'), JSON.stringify({ version: '0.10.0' }));
    writeFileSync(join(checkout, 'package-lock.json'), JSON.stringify({ version: '0.10.0', packages: { '': { version: '0.10.0' } } }));
    writeFileSync(join(checkout, 'src-tauri/tauri.conf.json'), JSON.stringify({ version: '0.10.0', plugins: { updater: { pubkey } } }));
    writeFileSync(join(checkout, 'src-tauri/Cargo.toml'), '[package]\nversion = "0.10.0"\n');
    writeFileSync(join(checkout, 'src-tauri/Cargo.lock'), 'name = "loginsight"\nversion = "0.10.0"\n');
    writeFileSync(join(checkout, 'docs/releases/v0.10.0.md'), '# Fixture release\n');
    command(['init']); command(['add', '.']); command(['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-m', 'trusted fixture']);
    const sha = command(['rev-parse', 'HEAD']), gitTree = command(['rev-parse', 'HEAD^{tree}']);
    process.chdir(checkout);
    const f = fixture(); f.run.head_sha = sha; for (const job of f.jobs) job.head_sha = sha;
    for (const artifact of f.artifacts) artifact.workflow_run.head_sha = sha;
    const env = { ...process.env, GITHUB_ACTIONS: 'true', GITHUB_REPOSITORY: REPOSITORY, GITHUB_REPOSITORY_ID: String(f.repository.id), GITHUB_REF: 'refs/heads/main', GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_TOKEN: 'fixture-only', GITHUB_SHA: sha, GITHUB_WORKFLOW_SHA: sha, GITHUB_WORKFLOW_REF: `${REPOSITORY}/.github/workflows/promote.yml@refs/heads/main`, SOURCE_RUN_ID: String(f.run.id), SOURCE_RUN_ATTEMPT: '1', GITHUB_RUN_ID: '999', GITHUB_RUN_ATTEMPT: '1', PUBLISH_RELEASE: 'false' };
    delete env.GITHUB_STEP_SUMMARY;
    const archives = new Map();
    function makeArtifacts(signedVersion = '0.10.0', ref = 'refs/heads/main') {
      for (const [index, platform] of Object.keys(PLATFORMS).entries()) {
        const directory = join(work, `fixture-${platform}`); rmSync(directory, { recursive: true, force: true }); mkdirSync(directory);
        for (const extension of PLATFORMS[platform].extensions) {
          const name = `LogInsight_0.10.0${extension === '.exe' ? '-setup.exe' : extension}`, bytes = Buffer.alloc(100_100);
          Buffer.from({ '.exe': '4d5a', '.msi': 'd0cf11e0a1b11ae1', '.deb': '213c617263683e0a', '.rpm': 'edabeedb', '.AppImage': '7f454c46' }[extension], 'hex').copy(bytes);
          writeFileSync(join(directory, name), bytes); writeFileSync(join(directory, `${name}.sig`), signFile(bytes, name, signedVersion));
        }
        writeFileSync(join(directory, '.staged.json'), JSON.stringify(inventory(directory, platform)));
        recordProvenance(platform, directory, { ...env, GITHUB_JOB: 'build', GITHUB_REF: ref, GITHUB_RUN_ID: String(f.run.id), GITHUB_RUN_ATTEMPT: '1', GITHUB_WORKFLOW_REF: `${REPOSITORY}/${WORKFLOW}@${ref}`, UPDATE_E2E_OUTCOME: 'success' });
        const archive = join(work, `${platform}.zip`); rmSync(archive, { force: true });
        execFileSync(python, ['-m', 'zipfile', '-c', archive, ...readdirSync(directory).map(name => join(directory, name))], { stdio: 'pipe' });
        const bytes = readFileSync(archive); archives.set(String(f.artifacts[index].id), bytes);
        f.artifacts[index].size_in_bytes = bytes.length; f.artifacts[index].digest = `sha256:${sha256(bytes)}`;
      }
    }
    let scenario = 'success';
    global.fetch = async (value, options = {}) => {
      const url = new URL(value); assert.equal(options.method || 'GET', 'GET', 'validation may only read');
      if (url.hostname === 'storage.example.test') return new Response(archives.get(url.pathname.slice(1)));
      assert.equal(url.hostname, 'api.github.com'); const path = url.pathname.replace(`/repos/${REPOSITORY}`, '');
      if (!path) return Response.json(f.repository);
      if (path === '/actions/runs/456') return Response.json(f.run);
      if (path === '/actions/runs/456/attempts/1') return Response.json(scenario === 'rerun' ? { ...f.run, run_attempt: 2 } : f.run);
      if (path === '/actions/workflows/build.yml') return Response.json(f.workflow);
      if (path === '/actions/runs/456/attempts/1/jobs') return Response.json({ total_count: f.jobs.length, jobs: f.jobs });
      if (path === '/actions/runs/456/artifacts') return Response.json({ total_count: f.artifacts.length, artifacts: f.artifacts });
      if (path === '/git/ref/heads/main') return Response.json({ object: { type: 'commit', sha: scenario === 'main-moved' ? 'f'.repeat(40) : sha } });
      if (path === `/git/commits/${sha}`) return Response.json({ sha, tree: { sha: gitTree } });
      if (path === '/git/ref/tags/v0.10.0') return Response.json({ object: { type: 'tag', sha: 'e'.repeat(40) } });
      if (path === `/git/tags/${'e'.repeat(40)}`) return Response.json({ object: { type: 'commit', sha: scenario === 'tag-moved' ? 'f'.repeat(40) : sha } });
      const match = path.match(/^\/actions\/artifacts\/(\d+)\/zip$/);
      if (match) return new Response(null, { status: 302, headers: { location: `https://storage.example.test/${match[1]}` } });
      throw Error(`Unexpected fixture API path ${path}`);
    };
    makeArtifacts('0.10.0', 'refs/heads/candidate');
    assert.equal(JSON.parse(readFileSync(join(work, 'fixture-windows-x64/provenance-windows-x64.json'))).source.ref, 'refs/heads/candidate');
    makeArtifacts();
    const result = await promote(env); assert.equal(result.publication, 'not-requested'); assert.equal(result.sourceSha, sha);
    assert.equal(readdirSync('promotion-work/assets').length, 12);
    assert.equal(JSON.parse(readFileSync('promotion-record.json')).artifacts.length, 2);
    rmSync('promotion-work', { recursive: true }); rmSync('promotion-record.json');
    await assert.rejects(promote({ ...env, PUBLISH_RELEASE: 'true' }));
    assert.equal(JSON.parse(readFileSync('promotion-record.json')).publication, 'unconfirmed', 'failed publisher may already have made the release public');
    for (scenario of ['rerun', 'main-moved']) await assert.rejects(validateSource(githubClient('fixture'), '456', '1', env));
    scenario = 'success'; f.run.head_branch = 'v0.10.0'; f.artifacts.forEach(artifact => { artifact.workflow_run.head_branch = 'v0.10.0'; });
    await validateSource(githubClient('fixture'), '456', '1', env);
    scenario = 'tag-moved'; await assert.rejects(validateSource(githubClient('fixture'), '456', '1', env));
    scenario = 'success'; f.run.head_branch = 'main'; f.artifacts.forEach(artifact => { artifact.workflow_run.head_branch = 'main'; });
    rmSync('promotion-work', { recursive: true }); rmSync('promotion-record.json');
    // A self-consistent hash/provenance/archive with a wrong signed version still cannot pass prepare-assets.
    makeArtifacts('0.0.1'); await assert.rejects(promote(env)); assert.throws(() => readFileSync('promotion-record.json'));
  } finally { global.fetch = originalFetch; process.chdir(previous); rmSync(work, { recursive: true, force: true }); }
});
