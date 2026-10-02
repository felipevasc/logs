import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const read = name => readFileSync(new URL(`../../frontend/${name}`, import.meta.url), 'utf8');
const app = read('app.js');
const plain = value => JSON.parse(JSON.stringify(value));
const drain = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const deferred = () => { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; };

function fixture({ debounced = false, blurOnDisable = true } = {}) {
  let document;
  const owner = { storeId: 'store', caseId: 'case', analysisId: 'analysis' };
  const reference = id => ({ kind: 'native_evidence', schemaVersion: 1, owner, containerId: id,
    manifestId: `manifest-${id}`, manifestSha256: 'a'.repeat(64), memberCount: 2 });
  const container = reference => ({ kind: 'native_evidence_container', reference, preservedCount: reference.memberCount, preview: null });
  const c = { id: 'case', analysisContext: { ...owner, configRevision: 1, visibilityRevision: 1 },
    items: ['a', 'b'].map(id => ({ id: 'duplicate-label', label: id, rows: container(reference(id)) })) };
  const store = { store: { storeId: 'store', epoch: 'epoch', revision: '1' }, active: c.id, cases: [c], caseEvidence: [{ state: 'ready', owner }] };
  const state = { cases: store, sourcePublication: { generation: 8 }, currentArtifact: { id: 'source', loadedAt: 1 } };
  const listeners = new Map(), timers = new Map(), calls = [], messages = [];
  let clock = 0, sequence = 0, serial = 0, status = 'ready', prepareGate = null, saveGate = null, failSave = false, prepareError = null;
  class Node {
    constructor(tag, className, textContent = '') { Object.assign(this, { tagName: tag.toUpperCase(), className, textContent, children: [], attributes: {}, disabled: false }); }
    get isConnected() { return this === document?.body || this === area || !!this.parentElement?.isConnected; }
    appendChild(child) { this.children.push(child); child.parentElement = this; return child; }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    focus() { if (!this.disabled && this.isConnected) document.activeElement = this; }
    closest() { return null; }
    getClientRects() { return this.isConnected && !this.hidden ? [{}] : []; }
    get disabled() { return this._disabled; }
    set disabled(value) { this._disabled = value; if (value && blurOnDisable && document?.activeElement === this) document.activeElement = document.body; }
    remove() {
      if (this.contains(document?.activeElement)) document.activeElement = document.body;
      if (this.parentElement) this.parentElement.children.splice(this.parentElement.children.indexOf(this), 1); this.parentElement = null;
    }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    removeAttribute(name) { delete this.attributes[name]; }
  }
  const area = new Node('div', '');
  document = {
    addEventListener(name, fn) { if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name).add(fn); },
    removeEventListener(name, fn) { listeners.get(name)?.delete(fn); },
    dispatchEvent(event) { for (const fn of [...(listeners.get(event.type) || [])]) fn(event); },
  };
  document.body = new Node('body', ''); document.activeElement = document.body;
  const pageSize = document.body.appendChild(new Node('select', '')), caseSelect = document.body.appendChild(new Node('select', ''));
  const otherControl = document.body.appendChild(new Node('button', ''));
  const dispatch = name => document.dispatchEvent({ type: name });
  const activeCase = () => state.cases.cases.find(item => item.id === state.cases.active);
  const context = vm.createContext({ window: {}, state, document, structuredClone, TextEncoder, activeCase,
    el: (...args) => new Node(...args), $: selector => ({ '#toast-area': area, '#page-size': pageSize, '#case-select': caseSelect })[selector], getComputedStyle: () => ({ visibility: 'visible' }), toast: (text, type) => messages.push({ text, type }),
    setTimeout(fn, ms) { const id = ++serial; timers.set(id, { fn, at: clock + ms }); return id; },
    clearTimeout(id) { timers.delete(id); }, updateAnalysisBadge() {}, renderAnalysis() {}, filtersChanged() {},
  });
  for (const file of ['case-evidence.js', 'analysis-context.js', 'case-evidence-actions.js']) vm.runInContext(read(file), context);
  context.window.CaseEvidence.active = true;
  const client = {
    prepareMembership(request) { return { request }; },
    async prepare(ticket) {
      calls.push({ kind: 'prepare', request: plain(ticket.request) });
      if (prepareGate) await prepareGate.promise;
      if (prepareError) throw Error(prepareError);
      const before = ticket.request.reference, action = ticket.request.action;
      const count = action.kind === 'remove' ? before.memberCount - action.members.length : action.target.memberCount;
      return { reference: { ...before, kind: 'pending_native_evidence', manifestId: `prepared-${++sequence}`, token: `token-${sequence}`, memberCount: count },
        undo: before, changedCount: Math.abs(count - before.memberCount), remainingCount: count };
    },
    async discard(ticket) { calls.push({ kind: 'discard', request: plain(ticket.request) }); },
  };
  const save = async () => {
    status = 'saving'; calls.push({ kind: 'save' });
    if (saveGate) await saveGate.promise;
    if (failSave) { status = 'retry_required'; dispatch('case-evidence-state'); return false; }
    for (const item of state.cases.cases.flatMap(item => item.items)) if (item.rows.reference.kind === 'pending_native_evidence') {
      const committed = { ...item.rows.reference, kind: 'native_evidence' }; delete committed.token; item.rows = container(committed);
    }
    store.store.revision = String(+store.store.revision + 1);
    dispatch('case-evidence-state'); status = 'ready'; return true;
  };
  const session = { status: () => status, markDirty() {}, save };
  const actions = context.window.CaseEvidenceActions.create({ client, session,
    getStore: () => state.cases, currentCase: activeCase, save: () => context.saveCases(), selectionOwner: () => context.window.AnalysisContexts.capture(),
    selectionCurrent: captured => context.window.AnalysisContexts.isCurrent(captured), scope: () => 'dataset', station: () => null });
  context.saveCases = save; context.nativeEvidenceServices = () => ({ actions, session });
  if (debounced) vm.runInContext(app.slice(app.indexOf('let casesSaveQueue ='), app.indexOf('function defaultCaseWorkspace(')), context);
  vm.runInContext(app.slice(app.indexOf('function recordFocusAvailable('), app.indexOf('function resolveRecordFocus(')), context);
  vm.runInContext(app.slice(app.indexOf('let lastCaseRemoval ='), app.indexOf('async function removeEventFromCase(')), context);
  const advance = ms => {
    const end = clock + ms;
    while ([...timers.values()].some(timer => timer.at <= end)) {
      const [id, timer] = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
      clock = timer.at; timers.delete(id); timer.fn();
    }
    clock = end;
  };
  const remove = (index = 0) => {
    const selected = activeCase(), item = selected.items[index], ref = item.rows.reference;
    return context.removeCaseOccurrences(selected, [{ item, member: { containerId: ref.containerId, manifestId: ref.manifestId, occurrenceId: 'occurrence-1' } }]);
  };
  return { c, store, state, context, actions, calls, messages, area, listeners, timers, dispatch, advance, remove, document, pageSize, caseSelect, otherControl,
    get receipt() { return vm.runInContext('lastCaseRemoval?.receipt', context); },
    get button() { return area.children.at(-1)?.children[0]; },
    prepareGate(value) { prepareGate = value; }, saveGate(value) { saveGate = value; },
    failSave(value = true) { failSave = value; }, prepareError(value) { prepareError = value; },
    status(value) { status = value; dispatch('case-evidence-state'); },
  };
}

