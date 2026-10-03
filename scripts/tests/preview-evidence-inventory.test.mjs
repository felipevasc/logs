import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { artifactFor, recordEvidence } from '../preview/record-evidence.mjs';

test('browser evidence inventory links intact recordings to scenario archives and exact CI identity', async () => {
  const root = await mkdtemp(join(tmpdir(), 'preview-evidence-'));
  try {
    await mkdir(join(root, 'video-raw'));
    const contents = Buffer.from('synthetic fixture bytes, not a generated video');
    for (const name of ['waiting-visuals-real-preview.webm', 'waiting-reactions-real-preview.webm',
      'waiting-verification-real-preview.webm', 'case-report-waiting-real-preview.webm', 'view.png']) await writeFile(join(root, name), contents);
    await writeFile(join(root, 'video-raw', 'partial.webm'), contents);
    await writeFile(join(root, 'evidence-inventory.json'), 'old inventory');
    const result = await recordEvidence(root, { EVIDENCE_HEAD_SHA: 'head', GITHUB_SHA: 'checkout', GITHUB_RUN_ID: '65', GITHUB_RUN_ATTEMPT: '2' });
    assert.equal(result.headSha, 'head'); assert.equal(result.checkoutSha, 'checkout'); assert.equal(result.runId, '65'); assert.equal(result.runAttempt, '2');
    assert.equal(result.files.length, 6);
    const digest = createHash('sha256').update(contents).digest('hex');
    for (const file of result.files) {
      assert.equal(file.bytes, contents.length); assert.equal(file.sha256, digest);
      assert.equal(file.artifact, artifactFor(file.path));
      assert.deepEqual(await readFile(join(root, file.path)), contents, 'inventory does not modify evidence');
    }
    assert.equal(artifactFor('video-raw/partial.webm'), 'preview-recordings-other');
    assert.equal(artifactFor('view.png'), 'preview-diagnostics');
    assert.equal(artifactFor('waiting-verification-real-preview.webm'), 'preview-recordings-verification');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('inventory does not follow links or hash its own prior output', async t => {
  const root = await mkdtemp(join(tmpdir(), 'preview-evidence-'));
  try {
    await mkdir(join(root, 'evidence'));
    await writeFile(join(root, 'outside.txt'), 'outside');
    try { await symlink(join(root, 'outside.txt'), join(root, 'evidence', 'linked.txt')); }
    catch (error) {
      if (process.platform === 'win32' && error.code === 'EPERM') { t.skip('Windows account cannot create symbolic links'); return; }
      throw error;
    }
    const result = await recordEvidence(join(root, 'evidence'), {});
    assert.deepEqual(result.files, []); assert.equal(result.headSha, null);
    assert.deepEqual((await recordEvidence(join(root, 'evidence'), {})).files, []);
  } finally { await rm(root, { recursive: true, force: true }); }
});
