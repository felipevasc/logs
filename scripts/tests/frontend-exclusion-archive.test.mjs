import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../../frontend/exclusion-archive.js', import.meta.url), 'utf8');
const tasksSource = readFileSync(new URL('../../frontend/tasks.js', import.meta.url), 'utf8');
const transformSource = readFileSync(new URL('../../frontend/field-transform.js', import.meta.url), 'utf8');
const evidenceSource = readFileSync(new URL('../../frontend/evidence-ui.js', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const settle = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const member = (id, sourceKey = 'source-key') => ({ key: { sourceKey, locator: { byte_offset: id } }, eventRef: `event-ref:${id}` });
const batch = (id = 'batch-1') => ({ id, createdAtMs: 1700000000000, label: 'Archived login noise', reason: 'Review later',
  scope: { kind: 'selected', ids: [1, 2, 3] }, sourceReceipt: { sourceGeneration: 10 }, members: 1000, active: true, restoredAtMs: null });

function fixture({ available = true, scope = 'dataset', activeMembers = null, tasks = false } = {}) {
  const nodes = new Map(), calls = [], cancelled = [], messages = [], order = [], handlers = new Map();
  const document = { activeElement: null, listeners: {}, addEventListener(type, callback) { (this.listeners[type] ||= []).push(callback); } };
  const state = { loaded: true, filters: [{ column: 'level', op: 'equals_exact', value: 'Informação' }],
    rows: [{ id: 1, message: 'raw unchanged', fields: { path: 'source.log' } }], selectedEventRows: new Map(),
    analysis: { caseId: 'case-a', analysisId: 'analysis-a', configRevision: 2, visibilityRevision: 3 },
    generation: 10, instance: 1, active: 'case-a' };
  let capabilityAvailable = available, prepareHandler = null, previewNumber = 0;
  let batches = [batch()];
  function node(tag = 'div', className = '', textContent = '') {
    const classes = new Set(className.split(/\s+/).filter(Boolean));
    const item = { tag, className, textContent, value: '', hidden: false, disabled: false, checked: false, isConnected: true,
      children: [], attrs: {}, listeners: {}, parent: null,
      classList: { add: name => classes.add(name), remove: name => classes.delete(name), contains: name => classes.has(name),
        toggle(name, on) { if (on ?? !classes.has(name)) classes.add(name); else classes.delete(name); } },
      get id() { return this.attrs.id; }, set id(value) { this.attrs.id = value; nodes.set(`#${value}`, this); },
      setAttribute(name, value) { this.attrs[name] = String(value); }, getAttribute(name) { return this.attrs[name]; },
      append(...items) { for (const child of items) { child.parent = this; this.children.push(child); } },
      before(child) { child.parent = this.parent; if (this.parent) this.parent.children.splice(this.parent.children.indexOf(this), 0, child); },
      replaceChildren(...items) { this.children = []; this.append(...items); },
      querySelector(selector) { return nodes.get(selector) || null; },
      querySelectorAll(selector) {
        const result = [], visit = parent => { for (const child of parent.children) { if (child.matches(selector)) result.push(child); visit(child); } };
        visit(this); return result;
      },
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
          child.disabled = /\bdisabled\b/.test(match[2]); this.append(child);
        }
      },
    };
    return item;
  }
  document.body = node('body');
  document.body.dataset = { page: 'explore' }; document.documentElement = { dataset: { zone: 'analysis' } };
  const $ = selector => {
    if (!nodes.has(selector)) { const item = node(selector === '#case-select' ? 'select' : 'button'); nodes.set(selector, item); document.body.append(item); }
    return nodes.get(selector);
  };
  const capture = () => ({ caseId: state.active, instance: state.instance, identity: structuredClone(state.analysis),
    sourceGeneration: state.generation, sourceKey: JSON.stringify([state.generation, 'source-one']) });
  const assertOwner = (owner, { revisions = true } = {}) => {
    const actual = capture();
    if (owner.caseId !== actual.caseId || owner.instance !== actual.instance || owner.identity.analysisId !== actual.identity.analysisId
      || owner.sourceKey !== actual.sourceKey || revisions && JSON.stringify(owner.identity) !== JSON.stringify(actual.identity)) throw Error('ANALYSIS_CONTEXT_CHANGED');
  };
  const snapshot = () => ({ schemaVersion: 1, ...structuredClone(state.analysis), config: { derivedFields: [], references: [] }, migrationDiagnostics: [], legacyRaw: null });
  function receipt(batchId = 'batch-1', extra = {}) {
    state.analysis.visibilityRevision++;
    return { analysisContext: snapshot(), batchId, selectedMembers: 1000, ...extra };
  }
  const defaultResponse = (cmd, args) => {
    if (cmd === 'exclusion_capabilities') return { available: capabilityAvailable, reason: capabilityAvailable ? undefined : 'Native exclusion guard unavailable' };
    if (cmd === 'cancel_task' || cmd === 'exclusion_discard') return true;
    if (cmd === 'exclusion_preview') return { previewToken: `prepared-token-${++previewNumber}`, selectedMembers: 3,
      analysisContext: structuredClone(state.analysis), sourceGeneration: scope === 'case' ? null : state.generation };
    if (cmd === 'exclusion_commit') return receipt('batch-1', { selectedMembers: 3 });
    if (cmd === 'exclusion_list') return { analysis: structuredClone(state.analysis), batches: structuredClone(batches), nextCursor: null };
    if (cmd === 'exclusion_archive_page') return { analysis: structuredClone(state.analysis), batch: structuredClone(batches.find(value => value.id === args.batchId) || batch(args.batchId)),
      sources: { 'source-key': { version: 'version-sha256', recordSpace: 'file-byte-offsets-v1', label: '/saved/source.log' } },
      rows: [{ member: member(100), activeInBatch: true, unavailableReason: 'The original source version is unavailable' },
        { member: member(200), activeInBatch: true, event: { id: 2, timestamp: 1700000000000, level: 'Alerta', message: 'available archived content' } },
        { member: member(300), activeInBatch: false, unavailableReason: 'Source not open' }], nextCursor: null, activeMembers };
    if (cmd === 'exclusion_restore_batch' || cmd === 'exclusion_restore_selected') {
      if (cmd === 'exclusion_restore_batch') batches = batches.map(value => value.id === args.batchId ? { ...value, active: false, restoredAtMs: 1700000001000 } : value);
      return receipt(args.batchId, { selectedMembers: cmd === 'exclusion_restore_selected' ? args.members.length : 1000 });
    }
    throw Error(`Unexpected command: ${cmd}`);
  };
  const context = vm.createContext({
    state, structuredClone, document, $, el: node, fmtNum: String, trunc: (value, max) => String(value).slice(0, max), performance,
    setTimeout() {}, clearTimeout() {}, setInterval() {}, clearInterval() {}, requestAnimationFrame() {},
    toast: message => messages.push(message), activeCase: () => ({ id: state.active, name: 'Investigation A' }),
    workspaceScope: () => scope, backendFilters: () => state.filters, caseEvents: () => state.rows, caseSig: () => JSON.stringify(state.rows),
    chipLabel: filter => `${filter.column}: ${filter.value}`, ensureSelectionOwner() {},
    window: { AnalysisContexts: { capture, assertOwner, isCurrent: owner => { try { assertOwner(owner); return true; } catch { return false; } },
      signature: value => JSON.stringify(value ? [value.caseId, value.analysisId, value.configRevision, value.visibilityRevision] : null),
      prepare: async owner => { if (prepareHandler) await prepareHandler(owner); assertOwner(owner); return owner; },
      receipt: async () => ({ accepted: false, reason: 'unchanged' }),
      validSnapshot: value => value?.schemaVersion === 1 && !!value.config && Number.isSafeInteger(value.visibilityRevision) },
      Tasks: { cancelLatest: key => cancelled.push(key) },
      PerformanceTools: { queue: () => ({ add: (run, active = () => true) => Promise.resolve().then(() => active() ? run() : Promise.reject(Error('Operação cancelada.'))) }) },
      FieldTransforms: { bounded: (value, limit) => JSON.stringify(value).slice(0, limit) } },
    caseArgs: async args => { if (!args.caseEvents) return args; const { caseEvents, ...rest } = args; return { ...rest, caseKey: 'captured-case-evidence' }; },
    api: async (cmd, args = {}, opts = {}) => {
      calls.push({ cmd, args, opts }); order.push(cmd);
      return handlers.has(cmd) ? handlers.get(cmd)(args, opts) : defaultResponse(cmd, args);
    },
    loadDerivedFields: async owner => { assertOwner(owner); order.push('loadDerivedFields'); return true; },
    refresh: async () => { order.push('refresh'); return true; },
  });
  vm.runInContext(evidenceSource, context, { filename: 'evidence-ui.js' });
  vm.runInContext(transformSource, context, { filename: 'field-transform.js' });
  vm.runInContext(source, context, { filename: 'exclusion-archive.js' });
  if (tasks) vm.runInContext(tasksSource, context, { filename: 'tasks.js' });
  const archive = context.window.ExclusionArchive, field = name => nodes.get(`#ex-${name}`), overlay = nodes.get('#exclusion-modal');
  return { archive, context, state, calls, cancelled, messages, order, document, $, field, overlay, capture, receipt, defaultResponse,
    on: (cmd, handler) => handlers.set(cmd, handler), setScope: value => { scope = value; },
    setPrepare: handler => { prepareHandler = handler; }, setAvailable: value => { capabilityAvailable = value; },
    records: () => field('records').children.filter(item => item.className === 'ex-record'),
    choose: index => { const checkbox = field('records').children[index].children[0]; checkbox.checked = true; checkbox.onchange(); return checkbox; },
    emit: type => { for (const listener of document.listeners[type] || []) listener(); } };
}