test('confirmed native removal renders a distinct actionable receipt only after save, preserving exact occurrence identity', async () => {
  const f = fixture(), gate = deferred(), untouched = f.c.items[1].rows, before = plain(f.c.items[0].rows.reference);
  f.saveGate(gate); const removing = f.remove(); await drain();
  assert.equal(f.area.children.length, 0); assert.equal(f.messages.some(message => message.type === 'ok'), false);
  gate.resolve(); assert.equal(await removing, true); f.advance(0);
  assert.equal(f.area.children.length, 1); const notice = f.area.children[0], button = f.button;
  assert.equal(notice.className, 'toast ok native-removal-notice'); assert.equal(notice.textContent, '1 ocorrência(s) removida(s). ');
  assert.equal(notice.attributes.role, 'status'); assert.equal(button.tagName, 'BUTTON'); assert.equal(button.type, 'button');
  assert.equal(button.textContent, 'Desfazer'); assert.equal(button.disabled, false); assert.equal(notice.parentElement, f.area);
  assert.equal(f.c.items[1].rows, untouched);
  assert.deepEqual(f.calls[0].request.action.members, [{ containerId: 'a', manifestId: before.manifestId, occurrenceId: 'occurrence-1' }]);
  await button.onclick();
  assert.equal(f.area.children.length, 0); assert.equal(f.c.items[0].rows.preservedCount, 2);
  assert.deepEqual(f.calls[2].request.action, { kind: 'restore', target: before }); assert.equal(f.c.items[1].rows, untouched);
  assert.deepEqual(f.messages.filter(message => message.type === 'ok'), [{ text: 'Remoção desfeita.', type: 'ok' }]);
});

