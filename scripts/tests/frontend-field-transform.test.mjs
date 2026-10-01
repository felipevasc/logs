import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../../frontend/field-transform.js', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const settle = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const supportedSteps = ['base64_decode', 'base64_url_decode', 'base64_encode', 'base64_url_encode', 'url_decode', 'url_encode',
  'form_decode', 'hex_to_text', 'text_to_hex', 'parse_json', 'parse_xml', 'parse_query', 'jwt_payload'];
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture({ definitions = [], event = { id: 7, message: 'original message', fields: { payload: 'SGVsbG8=' } }, scope = 'dataset' } = {}) {
  const nodes = new Map(), calls = [], order = [], cancelled = [], messages = [], reloadOwners = [];
  const document = { activeElement: null };
  const state = { columns: ['id', 'timestamp', 'message', 'payload', ...definitions.map(item => item.name)],
    visibleCols: ['message', 'payload'], derivedFields: structuredClone(definitions), rows: [event],
    datasetProfiles: null, caseTreeProfiles: {}, caseProfilesLoading: false,
    analysis: { caseId: 'case-a', analysisId: 'analysis-a', configRevision: 2, visibilityRevision: 3 },
    generation: 10, instance: 1, active: 'case-a', currentArtifact: { id: 'source-one', loadedAt: 100 } };
  const nativeDefinitions = new Map(definitions.map(item => [item.name, structuredClone(item)]));
  let nativeHandler = null, reloadHandler = null, refreshHandler = null, created = 0;
  function node(tag = 'div', className = '', textContent = '') {
    const classes = new Set(className.split(/\s+/).filter(Boolean));
    const item = { tag, className, textContent, value: '', hidden: false, disabled: false, isConnected: true,
      children: [], attrs: {}, listeners: {}, parent: null,
      classList: { add: name => classes.add(name), remove: name => classes.delete(name), contains: name => classes.has(name),
        toggle(name, on) { if (on ?? !classes.has(name)) classes.add(name); else classes.delete(name); } },
      setAttribute(name, value) { this.attrs[name] = String(value); }, getAttribute(name) { return this.attrs[name]; },
      append(...items) { for (const child of items) { child.parent = this; this.children.push(child); if (this.tag === 'select' && this.children.length === 1) this.value = child.value; } },
      replaceChildren(...items) { this.children = []; this.append(...items); },
      querySelector(selector) { return nodes.get(selector) || null; },
      querySelectorAll() { return [...nodes.values()].filter(value => ['button', 'input', 'select', 'textarea'].includes(value.tag)); },
      addEventListener(type, callback) { (this.listeners[type] ||= []).push(callback); },
      closest(selector) { let current = this; while (current) { if (selector.includes('[hidden]') && current.hidden) return current; current = current.parent; } return null; },
      matches(selector) { return selector.split(',').some(part => part === this.tag
        || part === '[tabindex]' && Object.hasOwn(this.attrs, 'tabindex')
        || part === 'a[href]' && this.tag === 'a' && Object.hasOwn(this.attrs, 'href')); },
      focus() { document.activeElement = this; },
      set innerHTML(html) {
        this.children = [];
        for (const match of html.matchAll(/<(\w+)\b([^>]*\bid="([^"]+)"[^>]*)>/g)) {
          const child = node(match[1]); child.id = match[3]; child.hidden = /\bhidden\b/.test(match[2]);
          nodes.set(`#${child.id}`, child); this.append(child);
        }
      },
    };
    created++; return item;
  }
  document.body = node('body');
  const $ = selector => {
    if (!nodes.has(selector)) nodes.set(selector, node(selector === '#btn-colpicker' ? 'button' : selector === '#quick-search' ? 'input' : 'div'));
    return nodes.get(selector);
  };
  const capture = () => ({ caseId: state.active, instance: state.instance, identity: structuredClone(state.analysis),
    sourceGeneration: state.generation, sourceKey: JSON.stringify([state.generation, state.currentArtifact.id, state.currentArtifact.loadedAt]) });
  const assertOwner = (owner, { revisions = true } = {}) => {
    const current = capture();
    if (owner.caseId !== current.caseId || owner.instance !== current.instance || owner.identity.analysisId !== current.identity.analysisId
      || owner.sourceKey !== current.sourceKey || revisions && JSON.stringify(owner.identity) !== JSON.stringify(current.identity)) {
      throw Error('ANALYSIS_CONTEXT_CHANGED: O Caso, a configuração ou a fonte mudou.');
    }
  };
  function commit(args) {
    nativeDefinitions.set(args.name, structuredClone(args)); state.analysis.configRevision++;
    return { analysisContext: { schemaVersion: 1, ...structuredClone(state.analysis),
      config: { derivedFields: [...nativeDefinitions.values()], references: [] }, migrationDiagnostics: [], legacyRaw: null } };
  }
  const context = vm.createContext({
    state, structuredClone, TextEncoder, document, $,
    el: node, fmtNum: String, colLabel: String, toast: message => messages.push(message),
    activeCase: () => ({ id: state.active, name: 'Investigation A' }), workspaceScope: () => scope,
    caseEvents: () => [event], caseSig: () => 'case-evidence-v1',
    window: { AnalysisContexts: { capture, assertOwner }, Tasks: { cancelLatest: key => cancelled.push(key) } },
    api: async (cmd, args, opts) => {
      calls.push({ cmd, args, opts }); order.push(cmd);
      if (nativeHandler) return nativeHandler(cmd, args, opts);
      if (cmd === 'preview_field_transform') return { value: args.value, notices: [] };
      if (cmd === 'save_derived_field') return commit(args);
      if (cmd === 'profile_fields') return [...state.columns.map(name => ({ name })), ...[...nativeDefinitions.keys()].map(name => ({ name }))];
      throw Error(`Unexpected command: ${cmd}`);
    },
    loadDerivedFields: async owner => {
      order.push('loadDerivedFields'); reloadOwners.push(owner);
      assertOwner(owner);
      const allowed = reloadHandler ? await reloadHandler(owner) : true;
      if (allowed) state.derivedFields = [...nativeDefinitions.values()].map(item => structuredClone(item));
      return allowed;
    },
    fillColumnControls: () => order.push('fillColumnControls'), renderExploreTree: () => order.push('renderExploreTree'),
    refresh: async () => { order.push('refresh'); return refreshHandler ? refreshHandler() : true; },
  });
  vm.runInContext(source, context, { filename: 'field-transform.js' });
  const editor = context.window.FieldTransforms;
  const overlay = document.body.children.find(item => item.id === 'field-transform-modal');
  const field = name => nodes.get(`#ft-${name}`);
  const add = step => { field('step-choice').value = step; field('add-step').onclick(); };
  const row = index => field('steps').children[index];
  return { context, state, event, editor, overlay, field, add, row, calls, order, cancelled, messages, reloadOwners, document, $, capture, commit,
    setNative: handler => { nativeHandler = handler; }, setReload: handler => { reloadHandler = handler; },
    setRefresh: handler => { refreshHandler = handler; }, get nativeDefinitions() { return nativeDefinitions; } };
}