const exclusionCalls = f => f.calls.filter(call => call.cmd !== 'exclusion_capabilities');
const textIn = node => [node.textContent, ...node.children.map(textIn)].filter(Boolean).join('\n');

test('unavailable capabilities disable controls and gate direct preview, commit, list and restore paths', async () => {
  const f = fixture({ available: false }); await settle();
  assert.equal(f.$('#btn-exclusion-archive').disabled, true);
  assert.equal(f.$('#btn-exclusion-filtered').disabled, true);
  f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
  await f.archive.commit();
  assert.match(f.field('status').textContent, /guard unavailable/);
  await f.archive.openArchive(); await f.archive.restore(false); await f.archive.restore(true);
  const action = f.archive.selectedMenuItem({ id: 1 });
  assert.equal(action.disabled, true);
  action.onClick(); await settle();
  assert.equal(exclusionCalls(f).length, 0);
});

test('capability failures and malformed responses fail closed without touching exclusion data', async () => {
  for (const result of [null, {}, { available: 'yes' }, new Error('native command absent')]) {
    const f = fixture();
    f.on('exclusion_capabilities', async () => { if (result instanceof Error) throw result; return result; });
    await settle();
    await f.archive.openArchive();
    f.archive.openPreview({ kind: 'filtered', filters: [] }); await settle(); await f.archive.commit();
    assert.equal(exclusionCalls(f).length, 0);
    assert.equal(f.$('#btn-exclusion-archive').disabled, true);
  }
});