test('repeated shortcut and menu clicks do not duplicate an in-flight native restore or claim early success', async () => {
  const f = fixture(); await f.remove(); const button = f.button, notice = f.area.children[0], gate = deferred();
  f.saveGate(gate); const undoing = button.onclick(); await drain();
  assert.equal(button.disabled, true); assert.equal(button.attributes['aria-busy'], 'true'); assert.equal(notice.isConnected, true);
  await button.onclick(); assert.equal(await f.context.undoCaseOccurrenceRemoval(), false);
  assert.equal(f.calls.filter(call => call.request?.action.kind === 'restore').length, 1);
  assert.equal(f.messages.some(message => message.type === 'ok'), false);
  gate.resolve(); await undoing; assert.equal(notice.isConnected, false); assert.equal(f.c.items[0].rows.preservedCount, 2);
});

test('older receipt buttons remain distinct and cannot undo a newer removal', async () => {
  const f = fixture(); await f.remove(0); const oldNotice = f.area.children[0], oldButton = f.button, oldReceipt = f.receipt;
  await f.remove(1); assert.equal(f.area.children.length, 2); assert.equal(f.area.children[0], oldNotice);
  assert.notEqual(oldReceipt, f.receipt); assert.equal(oldButton.disabled, true); assert.equal(f.button.disabled, false);
  const calls = f.calls.length; await oldButton.onclick(); assert.equal(f.calls.length, calls);
  await f.button.onclick(); assert.equal(f.c.items[0].rows.preservedCount, 1); assert.equal(f.c.items[1].rows.preservedCount, 2);
  assert.equal(oldNotice.isConnected, true); assert.equal(oldButton.disabled, true);
});

test('Case/store/manifest authority is checked again at activation without rebinding the receipt', async () => {
  for (const change of ['case', 'store', 'epoch', 'manifest', 'protected']) {
    const f = fixture(); await f.remove(); const button = f.button, calls = f.calls.length;
    if (change === 'case') f.store.active = 'other';
    if (change === 'store') f.state.cases = structuredClone(f.store);
    if (change === 'epoch') f.store.store.epoch = 'replacement-epoch';
    if (change === 'manifest') f.c.items[0].rows.reference.manifestId = 'replacement-manifest';
    if (change === 'protected') f.store.caseEvidence[0].state = 'unavailable';
    await button.onclick(); assert.equal(f.calls.length, calls, change); assert.equal(button.disabled, true, change);
    assert.equal(f.messages.some(message => message.type === 'ok'), false, change);
  }
});

test('Case return refreshes eligibility, while source/config changes retain authoritative immutable-history Undo', async () => {
  const f = fixture(); await f.remove(); const button = f.button;
  f.store.active = 'other'; f.dispatch('workspace-context-change'); f.advance(0); assert.equal(button.disabled, true);
  f.store.active = f.c.id; f.dispatch('workspace-context-change'); f.advance(0); assert.equal(button.disabled, false);
  f.state.sourcePublication.generation++; f.state.currentArtifact = { id: 'different-source', loadedAt: 99 };
  f.c.analysisContext.configRevision++; f.c.analysisContext.visibilityRevision++;
  f.dispatch('analysis-context-change'); f.advance(0);
  assert.equal(f.actions.canUndo(f.receipt), true); assert.equal(button.disabled, false);
  await button.onclick(); assert.equal(f.c.items[0].rows.preservedCount, 2);
});

test('lost removal acknowledgement publishes no receipt and leaves the native pending draft untouched', async () => {
  const f = fixture(); f.failSave(); assert.equal(await f.remove(), false);
  assert.equal(f.area.children.length, 0); assert.equal(f.receipt, undefined);
  assert.equal(f.c.items[0].rows.reference.kind, 'pending_native_evidence'); assert.equal(f.c.items[0].rows.preservedCount, 1);
  assert.equal(f.messages.some(message => message.type === 'ok'), false); assert.match(f.messages.at(-1).text, /SAVE_RETRY_REQUIRED/);
});