test('opening, typing and step editing never invoke a preview automatically', async () => {
  const f = fixture(); f.editor.open('payload');
  assert.deepEqual(f.field('step-choice').children.map(option => option.value), supportedSteps);
  f.field('sample').value = 'new sample'; f.field('sample').oninput();
  f.field('name').value = 'decoded_payload'; f.field('name').oninput();
  f.add('base64_decode'); f.add('parse_json');
  f.row(1).children[1].onclick(); f.row(0).children[3].onclick();
  await settle();
  assert.equal(f.calls.length, 0);
  assert.match(f.field('output').textContent, /Clique em Prévia/);
  await f.editor.preview();
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].cmd, 'preview_field_transform');
  assert.deepEqual(plain(f.calls[0].args), { value: 'new sample', steps: ['base64_decode'] });
  assert.equal(f.calls[0].opts.latest, 'field-transform-preview');
});

test('at most eight string steps can be added, reordered and removed without invoking native work', async () => {
  const f = fixture(); f.editor.open('payload');
  for (const step of supportedSteps.slice(0, 9)) f.add(step);
  assert.equal(f.field('step-count').textContent, '8 / 8');
  assert.equal(f.field('add-step').disabled, true);
  assert.equal(f.row(0).children[1].disabled, true);
  assert.equal(f.row(7).children[2].disabled, true);
  f.row(1).children[1].onclick();
  f.row(0).children[2].onclick();
  f.row(3).children[3].onclick();
  assert.equal(f.field('step-count').textContent, '7 / 8');
  assert.equal(f.field('add-step').disabled, false);
  f.add('jwt_payload');
  assert.equal(f.calls.length, 0);
  await f.editor.preview();
  assert.deepEqual(plain(f.calls[0].args.steps), [...supportedSteps.slice(0, 3), ...supportedSteps.slice(4, 8), 'jwt_payload']);
  assert.ok(f.calls[0].args.steps.every(step => typeof step === 'string'));
});