test('preview stages a captured selection and commit sends its token with preserved label and reason', async () => {
  const f = fixture(), selection = { kind: 'selected', ids: [1, 2] }, rowsBefore = structuredClone(f.state.rows);
  await settle();
  f.archive.openPreview(selection); selection.ids.push(99); await settle();
  const preview = f.calls.find(call => call.cmd === 'exclusion_preview');
  assert.deepEqual(plain(preview.args), { analysisContext: { caseId: 'case-a', analysisId: 'analysis-a', configRevision: 2, visibilityRevision: 3 },
    sourceGeneration: 10, scope: { kind: 'selected', ids: [1, 2] } });
  assert.equal(preview.opts.latest, 'exclusion-preview');
  assert.match(f.field('preview-count').textContent, /^3 registro/);
  assert.equal(f.field('commit').disabled, false);
  f.field('label').value = '  staged label  '; f.field('reason').value = '  staged reason  ';
  await f.archive.commit();
  const commit = f.calls.find(call => call.cmd === 'exclusion_commit');
  assert.deepEqual(plain(commit.args), { previewToken: 'prepared-token-1', label: 'staged label', reason: 'staged reason' });
  assert.equal(commit.opts.analysisOwner.identity.visibilityRevision, 3);
  assert.deepEqual(f.state.rows, rowsBefore);
  assert.equal(f.overlay.hidden, true);
  assert.match(f.messages.at(-1), /3 registro/);
});

test('filtered previews preserve their original filters and Case evidence transports only its caseKey', async () => {
  const f = fixture({ scope: 'case' }); await settle();
  const filters = structuredClone(f.state.filters);
  f.archive.openPreview({ kind: 'filtered', filters }); filters[0].value = 'changed outside';
  f.state.filters = [{ column: 'message', op: 'contains', value: 'new active filter' }];
  await settle();
  const preview = f.calls.find(call => call.cmd === 'exclusion_preview');
  assert.equal(preview.args.caseKey, 'captured-case-evidence');
  assert.equal(preview.args.caseEvents, undefined);
  assert.equal(preview.args.scope.filters[0].value, 'Informação');
  assert.equal(f.field('commit').disabled, false, 'Case evidence accepts its null native source generation');
});

test('commit transports the admitted preview generation for Dataset and Case tokens', async () => {
  for (const scope of ['dataset', 'case']) {
    const f = fixture({ scope, tasks: true }); await settle();
    f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
    const captured = f.capture(), admittedGeneration = scope === 'case' ? null : captured.sourceGeneration;
    f.on('exclusion_commit', args => {
      assert.equal(args.sourceGeneration, admittedGeneration, 'token admission uses the preview receipt, including Case null');
      assert.deepEqual(plain(args.analysisContext), captured.identity);
      return f.defaultResponse('exclusion_commit', args);
    });
    await f.archive.commit();
    const commit = f.calls.find(call => call.cmd === 'exclusion_commit');
    assert.equal(commit.args.sourceGeneration, admittedGeneration);
    assert.equal(f.overlay.hidden, true);
    assert.match(f.messages.at(-1), /Lote arquivado/);
  }
});

test('failed preview and expired token preserve labels and captured membership for recalculation', async () => {
  const f = fixture(); await settle();
  let failPreview = true;
  f.on('exclusion_preview', args => { if (failPreview) { failPreview = false; throw Error('temporary preview failure'); } return f.defaultResponse('exclusion_preview', args); });
  f.archive.openPreview({ kind: 'selected', ids: [1, 2] });
  f.field('label').value = 'keep label'; f.field('reason').value = 'keep reason'; await settle();
  assert.equal(f.field('label').value, 'keep label');
  assert.equal(f.field('commit').disabled, true);
  await f.archive.preview();
  f.on('exclusion_commit', async () => { throw Error('PREVIEW_TOKEN_EXPIRED'); });
  await f.archive.commit();
  assert.equal(f.field('label').value, 'keep label'); assert.equal(f.field('reason').value, 'keep reason');
  assert.match(f.field('status').textContent, /PREVIEW_TOKEN_EXPIRED/);
  assert.equal(f.field('commit').disabled, true);
  const commitCount = f.calls.filter(call => call.cmd === 'exclusion_commit').length;
  await f.archive.commit();
  assert.equal(f.calls.filter(call => call.cmd === 'exclusion_commit').length, commitCount, 'an expired token cannot be submitted twice');
  await f.archive.preview();
  for (const call of f.calls.filter(call => call.cmd === 'exclusion_preview')) assert.deepEqual(plain(call.args.scope), { kind: 'selected', ids: [1, 2] });
  assert.equal(f.field('label').value, 'keep label');
  assert.equal(f.field('commit').disabled, false);
});

