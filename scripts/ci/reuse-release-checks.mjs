// Bounded reuse for the reviewed 0.12.1 regression run, never a generic bypass.
// A green scoped PR is not evidence of a full regression. This exact run/head/
// tree was reviewed with all Node, browser and Windows/Linux native checks.
import { appendFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export const approvedChecks = Object.freeze({
  repository: 'felipevasc/logs',
  runId: '37134532695',
  head: '24a1b2d53638c40949fc4ea66e28c95e72263e46',
  tree: 'b7beef8bd61d20789d1c5dbc4f1dd7191ae87616',
});
// Exact paths only: the seven reviewed supervisor/CI changes plus this opt-in.
// Never allow product code, dependencies, native sources or build inputs here.
export const allowedReusePaths = Object.freeze([
  '.github/workflows/checks.yml',
  'scripts/ci/validation-plan.mjs',
  'scripts/preview/managed-child.mjs',
  'scripts/preview/supervisor-handshake.mjs',
  'scripts/preview/windows-job.ps1',
  'scripts/tests/supervisor-handshake.test.mjs',
  'scripts/tests/validation-plan.test.mjs',
  '.github/workflows/build.yml',
  'scripts/ci/reuse-release-checks.mjs',
  'scripts/tests/reuse-release-checks.test.mjs',
]);
export const requiredJobs = Object.freeze({
  'Select required validation': ['Select essential and affected checks'],
  'Frontend and interaction regressions': ['Run npm run check', 'Essential and affected browser checks', 'Record browser evidence identity'],
  'Backend tests (ubuntu-22.04)': ['Backend tests'],
  'Backend tests (windows-2022)': ['Windows backend tests with loader diagnostics', 'Verify Windows unit harness manifest'],
  'Required validation': ['Require every selected gate to succeed'],
});
const completed = item => item?.status === 'completed' && item.conclusion === 'success';
const assert = (condition, reason) => { if (!condition) throw Error(`Cannot reuse checks: ${reason}`); };

export function verifyRunEvidence({ run, workflow, jobs }, repository, runId) {
  assert(repository === approvedChecks.repository && runId === approvedChecks.runId, 'unreviewed repository or run ID');
  assert(run?.id === Number(runId) && completed(run), 'source run is not completed successfully');
  assert(run.repository?.full_name === repository && run.head_repository?.full_name === repository, 'source repository mismatch');
  assert(run.path === '.github/workflows/checks.yml' && workflow?.path === run.path && workflow.id === run.workflow_id, 'source is not this repository\'s checks.yml');
  assert(run.head_sha === approvedChecks.head && run.event === 'pull_request', 'source head/event does not match the reviewed full regression');
  assert(Number.isSafeInteger(run.run_attempt) && run.run_attempt > 0, 'missing run attempt');
  assert(Array.isArray(jobs) && jobs.length > 0, 'missing jobs');
  for (const [name, steps] of Object.entries(requiredJobs)) {
    const matches = jobs.filter(job => job.name === name);
    assert(matches.length === 1, `missing or duplicate required job: ${name}`);
    const job = matches[0];
    assert(job.run_id === run.id && job.run_attempt === run.run_attempt && completed(job), `required job did not succeed in this attempt: ${name}`);
    assert(Array.isArray(job.steps), `missing step evidence: ${name}`);
    for (const step of steps) {
      const matches = job.steps.filter(item => item.name === step);
      assert(matches.length === 1 && completed(matches[0]), `required step did not run successfully: ${step}`);
    }
  }
  return run.head_sha;
}

export function verifyChangedPaths(output) {
  if (output === '') return [];
  const parts = output.split('\0');
  assert(parts.pop() === '' && parts.length % 2 === 0, 'invalid Git changed-path response');
  const files = [];
  for (let i = 0; i < parts.length; i += 2) {
    const [status, path] = parts.slice(i, i + 2);
    assert((status === 'A' || status === 'M') && allowedReusePaths.includes(path), `unapproved changed path or status: ${status} ${path}`);
    assert(!files.includes(path), 'duplicate changed path');
    files.push(path);
  }
  return files;
}

export function verifyGitEvidence(head, current, git = args => execFileSync('git', args, { encoding: 'utf8' })) {
  assert(head === approvedChecks.head && /^[a-f0-9]{40}$/.test(current || ''), 'invalid commit identity');
  assert(git(['rev-parse', 'HEAD']).trim() === current, 'checkout does not match the release commit');
  assert(git(['rev-parse', `${head}^{tree}`]).trim() === approvedChecks.tree, 'source tree identity mismatch');
  // Missing history or a non-ancestor exits nonzero; neither may become reuse.
  git(['merge-base', '--is-ancestor', head, current]);
  const files = verifyChangedPaths(git(['diff', '--name-status', '--no-renames', '-z', head, current]));
  for (const path of files) {
    const entry = git(['ls-tree', current, '--', path]).trim();
    assert(/^100644 blob [a-f0-9]{40}\t/.test(entry) && entry.split('\t')[1] === path, `changed path is not a regular file: ${path}`);
  }
  return files;
}

export async function selectReleaseChecks(env = process.env, { fetchImpl = fetch, git } = {}) {
  const runId = env.REUSE_CHECKS_RUN_ID ?? '';
  if (runId === '') return { reused: false };
  assert(env.GITHUB_EVENT_NAME === 'workflow_dispatch' && env.GITHUB_REF === 'refs/heads/main', 'reuse is only available for manual builds of main');
  assert(runId === approvedChecks.runId && env.GITHUB_REPOSITORY === approvedChecks.repository, 'only the reviewed full regression run is eligible');
  assert(!!env.GITHUB_TOKEN, 'missing ephemeral read token');
  const base = `https://api.github.com/repos/${approvedChecks.repository}/actions`;
  const request = async path => {
    const response = await fetchImpl(`${base}/${path}`, {
      headers: { Authorization: `Bearer ${env.GITHUB_TOKEN}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28' },
      redirect: 'error', signal: AbortSignal.timeout(30_000),
    });
    assert(response.ok, `GitHub API returned ${response.status}`);
    return response.json();
  };
  const run = await request(`runs/${runId}`);
  assert(Number.isSafeInteger(run.run_attempt) && run.run_attempt > 0, 'missing run attempt');
  const workflow = await request('workflows/checks.yml');
  const page = await request(`runs/${runId}/attempts/${run.run_attempt}/jobs?per_page=100`);
  // This bounded workflow has five jobs. Reject truncated or malformed evidence.
  assert(Array.isArray(page.jobs) && page.jobs.length === page.total_count && page.total_count <= 100, 'incomplete job evidence');
  const head = verifyRunEvidence({ run, workflow, jobs: page.jobs }, env.GITHUB_REPOSITORY, runId);
  const files = verifyGitEvidence(head, env.GITHUB_SHA, git);
  return { reused: true, runId, head, tree: approvedChecks.tree, releaseHead: env.GITHUB_SHA, files };
}

async function main() {
  const result = await selectReleaseChecks();
  // Write the gate output only after every proof passed. Never fall back to full
  // tests on an invalid requested reuse: fail the job and block its dependents.
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `reused=${result.reused}\n`);
  console.log(JSON.stringify(result, null, 2));
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, result.reused
    ? `## Reused previously passed regressions\n\n[Checks run ${result.runId}](https://github.com/${approvedChecks.repository}/actions/runs/${result.runId}), head ${result.head}, tree ${result.tree}.\n\nRelease commit: ${result.releaseHead}. Only the reviewed infrastructure paths differ:\n${result.files.map(path => `- ${path}`).join('\n')}\n\nNode/full browser/Rust regressions are reused, not rerun. Packaging, signatures/version, Windows process cleanup and native desktop acceptance, and installed updater E2E on Windows/Linux remain required.\n`
    : '## Release validation\n\nNo reuse requested: full release regression remains required.\n');
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