test('preview receives the entire underlying typed value and never mutates the original event', async () => {
  for (const value of [{ nested: { flags: [true, false], count: 4 }, blob: 'x'.repeat(12000) }, [1, 'two', false], 0, 42.5, true, false, null, 'x'.repeat(15000)]) {
    const event = { id: 7, fields: { payload: structuredClone(value) } }, before = structuredClone(event);
    const f = fixture({ event }); f.editor.open('payload', { event }); f.add('parse_json');
    await f.editor.preview();
    assert.deepEqual(plain(f.calls[0].args.value), value);
    assert.deepEqual(event, before);
    assert.ok(f.field('original').textContent.length < 4200);
    assert.ok(f.field('output').textContent.length < 8300);
    const expectedType = value == null ? 'Nulo' : Array.isArray(value) ? 'Lista' : ({ object: 'Objeto', string: 'Texto', number: 'Número', boolean: 'Booleano' })[typeof value];
    assert.equal(f.field('result-type').textContent, expectedType);
  }
});

test('bounded previews retain Unicode characters and leave the full native result untouched', async () => {
  const f = fixture(), fullValue = 'x'.repeat(8191) + '🚀' + 'tail'.repeat(4000);
  f.setNative(async () => ({ value: fullValue, notices: ['value_is_example'] }));
  f.editor.open('payload'); f.add('base64_decode'); await f.editor.preview();
  const shown = f.field('output').textContent;
  assert.ok(shown.length < 8300);
  assert.equal(shown.slice(0, 8191), 'x'.repeat(8191));
  assert.equal(shown.charCodeAt(8191), 10, 'truncation never leaves a lone high surrogate');
  assert.match(shown, /prévia de/);
  assert.equal(fullValue.length, 8191 + 2 + 16000);
  assert.equal(f.field('notices').textContent, 'value_is_example');
});

test('an explicitly edited empty sample remains a string even when the original value is null', async () => {
  const event = { id: 7, fields: { payload: null } }, f = fixture({ event });
  f.editor.open('payload', { event }); f.add('parse_json');
  assert.equal(f.field('sample').value, 'null');
  f.field('sample').value = ''; f.field('sample').oninput();
  await f.editor.preview();
  assert.equal(f.calls[0].args.value, '');
  assert.equal(f.field('result-type').textContent, 'Texto');
});

test('JWT signature warnings remain mandatory when native notices omit the warning', async () => {
  const f = fixture(); f.editor.open('payload'); f.add('jwt_payload');
  assert.equal(f.field('jwt-warning').hidden, false);
  f.setNative(async () => ({ value: { sub: 'example' }, notices: [] }));
  await f.editor.preview();
  assert.equal(f.field('jwt-warning').hidden, false);
  f.row(0).children[3].onclick();
  assert.equal(f.field('jwt-warning').hidden, true);
  f.add('parse_json');
  f.setNative(async () => ({ value: {}, notices: ['jwt_signature_not_verified', 'another_notice'] }));
  await f.editor.preview();
  assert.equal(f.field('jwt-warning').hidden, false);
  assert.equal(f.field('notices').textContent, 'another_notice');
});

test('UTF-8 input above 256 KiB is rejected before invoking native preview', async () => {
  const event = { id: 7, fields: { payload: '🚀'.repeat(70000) } }, before = event.fields.payload;
  const f = fixture({ event }); f.editor.open('payload'); f.add('base64_decode');
  assert.equal(f.field('sample').value, '');
  assert.match(f.field('sample-hint').textContent, /256 KiB/);
  f.field('sample').value = before; f.field('sample').oninput();
  await f.editor.preview();
  assert.equal(f.calls.length, 0);
  assert.match(f.field('status').textContent, /excede 256 KiB/);
  assert.equal(event.fields.payload, before);
});

