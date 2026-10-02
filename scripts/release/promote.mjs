// This entry point only executes code from the current trusted main checkout. Downloaded files are data.
import { appendFileSync, createWriteStream, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { execFileSync } from 'node:child_process';
import { REPOSITORY, WORKFLOW, PLATFORMS, inventory, isSha, positiveId, requireThat, sha256, validateMetadata, validateProvenance } from './promotion-guards.mjs';
const git = args => execFileSync('git', args, { encoding: 'utf8' }).trim();
export function verifyGitSource({ mainSha, sourceSha, sourceCommit, checkoutSha }) {
  requireThat(isSha(mainSha) && isSha(sourceSha) && mainSha === checkoutSha && git(['rev-parse', 'HEAD']) === mainSha, 'Promotion checkout must still be current main.');
  requireThat(sourceCommit.sha === sourceSha && isSha(sourceCommit.tree?.sha), 'Invalid source commit metadata.');
  // Full object/tree comparison includes every build input, scripts, lockfiles and workflow, not a path allowlist.
  execFileSync('git', ['merge-base', '--is-ancestor', sourceSha, mainSha], { stdio: 'pipe' });
  const tree = git(['rev-parse', `${sourceSha}^{tree}`]);
  requireThat(tree === sourceCommit.tree.sha && tree === git(['rev-parse', `${mainSha}^{tree}`]), 'Source commit is not the complete tree currently approved on main.');
  const sourceWorkflow = execFileSync('git', ['show', `${sourceSha}:${WORKFLOW}`]);
  requireThat(sourceWorkflow.equals(execFileSync('git', ['show', `${mainSha}:${WORKFLOW}`])), 'Source build workflow differs from current main.');
  return { tree, workflowSha256: sha256(sourceWorkflow) };
}
export function githubClient(token, fetchImpl = fetch) {
  const headers = { Authorization: `Bearer ${token}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28' };
  const request = path => fetchImpl(`https://api.github.com/repos/${REPOSITORY}${path}`, { headers, redirect: 'manual', signal: AbortSignal.timeout(60_000) });
  return {
    async get(path) {
      const response = await request(path);
      requireThat(response.ok, `GitHub GET ${path}: HTTP ${response.status}`);
      return response.json();
    },
    async list(path, field) {
      const values = []; let count;
      for (let page = 1; ; page++) {
        const data = await this.get(`${path}?per_page=100&page=${page}`);
        requireThat(Number.isSafeInteger(data.total_count) && data.total_count >= 0 && data.total_count <= 1000 && Array.isArray(data[field]), 'Incomplete or oversized GitHub metadata listing.');
        count ??= data.total_count;
        requireThat(count === data.total_count && data[field].length <= 100, 'GitHub listing changed during validation.');
        values.push(...data[field]);
        if (values.length >= count) { requireThat(values.length === count, 'Duplicate GitHub metadata page.'); return values; }
        requireThat(data[field].length === 100 && page < 10, 'Truncated GitHub metadata listing.');
      }
    },
    async download(artifact, path) {
      const response = await request(`/actions/artifacts/${artifact.id}/zip`);
      requireThat(response.status === 302, `Artifact download unavailable: HTTP ${response.status}`);
      const url = new URL(response.headers.get('location'));
      requireThat(url.protocol === 'https:' && !url.username && !url.password, 'Unsafe artifact redirect.');
      // The redirect is supplied by GitHub; never forward the repository token to storage.
      const download = await fetchImpl(url, { redirect: 'error', signal: AbortSignal.timeout(300_000) });
      requireThat(download.ok && download.body, `Artifact storage: HTTP ${download.status}`);
      const digest = createHash('sha256'); let size = 0;
      const hash = new Transform({ transform(chunk, _encoding, next) {
        size += chunk.length;
        if (size > 3 * 1024 ** 3 || size > artifact.size_in_bytes) return next(Error('Artifact archive exceeds declared size.'));
        digest.update(chunk); next(null, chunk);
      } });
      await pipeline(Readable.fromWeb(download.body), hash, createWriteStream(path, { flags: 'wx' }));
      requireThat(size === artifact.size_in_bytes && `sha256:${digest.digest('hex')}` === artifact.digest, 'Downloaded archive digest/size differs from GitHub metadata.');
    },
  };
}
export async function validateSource(api, runId, attempt, env) {
  const [repository, run, runAttempt, workflow, jobs, artifacts, main] = await Promise.all([
    api.get(''), api.get(`/actions/runs/${runId}`), api.get(`/actions/runs/${runId}/attempts/${attempt}`), api.get('/actions/workflows/build.yml'),
    api.list(`/actions/runs/${runId}/attempts/${attempt}/jobs`, 'jobs'), api.list(`/actions/runs/${runId}/artifacts`, 'artifacts'), api.get('/git/ref/heads/main'),
  ]);
  requireThat(repository.id === Number(env.GITHUB_REPOSITORY_ID), 'Promotion repository ID mismatch.');
  // Reject source reruns, stale attempts and attempt metadata disagreement rather than mixing platform results.
  for (const key of ['id', 'run_attempt', 'workflow_id', 'path', 'head_sha', 'head_branch', 'event', 'status', 'conclusion']) requireThat(run[key] === runAttempt[key], 'Source run attempt changed or does not match.');
  const selected = validateMetadata({ repository, run, workflow, jobs, artifacts, runId, attempt });
  requireThat(main.object?.type === 'commit' && main.object.sha === env.GITHUB_SHA, 'Main advanced; start a new promotion from current main.');
  const commit = await api.get(`/git/commits/${run.head_sha}`);
  const source = verifyGitSource({ mainSha: main.object.sha, sourceSha: run.head_sha, sourceCommit: commit, checkoutSha: env.GITHUB_SHA });
  const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
  if (run.head_branch !== 'main') {
    requireThat(run.head_branch === `v${version}`, 'Source tag does not match the signed release version.');
    let object = (await api.get(`/git/ref/tags/${encodeURIComponent(run.head_branch)}`)).object;
    for (let depth = 0; object?.type === 'tag' && depth < 5; depth++) {
      requireThat(isSha(object.sha), 'Invalid annotated tag.');
      object = (await api.get(`/git/tags/${object.sha}`)).object;
    }
    requireThat(object?.type === 'commit' && object.sha === run.head_sha, 'Source tag moved or does not resolve to the validated commit.');
  }
  return { repository, run, source, selected, version };
}
export async function promote(env = process.env) {
  requireThat(env.GITHUB_ACTIONS === 'true' && env.GITHUB_REPOSITORY === REPOSITORY && env.GITHUB_REF === 'refs/heads/main' && env.GITHUB_EVENT_NAME === 'workflow_dispatch' && env.GITHUB_TOKEN, 'Promotion requires a manual dispatch from this repository main.');
  requireThat(env.GITHUB_WORKFLOW_REF === `${REPOSITORY}/.github/workflows/promote.yml@refs/heads/main` && env.GITHUB_WORKFLOW_SHA === env.GITHUB_SHA, 'Unexpected promotion workflow origin.');
  requireThat(positiveId(env.SOURCE_RUN_ID) && positiveId(env.SOURCE_RUN_ATTEMPT) && positiveId(env.GITHUB_RUN_ID) && positiveId(env.GITHUB_RUN_ATTEMPT) && positiveId(env.GITHUB_REPOSITORY_ID) && env.SOURCE_RUN_ID !== env.GITHUB_RUN_ID, 'Specify an explicit prior build run and attempt.');
  requireThat(['true', 'false'].includes(env.PUBLISH_RELEASE), 'Explicit publication choice is required.');
  const api = githubClient(env.GITHUB_TOKEN);
  const verified = await validateSource(api, env.SOURCE_RUN_ID, env.SOURCE_RUN_ATTEMPT, env);
  const work = resolve('promotion-work'); mkdirSync(work);
  const downloads = join(work, 'installers'); mkdirSync(downloads);
  for (const platform of Object.keys(PLATFORMS)) {
    const artifact = verified.selected[platform].artifact, archive = join(work, `${artifact.id}.zip`), directory = join(downloads, platform);
    await api.download(artifact, archive);
    execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['scripts/release/unpack-artifact.py', archive, directory], { stdio: 'inherit' });
    const name = `provenance-${platform}.json`, record = JSON.parse(readFileSync(join(directory, name), 'utf8'));
    validateProvenance(record, { ...verified, platform, files: inventory(directory, platform, [name]) });
  }
  execFileSync(process.execPath, ['scripts/release/verify-version.mjs'], { stdio: 'inherit' });
  const assets = join(work, 'assets');
  // Preserves existing five-format, magic, signed version, filename and updater signature verification.
  execFileSync(process.execPath, ['scripts/release/prepare-assets.mjs', downloads, assets], { stdio: 'inherit' });
  const final = await validateSource(api, env.SOURCE_RUN_ID, env.SOURCE_RUN_ATTEMPT, env);
  for (const platform of Object.keys(PLATFORMS)) for (const key of ['id', 'digest']) requireThat(final.selected[platform].artifact[key] === verified.selected[platform].artifact[key], 'Artifact identity changed during verification.');
  const record = {
    schema: 1, repository: REPOSITORY, version: verified.version, sourceRunId: verified.run.id, sourceRunAttempt: verified.run.run_attempt,
    sourceSha: verified.run.head_sha, sourceTree: verified.source.tree, workflow: WORKFLOW, workflowSha256: verified.source.workflowSha256,
    trustedMainSha: env.GITHUB_SHA, promotionRunId: Number(env.GITHUB_RUN_ID), promotionRunAttempt: Number(env.GITHUB_RUN_ATTEMPT), validatedAt: new Date().toISOString(), publication: env.PUBLISH_RELEASE === 'true' ? 'not-started' : 'not-requested',
    artifacts: Object.entries(verified.selected).map(([platform, { job, artifact }]) => ({ platform, jobId: job.id, artifactId: artifact.id, archiveDigest: artifact.digest })),
  };
  const saveRecord = () => writeFileSync('promotion-record.json', `${JSON.stringify(record, null, 2)}\n`); saveRecord();
  if (env.GITHUB_STEP_SUMMARY) appendFileSync(env.GITHUB_STEP_SUMMARY, `## Validated release source\n\n- Build: https://github.com/${REPOSITORY}/actions/runs/${record.sourceRunId}/attempts/${record.sourceRunAttempt}\n- Source commit: ${record.sourceSha}\n- Full Git tree: ${record.sourceTree}\n- Trusted main: ${record.trustedMainSha}\n- Build workflow SHA-256: ${record.workflowSha256}\n- Version: ${record.version}\n- Artifacts: ${record.artifacts.map(item => `${item.platform}: ${item.artifactId} (${item.archiveDigest})`).join('; ')}\n\n${env.PUBLISH_RELEASE === 'true' ? 'All checks passed; invoking the existing guarded publisher.' : 'Validation only; publication was not requested.'}\n`);
  if (env.PUBLISH_RELEASE === 'true') {
    // Keep all existing version/tag/draft/asset/feed guards. Tag and release notes point to the original build.
    record.publication = 'attempting'; saveRecord();
    try {
      execFileSync(process.execPath, ['scripts/release/publish.mjs', assets], { stdio: 'inherit', env: { ...env, GITHUB_SHA: verified.run.head_sha, GITHUB_RUN_ID: String(verified.run.id) } });
      record.publication = 'confirmed'; saveRecord();
    } catch (error) {
      // The publisher may have made the release public before its final served-feed check failed.
      record.publication = 'unconfirmed'; saveRecord();
      if (env.GITHUB_STEP_SUMMARY) appendFileSync(env.GITHUB_STEP_SUMMARY, '\nPublication did not finish cleanly. A public release may already exist; inspect the release and served update feed before retrying.\n');
      throw error;
    }
  }
  return record;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await promote();
