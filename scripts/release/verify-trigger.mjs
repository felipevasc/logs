import { readFileSync } from 'node:fs';

// Also called by the publisher itself: invoking it directly must not bypass the
// same check used before dependency installation and builds in CI.
export function verifyTrigger(version, env = process.env) {
  const tag = `v${version}`;
  const refTag = env.GITHUB_REF?.startsWith('refs/tags/') ? env.GITHUB_REF.slice(10) : null;
  const namedTag = env.GITHUB_REF_TYPE === 'tag' ? env.GITHUB_REF_NAME : null;
  if (env.GITHUB_REF_TYPE === 'tag' && !namedTag) throw Error('Missing triggering tag name.');
  for (const actual of [refTag, namedTag]) {
    if (actual != null && actual !== tag) throw Error(`Triggering tag ${actual} does not match package version ${tag}.`);
  }
  return tag;
}

verifyTrigger(JSON.parse(readFileSync('package.json', 'utf8')).version);