test('preview rejects malformed exact counts, mismatched identities and stale source receipts', async () => {
  for (const alter of [value => { value.selectedMembers = -1; }, value => { value.selectedMembers = 1.5; },
    value => { value.previewToken = ''; }, value => { value.analysisContext.analysisId = 'foreign'; },
    value => { value.analysisContext.visibilityRevision++; }, value => { value.sourceGeneration++; }]) {
    const f = fixture(); await settle();
    f.on('exclusion_preview', args => { const value = f.defaultResponse('exclusion_preview', args); alter(value); return value; });
    f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
    assert.equal(f.field('commit').disabled, true);
    await f.archive.commit();
    assert.equal(f.calls.some(call => call.cmd === 'exclusion_commit'), false);
  }
});

test('native expired and changed-source messages require restaging instead of uncertain commit replay', async () => {
  for (const message of ['A prévia expirou. Prepare a seleção novamente.', 'O Caso ou a fonte da prévia mudou. Prepare a seleção novamente.']) {
    const f = fixture(); await settle(); f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
    f.field('label').value = 'Keep draft'; f.on('exclusion_commit', () => { throw Error(message); });
    await f.archive.commit(); await settle();
    assert.equal(f.field('commit').disabled, true);
    assert.equal(f.field('label').value, 'Keep draft');
    assert.equal(f.field('label').disabled, false);
    assert.equal(f.calls.filter(call => call.cmd === 'exclusion_discard').length, 1);
    assert.doesNotMatch(f.field('status').textContent, /mesmo lote/);
  }
});

test('owner, source, revision and workspace-scope changes reject a pending preview result', async () => {
  for (const change of ['owner', 'source', 'revision', 'scope', 'instance']) {
    const f = fixture(), pending = deferred(); await settle();
    f.on('exclusion_preview', () => pending.promise);
    f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
    const result = f.defaultResponse('exclusion_preview', {});
    if (change === 'owner') f.state.active = 'case-b';
    if (change === 'source') f.state.generation++;
    if (change === 'revision') f.state.analysis.visibilityRevision++;
    if (change === 'scope') f.setScope('case');
    if (change === 'instance') f.state.instance++;
    pending.resolve(result); await settle();
    assert.equal(f.field('commit').disabled, true, change);
    assert.match(f.field('status').textContent, /CHANGED|mudou/, change);
  }
});

test('changed Case evidence invalidates preview and commit while archive restoration stays metadata-only', async () => {
  for (const stage of ['preview', 'commit']) {
    const f = fixture({ scope: 'case' }), pending = deferred(); await settle();
    if (stage === 'preview') f.on('exclusion_preview', () => pending.promise);
    f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
    f.state.rows = [{ id: 2, message: 'replaced Case evidence' }];
    if (stage === 'preview') { pending.resolve(f.defaultResponse('exclusion_preview', {})); await settle(); }
    else await f.archive.commit();
    assert.match(f.field('status').textContent, /registros do Caso mudaram/, stage);
    assert.equal(f.calls.some(call => call.cmd === 'exclusion_commit'), false);
    assert.equal(f.field('commit').disabled, true);
  }
  const f = fixture({ scope: 'case' }); await settle(); await f.archive.openArchive();
  f.state.rows = [{ id: 99, message: 'unrelated replacement evidence' }];
  await f.archive.restore(false);
  assert.equal(f.calls.filter(call => call.cmd === 'exclusion_restore_batch').length, 1);
  assert.match(f.messages.at(-1), /Lote restaurado/);
});

test('archive retains unavailable source provenance and labels unknown remaining counts honestly', async () => {
  const f = fixture({ activeMembers: null }); await settle(); await f.archive.openArchive();
  assert.match(f.field('batch-meta').textContent, /1000 registro\(s\) no lote original/);
  assert.match(f.field('batch-meta').textContent, /quantidade restante não calculada/);
  assert.doesNotMatch(f.field('batch-meta').textContent, /0 exclusão/);
  const unavailable = textIn(f.records()[0]);
  for (const value of ['/saved/source.log', 'version-sha256', 'file-byte-offsets-v1', 'Posição: 100', 'event-ref:100', 'original source version is unavailable']) assert.ok(unavailable.includes(value), value);
  assert.match(textIn(f.records()[1]), /available archived content/);
  assert.equal(f.records()[0].children[0].disabled, false, 'unavailable content still permits metadata-only restoration');
  assert.equal(f.records()[2].children[0].disabled, true, 'already-restored membership cannot be selected');
});