test('lost Undo acknowledgement keeps the exact pending draft and disables the shortcut without rollback or success', async () => {
  const f = fixture(); await f.remove(); const button = f.button, notice = f.area.children[0]; f.failSave();
  await button.onclick(); f.advance(0);
  assert.equal(notice.isConnected, true); assert.equal(button.disabled, true); assert.equal(button.attributes['aria-busy'], undefined);
  assert.equal(f.c.items[0].rows.reference.kind, 'pending_native_evidence'); assert.equal(f.c.items[0].rows.preservedCount, 2);
  assert.equal(f.messages.some(message => message.type === 'ok'), false); const calls = f.calls.length;
  await button.onclick(); assert.equal(f.calls.length, calls);
});

test('expired preparation errors preserve the receipt and do not fabricate a restore or success', async () => {
  const f = fixture(); await f.remove(); const before = plain(f.c.items[0].rows), notice = f.area.children[0];
  f.prepareError('EVIDENCE_PREPARATION_EXPIRED: Prepare novamente.'); await f.button.onclick();
  assert.deepEqual(plain(f.c.items[0].rows), before); assert.equal(notice.isConnected, true);
  assert.equal(f.messages.some(message => message.type === 'ok'), false); assert.match(f.messages.at(-1).text, /EXPIRED/);
  f.prepareError(null); await f.button.onclick(); assert.equal(notice.isConnected, false); assert.equal(f.c.items[0].rows.preservedCount, 2);
});

test('shortcut expiry clears listeners only; native receipt and existing menu Undo remain valid', async () => {
  const f = fixture(); await f.remove(); const receipt = f.receipt, button = f.button;
  f.advance(14999); assert.equal(f.area.children.length, 1);
  f.advance(1); assert.equal(f.area.children.length, 0); assert.equal(f.timers.size, 0);
  assert.equal([...f.listeners.values()].reduce((n, list) => n + list.size, 0), 0);
  assert.equal(f.actions.canUndo(receipt), true); const calls = f.calls.length; await button.onclick(); assert.equal(f.calls.length, calls);
  assert.equal(await f.context.undoCaseOccurrenceRemoval(), true); assert.equal(f.c.items[0].rows.preservedCount, 2);
});

test('menu Undo consumes and removes its shortcut; state saves briefly disable it and release normally', async () => {
  const f = fixture(); await f.remove(); const button = f.button;
  f.status('saving'); f.advance(0); assert.equal(button.disabled, true);
  f.status('ready'); f.advance(0); assert.equal(button.disabled, false);
  assert.equal(await f.context.undoCaseOccurrenceRemoval(), true); assert.equal(f.area.children.length, 0);
  const calls = f.calls.length; await button.onclick(); assert.equal(f.calls.length, calls);
});

test('protected unavailable Cases never produce a removal receipt', async () => {
  const f = fixture(); f.c.kind = 'preserved_case_unavailable';
  assert.equal(await f.remove(), false); assert.equal(f.calls.length, 0); assert.equal(f.area.children.length, 0);
  assert.equal(f.messages.some(message => message.type === 'ok'), false);
});

test('Case navigation during preparation or save never makes a stale receipt actionable in the new Case', async () => {
  for (const phase of ['prepare', 'save']) {
    const f = fixture(), gate = deferred(), before = plain(f.c.items[0].rows);
    if (phase === 'prepare') f.prepareGate(gate); else f.saveGate(gate);
    const removing = f.remove(); await drain(); f.store.active = 'other'; gate.resolve();
    assert.equal(await removing, phase === 'save');
    assert.equal(f.area.children.length, 0);
    if (phase === 'prepare') assert.deepEqual(plain(f.c.items[0].rows), before);
    else { assert.equal(f.c.items[0].rows.preservedCount, 1); assert.equal(f.actions.canUndo(f.receipt), false); }
  }
});

test('a detached shortcut tears down its listeners without changing receipt lifetime or native state', async () => {
  const f = fixture(); await f.remove(); const receipt = f.receipt, notice = f.area.children[0], calls = f.calls.length;
  notice.remove(); f.dispatch('case-evidence-state'); f.advance(0);
  assert.equal([...f.listeners.values()].reduce((n, list) => n + list.size, 0), 0); assert.equal(f.timers.size, 0);
  assert.equal(f.calls.length, calls); assert.equal(f.actions.canUndo(receipt), true);
});

