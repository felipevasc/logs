import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const section = (start, end) => app.slice(app.indexOf(start), app.indexOf(end, app.indexOf(start)));
const plain = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const A = 'C:\\Caso A\\servidor\\same.log', B = 'D:\\Evidências\\pacote.zip!/área <literal>/same.log';
const configured = { timezone_offset_minutes: -180, clock_adjustment_ms: 1234, sources: ['message', 'arquivo'], rules: [{ regex: 'SOURCE_A_(.*)', template: '$1' }], format: '%Y CUSTOM %m', complement: '2001-01-01' };
const known = { timezone_offset_minutes: 0, clock_adjustment_ms: 2000, sources: ['linha'], rules: [], format: 'epoch_ms', complement: null };
const empty = { timezone_offset_minutes: null, clock_adjustment_ms: 0, sources: [], rules: [], format: '%Y-%m-%d %H:%M:%S%.f', complement: null };

function fixture() {
  let document;
  class Node {
    constructor(tag = 'div', cls = '', text = '') { Object.assign(this, { tagName: tag.toUpperCase(), className: cls, textContent: text, children: [], attrs: {}, value: '', hidden: false, disabled: false }); }
    get isConnected() { return this === document.body || !!this.parentElement?.isConnected; }
    get classList() { return { add() {}, remove() {}, toggle() {} }; }
    set innerHTML(value) { this.replaceChildren(); this.textContent = value; }
    get innerHTML() { return this.textContent; }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key] ?? null; }
    remove() { if (this.contains(document.activeElement)) document.activeElement = document.body; if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this); this.parentElement = null; }
    append(...children) { for (const child of children) this.appendChild(child); }
    appendChild(child) { child.remove(); child.parentElement = this; this.children.push(child); return child; }
    replaceChildren(...children) { for (const child of [...this.children]) child.remove(); this.append(...children); }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    matches(selector) { return selector.split(',').some(item => {
      item = item.trim(); const parts = item.split(/\s+/), last = parts.pop();
      if (parts.length) { if (!this.matches(last)) return false; let parent = this.parentElement; while (parent && !parent.matches(parts.join(' '))) parent = parent.parentElement; return !!parent; }
      if (item === '[hidden]') return !!this.hidden;
      if (item === '[inert]') return !!this.inert;
      if (item === '[aria-hidden="true"]') return this.attrs['aria-hidden'] === 'true';
      if (item.startsWith('#')) return this.id === item.slice(1);
      if (item.startsWith('.')) return this.className.split(' ').includes(item.slice(1));
      return this.tagName.toLowerCase() === item;
    }); }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    getClientRects() { return this.isConnected && !this.closest('[hidden],[inert],[aria-hidden="true"]') ? [{}] : []; }
    focus() { if (!this.disabled && !this.closest('[hidden]')) document.activeElement = this; }
  }
  document = { createElement: tag => new Node(tag) };
  document.body = new Node('body'); document.activeElement = document.body;
  document.querySelector = selector => document.body.querySelector(selector);
  document.querySelectorAll = selector => document.body.querySelectorAll(selector);
  const node = (tag, id, parent = document.body, cls = '') => { const value = new Node(tag, cls); value.id = id; parent.append(value); return value; };
  const modal = node('div', 'ts-modal'), body = node('div', '', modal, 'modal-body'); modal.hidden = true;
  for (const id of ['ts-close', 'ts-help-btn']) node('button', id, modal);
  for (const id of ['ts-zone', 'ts-format']) node('select', id, body);
  for (const id of ['ts-clock', 'ts-regex', 'ts-template', 'ts-format-custom', 'ts-complement']) node('input', id, body);
  for (const id of ['ts-sources', 'ts-rules', 'ts-assembled', 'ts-help', 'ts-example', 'ts-test-result', 'ts-status']) node('div', id, body);
  for (const id of ['ts-retry', 'ts-add-rule', 'ts-test', 'ts-reset', 'ts-apply']) node('button', id, body);
  const trigger = node('button', 'trigger'), outside = node('button', 'outside'); node('input', 'file-path').value = `${A};${B}`;
  const state = { tsSources: [], columns: ['message'], loaded: true, rows: [], cases: { active: 'case-a' }, sourcePublication: { generation: 1 }, currentArtifact: { id: 'bundle', path: A, source: { kind: 'file', paths: [A, B] } } };
  const requests = [], messages = [], overlays = [], invalidated = [];
  const helper = { capture: () => ({ caseId: state.cases.active, generation: state.sourcePublication.generation, artifact: state.currentArtifact.id }),
    isCurrent: owner => owner.caseId === state.cases.active && owner.generation === state.sourcePublication.generation && owner.artifact === state.currentArtifact.id,
    assertOwner: owner => { if (!helper.isCurrent(owner)) throw Error('ANALYSIS_CONTEXT_CHANGED'); } };
  const context = vm.createContext({ state, document, $: document.querySelector, window: { AnalysisContexts: helper },
    el: (tag, cls, text) => new Node(tag, cls, text), colLabel: String, esc: String, currentSource: () => 'file', cellValue: (event, key) => event.fields?.[key] || '',
    api: (command, args, options) => { const pending = deferred(); requests.push({ command, args: plain(args), options, ...pending }); return pending.promise; },
    toast: (text, type) => messages.push({ text, type }), invalidateAnalysisComputedData: value => invalidated.push(value), refresh() {},
    btnBusy: button => { button.disabled = true; return () => { button.disabled = false; }; },
    showLoadOverlay: (...args) => { state.loadOverlayVersion = (state.loadOverlayVersion || 0) + 1; state.loadOverlayProgressKey = args[1]; overlays.push(args); }, hideLoadOverlay: value => overlays.push(value),
  });
  vm.runInContext(section('// ------------------------------------------------------------------ data/hora', '// ------------------------------------------------------------------ campo derivado'), context);
  vm.runInContext(section('async function openTsModal(', 'function setColumnVisible('), context);
  context.fillTsFormats();
  const $ = document.querySelector;
  const read = async (path, config = null) => { trigger.focus(); const work = context.openTsModal(path); requests.at(-1).resolve(config); assert.equal(await work, true); return work; };
  const config = () => plain(context.buildTsConfig());
  return { context, document, $, state, requests, messages, overlays, invalidated, read, config, helper, trigger, outside };
}