test('editing during a pending preview rejects stale output and leaves Preview usable', async () => {
  for (const edit of ['sample', 'name', 'steps']) {
    const f = fixture(), pending = deferred();
    f.setNative(() => pending.promise); f.editor.open('payload'); f.add('base64_decode');
    const preview = f.editor.preview(); await settle();
    assert.equal(f.field('preview').disabled, true);
    if (edit === 'steps') f.add('parse_json');
    else { f.field(edit).value = 'edited draft'; f.field(edit).oninput(); }
    assert.equal(f.field('preview').disabled, false, edit);
    pending.resolve({ value: 'obsolete result', notices: [] }); await preview;
    assert.doesNotMatch(f.field('output').textContent, /obsolete result/, edit);
    assert.equal(f.field('preview').disabled, false, edit);
    assert.equal(f.calls.length, 1, 'editing does not itself start a replacement preview');
    f.setNative(async () => ({ value: 'fresh result', notices: [] }));
    await f.editor.preview();
    assert.equal(f.field('output').textContent, 'fresh result');
  }
});

test('a canceled preview cannot write into a subsequently opened editor', async () => {
  const f = fixture(), pending = deferred();
  f.setNative(() => pending.promise); f.editor.open('payload'); f.add('base64_decode');
  const preview = f.editor.preview(); await settle();
  f.field('cancel').onclick();
  assert.equal(f.overlay.hidden, true);
  f.editor.open('message');
  pending.resolve({ value: 'old field output', notices: [] }); await preview;
  assert.equal(f.field('name').value, 'message_transformado');
  assert.doesNotMatch(f.field('output').textContent, /old field output/);
  assert.equal(f.field('preview').disabled, false);
});

test('context changes reject pending preview results while preserving the open draft', async () => {
  for (const change of ['case', 'source', 'revision', 'instance']) {
    const f = fixture(), pending = deferred();
    f.setNative(() => pending.promise); f.editor.open('payload'); f.add('base64_decode');
    f.field('name').value = 'keep_draft';
    const preview = f.editor.preview(); await settle();
    if (change === 'case') f.state.active = 'case-b';
    if (change === 'source') f.state.generation++;
    if (change === 'revision') f.state.analysis.configRevision++;
    if (change === 'instance') f.state.instance++;
    pending.resolve({ value: 'stale output', notices: [] }); await preview;
    assert.equal(f.overlay.hidden, false, change);
    assert.equal(f.field('name').value, 'keep_draft');
    assert.doesNotMatch(f.field('output').textContent, /stale output/, change);
    assert.match(f.field('status').textContent, /ANALYSIS_CONTEXT_CHANGED/, change);
  }
});

test('editing a derived field preserves its regex-before-pipeline definition and captured Case owner', async () => {
  const definition = { name: 'decoded', source: 'payload', rules: [{ pattern: 'token=(.*)', group: 1, flags: 'i' }], steps: ['url_decode', 'base64_decode'] };
  const f = fixture({ definitions: [definition] }), original = structuredClone(definition), eventBefore = structuredClone(f.event);
  const initialOwner = f.capture();
  f.editor.open('decoded');
  assert.equal(f.field('name').disabled, true);
  assert.equal(f.field('sample').value, '');
  assert.equal(f.field('rules-note').hidden, false);
  assert.match(f.field('rules-note').textContent, /antes das transformações/);
  await f.editor.preview();
  assert.equal(f.calls.length, 0, 'regex-derived fields require an extracted example');
  f.field('sample').value = 'SGVsbG8='; f.field('sample').oninput(); f.add('parse_json');
  await f.editor.save();
  const save = f.calls.find(call => call.cmd === 'save_derived_field');
  assert.deepEqual(plain(save.args), { ...definition, steps: ['url_decode', 'base64_decode', 'parse_json'] });
  assert.deepEqual(plain(save.opts.analysisOwner), initialOwner);
  assert.equal(save.opts.analysisOwner.sourceGeneration, 10);
  assert.deepEqual(definition, original);
  assert.deepEqual(f.event, eventBefore);
  assert.equal(f.overlay.hidden, true);
});

