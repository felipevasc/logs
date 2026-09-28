import { readFileSync, readdirSync, statSync, createReadStream } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

const directory = process.argv[2], token = process.env.GITHUB_TOKEN, repo = process.env.GITHUB_REPOSITORY, sha = process.env.GITHUB_SHA;
if (!directory || !token || !/^[\w.-]+\/[\w.-]+$/.test(repo || '') || !/^[a-f0-9]{40}$/.test(sha || '')) throw Error('Missing trusted release environment.');
const version = JSON.parse(readFileSync('package.json', 'utf8')).version, tag = `v${version}`;
const names = readdirSync(directory).sort();
const extensions = ['.exe', '.msi', '.deb', '.rpm', '.AppImage'];
const installers = extensions.map(ext => names.filter(name => name.endsWith(ext)));
if (names.length !== 12 || !names.includes('SHA256SUMS.txt') || !names.includes('latest.json') || installers.some(found => found.length !== 1 || !names.includes(`${found[0]}.sig`))) throw Error('Release assets are incomplete.');
const headers = { Authorization: `Bearer ${token}`, Accept: 'application/vnd.github+json', 'Content-Type': 'application/json', 'X-GitHub-Api-Version': '2022-11-28' };
async function api(path, method = 'GET', body) {
  const response = await fetch(`https://api.github.com/repos/${repo}${path}`, { method, headers, ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(60_000) });
  if (response.status === 404 && method === 'GET') return null;
  if (!response.ok) throw Error(`GitHub ${method} ${path}: HTTP ${response.status}`);
  return response.status === 204 ? null : response.json();
}
const parts = value => String(value).replace(/^v/, '').split('.').map(Number);
const newer = (a, b) => { const [x, y] = [parts(a), parts(b)]; for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i]; return false; };
// Installed apps only update to a higher version; publishing a lower one as latest would stall them.
const latest = await api('/releases/latest');
if (latest && latest.tag_name !== tag && !newer(version, latest.tag_name)) throw Error(`${tag} is not newer than the published ${latest.tag_name}.`);
const ref = await api(`/git/ref/tags/${tag}`);
if (ref) {
  let object = ref.object;
  for (let i = 0; object.type === 'tag' && i < 5; i++) object = (await api(`/git/tags/${object.sha}`)).object;
  if (object.type !== 'commit' || object.sha !== sha) throw Error(`${tag} already refers to a different commit.`);
}
let release = await api(`/releases/tags/${tag}`);
if (release && !release.draft) throw Error(`${tag} is already published; use a new version.`);
const body = `${readFileSync(`docs/releases/${tag}.md`, 'utf8').trim()}\n\nCommit: [\`${sha.slice(0, 8)}\`](https://github.com/${repo}/commit/${sha}) · [Build e testes](https://github.com/${repo}/actions/runs/${process.env.GITHUB_RUN_ID})\n`;
if (!release) release = await api('/releases', 'POST', { tag_name: tag, target_commitish: sha, name: `LogInsight ${tag}`, body, draft: true, prerelease: false });
else await api(`/releases/${release.id}`, 'PATCH', { body });
if (release.assets.some(asset => !names.includes(asset.name))) throw Error('Draft contains unexpected assets; refusing to publish an ambiguous release.');
const hashes = new Map();
for (const name of names) {
  const path = join(directory, name), size = statSync(path).size;
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  hashes.set(name, { size, digest: `sha256:${hash.digest('hex')}` });
  const previous = release.assets.find(asset => asset.name === name);
  if (previous) await api(`/releases/assets/${previous.id}`, 'DELETE');
  const url = new URL(release.upload_url.split('{')[0]);
  if (url.hostname !== 'uploads.github.com') throw Error('Unexpected upload host.');
  url.searchParams.set('name', name);
  const response = await fetch(url, { method: 'POST', headers: { ...headers, 'Content-Type': 'application/octet-stream', 'Content-Length': String(size) }, body: createReadStream(path), duplex: 'half', signal: AbortSignal.timeout(300_000) });
  if (!response.ok) throw Error(`Upload failed for ${name}: HTTP ${response.status}`);
  console.log(`Uploaded ${name} (${size} bytes).`);
}
const assets = await api(`/releases/${release.id}/assets?per_page=100`);
if (assets.length !== names.length || assets.some(asset => !hashes.has(asset.name) || asset.state !== 'uploaded' || asset.size !== hashes.get(asset.name).size || (asset.digest && asset.digest !== hashes.get(asset.name).digest))) throw Error('Uploaded asset verification failed; release remains a draft.');
release = await api(`/releases/${release.id}`, 'PATCH', { draft: false, make_latest: 'true' });
console.log(`Published ${release.html_url}`);

// The address every installed app reads must now announce this version.
const attempts = Number(process.env.RELEASE_CHECK_ATTEMPTS || 10);
for (let attempt = 1; ; attempt++) {
  let served = null;
  try {
    const response = await fetch(`https://github.com/${repo}/releases/latest/download/latest.json`, { signal: AbortSignal.timeout(30_000) });
    served = response.ok ? await response.json() : { status: response.status };
  } catch (error) { served = { error: error.message }; }
  if (served?.version === version) { console.log(`Update manifest for ${version} is served to installed apps.`); break; }
  if (attempt >= attempts) throw Error(`Published, but the update manifest still answers ${JSON.stringify(served)}.`);
  await new Promise(done => setTimeout(done, 6_000));
}