test('configured source to empty source replaces all controls, optional values and prior results', async () => {
  const f = fixture(); await f.read(A, configured); assert.deepEqual(f.config(), configured);
  f.$('#ts-test-result').innerHTML = 'A test result'; f.$('#ts-template').value = 'old template'; f.$('#ts-help').hidden = false;
  const pending = f.context.openTsModal(B);
  assert.deepEqual(f.config(), empty); assert.equal(f.$('#ts-help').hidden, true);
  for (const id of ['ts-apply', 'ts-test', 'ts-reset', 'ts-format', 'ts-zone']) assert.equal(f.$(`#${id}`).disabled, true);
  assert.match(f.$('#ts-status').textContent, /Carregando/); assert.ok(f.$('#ts-status').textContent.includes(B));
  await f.context.applyTsConfig(); await f.context.testTsConfig(); await f.context.resetTsConfig(); assert.equal(f.requests.length, 2);
  f.requests.at(-1).resolve(null); await pending;
  assert.deepEqual(f.config(), empty); assert.equal(f.$('#ts-rules').children.length, 1);
  assert.equal(f.$('#ts-format-custom').value, ''); assert.equal(f.$('#ts-format-custom').hidden, true);
  assert.equal(f.$('#ts-test-result').innerHTML, ''); assert.equal(f.$('#ts-template').value, '');
});

test('empty to configured and custom to known hide and clear the custom field', async () => {
  const f = fixture(); await f.read(B); await f.read(A, configured); assert.deepEqual(f.config(), configured);
  await f.read(B, known); assert.deepEqual(f.config(), known);
  assert.equal(f.$('#ts-format-custom').hidden, true); assert.equal(f.$('#ts-format-custom').value, '');
  const legacy = { ...known, rules: undefined, regex: '(legacy)', template: '$1' };
  await f.read(A, legacy); assert.deepEqual(f.config().rules, [{ regex: '(legacy)', template: '$1' }]);
});