test('available archive rows expose bounded safe summaries while collapsed and retain original values', async () => {
  const f = fixture(); await settle();
  const event = { id: 2, timestamp: 0, level: 'Alerta', message: 'Sign-in completed: token=private-token',
    fields: { normal: 'original', client_secret: 'private-secret', body: 'x'.repeat(1000000) } };
  f.on('exclusion_archive_page', args => {
    const reply = f.defaultResponse('exclusion_archive_page', args); reply.rows[1].event = event; return reply;
  });
  await f.archive.openArchive();
  const body = f.records()[1].children[1], summary = body.children.find(item => item.className === 'ex-record-summary');
  const meta = body.children.find(item => item.className === 'ex-record-meta');
  assert.match(summary.textContent, /Sign-in completed: token=private-token/); assert.doesNotMatch(summary.textContent, /\[oculto\]/);
  assert.equal(meta.textContent, `${new Date(0).toLocaleString('pt-BR')} · Alerta`); assert.ok(summary.textContent.length <= 320);
  assert.equal(f.records()[0].children[1].children.some(item => item.className === 'ex-record-summary'), false, 'missing content retains provenance without an invented message');
  const details = body.children.find(item => item.className === 'ex-event-preview'), preview = details.children.find(item => item.tag === 'pre');
  assert.ok(preview.textContent.length <= 4096); assert.match(preview.textContent, /prévia truncada/);
  assert.doesNotMatch(textIn(body), /\[oculto\]/, 'archived values are shown as recorded');
  assert.equal(event.message, 'Sign-in completed: token=private-token'); assert.equal(event.fields.client_secret, 'private-secret');
  assert.equal(event.fields.body.length, 1000000);
});

test('full-batch restore sends metadata only and does not claim original batch size became newly visible', async () => {
  const f = fixture(), originalRows = structuredClone(f.state.rows); await settle(); await f.archive.openArchive();
  await f.archive.restore(false);
  const request = f.calls.find(call => call.cmd === 'exclusion_restore_batch');
  assert.deepEqual(plain(request.args), { analysisContext: { caseId: 'case-a', analysisId: 'analysis-a', configRevision: 2, visibilityRevision: 3 }, batchId: 'batch-1' });
  assert.equal(request.args.members, undefined);
  assert.equal(request.args.caseEvents, undefined);
  assert.deepEqual(f.state.rows, originalRows);
  assert.match(f.messages.at(-1), /Outros lotes ainda podem manter registros ocultos/);
  assert.doesNotMatch(f.messages.at(-1), /1000.*visível/);
  assert.equal(f.field('restore-batch').disabled, true);
  assert.ok(f.order.indexOf('loadDerivedFields') < f.order.indexOf('refresh'));
});

test('restore reports only native newlyVisible counts, including an exact zero', async () => {
  for (const newlyVisible of [0, 2]) {
    const f = fixture(); await settle(); await f.archive.openArchive();
    f.on('exclusion_restore_batch', args => f.receipt(args.batchId, { newlyVisible }));
    await f.archive.restore(false);
    assert.match(f.messages.at(-1), new RegExp(`visíveis: ${newlyVisible}\\.`));
    assert.doesNotMatch(f.messages.at(-1), /1000.*visível/);
  }
});

test('opaque member cursors pass through unchanged and partial restore contains only selected current-page members', async () => {
  const f = fixture(), opaque = { sourceKey: 'cursor:key/=opaque', locator: { stable_record: 'record:next?42' } };
  await settle();
  const firstMember = member(100), secondMember = { key: { sourceKey: 'source-second', locator: { stable_record: 'record/200' } }, eventRef: 'stable-ref:200' };
  f.on('exclusion_archive_page', args => {
    const value = f.defaultResponse('exclusion_archive_page', args);
    return { ...value, sources: { ...value.sources, 'source-second': { version: 'second-version', recordSpace: 'stable-record-v1', label: 'second-source' } },
      rows: [{ member: structuredClone(args.cursor ? secondMember : firstMember), activeInBatch: true, unavailableReason: 'not loaded' }],
      nextCursor: args.cursor ? null : structuredClone(opaque), activeMembers: null };
  });
  await f.archive.openArchive(); f.choose(0);
  assert.equal(f.field('restore-selected').disabled, false);
  f.field('members-next').onclick(); await settle();
  const pageRequests = f.calls.filter(call => call.cmd === 'exclusion_archive_page');
  assert.deepEqual(plain(pageRequests[1].args.cursor), opaque);
  assert.equal(pageRequests[1].args.limit, 100);
  assert.equal(f.field('restore-selected').disabled, true, 'selection is reset when changing page');
  assert.equal(f.records()[0].children[0].checked, false);
  f.choose(0); await f.archive.restore(true);
  const restore = f.calls.find(call => call.cmd === 'exclusion_restore_selected');
  assert.deepEqual(plain(restore.args.members), [secondMember]);
  assert.ok(restore.args.members.length <= 500);
  assert.equal(restore.args.batchId, 'batch-1');
  assert.equal(restore.args.members.some(value => value.eventRef === firstMember.eventRef), false);
});

test('opaque batch cursors are returned verbatim and previous navigation restores the prior page cursor', async () => {
  const f = fixture(), opaque = 'opaque-list-cursor/+==:v2'; await settle();
  f.on('exclusion_list', args => ({ analysis: structuredClone(f.state.analysis), batches: [batch(args.cursor ? 'batch-2' : 'batch-1')], nextCursor: args.cursor ? null : opaque }));
  await f.archive.openArchive(); f.field('batches-next').onclick(); await settle();
  assert.equal(f.calls.filter(call => call.cmd === 'exclusion_list')[1].args.cursor, opaque);
  assert.equal(f.field('batches-next').disabled, true);
  f.field('batches-prev').onclick(); await settle();
  assert.equal(f.calls.filter(call => call.cmd === 'exclusion_list')[2].args.cursor, null);
  assert.equal(f.field('batches-prev').disabled, true);
});

