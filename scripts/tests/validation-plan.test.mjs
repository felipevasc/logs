import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { fullPreview, essentialPreview, planValidation, parseChangedPaths } from '../ci/validation-plan.mjs';

const contains = (actual, expected) => { for (const value of expected) assert(actual.includes(value), `Missing ${value}`); };
test('UI-only edits avoid native compilation and keep all essential safety smoke', () => {
  const plan = planValidation(['frontend/updates.js']);
  assert.equal(plan.native, false);
  assert.equal(plan.scope, 'essential-and-affected');
  assert.deepEqual(plan.preview, [...essentialPreview].sort());
  assert(plan.preview.length < fullPreview.length);
});
test('shared application monolith keeps full browser coverage without native compilation', () => {
  for (const path of ['frontend/app.js', 'frontend/index.html', 'frontend/styles.css', 'frontend/analysis-context.js', 'frontend/performance-core.js', 'scripts/ci/validation-plan.mjs']) {
    const plan = planValidation([path]);
    assert.equal(plan.native, false, path);
    assert.equal(plan.scope, 'full', path);
    contains(plan.preview, fullPreview);
  }
});
test('each affected UI area keeps its focused browser regression', () => {
  const plan = planValidation(['frontend/workspace-context.js', 'frontend/remote-sources.js', 'frontend/tasks.js']);
  contains(plan.preview, [...essentialPreview, 'test-workspace-context.mjs', 'test-remote-sources.mjs']);
  assert.equal(plan.native, false);
});
test('exclusion, field, timeline and Java edits select the corresponding regression', () => {
  const plan = planValidation(['frontend/case-removals.js', 'frontend/canonical-fields.js', 'frontend/timeline.js', 'frontend/java-trace.css']);
  contains(plan.preview, ['test-exclusion-archive.mjs', 'test-field-transform.mjs', 'test-canonical-field-actions.mjs', 'test-explorer-timeline.mjs', 'test-java-trace.mjs']);
  assert.equal(plan.native, false);
});
test('context menus and semantic waits retain their dedicated browser pilots', () => {
  for (const [path, preview] of [
    ['frontend/context-menu.js', 'test-context-menu.mjs'],
    ['frontend/command-palette.js', 'test-context-menu.mjs'],
    ['frontend/waiting-visuals.css', 'test-waiting-visuals.mjs'],
    ['frontend/waiting-progress.js', 'test-waiting-visuals.mjs'],
    ['frontend/tasks.js', 'test-waiting-visuals.mjs'],
  ]) {
    const plan = planValidation([path]);
    assert.equal(plan.native, false, path);
    contains(plan.preview, [...essentialPreview, preview]);
    assert(fullPreview.includes(preview));
  }
});
test('native, build, workflow and unknown changes retain the complete matrix', () => {
  for (const path of ['src-tauri/src/workspace/canonical.rs', 'src-tauri/Cargo.lock', 'src-tauri/windows/installer-hooks.nsh', 'package-lock.json', '.github/workflows/checks.yml', 'scripts/release/update-e2e.mjs', 'scripts/prepare-frontend.mjs', 'frontend/new-unmapped.js', 'new-tool.sh']) {
    const plan = planValidation([path]);
    assert.equal(plan.native, true, path);
    assert.equal(plan.scope, 'full', path);
    contains(plan.preview, fullPreview);
  }
});
test('Timeline export and shared waiting changes retain both real output pilots', () => {
  for (const path of ['frontend/timeline-export.js', 'frontend/timeline-export.css', 'frontend/waiting-visuals.js', 'frontend/waiting-visuals.css', 'frontend/tasks.js']) {
    const plan = planValidation([path]);
    assert.equal(plan.native, false, path);
    contains(plan.preview, [...essentialPreview, 'test-timeline-export-waiting.mjs']);
    if (path.startsWith('frontend/timeline-export.')) contains(plan.preview, ['test-explorer-timeline.mjs']);
    else contains(plan.preview, ['test-case-report-waiting.mjs']);
  }
  assert(fullPreview.includes('test-timeline-export-waiting.mjs'));
});
test('source views and shared menus retain exact-path and compact source actions checks', () => {
  for (const path of ['frontend/workspace.js', 'frontend/workspace.css', 'frontend/context-menu.js']) {
    const plan = planValidation([path]);
    assert.equal(plan.native, false, path);
    contains(plan.preview, [...essentialPreview, 'test-source-actions.mjs']);
  }
  assert(fullPreview.includes('test-source-actions.mjs'));
  assert(fullPreview.includes('test-columns.mjs'));
});
test('full request and missing comparison data cannot produce an empty or focused green', () => {
  for (const plan of [planValidation([]), planValidation(['docs/readme.md'], { full: true })]) {
    assert.equal(plan.native, true);
    assert.deepEqual(plan.preview, fullPreview);
  }
  assert.throws(() => planValidation(['frontend/app.js\nspoof']));
});
test('docs-only scope explicitly reports inapplicability rather than claiming tests passed', () => {
  const plan = planValidation(['docs/ci-validation.md', 'README.md']);
  assert.deepEqual(plan.preview, []);
  assert.equal(plan.native, false);
  assert.equal(plan.scope, 'documentation');
});
test('preview infrastructure runs full browser coverage and changed tests run directly', () => {
  contains(planValidation(['scripts/preview/mock-tauri.js']).preview, fullPreview);
  contains(planValidation(['scripts/preview/test-review-workspace.mjs']).preview, ['test-review-workspace.mjs', ...essentialPreview]);
});
test('deleted and renamed preview tests select remaining coverage, never execute missing paths', () => {
  const changes = parseChangedPaths('D\0scripts/preview/test-old.mjs\0A\0scripts/preview/test-new.mjs\0');
  const plan = planValidation(changes.files, changes);
  assert.equal(plan.preview.includes('test-old.mjs'), false);
  contains(plan.preview, [...fullPreview, 'test-new.mjs']);
  assert.throws(() => parseChangedPaths('M\0path-without-terminator'));
  assert.throws(() => parseChangedPaths('R100\0old\0new\0'));
});
test('all maintained default and mapped checks exist, with no duplicate defaults', () => {
  const selections = [...fullPreview, ...essentialPreview,
    ...planValidation(['frontend/analysis-fields.js', 'frontend/discovery.js', 'frontend/canonical-fields.js', 'frontend/exclusion-archive.js', 'frontend/timeline.js', 'frontend/java-trace.js', 'frontend/journeys.js', 'frontend/remote-sources.js', 'frontend/workspace.js', 'frontend/styles.css', 'frontend/context-menu.js', 'frontend/tasks.js']).preview];
  for (const file of selections) assert(existsSync(new URL(`../preview/${file}`, import.meta.url)), file);
  assert.equal(new Set(fullPreview).size, fullPreview.length);
});
test('workflow contract retains native and installed-update gates, shares cache and makes portable build opt-in', () => {
  const checks = readFileSync(new URL('../../.github/workflows/checks.yml', import.meta.url), 'utf8');
  const build = readFileSync(new URL('../../.github/workflows/build.yml', import.meta.url), 'utf8');
  for (const workflow of [checks, build]) {
    assert.match(workflow, /shared-key: desktop-release-v1/);
    assert.match(workflow, /RUST_TEST_THREADS: '1'/);
    assert.match(workflow, /cargo test --manifest-path src-tauri\/Cargo.toml --release --locked --tests -- --test-threads=1/);
    assert.match(workflow, /windows-native-tests\.ps1/);
    assert.match(workflow, /verify-windows-manifest\.ps1 -Target harness/);
    assert.doesNotMatch(workflow, /continue-on-error/);
  }
  for (const name of ['Build Windows portable review executable', 'Verify Windows app manifest', 'Record portable executable identity', 'Upload Windows portable review build']) {
    const step = checks.slice(checks.indexOf(`- name: ${name}`)).split('\n      - ')[0];
    assert.match(step, /if: runner.os == 'Windows' && inputs.portable_windows/, name);
  }
  assert.match(build, /run: node scripts\/release\/update-e2e\.mjs/);
  assert.match(build, /needs: build/);
  assert.match(build, /run: node scripts\/release\/publish\.mjs release-assets/);
  assert.match(checks, /name: Required validation/);
  assert.match(checks, /if: always\(\)/);
});
