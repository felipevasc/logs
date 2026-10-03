// Artifact inventory only: does not change, transcode or discard browser evidence.
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, readdir, stat, writeFile } from 'node:fs/promises';
import { resolve, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

export function artifactFor(path) {
  const recordings = {
    'waiting-visuals-real-preview.webm': 'scenes',
    'waiting-reactions-real-preview.webm': 'reactions',
    'waiting-verification-real-preview.webm': 'verification',
    'case-report-waiting-real-preview.webm': 'report',
  };
  return path.endsWith('.webm') ? `preview-recordings-${recordings[path] || 'other'}` : 'preview-diagnostics';
}

export async function recordEvidence(directory = resolve('output/playwright'), environment = process.env) {
  await mkdir(directory, { recursive: true });
  const files = [];
  async function visit(folder) {
    for (const entry of (await readdir(folder, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
      const path = resolve(folder, entry.name);
      if (entry.isDirectory()) { await visit(path); continue; }
      if (!entry.isFile()) continue; // No symlinks or paths outside this evidence tree.
      const name = relative(directory, path).split(sep).join('/');
      if (name === 'evidence-inventory.json') continue;
      const hash = createHash('sha256');
      for await (const chunk of createReadStream(path)) hash.update(chunk);
      files.push({ path: name, bytes: (await stat(path)).size, sha256: hash.digest('hex'), artifact: artifactFor(name) });
    }
  }
  await visit(directory);
  const inventory = { schemaVersion: 1, headSha: environment.EVIDENCE_HEAD_SHA || environment.GITHUB_SHA || null,
    checkoutSha: environment.GITHUB_SHA || null, runId: environment.GITHUB_RUN_ID || null,
    runAttempt: environment.GITHUB_RUN_ATTEMPT || null, files };
  await writeFile(resolve(directory, 'evidence-inventory.json'), JSON.stringify(inventory, null, 2) + '\n');
  return inventory;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const result = await recordEvidence();
  const totals = {};
  for (const file of result.files) totals[file.artifact] = (totals[file.artifact] || 0) + file.bytes;
  console.log(JSON.stringify({ headSha: result.headSha, runId: result.runId, files: result.files.length, uncompressedBytes: totals }));
}
