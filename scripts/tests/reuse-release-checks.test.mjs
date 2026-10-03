import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { approvedChecks, allowedReusePaths, requiredJobs, verifyRunEvidence, verifyChangedPaths, verifyGitEvidence, selectReleaseChecks } from '../ci/reuse-release-checks.mjs';

const current = 'c'.repeat(40);
const good = () => ({
  run: { id: Number(approvedChecks.runId), repository: { full_name: approvedChecks.repository }, head_repository: { full_name: approvedChecks.repository }, path: '.github/workflows/checks.yml', workflow_id: 123, head_sha: approvedChecks.head, status: 'completed', conclusion: 'success', event: 'pull_request', run_attempt: 1 },
  workflow: { id: 123, path: '.github/workflows/checks.yml' },
  jobs: Object.entries(requiredJobs).map(([name, steps]) => ({ name, run_id: Number(approvedChecks.runId), run_attempt: 1, status: 'completed', conclusion: 'success', steps: steps.map(name => ({ name, status: 'completed', conclusion: 'success' })) })),
});
const env = () => ({ REUSE_CHECKS_RUN_ID: approvedChecks.runId, GITHUB_REPOSITORY: approvedChecks.repository, GITHUB_SHA: current, GITHUB_EVENT_NAME: 'workflow_dispatch', GITHUB_REF: 'refs/heads/main', GITHUB_TOKEN: 'test-only-not-a-credential' });
const git = args => {
  if (args[0] === 'rev-parse') return args[1] === 'HEAD' ? `${current}\n` : `${approvedChecks.tree}\n`;
  if (args[0] === 'merge-base') return '';
  if (args[0] === 'diff') return 'M\0.github/workflows/build.yml\0A\0scripts/ci/reuse-release-checks.mjs\0';
  if (args[0] === 'ls-tree') return `100644 blob ${'a'.repeat(40)}\t${args.at(-1)}\n`;
  throw Error(`Unexpected Git command: ${args}`);
};
const api = (evidence = good()) => async url => {
  const payload = url.includes('/jobs?') ? { jobs: evidence.jobs, total_count: evidence.jobs.length } : url.endsWith('/workflows/checks.yml') ? evidence.workflow : evidence.run;
  return new Response(JSON.stringify(payload));
};

