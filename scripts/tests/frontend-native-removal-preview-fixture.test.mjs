import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
const preview = read('scripts/preview/test-toast-feedback.mjs'), app = read('frontend/app.js'), workspace = read('frontend/workspace-context.js');
const start = preview.indexOf('    // The production app handler, receipt guard and native action controller stay');
const end = preview.indexOf('\n  });\n  await page.waitForFunction(() => !!__nativeUndoFixture.release);', start);
assert.ok(start >= 0 && end > start, 'exercise the installed preview producer rather than duplicating its reference schema');
const drain = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };

async function fixture() {
  const errors = [], nodes = new Map(), originalStore = { cases: [] };
  const context = vm.createContext({ window: {}, structuredClone, TextEncoder,
    state: { cases: originalStore, caseTreeProfiles: {} }, STANDARD: [], scope: 'dataset',
    nativeRemovalNotices: new Map(), lastCaseRemoval: null,
    document: { documentElement: { dataset: {} }, body: { dataset: {} },
      dispatchEvent() {
        // The first real workspace membership-listener step reads the summary.
        // Browser event-handler exceptions become pageerrors, not action rejections.
        try { context.updateToggle(); } catch (error) { errors.push(error); }
      } },
    CustomEvent: class { constructor(type) { this.type = type; } },
    $: selector => { if (!nodes.has(selector)) nodes.set(selector, {}); return nodes.get(selector); },
    refreshCaseRemoval() {}, activeCase: () => null, nativeEvidenceServices: () => null,
  });
  for (const name of ['case-evidence.js', 'case-evidence-actions.js']) {
    vm.runInContext(read(`frontend/${name}`), context, { filename: name });
  }
  context.CaseEvidence = context.window.CaseEvidence; context.CaseEvidenceActions = context.window.CaseEvidenceActions;
  vm.runInContext(app.slice(app.indexOf('const caseObjectIds ='), app.indexOf('function caseEvents(mode')), context);
  vm.runInContext(app.slice(app.indexOf('function caseTreeProfilesPeek()'), app.indexOf('// interpreta "20 MB"')), context);
  vm.runInContext(workspace.slice(workspace.indexOf('  function updateToggle()'), workspace.indexOf('  async function showUnavailable()')), context);
  // Keep actual native actions, view readers and the exact preview transport.
  // Toast presentation is covered by frontend-native-removal-feedback.test.mjs.
  context.removeCaseOccurrences = (c, targets) => context.nativeEvidenceServices().actions.remove(c, targets);
  vm.runInContext(`(() => {${preview.slice(start, end)}})()`, context);
  const f = context.window.__nativeUndoFixture; await drain();
  return { context, f, errors, originalStore, actions: context.nativeEvidenceServices().actions,
    release() { f.hold = false; f.release(); },
    shape: () => context.CaseEvidence.analysisShape(f.c),
  };
}

test('actual preview producer emits a valid pending descriptor and canonical committed references', async () => {
  const h = await fixture(), before = h.f.c.items[0].rows.reference;
  assert.equal(before.kind, 'pending_native_evidence'); assert.doesNotThrow(h.shape);
  assert.doesNotThrow(() => h.context.CaseEvidence.validate.pendingReference(before));
  h.release(); const receipt = await h.f.pending;
  const committed = h.f.c.items[0].rows.reference;
  assert.equal(committed.kind, 'native_evidence'); assert.equal(committed.memberCount, 1);
  assert.doesNotThrow(() => h.context.CaseEvidence.validate.reference(committed));
  assert.deepEqual(Object.keys(committed).sort(), ['kind', 'schemaVersion', 'owner', 'containerId', 'manifestId', 'manifestSha256', 'memberCount'].sort());
  await h.actions.undo(receipt);
  assert.equal(h.f.c.items[0].rows.reference.memberCount, 2); assert.doesNotThrow(h.shape); assert.deepEqual(h.errors, []);
  h.f.restore(); assert.equal(h.context.state.cases, h.originalStore);
});

test('lost removal acknowledgement remains an expected action error without a separate real-reader pageerror', async () => {
  const h = await fixture(); h.f.fail = true; h.release();
  await assert.rejects(h.f.pending, /EVIDENCE_SAVE_RETRY_REQUIRED/);
  assert.equal(h.f.status, 'retry_required'); assert.equal(h.f.c.items[0].rows.reference.kind, 'pending_native_evidence');
  assert.equal(h.f.c.items[0].rows.reference.memberCount, 1); assert.doesNotThrow(h.shape); assert.deepEqual(h.errors, []);
});

test('lost Undo acknowledgement keeps a valid pending draft when real workspace summary listeners inspect it', async () => {
  const h = await fixture(); h.release(); const receipt = await h.f.pending;
  h.f.fail = true; await assert.rejects(h.actions.undo(receipt), /EVIDENCE_SAVE_RETRY_REQUIRED/);
  const pending = h.f.c.items[0].rows.reference;
  assert.equal(pending.kind, 'pending_native_evidence'); assert.equal(pending.memberCount, 2);
  assert.equal(h.actions.canUndo(receipt), false); assert.doesNotThrow(h.shape); assert.deepEqual(h.errors, []);
});

test('the real pending-reference guard still rejects missing authority fields in fixture output', async () => {
  const h = await fixture(), pending = h.f.c.items[0].rows.reference;
  for (const field of ['requestId', 'bytes', 'expiresAt', 'purpose']) {
    const broken = structuredClone(pending); delete broken[field];
    assert.throws(() => h.context.CaseEvidence.validate.pendingReference(broken), /EVIDENCE_VIEW_INVALID/, field);
  }
  h.release(); await h.f.pending;
});
