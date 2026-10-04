import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { fullPreview, essentialPreview, planValidation, parseChangedPaths } from '../ci/validation-plan.mjs';

test('the core matrix exists and shared or unknown changes cannot skip it', () => {
  assert(fullPreview.length <= 10);
  assert(fullPreview.includes('test-workspace-context.mjs'));
  assert(fullPreview.includes('test-investigation.mjs'));
  for (const name of fullPreview) assert(existsSync(new URL(`../preview/${name}`, import.meta.url)));
  for (const file of ['frontend/app.js', 'src-tauri/src/lib.rs', '.github/workflows/build.yml', 'scripts/ci/native-essential.mjs', 'new-runtime-file']) {
    const plan = planValidation([file]);
    assert.deepEqual([...plan.preview].sort(), [...fullPreview].sort());
    assert.equal(plan.native, file !== 'frontend/app.js');
  }
});
test('focused changes preserve ownership and affected behavior, docs skip runtime work', () => {
  const plan = planValidation(['frontend/remote-sources.js']);
  for (const name of [...essentialPreview, 'test-remote-sources.mjs']) assert(plan.preview.includes(name));
  assert.equal(plan.native, false);
  assert.deepEqual(planValidation(['docs/usage.md']).preview, []);
  assert.equal(planValidation(['docs/usage.md']).native, false);
  assert.deepEqual(planValidation([]).preview, fullPreview);
});
test('deleted and malformed comparison paths cannot produce a false green', () => {
  assert.deepEqual(parseChangedPaths('D\0frontend/app.js\0M\0README.md\0'), { files: ['frontend/app.js', 'README.md'], deleted: ['frontend/app.js'] });
  assert.throws(() => parseChangedPaths('M\0partial'));
  assert.throws(() => planValidation(['frontend/app.js\nspoof']));
  assert.throws(() => planValidation(['README.md'], { deleted: ['other'] }));
  assert.equal(planValidation(['scripts/preview/test-updates.mjs'], { deleted: ['scripts/preview/test-updates.mjs'] }).scope, 'full');
});