test('overlapping A/B loads reject the late A response, with no late focus movement', async () => {
  const f = fixture(), first = f.context.openTsModal(A), second = f.context.openTsModal(B);
  f.requests[1].resolve(known); await second; f.outside.focus();
  f.requests[0].resolve(configured); assert.equal(await first, false);
  assert.deepEqual(f.config(), known); assert.equal(f.state.tsEditingPath, B); assert.equal(f.document.activeElement, f.outside);
});

test('close/reopen discards unsaved input and blocks closed or superseded pending replies', async () => {
  const f = fixture(); await f.read(A, configured); f.$('#ts-complement').value = 'unsaved';
  f.$('#ts-close').focus(); f.context.closeTsModal(); assert.equal(f.document.activeElement, f.trigger); assert.equal(f.state.tsEditingPath, null);
  await f.read(A, configured); assert.deepEqual(f.config(), configured);
  const pending = f.context.openTsModal(B); f.context.closeTsModal(); f.outside.focus();
  f.requests.at(-1).resolve(known); assert.equal(await pending, false); assert.equal(f.$('#ts-modal').hidden, true); assert.equal(f.document.activeElement, f.outside);
});

test('background load reads never overwrite a ready or pending editor and read failures stay auxiliary', async () => {
  const f = fixture(); await f.read(A, configured);
  const background = f.context.loadTsConfig(B); f.requests.at(-1).resolve(null); await background; assert.deepEqual(f.config(), configured);
  const open = f.context.openTsModal(B), read = f.requests.at(-1), failed = f.context.loadTsConfig(A); f.requests.at(-1).reject(Error('unreadable'));
  assert.match(String((await failed).error), /unreadable/); assert.equal(f.state.loaded, true); assert.equal(f.$('#ts-apply').disabled, true);
  read.resolve(known); await open; assert.deepEqual(f.config(), known);
});

test('read failure shows a disabled clean form, focuses Retry and permits the exact same read', async () => {
  const f = fixture(); await f.read(A, configured); const pending = f.context.openTsModal(B);
  f.requests.at(-1).reject(Error('read refused')); assert.equal(await pending, false);
  assert.deepEqual(f.config(), empty); assert.equal(f.$('#ts-apply').disabled, true); assert.equal(f.document.activeElement, f.$('#ts-retry'));
  assert.match(f.$('#ts-status').textContent, /read refused/); await f.context.applyTsConfig(); assert.equal(f.requests.length, 2);
  const retry = f.$('#ts-retry').onclick(); assert.equal(f.requests.at(-1).args.path, B); f.requests.at(-1).resolve(known); await retry;
  assert.deepEqual(f.config(), known); assert.equal(f.$('#ts-retry').hidden, true); assert.equal(f.$('#ts-apply').disabled, false);
});

test('owner/caller changes during a read discard it and close quietly without stealing focus', async () => {
  for (const change of ['case', 'publication', 'artifact', 'caller']) {
    const f = fixture(); let current = true; const pending = f.context.openTsModal(A, () => current);
    if (change === 'case') f.state.cases.active = 'case-b';
    if (change === 'publication') f.state.sourcePublication.generation++;
    if (change === 'artifact') f.state.currentArtifact.id = 'other';
    if (change === 'caller') current = false;
    f.outside.focus(); f.requests[0].resolve(configured); assert.equal(await pending, false, change);
    assert.equal(f.$('#ts-modal').hidden, true); assert.equal(f.document.activeElement, f.outside);
  }
});

test('owner changes after open block Apply, Test and Reset before any native request', async () => {
  const f = fixture(); await f.read(A, configured); f.state.cases.active = 'case-b';
  await f.context.applyTsConfig(); await f.context.testTsConfig(); await f.context.resetTsConfig();
  assert.equal(f.requests.length, 1); assert.deepEqual(f.config(), configured); assert.ok(f.messages.every(message => message.type === 'info'));
});