test('invalid membership keys and oversized archive pages cannot become selectable rows', async () => {
  const f = fixture(); await settle();
  for (const value of [null, { key: { sourceKey: '', locator: { byte_offset: 1 } }, eventRef: 'ref' },
    { key: { sourceKey: 'source', locator: { byte_offset: -1 } }, eventRef: 'ref' },
    { key: { sourceKey: 'source', locator: { stable_record: '' } }, eventRef: 'ref' },
    { key: { sourceKey: 'source', locator: { byte_offset: 1, stable_record: 'also' } }, eventRef: 'ref' }]) assert.equal(f.archive.validateMember(value), false);
  assert.equal(f.archive.validateMember(member(0)), true);
  f.on('exclusion_archive_page', args => ({ ...f.defaultResponse('exclusion_archive_page', args),
    rows: Array.from({ length: 501 }, (_, index) => ({ member: member(index), activeInBatch: true })) }));
  await f.archive.openArchive();
  assert.match(f.field('status').textContent, /Página do arquivo inválida/);
  assert.equal(f.records().length, 0);
  assert.equal(f.field('restore-selected').disabled, true);
});

test('archive batch metadata and source descriptors are validated before rendering rows', async () => {
  for (const mutate of [
    value => { value.batch.createdAtMs = -1; }, value => { value.batch.label = 12; }, value => { value.batch.reason = null; },
    value => { delete value.sources['source-key']; }, value => { value.sources['source-key'].recordSpace = null; },
    value => { value.sources['source-key'].version = {}; }, value => { value.activeMembers = -1; },
  ]) {
    const f = fixture(); await settle();
    f.on('exclusion_archive_page', args => { const value = f.defaultResponse('exclusion_archive_page', args); mutate(value); return value; });
    await f.archive.openArchive();
    assert.match(f.field('status').textContent, /Página do arquivo inválida/);
    assert.equal(f.records().length, 0);
  }
});

test('inactive batches cannot restore members even when a row flag inconsistently says active', async () => {
  const f = fixture(); await settle();
  f.on('exclusion_archive_page', args => { const value = f.defaultResponse('exclusion_archive_page', args); value.batch.active = false; return value; });
  await f.archive.openArchive();
  assert.equal(f.field('restore-batch').disabled, true);
  assert.equal(f.records()[0].children[0].disabled, true);
  assert.match(textIn(f.records()[0]), /Restaurado neste lote/);
  await f.archive.restore(false);
  assert.equal(f.calls.some(call => call.cmd === 'exclusion_restore_batch'), false);
});

test('mutation receipts must prove the correct staged count, batch and nonnegative newly-visible count', async () => {
  for (const mismatch of ['preview-count', 'restore-batch', 'negative-visible']) {
    const f = fixture(); await settle();
    if (mismatch === 'preview-count') {
      f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
      f.on('exclusion_commit', () => f.receipt('batch-1', { selectedMembers: 4 }));
      await f.archive.commit();
    } else {
      await f.archive.openArchive();
      f.on('exclusion_restore_batch', () => f.receipt(mismatch === 'restore-batch' ? 'foreign-batch' : 'batch-1',
        mismatch === 'negative-visible' ? { newlyVisible: -1 } : {}));
      await f.archive.restore(false);
    }
    assert.match(f.field('status').textContent, /Confirmação de exclusão inválida/, mismatch);
    assert.equal(f.messages.length, 0, 'an invalid receipt produces no success notice');
    assert.equal(f.overlay.hidden, false);
    assert.equal(f.order.includes('refresh'), false);
  }
});

test('closing during capability admission prevents commit and restore mutations from starting', async () => {
  for (const action of ['commit', 'restore']) {
    const f = fixture(); await settle();
    if (action === 'commit') { f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle(); }
    else await f.archive.openArchive();
    const gate = deferred(); f.setPrepare(() => gate.promise);
    const running = action === 'commit' ? f.archive.commit() : f.archive.restore(false);
    await settle(); f.archive.close(); gate.resolve(); await running; await settle();
    assert.equal(f.calls.some(call => call.cmd === `exclusion_${action === 'commit' ? 'commit' : 'restore_batch'}`), false, action);
    assert.equal(f.overlay.hidden, true);
  }
});

test('opening another draft during capability admission never submits an old token with the new labels', async () => {
  const f = fixture(); await settle(); f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
  f.field('label').value = 'first draft';
  const gate = deferred(); f.setPrepare(() => gate.promise);
  const commit = f.archive.commit(); await settle();
  f.archive.openPreview({ kind: 'selected', ids: [2] }); f.field('label').value = 'second draft';
  gate.resolve(); await commit; await settle();
  assert.equal(f.calls.some(call => call.cmd === 'exclusion_commit'), false);
  assert.equal(f.overlay.hidden, false); assert.equal(f.field('label').value, 'second draft');
});

