// Stage production bytes before updater E2E rebuilds the bundle directories; record only after it passes.
import { copyFileSync, mkdirSync, readdirSync, readFileSync, writeFileSync, unlinkSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import { inventory, PLATFORMS, REPOSITORY, WORKFLOW, requireThat, positiveId, isSha, sha256 } from './promotion-guards.mjs';
export function stageInstallers(platform, output, bundleRoot = 'src-tauri/target/release/bundle') {
  requireThat(PLATFORMS[platform], 'Unsupported platform.');
  mkdirSync(output, { recursive: true }); requireThat(readdirSync(output).length === 0, 'Staging directory must be empty.');
  const directories = { '.exe': 'nsis', '.msi': 'msi', '.deb': 'deb', '.AppImage': 'appimage', '.rpm': 'rpm' };
  for (const extension of PLATFORMS[platform].extensions) {
    const directory = join(bundleRoot, directories[extension]);
    const names = readdirSync(directory).filter(name => name.endsWith(extension));
    requireThat(names.length === 1, `Expected exactly one production ${extension} before E2E.`);
    for (const name of [names[0], `${names[0]}.sig`]) copyFileSync(join(directory, name), join(output, name));
  }
  const files = inventory(output, platform);
  writeFileSync(join(output, '.staged.json'), JSON.stringify(files));
}
export function recordProvenance(platform, output, env = process.env) {
  requireThat(env.GITHUB_ACTIONS === 'true' && env.GITHUB_REPOSITORY === REPOSITORY && env.GITHUB_JOB === 'build' && env.UPDATE_E2E_OUTCOME === 'success', 'Provenance requires a successful real build-job updater gate.');
  requireThat(positiveId(env.GITHUB_RUN_ID) && positiveId(env.GITHUB_RUN_ATTEMPT) && positiveId(env.GITHUB_REPOSITORY_ID) && isSha(env.GITHUB_SHA) && env.GITHUB_WORKFLOW_SHA === env.GITHUB_SHA, 'Missing trusted build identity.');
  requireThat(/^refs\/heads\/[^\s]+$/.test(env.GITHUB_REF) || /^refs\/tags\/v\d+\.\d+\.\d+$/.test(env.GITHUB_REF), 'Unapproved source ref.');
  requireThat(env.GITHUB_WORKFLOW_REF === `${REPOSITORY}/${WORKFLOW}@${env.GITHUB_REF}`, 'Unexpected workflow ref.');
  const git = args => execFileSync('git', args, { encoding: 'utf8' }).trim();
  requireThat(git(['rev-parse', 'HEAD']) === env.GITHUB_SHA, 'Checkout differs from source SHA.');
  const files = inventory(output, platform, ['.staged.json']);
  requireThat(JSON.stringify(files) === readFileSync(join(output, '.staged.json'), 'utf8'), 'Staged installer bytes changed during E2E.');
  const record = {
    schema: 1, platform, repository: REPOSITORY, repositoryId: Number(env.GITHUB_REPOSITORY_ID), runId: Number(env.GITHUB_RUN_ID), runAttempt: Number(env.GITHUB_RUN_ATTEMPT), job: 'build',
    version: JSON.parse(readFileSync('package.json', 'utf8')).version, updateE2E: 'success',
    source: { sha: env.GITHUB_SHA, tree: git(['rev-parse', `${env.GITHUB_SHA}^{tree}`]), ref: env.GITHUB_REF, workflowPath: WORKFLOW, workflowRef: env.GITHUB_WORKFLOW_REF, workflowSha: env.GITHUB_WORKFLOW_SHA, workflowSha256: sha256(execFileSync('git', ['show', `${env.GITHUB_SHA}:${WORKFLOW}`])) },
    files,
  };
  writeFileSync(join(output, `provenance-${platform}.json`), `${JSON.stringify(record, null, 2)}\n`);
  unlinkSync(join(output, '.staged.json'));
  return record;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [mode, platform, output] = process.argv.slice(2);
  requireThat(output && ['stage', 'record'].includes(mode), 'Usage: provenance.mjs stage|record platform directory');
  if (mode === 'stage') stageInstallers(platform, output); else recordProvenance(platform, output);
}