test('individual/general/individual opens capture exact paths and preserve publication receipt sequencing', async () => {
  const f = fixture();
  for (const [path, paths] of [[B, [B]], [null, [A, B]], [A, [A]]]) {
    await f.read(path, configured); const pending = f.context.applyTsConfig();
    for (const expected of paths) {
      const request = f.requests.at(-1); assert.equal(request.command, 'set_ts_config'); assert.equal(request.args.path, expected);
      assert.deepEqual(request.args.config, configured); assert.equal(request.options.analysisOwner.generation, f.state.sourcePublication.generation);
      request.resolve({ publication: { generation: f.state.sourcePublication.generation + 1 } }); await new Promise(resolve => setImmediate(resolve));
    }
    await pending; assert.equal(f.$('#ts-apply').disabled, false); assert.equal(f.state.tsAnalysisOwner.generation, f.state.sourcePublication.generation);
    f.context.closeTsModal();
  }
  assert.equal(f.invalidated.length, 4);
});

test('Apply after empty-source selection sends only defaults for that exact source', async () => {
  const f = fixture(); await f.read(A, configured); await f.read(B); f.state.tsSources.push('message');
  const work = f.context.applyTsConfig(), write = f.requests.at(-1);
  assert.deepEqual(write.args, { path: B, config: { ...empty, sources: ['message'] } }); write.resolve({}); await work;
});

test('an older Apply completion cannot adopt its owner or enable a newer editor', async () => {
  const f = fixture(); await f.read(A, configured);
  const applying = f.context.applyTsConfig(), write = f.requests.at(-1);
  const opening = f.context.openTsModal(B), read = f.requests.at(-1), ownerB = f.state.tsAnalysisOwner;
  write.resolve({ publication: { generation: 2 } }); await applying;
  assert.equal(f.state.tsAnalysisOwner, ownerB); assert.equal(f.$('#ts-apply').disabled, true);
  f.outside.focus(); read.resolve(known); assert.equal(await opening, false);
  assert.equal(f.$('#ts-modal').hidden, true); assert.equal(f.document.activeElement, f.outside);
});

test('close/reopen Apply rejects older foreground success and failure settlements', async () => {
  for (const outcome of ['success', 'failure']) {
    const f = fixture(); await f.read(A, configured);
    const first = f.context.applyTsConfig(), writeA = f.requests.at(-1);
    f.context.closeTsModal(); await f.read(B, known);
    const second = f.context.applyTsConfig(), writeB = f.requests.at(-1);
    if (outcome === 'success') writeA.resolve({}); else writeA.reject(Error('cancelled old write'));
    await first;
    assert.equal(f.overlays.length, 2, 'older operation cannot hide the newer foreground');
    assert.equal(f.messages.length, 0); assert.equal(f.$('#ts-apply').disabled, true);
    writeB.resolve({}); await second;
    assert.equal(f.overlays.at(-1), true); assert.equal(f.messages.length, 1); assert.equal(f.$('#ts-apply').disabled, false);
  }
});

test('closing an editor still permits settlement of its own foreground overlay', async () => {
  const f = fixture(); await f.read(A, configured);
  const work = f.context.applyTsConfig(), write = f.requests.at(-1); f.context.closeTsModal();
  write.resolve({ publication: { generation: 2 } }); await work;
  assert.equal(f.overlays.at(-1), true); assert.equal(f.state.sourcePublication.generation, 2); assert.equal(f.$('#ts-modal').hidden, true);
});

test('Apply/Reset invalidate pending Test results on success and error', async () => {
  for (const action of ['applyTsConfig', 'resetTsConfig']) for (const outcome of ['success', 'failure']) {
    const f = fixture(); await f.read(A, configured);
    const testing = f.context.testTsConfig(), result = f.requests.at(-1);
    const writing = f.context[action](); f.requests.at(-1).resolve({}); await writing;
    if (outcome === 'success') result.resolve([['old input', 'old match']]); else result.reject(Error('ANALYSIS_CONTEXT_CHANGED'));
    await testing; assert.equal(f.$('#ts-test-result').innerHTML, '');
  }
});