test('staging labels stay editable but commit captures and locks their values before capability admission', async () => {
  const f = fixture(), staged = deferred(); await settle();
  f.on('exclusion_preview', () => staged.promise);
  f.archive.openPreview({ kind: 'selected', ids: [1] }); await settle();
  assert.equal(f.field('label').disabled, false); assert.equal(f.field('reason').disabled, false);
  f.field('label').value = 'captured label'; f.field('reason').value = 'captured reason';
  staged.resolve(f.defaultResponse('exclusion_preview', {})); await settle();
  const gate = deferred(); f.setPrepare(() => gate.promise);
  const commit = f.archive.commit(); await settle();
  assert.equal(f.field('label').disabled, true); assert.equal(f.field('reason').disabled, true);
  // Programmatic DOM changes also must not change the already-confirmed request.
  f.field('label').value = 'later label'; f.field('reason').value = 'later reason';
  gate.resolve(); await commit;
  const sent = f.calls.find(call => call.cmd === 'exclusion_commit');
  assert.equal(sent.args.label, 'captured label'); assert.equal(sent.args.reason, 'captured reason');
});

test('Tasks rejects a canceled preview Identity receipt while preserving a committed mutation Snapshot', async () => {
  const f = fixture({ tasks: true }); await settle();
  const previewResponse = deferred(); f.on('exclusion_preview', () => previewResponse.promise);
  const preview = f.context.api('exclusion_preview', { scope: { kind: 'selected', ids: [1] } }, { latest: 'exclusion-preview', silent: true })
    .then(value => ({ value }), error => ({ error }));
  await settle(); f.context.window.Tasks.cancelLatest('exclusion-preview');
  previewResponse.resolve(f.defaultResponse('exclusion_preview', {}));
  const rejected = await preview;
  assert.match(String(rejected.error), /cancelad/); assert.equal(rejected.value, undefined);
  assert.equal(f.calls.filter(call=>call.cmd==='exclusion_discard').length,1,'a token completing just before cancellation is still released');
  assert.equal(f.calls.find(call=>call.cmd==='exclusion_discard').args.previewToken,'prepared-token-1');
  const commitResponse = deferred(); f.on('exclusion_commit', () => commitResponse.promise);
  const commit = f.context.api('exclusion_commit', { previewToken: 'prepared-token' }, { latest: 'exclusion-commit', silent: true });
  await settle(); f.context.window.Tasks.cancelLatest('exclusion-commit');
  commitResponse.resolve(f.receipt('batch-1', { selectedMembers: 3 }));
  assert.equal((await commit).batchId, 'batch-1');
});

test('canceled preview and archive reads cannot update a closed or replacement dialog', async () => {
  for (const command of ['exclusion_preview', 'exclusion_list', 'exclusion_archive_page']) {
    const f = fixture(), pending = deferred(); await settle();
    f.on(command, () => pending.promise);
    const operation = command === 'exclusion_preview' ? f.archive.openPreview({ kind: 'selected', ids: [1] }) : f.archive.openArchive();
    await settle(); f.archive.close();
    const before = textIn(f.overlay);
    pending.resolve(f.defaultResponse(command, { batchId: 'batch-1' }));
    await operation; await settle();
    assert.equal(f.overlay.hidden, true, command);
    assert.equal(textIn(f.overlay), before, command);
  }
});

test('archive list and page replies reject stale owner, source and scope changes', async () => {
  for (const command of ['exclusion_list', 'exclusion_archive_page']) {
    for (const change of ['owner', 'source', 'scope']) {
      const f = fixture(), pending = deferred(); await settle();
      f.on(command, () => pending.promise);
      const opened = f.archive.openArchive(); await settle();
      const response = f.defaultResponse(command, { batchId: 'batch-1' });
      if (change === 'owner') f.state.active = 'case-b';
      if (change === 'source') f.state.generation++;
      if (change === 'scope') f.setScope('case');
      pending.resolve(response); await opened; await settle();
      assert.match(f.field('status').textContent, /CHANGED|mudou/, `${command}:${change}`);
      assert.equal(f.records().length, 0);
    }
  }
});

test('Cancel returns focus to a usable anchor and falls back from nonfocusable table cells', async () => {
  const f = fixture(); await settle();
  const anchor = f.$('#anchor');
  f.archive.openPreview({ kind: 'selected', ids: [1] }, { anchor }); await settle(); f.field('cancel').onclick();
  assert.equal(f.document.activeElement, anchor);
  anchor.tag = 'td';
  f.archive.openPreview({ kind: 'selected', ids: [1] }, { anchor }); await settle(); f.archive.close();
  assert.equal(f.document.activeElement, f.$('#btn-exclusion-archive'));
  assert.deepEqual(f.cancelled.slice(-3), ['exclusion-preview', 'exclusion-list', 'exclusion-page']);
});

test('unconfirmed Dataset identity blocks staging while archive provenance stays available', async () => {
  const f = fixture(); await settle(); f.state.sourceIdentityUnconfirmed = true; f.archive.updateButtons();
  assert.equal(f.$('#btn-exclusion-filtered').disabled, true);
  assert.equal(f.archive.selectedMenuItem({id:1}).disabled, true);
  f.archive.openPreview({kind:'selected',ids:[1]}); await settle();
  assert.equal(f.calls.some(call=>call.cmd==='exclusion_preview'), false);
  assert.match(f.field('status').textContent,/Confirme a fonte/);
  await f.archive.openArchive();
  assert.ok(f.calls.some(call=>call.cmd==='exclusion_archive_page'),'read-only history does not require an open verified source');
});