test('real save debounce preserves a newer Case receipt when an earlier Case Undo completes after the shared save', async () => {
  const f = fixture({ debounced: true }), first = f.remove(); await drain();
  assert.equal(f.area.children.length, 0); f.advance(200); await first;
  const aNotice = f.area.children[0], aButton = f.button;
  const undoA = aButton.onclick(); await drain();
  assert.equal(f.c.items[0].rows.reference.kind, 'pending_native_evidence');
  const b = structuredClone(f.c), bOwner = { storeId: 'store', caseId: 'case-b', analysisId: 'analysis-b' };
  b.id = bOwner.caseId; b.analysisContext = { ...bOwner, configRevision: 1, visibilityRevision: 1 };
  b.items = [{ id: 'B', rows: { kind: 'native_evidence_container', preservedCount: 2, preview: null,
    reference: { kind: 'native_evidence', schemaVersion: 1, owner: bOwner, containerId: 'b-only', manifestId: 'manifest-b-only', manifestSha256: 'a'.repeat(64), memberCount: 2 } } }];
  f.store.cases.push(b); f.store.caseEvidence.push({ state: 'ready', owner: bOwner }); f.store.active = b.id;
  const removingB = f.remove(); await drain(); f.advance(200); await Promise.all([undoA, removingB]); f.advance(0);
  const bReceipt = f.receipt, bButton = f.button;
  assert.equal(f.area.children.length, 1); assert.equal(aNotice.isConnected, false);
  assert.equal(f.actions.canUndo(bReceipt), true); assert.equal(bButton.disabled, false);
  assert.equal(vm.runInContext('lastCaseRemoval.c.id', f.context), b.id);
  const menuCanUndo = app.match(/canUndoRemoval: c => (.+),\n  undoRemoval:/)[1];
  f.context.menuCase = b; assert.equal(vm.runInContext(`(c => ${menuCanUndo})(menuCase)`, f.context), true);
  const undoB = bButton.onclick(); await drain(); f.advance(200); await undoB;
  assert.equal(b.items[0].rows.preservedCount, 2); assert.equal(f.c.items[0].rows.preservedCount, 2);
  assert.equal(f.calls.filter(call => call.request?.action.kind === 'restore').length, 2);
});

test('keyboard Undo restores lost focus to a stable visible control with either native disable-focus behavior', async () => {
  for (const blurOnDisable of [true, false]) {
    const f = fixture({ blurOnDisable }); await f.remove(); const button = f.button; button.focus();
    await button.onclick(); assert.equal(f.document.activeElement, f.pageSize);
    assert.equal(f.area.children.length, 0); assert.equal(f.document.activeElement.isConnected, true);
  }
});

test('recoverable Undo failure reenables its action without leaving keyboard focus on body', async () => {
  for (const blurOnDisable of [true, false]) {
    const f = fixture({ blurOnDisable }); await f.remove(); const button = f.button; button.focus();
    f.prepareError('EVIDENCE_PREPARATION_EXPIRED'); await button.onclick();
    assert.equal(button.disabled, false); assert.equal(f.area.children.length, 1);
    assert.equal(f.document.activeElement, blurOnDisable ? f.pageSize : button);
  }
});

test('Undo completion does not take focus back after the user moved elsewhere during the save', async () => {
  const f = fixture(); await f.remove(); const gate = deferred(), button = f.button; f.saveGate(gate); button.focus();
  const undo = button.onclick(); await drain(); f.otherControl.focus(); gate.resolve(); await undo;
  assert.equal(f.document.activeElement, f.otherControl);
});

test('focused expiry and context invalidation use stable current-context fallbacks, never stale records', async () => {
  for (const mode of ['expiry', 'new-case-expiry', 'new-case-event', 'hidden-page-control']) {
    const f = fixture(); await f.remove(); f.button.focus();
    if (mode.startsWith('new-case')) f.store.active = 'other';
    if (mode === 'hidden-page-control') f.pageSize.hidden = true;
    if (mode === 'new-case-event') { f.dispatch('workspace-context-change'); f.advance(0); }
    else f.advance(15000);
    assert.equal(f.document.activeElement, mode === 'expiry' ? f.pageSize : f.caseSelect, mode);
  }
  const f = fixture(); await f.remove(); f.otherControl.focus(); f.advance(15000);
  assert.equal(f.document.activeElement, f.otherControl, 'expiry does not steal unrelated focus');
});