test('saving a new field cannot overwrite raw fields or unrelated derived names', async () => {
  for (const name of ['payload', 'message', 'existing']) {
    const f = fixture({ definitions: [{ name: 'existing', source: 'message', rules: [], steps: ['url_decode'] }] });
    f.editor.open('payload'); f.add('base64_decode'); f.field('name').value = name;
    await f.editor.save();
    assert.equal(f.calls.length, 0, name);
    assert.equal(f.overlay.hidden, false);
    assert.match(f.field('status').textContent, /Já existe um campo/, name);
    assert.equal(f.field('name').value, name);
  }
});

test('saving reloads receipt-owned runtime definitions and profiles before refreshing the view', async () => {
  for (const scope of ['dataset', 'case']) {
    const f = fixture({ scope }); f.editor.open('payload'); f.add('base64_decode');
    await f.editor.save();
    assert.deepEqual(f.order, ['save_derived_field', 'loadDerivedFields', 'profile_fields', 'fillColumnControls', 'renderExploreTree', 'refresh']);
    assert.equal(f.reloadOwners[0].identity.configRevision, 3);
    const profiles = f.calls.find(call => call.cmd === 'profile_fields');
    assert.equal(profiles.opts.analysisOwner.identity.configRevision, 3);
    assert.deepEqual(plain(profiles.args.filters), []);
    assert.equal(!!profiles.args.caseEvents, scope === 'case');
    assert.equal(f.state.derivedFields[0].name, 'payload_transformado');
    assert.ok(f.state.columns.includes('payload_transformado'));
    if (scope === 'case') assert.ok(f.state.caseTreeProfiles['case-a']);
    else assert.ok(f.state.datasetProfiles);
    assert.equal(f.overlay.hidden, true);
  }
});

test('a failed native save keeps the draft open and a retry performs the save once more', async () => {
  const f = fixture(); f.editor.open('payload'); f.add('base64_decode');
  f.field('name').value = 'retry_name';
  let attempts = 0;
  f.setNative(async (cmd, args) => {
    if (cmd === 'save_derived_field') { if (++attempts === 1) throw Error('disk unavailable'); return f.commit(args); }
    if (cmd === 'profile_fields') return [{ name: 'payload' }, { name: 'retry_name' }];
    throw Error(cmd);
  });
  await f.editor.save();
  assert.equal(f.overlay.hidden, false);
  assert.equal(f.field('name').value, 'retry_name');
  assert.equal(f.field('step-count').textContent, '1 / 8');
  assert.equal(f.field('save').disabled, false);
  assert.match(f.field('status').textContent, /disk unavailable/);
  await f.editor.save();
  assert.equal(attempts, 2);
  assert.equal(f.overlay.hidden, true);
});

test('after a committed save, catalog/profile/view failure preserves the draft and retry never re-saves', async () => {
  for (const failure of ['catalog', 'profile', 'view']) {
    const f = fixture(); f.editor.open('payload'); f.add('base64_decode');
    let failed = false;
    if (failure === 'catalog') f.setReload(async () => { if (!failed) { failed = true; return false; } return true; });
    if (failure === 'profile') f.setNative(async (cmd, args) => {
      if (cmd === 'save_derived_field') return f.commit(args);
      if (cmd === 'profile_fields') { if (!failed) { failed = true; throw Error('profile failed'); } return [{ name: 'payload' }, { name: 'payload_transformado' }]; }
      throw Error(cmd);
    });
    if (failure === 'view') f.setRefresh(async () => { if (!failed) { failed = true; return false; } return true; });
    await f.editor.save();
    assert.equal(f.overlay.hidden, false, failure);
    assert.equal(f.field('step-count').textContent, '1 / 8');
    assert.match(f.field('status').textContent, /configuração já foi salva/i, failure);
    assert.equal(f.field('save').textContent, 'Atualizar campos');
    assert.equal(f.field('name').disabled, true);
    // A preview-only sample edit must not reset the successful configuration commit.
    f.field('sample').value = 'edited preview sample'; f.field('sample').oninput();
    await f.editor.save();
    assert.equal(f.calls.filter(call => call.cmd === 'save_derived_field').length, 1, failure);
    assert.equal(f.overlay.hidden, true, failure);
  }
});