test('closing a completed preview discards with its captured owner even after changing Cases', async () => {
  const f=fixture({tasks:true});await settle();f.archive.openPreview({kind:'selected',ids:[1]});await settle();
  const captured=plain(f.capture());f.state.active='case-b';f.state.analysis.caseId='case-b';f.state.analysis.analysisId='analysis-b';f.state.generation++;
  f.archive.close();await settle();
  const discarded=f.calls.filter(call=>call.cmd==='exclusion_discard');assert.equal(discarded.length,1);
  assert.equal(discarded[0].args.previewToken,'prepared-token-1');assert.deepEqual(plain(discarded[0].args.analysisContext),captured.identity);
  assert.equal(discarded[0].args.sourceGeneration,captured.sourceGeneration);assert.equal(discarded[0].opts.silent,true);
  f.archive.close();await settle();assert.equal(f.calls.filter(call=>call.cmd==='exclusion_discard').length,1,'closing twice does not discard twice');
});

test('restaging or replacing a completed preview releases its old prepared payload', async () => {
  const f=fixture({scope:'case'});await settle();f.archive.openPreview({kind:'selected',ids:[1]});await settle();
  await f.archive.preview();const first=f.calls.find(call=>call.cmd==='exclusion_discard');
  assert.equal(first.args.previewToken,'prepared-token-1');assert.equal(first.args.sourceGeneration,null,'Case evidence preserves the admitted nullable source generation');
  assert.ok(f.order.indexOf('exclusion_discard')<f.order.lastIndexOf('exclusion_preview'));
  await f.archive.openArchive();assert.deepEqual(f.calls.filter(call=>call.cmd==='exclusion_discard').map(call=>call.args.previewToken),['prepared-token-1','prepared-token-2']);
});

test('discard is best effort and never touches a token whose commit is in flight', async () => {
  const f=fixture();await settle();f.archive.openPreview({kind:'selected',ids:[1]});await settle();
  const result=deferred();f.on('exclusion_commit',()=>result.promise);const committing=f.archive.commit();await settle();
  assert.equal(f.calls.filter(call=>call.cmd==='exclusion_commit').length,1);f.archive.close();await settle();
  assert.equal(f.calls.filter(call=>call.cmd==='exclusion_discard').length,0);
  result.resolve(f.receipt('batch-1',{selectedMembers:3}));await committing;assert.equal(f.calls.filter(call=>call.cmd==='exclusion_discard').length,0);
  const failed=fixture();await settle();failed.archive.openPreview({kind:'selected',ids:[1]});await settle();
  failed.on('exclusion_discard',()=>{throw Error('cleanup unavailable');});failed.archive.close();await settle();
  assert.equal(failed.overlay.hidden,true);assert.equal(failed.messages.length,0);assert.equal(failed.calls.filter(call=>call.cmd==='exclusion_discard').length,1);
});

test('an uncertain commit replays the same opaque token and original payload explicitly', async () => {
  const f=fixture();await settle();f.archive.openPreview({kind:'selected',ids:[1]});await settle();
  f.field('label').value='original label';f.field('reason').value='original reason';let attempts=0;
  f.on('exclusion_commit',()=>{if(++attempts===1)throw Error('response unavailable');return f.receipt('batch-1',{selectedMembers:3});});
  await f.archive.commit();assert.equal(f.field('commit').disabled,false);assert.equal(f.field('label').disabled,true);
  assert.match(f.field('status').textContent,/mesmo lote/);f.field('label').value='programmatic replacement';await f.archive.commit();
  const attemptsSent=f.calls.filter(call=>call.cmd==='exclusion_commit');assert.equal(attemptsSent.length,2);
  assert.deepEqual(plain(attemptsSent[0].args),plain(attemptsSent[1].args));assert.equal(attemptsSent[1].args.label,'original label');
  assert.equal(f.overlay.hidden,true);assert.equal(f.calls.filter(call=>call.cmd==='exclusion_discard').length,0);
});

test('Case archive pages hydrate only captured evidence and reject replaced evidence during the request', async () => {
  const f=fixture({scope:'case'});await settle();const rows=f.state.rows;await f.archive.openArchive();
  const page=f.calls.find(call=>call.cmd==='exclusion_archive_page');
  assert.equal(page.args.caseKey,'captured-case-evidence');assert.equal(page.args.caseEvents,undefined);
  assert.equal(page.opts.caseEvents,rows,'the exact captured evidence remains available only for request-lifetime resync');
  assert.equal(page.args.sourceGeneration,f.capture().sourceGeneration);
  const moved=fixture({scope:'case'});await settle();const response=deferred();moved.on('exclusion_archive_page',()=>response.promise);
  const opening=moved.archive.openArchive();await settle();moved.state.rows=[{id:999,message:'replacement evidence'}];
  response.resolve(moved.defaultResponse('exclusion_archive_page',{batchId:'batch-1'}));await opening;
  assert.match(moved.field('status').textContent,/registros disponíveis do Caso mudaram/);assert.equal(moved.records().length,0);
  assert.ok(moved.field('batches').children.length,'batch history remains available after hydration becomes stale');
  const empty=fixture({scope:'case'});empty.state.rows=[];await settle();await empty.archive.openArchive();
  const emptyPage=empty.calls.find(call=>call.cmd==='exclusion_archive_page');assert.deepEqual(plain(emptyPage.opts.caseEvents),[]);
  assert.equal(empty.records().length,3,'provenance pages remain usable without matching current evidence');
});
