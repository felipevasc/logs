// All artifact claims are corroborated with GitHub API metadata before use.
import { createHash } from 'node:crypto';
import { lstatSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
export const REPOSITORY = 'felipevasc/logs';
export const WORKFLOW = '.github/workflows/build.yml';
export const PLATFORMS = {
  'windows-x64': { job: 'Windows x64', extensions: ['.exe', '.msi'], native: ['Windows backend tests with loader diagnostics', 'Verify Windows unit harness manifest', 'Verify Windows app manifest'] },
  'linux-x64': { job: 'Linux x64', extensions: ['.deb', '.AppImage', '.rpm'], native: ['Linux dependencies', 'Backend tests'] },
};
export const COMMON_STEPS = ['Install frontend dependencies', 'Verify version', 'Verify release safeguards', 'Verify promotion safeguards', 'Test event detail tree and formatters', 'Prepare frontend', 'Build installers', 'Stage release installers', 'Update end to end', 'Record validated provenance', 'Upload installers'];
export const sha256 = data => createHash('sha256').update(data).digest('hex');
export const requireThat = (condition, message) => { if (!condition) throw Error(message); };
export const positiveId = value => /^[1-9]\d*$/.test(String(value)) && Number.isSafeInteger(Number(value));
export const isSha = value => typeof value === 'string' && /^[a-f0-9]{40}$/.test(value);
const timestamp = value => { const result = typeof value === 'string' ? Date.parse(value) : NaN; requireThat(Number.isFinite(result), 'Missing or invalid timestamp.'); return result; };
const successful = item => item?.status === 'completed' && item.conclusion === 'success';
export function requiredStep(job, name) {
  const matches = job.steps?.filter(step => step.name === name) || [];
  requireThat(matches.length === 1 && successful(matches[0]), `${job.name}: required successful step missing, duplicated or skipped: ${name}.`);
  return matches[0];
}
export function validateMetadata({ repository, run, workflow, jobs, artifacts, runId, attempt, now = Date.now() }) {
  requireThat(repository.full_name === REPOSITORY && repository.fork === false && repository.default_branch === 'main' && positiveId(repository.id), 'Untrusted repository.');
  requireThat(positiveId(runId) && positiveId(attempt) && run.id === Number(runId) && run.run_attempt === Number(attempt) && successful(run), 'Source must be the requested completed successful run and latest attempt.');
  for (const value of [run.repository, run.head_repository]) requireThat(value?.id === repository.id && value.full_name === REPOSITORY && value.fork === false, 'Fork or repository mismatch.');
  requireThat(run.path === WORKFLOW && run.workflow_id === workflow.id && workflow.path === WORKFLOW && workflow.state === 'active' && positiveId(workflow.id), 'Wrong source workflow.');
  requireThat(['push', 'workflow_dispatch'].includes(run.event) && Array.isArray(run.pull_requests) && run.pull_requests.length === 0 && isSha(run.head_sha), 'PR, unknown event or invalid source SHA.');
  requireThat(run.head_branch === 'main' || (run.event === 'push' && /^v\d+\.\d+\.\d+$/.test(run.head_branch)), 'Only main or version-tag pushes can supply artifacts.');
  requireThat(Array.isArray(jobs) && new Set(jobs.map(job => job.id)).size === jobs.length, 'Duplicate or missing job identities.');
  requireThat(Array.isArray(artifacts) && new Set(artifacts.map(artifact => artifact.id)).size === artifacts.length, 'Duplicate or missing artifact identities.');
  const selected = {};
  for (const [platform, policy] of Object.entries(PLATFORMS)) {
    const matches = jobs.filter(job => job.name === policy.job);
    requireThat(matches.length === 1, `Expected exactly one ${policy.job} job in the requested attempt.`);
    const job = matches[0];
    requireThat(positiveId(job.id) && job.run_id === run.id && job.head_sha === run.head_sha && successful(job), `${policy.job}: failed or mismatched job.`);
    // The attempt-scoped API is authoritative even on API versions without job.run_attempt.
    requireThat(job.run_attempt === undefined || job.run_attempt === run.run_attempt, 'Job attempt mismatch.');
    for (const name of [...COMMON_STEPS, ...policy.native]) requiredStep(job, name);
    const stage = requiredStep(job, 'Stage release installers'), e2e = requiredStep(job, 'Update end to end');
    const provenance = requiredStep(job, 'Record validated provenance'), upload = requiredStep(job, 'Upload installers');
    requireThat(stage.number < e2e.number && e2e.number < provenance.number && provenance.number < upload.number, 'Installers must be staged, E2E-validated, recorded, then uploaded in order.');
    requireThat(timestamp(stage.completed_at) <= timestamp(e2e.started_at) && timestamp(e2e.completed_at) <= timestamp(provenance.started_at) && timestamp(provenance.completed_at) <= timestamp(upload.started_at), 'Gate timestamps are out of order.');
    const found = artifacts.filter(artifact => artifact.name === `LogInsight-${platform}`);
    requireThat(found.length === 1, `Missing or duplicate ${platform} artifact.`);
    const artifact = found[0], binding = artifact.workflow_run;
    requireThat(positiveId(artifact.id) && artifact.expired === false && timestamp(artifact.expires_at) > now && Number.isSafeInteger(artifact.size_in_bytes) && artifact.size_in_bytes > 0 && /^sha256:[a-f0-9]{64}$/.test(artifact.digest), 'Expired, empty or unhashed artifact.');
    requireThat(binding?.id === run.id && binding.repository_id === repository.id && binding.head_repository_id === repository.id && binding.head_sha === run.head_sha && binding.head_branch === run.head_branch, 'Artifact is not bound to this repository and run.');
    requireThat(timestamp(artifact.created_at) >= timestamp(upload.started_at) && timestamp(artifact.created_at) <= timestamp(upload.completed_at), 'Artifact was not created during the validated upload step in this attempt.');
    selected[platform] = { job, artifact };
  }
  requireThat(artifacts.filter(artifact => artifact.name.startsWith('LogInsight-')).length === 2, 'Unexpected release artifact identity.');
  return selected;
}
export function inventory(directory, platform, ignored = []) {
  const policy = PLATFORMS[platform]; requireThat(policy, 'Unsupported platform.');
  const names = readdirSync(directory).filter(name => !ignored.includes(name)).sort();
  requireThat(new Set(names.map(name => name.toLowerCase())).size === names.length, 'Duplicate case-insensitive file identity.');
  requireThat(names.length === policy.extensions.length * 2, 'Unexpected or missing installer files.');
  for (const extension of policy.extensions) {
    const matches = names.filter(name => name.endsWith(extension));
    requireThat(matches.length === 1 && names.includes(`${matches[0]}.sig`), `Expected exactly one signed ${extension} installer.`);
  }
  return names.map(name => {
    requireThat(/^[A-Za-z0-9][A-Za-z0-9._+-]*$/.test(name), 'Unsafe installer filename.');
    const path = join(directory, name), stat = lstatSync(path);
    requireThat(stat.isFile() && !stat.isSymbolicLink() && stat.size > 0, 'Only nonempty regular files are accepted.');
    return { name, size: stat.size, sha256: sha256(readFileSync(path)) };
  });
}
export function validateProvenance(record, { repository, run, platform, source, version, files }) {
  requireThat(record?.schema === 1 && record.platform === platform && record.repository === REPOSITORY && record.repositoryId === repository.id && record.runId === run.id && record.runAttempt === run.run_attempt && record.job === 'build' && record.version === version && record.updateE2E === 'success', 'Provenance identity or validation mismatch.');
  const ref = run.head_branch === 'main' ? 'refs/heads/main' : `refs/tags/${run.head_branch}`;
  const expected = { sha: run.head_sha, tree: source.tree, ref, workflowPath: WORKFLOW, workflowRef: `${REPOSITORY}/${WORKFLOW}@${ref}`, workflowSha: run.head_sha, workflowSha256: source.workflowSha256 };
  requireThat(JSON.stringify(record.source) === JSON.stringify(expected), 'Source provenance does not match independently verified GitHub/Git evidence.');
  requireThat(Array.isArray(record.files) && JSON.stringify(record.files) === JSON.stringify(files), 'Installer bytes, hashes or file identities do not match provenance.');
  return record;
}