test('no input preserves full validation without GitHub access or Git work', async () => {
  assert.deepEqual(await selectReleaseChecks({}, { fetchImpl: () => assert.fail('No API for default full validation'), git: () => assert.fail('No Git for default full validation') }), { reused: false });
});
test('only the reviewed run, workflow, source repository, head and successful attempt are reusable', () => {
  assert.equal(verifyRunEvidence(good(), approvedChecks.repository, approvedChecks.runId), approvedChecks.head);
  const mutations = [
    e => { e.run.id++; }, e => { e.run.status = 'in_progress'; }, e => { e.run.conclusion = 'failure'; },
    e => { e.run.repository.full_name = 'other/logs'; }, e => { e.run.head_repository.full_name = 'fork/logs'; },
    e => { e.run.path = '.github/workflows/build.yml'; }, e => { e.workflow.id++; }, e => { e.workflow.path = 'checks.yml'; },
    e => { e.run.head_sha = current; }, e => { e.run.event = 'push'; }, e => { e.run.run_attempt = 0; },
    e => { e.jobs.pop(); }, e => { e.jobs.push(e.jobs[0]); }, e => { e.jobs[0].run_id++; }, e => { e.jobs[0].run_attempt++; },
  ];
  for (const mutate of mutations) { const evidence = good(); mutate(evidence); assert.throws(() => verifyRunEvidence(evidence, approvedChecks.repository, approvedChecks.runId)); }
  assert.throws(() => verifyRunEvidence(good(), 'other/logs', approvedChecks.runId));
  assert.throws(() => verifyRunEvidence(good(), approvedChecks.repository, '123'));
});
test('every required job and executed gate must succeed; skipped and missing do not count', () => {
  for (let i = 0; i < good().jobs.length; i++) {
    for (const conclusion of ['skipped', 'failure', 'cancelled', 'neutral', 'timed_out', null]) {
      const evidence = good(); evidence.jobs[i].conclusion = conclusion;
      assert.throws(() => verifyRunEvidence(evidence, approvedChecks.repository, approvedChecks.runId));
      const stepEvidence = good(); stepEvidence.jobs[i].steps[0].conclusion = conclusion;
      assert.throws(() => verifyRunEvidence(stepEvidence, approvedChecks.repository, approvedChecks.runId));
    }
    const evidence = good(); evidence.jobs[i].steps = [];
    assert.throws(() => verifyRunEvidence(evidence, approvedChecks.repository, approvedChecks.runId));
  }
});
test('only exact reviewed regular infrastructure paths may change', () => {
  for (const path of allowedReusePaths) assert.deepEqual(verifyChangedPaths(`M\0${path}\0`), [path]);
  assert.deepEqual(verifyChangedPaths(''), []);
  for (const path of ['frontend/app.js', 'frontend/assets/a.png', 'package.json', 'package-lock.json', 'scripts/preview/package-lock.json', 'src-tauri/src/main.rs', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json', 'scripts/tauri-build.mjs', 'scripts/prepare-frontend.mjs', 'scripts/release/update-e2e.mjs', '.github/workflows/new.yml', 'scripts/ci/reuse-release-checks.mjs/other', '../scripts/ci/reuse-release-checks.mjs']) {
    assert.throws(() => verifyChangedPaths(`M\0${path}\0`), undefined, path);
  }
  for (const status of ['D', 'T', 'R100', 'C100', 'U', '']) assert.throws(() => verifyChangedPaths(`${status}\0.github/workflows/build.yml\0`));
  assert.throws(() => verifyChangedPaths('M\0.github/workflows/build.yml'));
  assert.throws(() => verifyChangedPaths('M\0.github/workflows/build.yml\0M\0.github/workflows/build.yml\0'));
});
test('source tree, release checkout, ancestry and regular file modes must be proven', () => {
  assert.equal(verifyGitEvidence(approvedChecks.head, current, git).length, 2);
  assert.throws(() => verifyGitEvidence(current, current, git));
  assert.throws(() => verifyGitEvidence(approvedChecks.head, 'main', git));
  for (const operation of ['HEAD', `${approvedChecks.head}^{tree}`, 'merge-base', 'diff', 'ls-tree']) {
    assert.throws(() => verifyGitEvidence(approvedChecks.head, current, args => {
      if (args[0] === operation || args[1] === operation) throw Error('Missing/invalid Git evidence');
      return git(args);
    }));
  }
  assert.throws(() => verifyGitEvidence(approvedChecks.head, current, args => args[0] === 'rev-parse' ? current : git(args)));
  assert.throws(() => verifyGitEvidence(approvedChecks.head, current, args => args[0] === 'ls-tree' ? `120000 blob ${current}\t${args.at(-1)}\n` : git(args)));
});
test('manual main reuse requests use the exact attempt API and emit proof only after all checks', async () => {
  const calls = [];
  const result = await selectReleaseChecks(env(), { git, fetchImpl: async (url, options) => {
    calls.push(url); assert.equal(new URL(url).origin, 'https://api.github.com'); assert.equal(options.redirect, 'error'); assert.equal(options.headers.Authorization, 'Bearer test-only-not-a-credential');
    return api()(url);
  } });
  assert.equal(result.reused, true); assert.equal(result.head, approvedChecks.head); assert.equal(result.releaseHead, current);
  assert(calls.some(url => url.endsWith(`/runs/${approvedChecks.runId}/attempts/1/jobs?per_page=100`)));
});
test('invalid input, API error and incomplete evidence fail rather than falling back to full tests', async () => {
  for (const change of [{ REUSE_CHECKS_RUN_ID: ' ' }, { REUSE_CHECKS_RUN_ID: '123' }, { GITHUB_EVENT_NAME: 'push' }, { GITHUB_REF: 'refs/heads/feature' }, { GITHUB_REPOSITORY: 'fork/logs' }, { GITHUB_TOKEN: '' }]) {
    await assert.rejects(selectReleaseChecks({ ...env(), ...change }, { git, fetchImpl: () => assert.fail('Invalid input must not reach the API') }));
  }
  for (const fetchImpl of [async () => new Response('{}', { status: 403 }), async () => { throw Error('Network failure'); }, async () => new Response('not JSON'), async url => url.includes('/jobs?') ? new Response(JSON.stringify({ jobs: good().jobs, total_count: 101 })) : api()(url)]) {
    await assert.rejects(selectReleaseChecks(env(), { git, fetchImpl }));
  }
  await assert.rejects(selectReleaseChecks(env(), { fetchImpl: api(), git: () => { throw Error('Non-ancestor'); } }));
});
test('workflow proof gates both jobs while package, installed updater and native acceptance remain mandatory', () => {
  const build = readFileSync(new URL('../../.github/workflows/build.yml', import.meta.url), 'utf8');
  const job = name => build.split(`\n  ${name}:\n`)[1].split(/\n  [a-z][a-z_-]*:\n/)[0];
  const step = name => build.split(`      - name: ${name}\n`)[1]?.split(/\n      - |\n  [a-z][a-z_-]*:\n/)[0];
  assert.match(build, /reuse_checks_run_id:[\s\S]*?type: string[\s\S]*?default: ''/);
  for (const name of ['validate', 'build']) assert.match(job(name), /needs: checks-proof/);
  assert.match(job('checks-proof'), /actions: read/);
  assert.match(job('checks-proof'), /fetch-depth: 0/);
  assert.equal((build.match(/actions: read/g) || []).length, 1);
  assert.match(job('validate'), /if: needs.checks-proof.outputs.reused != 'true'\s+run: npm run check/);
  for (const name of ['Full browser regression', 'Backend tests', 'Windows backend tests with loader diagnostics', 'Verify Windows unit harness manifest']) assert.match(step(name), /needs.checks-proof.outputs.reused != 'true'/);
  for (const name of ['Verify version', 'Verify release safeguards', 'Build installers', 'Update end to end']) {
    assert(step(name), name); assert.doesNotMatch(step(name), /if:|reused/);
  }
  for (const name of ['Verify Windows process cleanup', 'Verify Windows app manifest', 'Ensure WebView2 runtime', 'Windows desktop acceptance']) {
    assert.match(step(name), /if: runner.os == 'Windows'/); assert.doesNotMatch(step(name), /reused/);
  }
  assert.match(job('release'), /needs: \[build, validate\]/);
  assert.match(job('release'), /node scripts\/release\/prepare-assets.mjs release-downloads release-assets/);
  assert.doesNotMatch(build, /continue-on-error/);
});