test('editing a source, rule, format or optional control during Test rejects its old response', async () => {
  for (const edit of ['source', 'rule', 'format', 'custom', 'zone', 'clock', 'complement']) {
    const f = fixture(); await f.read(A, configured);
    const work = f.context.testTsConfig(), pending = f.requests.at(-1);
    if (edit === 'source') f.state.tsSources.push('linha');
    if (edit === 'rule') f.$('#ts-rules .dv-rule-pattern').value = 'new rule';
    if (edit === 'format') f.$('#ts-format').value = 'epoch_ms';
    if (edit === 'custom') f.$('#ts-format-custom').value = '%H:%M';
    if (edit === 'zone') f.$('#ts-zone').value = '0';
    if (edit === 'clock') f.$('#ts-clock').value = '7';
    if (edit === 'complement') f.$('#ts-complement').value = '2026-01-01';
    pending.resolve([['old input', 'old match']]); await work; assert.equal(f.$('#ts-test-result').innerHTML, '', edit);
  }
});

test('editing handlers clear rendered tests and pending tokens for source/rule/input controls', async () => {
  const f = fixture(); await f.read(A, configured);
  for (const edit of [() => f.$('#ts-sources button').onclick(), () => f.context.tsAddRule(), () => f.$('#ts-rules .dv-rule-del').onclick()]) {
    f.$('#ts-test-result').innerHTML = 'old result'; edit(); assert.equal(f.$('#ts-test-result').innerHTML, '');
  }
  assert.match(app, /\$\("#ts-modal"\)\.addEventListener\("input", clearTsTestResult\)/);
  assert.match(app, /\$\("#ts-modal"\)\.addEventListener\("change", clearTsTestResult\)/);
});

test('write errors preserve the editable draft; Reset clears every value only after success', async () => {
  const f = fixture(); await f.read(A, configured);
  for (const action of ['applyTsConfig', 'resetTsConfig']) {
    const work = f.context[action](); f.requests.at(-1).reject(Error('write failed')); await work;
    assert.deepEqual(f.config(), configured); assert.equal(f.$('#ts-apply').disabled, false); assert.equal(f.messages.at(-1).type, 'err');
  }
  const work = f.context.resetTsConfig(); assert.deepEqual(f.requests.at(-1).args, { path: A, config: null }); f.requests.at(-1).resolve({}); await work;
  assert.deepEqual(f.config(), empty); assert.equal(f.$('#ts-format-custom').value, ''); assert.equal(f.$('#ts-format-custom').hidden, true);
});

test('late Test and event-detail results cannot appear in a newer source editor', async () => {
  const f = fixture(); await f.read(A, configured);
  const testing = f.context.testTsConfig(), result = f.requests.at(-1);
  assert.equal(result.args.path, A); assert.deepEqual(result.args.config, configured);
  f.state.rows = [{ id: 1 }]; const example = f.context.updateTsExample(), detail = f.requests.at(-1);
  f.state.rows = []; await f.read(B, known);
  result.resolve([['SOURCE A', 'old match']]); detail.resolve({ fields: { caminho: A, message: 'SOURCE_A_old' } }); await Promise.all([testing, example]);
  assert.equal(f.$('#ts-test-result').innerHTML, ''); assert.equal(vm.runInContext('tsExampleEv', f.context), null);
});

test('an example from a different file in the bundle is not shown for the individual editor', async () => {
  const f = fixture(); await f.read(B, known); f.state.rows = [{ id: 1 }];
  const pending = f.context.updateTsExample(); f.requests.at(-1).resolve({ fields: { caminho: A, message: 'other source' } }); await pending;
  assert.equal(vm.runInContext('tsExampleEv', f.context), null);
});

test('close does not focus a detached caller or displace deliberate outside focus', async () => {
  const f = fixture(); await f.read(A); f.trigger.remove(); f.$('#ts-close').focus(); f.context.closeTsModal(); assert.notEqual(f.document.activeElement, f.trigger);
  await f.read(A); f.outside.focus(); f.context.closeTsModal(); assert.equal(f.document.activeElement, f.outside);
});