test('native preview failure preserves sample, steps and editor for an explicit retry', async () => {
  const f = fixture(); f.editor.open('payload'); f.add('base64_decode');
  f.field('sample').value = 'invalid encoding'; f.field('sample').oninput();
  f.setNative(async () => { throw Error('invalid base64 character'); });
  await f.editor.preview();
  assert.equal(f.overlay.hidden, false);
  assert.equal(f.field('sample').value, 'invalid encoding');
  assert.equal(f.field('step-count').textContent, '1 / 8');
  assert.equal(f.field('preview').disabled, false);
  assert.match(f.field('status').textContent, /invalid base64/);
  f.setNative(async () => ({ value: 'decoded', notices: [] }));
  await f.editor.preview();
  assert.equal(f.field('output').textContent, 'decoded');
  assert.equal(f.calls.length, 2);
});

test('Cancel and Escape restore focus to a usable anchor or visible fallback', () => {
  const f = fixture(), anchor = f.$('#anchor');
  anchor.tag = 'button';
  f.editor.open('payload', { anchor }); f.field('cancel').onclick();
  assert.equal(f.overlay.hidden, true);
  assert.equal(f.document.activeElement, anchor);
  f.editor.open('payload', { anchor }); anchor.isConnected = false;
  let prevented = false, stopped = false;
  f.overlay.listeners.keydown[0]({ key: 'Escape', preventDefault() { prevented = true; }, stopPropagation() { stopped = true; } });
  assert.equal(f.document.activeElement, f.$('#btn-colpicker'));
  assert.equal(prevented && stopped, true);
  assert.equal(f.cancelled.at(-1), 'field-transform-preview');
  const header = f.$('#column-header'); header.tag = 'th';
  f.editor.open('payload', { anchor: header }); f.field('cancel').onclick();
  assert.equal(f.document.activeElement, f.$('#btn-colpicker'), 'a nonfocusable table header falls back to a real control');
});

test('a stale menu owner cannot open the editor or save against another Case or source', async () => {
  const f = fixture(), action = f.editor.menuItem('payload');
  f.state.generation++;
  assert.equal(action.onClick(), false);
  assert.equal(f.overlay.hidden, true);
  f.editor.open('payload'); f.add('base64_decode'); f.state.active = 'case-b';
  await f.editor.save();
  assert.equal(f.calls.length, 0);
  assert.equal(f.overlay.hidden, false);
  assert.match(f.field('status').textContent, /ANALYSIS_CONTEXT_CHANGED/);
});

test('event diagnostics show bounded errors and the mandatory JWT warning without querying', () => {
  const f = fixture();
  assert.equal(f.editor.renderDiagnostics(f.event), null);
  const event = { ...f.event, derived_diagnostics: [
    { field: 'decoded', code: 'transform_error', message: '<script>' + 'x'.repeat(1000), warning: false },
    { field: 'jwt', code: 'jwt_signature_not_verified', message: 'ignored native wording', warning: true },
  ] };
  const block = f.editor.renderDiagnostics(event);
  assert.match(block.children[1].children[1].textContent, /^<script>/, 'error content stays plain text');
  assert.ok(block.children[1].children[1].textContent.length < 600);
  assert.match(block.children[2].children[1].textContent, /assinatura e as declarações não foram verificadas/);
  assert.equal(f.calls.length, 0, 'opening diagnostics adds no backend work');
  assert.equal(event.derived_diagnostics[0].message.length, 1008, 'original diagnostic stays intact');
});


test('preview samples honor canonical Event columns ahead of custom shadows', async () => {
  const event={id:7,timestamp:1000,message:'canonical message',level:'Informação',fields:{message:'shadow message',level:'shadow level',timestamp:'shadow timestamp',custom:null}};
  const f=fixture({event});f.editor.open('message');await f.editor.preview();
  assert.equal(f.calls.at(-1).args.value,'canonical message');
  assert.equal(f.editor.rawValue(event,'level'),'Informação');
  assert.equal(f.editor.rawValue(event,'timestamp'),'1970-01-01T00:00:01+00:00');
  assert.equal(f.editor.rawValue(event,'custom'),null);
  assert.equal(event.fields.message,'shadow message','custom source values remain untouched');
});
