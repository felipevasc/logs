import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const read = file => readFileSync(new URL(`../../frontend/${file}`, import.meta.url), 'utf8');
const source = read('workspace.js'), app = read('app.js'), html = read('index.html');
const section = (start, end) => {
  const from = source.indexOf(start), to = source.indexOf(end, from);
  assert.ok(from >= 0 && to > from, `Source section exists: ${start}`);
  return source.slice(from, to);
};
const formats = [...html.match(/<select id="ws-export-kind">(.*?)<\/select>/s)[1].matchAll(/value="([^"]+)"/g)].map(match => match[1]);
const plain = value => JSON.parse(JSON.stringify(value));

function fixture({ kind = 'jsonl', nativeCase = false, mask = true, scope = 'dataset', total = 3, pending = 'em cálculo' } = {}) {
  const nodes = new Map(), calls = [], messages = [];
  const document = { activeElement: null };
  class Element {
    constructor() { this.hidden = false; this.textContent = ''; this.attrs = {}; this.disabled = false; }
    focus() { document.activeElement = this; }
    setAttribute(name, value) { this.attrs[name] = value; }
    after(node) { nodes.set(`#${node.id}`, node); }
    dispatchEvent(event) { return this[`on${event.type}`]?.(event); }
  }
  const $ = selector => nodes.get(selector);
  for (const id of ['ws-export-modal', 'ws-export-scope', 'ws-export-kind', 'ws-mask', 'ws-export', 'ws-export-close', 'ws-export-save', 'ws-evidence-export']) {
    const node = new Element(); node.id = id; nodes.set(`#${id}`, node);
  }
  const maskLabel = new Element(); $('#ws-mask').closest = selector => { assert.equal(selector, 'label'); return maskLabel; };
  $('#ws-export-modal').hidden = true; $('#ws-export-kind').value = kind; $('#ws-mask').checked = mask;
  const c = { id: 'case-a', name: 'Draft investigation', items: [{ id: 'a', label: 'First', summary: 'token=synthetic-secret', rows: [] }, { id: 'b', label: 'Second', rows: [] }] };
  const state = { total, filters: [{ column: 'source', op: 'equals_exact', value: 'selected source' }], page: 2, cases: { active: c.id, cases: [c] } };
  const control = { c, scope, pending, path: '/tmp/chosen.licase', fail: false, nativeResult: { records: 3 }, busy: 0 };
  const context = vm.createContext({ $, document, state, Event: class { constructor(type) { this.type = type; } }, structuredClone,
    activeCase: () => control.c, workspaceScope: () => control.scope, fmtNum: value => String(value), pendingCountStatus: () => control.pending,
    el: () => new Element(), btnBusy: button => { control.busy++; button.disabled = true; return () => { control.busy--; button.disabled = false; }; },
    toast: (message, type) => messages.push([message, type]), synthesisMarkdown: () => '', fmtTsFull: String,
    CaseContent: { narrative: item => ({ summary: item.summary || '' }) }, savedRecordCount: () => 2,
    analyticsRequest: current => { calls.push(['scope', current]); return { filters: plain(state.filters), ...(current === 'case' ? { caseEvents: [] } : {}) }; },
    syncVisiblePreferenceMetadata: () => calls.push(['sync']),
    dialogApi: { save: async options => { calls.push(['save', plain(options)]); return control.path; } },
    api: async (command, args) => { calls.push([command, plain(args)]); if (control.fail) throw Error('Backend refused protected content'); return 3; },
    window: { CaseEvidence: { active: nativeCase },
      CaseTimeline: { rows: async () => ({ rows: [{ start: null, title: 'Preserved occurrence', source: 'saved', detail: 'detail', notes: [] }] }) },
      WorkspaceContext: { capture: () => calls.push(['capture']) },
      CaseEvidenceTransfer: { openExport: async (selected, options) => { calls.push(['native', selected, plain(options)]); if (control.fail) throw Error('Backend refused protected content'); return control.nativeResult; } },
      CaseReport: { open: () => calls.push(['pdf']) },
    },
  });
  vm.runInContext(app.split('\n').find(line => line.startsWith('function currentCountLabel(')), context);
  vm.runInContext(section('  async function report()', '  content.addEventListener("click"'), context);
  vm.runInContext(section('  $("#ws-export").onclick', '  $("#btn-load").onclick'), context);
  vm.runInContext(source.split('\n').find(line => line.trim().startsWith('$("#ws-evidence-export").onclick')), context);
  return { $, nodes, document, maskLabel, state, control, calls, messages, context,
    open: () => $('#ws-export').onclick(), save: () => $('#ws-export-save').onclick(),
    change: value => { $('#ws-export-kind').value = value; $('#ws-export-kind').dispatchEvent(new context.Event('change')); },
    text: () => `${$('#ws-export-scope').textContent}\n${$('#ws-export-help').textContent}`,
  };
}

test('every existing format renders its retained choice immediately on opening and reopening', () => {
  assert.deepEqual(formats, ['jsonl', 'csv', 'case-pdf', 'report', 'case']);
  for (const kind of formats) for (const mask of [true, false]) {
    const f = fixture({ kind, mask }); f.open(); const initial = f.text();
    assert.equal(f.$('#ws-export-modal').hidden, false);
    assert.equal(f.document.activeElement, f.$('#ws-export-kind'));
    assert.equal(f.$('#ws-export-kind').attrs['aria-describedby'], 'ws-export-scope ws-export-help');
    assert.equal(f.maskLabel.hidden, kind === 'case-pdf');
    assert.equal(f.$('#ws-mask').checked, mask);
    assert.match(initial, ['jsonl', 'csv'].includes(kind) ? /Recorte atual da Análise · 3 registros/ : /Caso completo · 2 itens/);
    assert.match(initial, new RegExp({ jsonl: 'JSONL', csv: 'CSV', 'case-pdf': 'PDF com cronologia', report: 'Markdown.*sem imagens', case: 'LICASE com evidências' }[kind]));
    f.change(kind); assert.equal(f.text(), initial);
    f.$('#ws-export-close').onclick(); assert.equal(f.$('#ws-export-modal').hidden, true);
    f.open(); assert.equal(f.text(), initial); assert.equal(f.$('#ws-export-kind').value, kind);
    f.$('#ws-export-modal').onclick({ target: f.$('#ws-export-modal') }); assert.equal(f.$('#ws-export-modal').hidden, true);
    f.open(); assert.equal(f.text(), initial); assert.equal(f.calls.length, 0);
    assert.equal(f.nodes.size, 9, 'the capability paragraph is reused');
  }
});

test('filtered exports identify Analysis or Case, all results and honest zero or pending totals', () => {
  for (const scope of ['dataset', 'case']) for (const kind of ['jsonl', 'csv']) for (const total of [0, 3, null]) {
    const f = fixture({ scope, kind, total }); f.open();
    assert.match(f.text(), scope === 'case' ? /Recorte atual do Caso/ : /Recorte atual da Análise/);
    assert.match(f.text(), /todos os resultados dos filtros, além da página visível/);
    assert.match(f.text(), total === null ? /Total de registros em cálculo/ : new RegExp(`${total} registros`));
    if (total === null) assert.doesNotMatch(f.text(), /0 registros/);
    assert.equal(f.calls.length, 0, 'rendering never requests a count or native data');
  }
  for (const pending of ['na fila', 'em cálculo', 'pausado']) {
    const f = fixture({ total: null, pending }); f.open(); assert.ok(f.text().includes(`Total de registros ${pending}`));
  }
});

test('whole-Case counts do not turn unavailable metadata or no active Case into zero', () => {
  for (const kind of ['report', 'case-pdf', 'case']) {
    const f = fixture({ kind, scope: 'case', total: 99 }); f.open(); assert.match(f.text(), /Caso completo · 2 itens/); assert.doesNotMatch(f.text(), /99/);
    f.control.c = { id: 'preserved', kind: 'preserved_case_unavailable' }; f.open();
    assert.match(f.text(), /Caso preservado · quantidade de itens indisponível/); assert.doesNotMatch(f.text(), /0 itens/);
    f.control.c = { id: 'missing-metadata' }; f.open(); assert.match(f.text(), /Caso completo · quantidade de itens indisponível/);
    f.control.c = { id: 'empty', items: [] }; f.open(); assert.match(f.text(), /Caso completo · 0 itens/);
    f.control.c = null; f.open(); assert.match(f.text(), /^Nenhum Caso selecionado/); assert.doesNotMatch(f.text(), /0 itens/);
  }
});

test('format changes retain masking, PDF has its own flow, and portable restrictions remain explicit', () => {
  for (const nativeCase of [false, true]) for (const mask of [true, false]) {
    const f = fixture({ nativeCase, mask }); f.open();
    for (const kind of formats) { f.change(kind); assert.equal(f.$('#ws-mask').checked, mask); assert.equal(f.maskLabel.hidden, kind === 'case-pdf'); }
    assert.match(f.text(), nativeCase ? /JSON recusa Casos com referências ou histórico de exclusões/ : /JSON legado não transporta os arquivos das referências e recusa histórico de exclusões/);
    assert.match(f.text(), /referências originais ou proveniência de exclusões.*recusada.*ocultação/);
    assert.match(f.text(), /Imagens não recebem ocultação/); assert.doesNotMatch(f.text(), /investigação JSON/);
    f.change('report'); assert.match(f.text(), nativeCase ? /cronologia preservada/ : /registros preservados/);
    f.change('case-pdf'); assert.match(f.text(), /próximo passo.*geração do relatório, com proteção de textos própria/);
    f.change('csv'); assert.equal(f.maskLabel.hidden, false); assert.equal(f.$('#ws-mask').checked, mask);
    f.$('#ws-evidence-export').onclick(); assert.equal(f.$('#ws-export-kind').value, 'case'); assert.match(f.text(), /Caso completo.*\nLICASE/);
    assert.equal(f.calls.length, 0);
  }
});

test('event exports preserve the existing scoped request and chosen mask in both storage modes', async () => {
  for (const nativeCase of [false, true]) for (const scope of ['dataset', 'case']) for (const kind of ['jsonl', 'csv']) for (const mask of [true, false]) {
    const f = fixture({ kind, nativeCase, scope, mask }); f.open(); await f.save();
    assert.deepEqual(f.calls.find(call => call[0] === 'export_events')[1], { path: '/tmp/chosen.licase', format: kind, filters: f.state.filters, ...(scope === 'case' ? { caseEvents: [] } : {}), mask });
    assert.deepEqual(f.calls.find(call => call[0] === 'scope'), ['scope', scope]);
    assert.equal(f.calls.some(call => ['native', 'export_investigation'].includes(call[0])), false);
    assert.equal(f.$('#ws-export-modal').hidden, true); assert.equal(f.control.busy, 0);
  }
});

test('native portable export delegates unchanged while legacy export keeps its own document and path', async () => {
  for (const nativeCase of [false, true]) for (const mask of [true, false]) {
    const f = fixture({ kind: 'case', nativeCase, mask }); f.open(); const before = plain(f.control.c); await f.save();
    assert.deepEqual(plain(f.control.c), before); assert.equal(f.$('#ws-mask').checked, mask);
    if (nativeCase) { assert.equal(f.calls.length, 1); assert.equal(f.calls[0][0], 'native'); assert.equal(f.calls[0][1], f.control.c); assert.deepEqual(f.calls[0][2], { mask }); }
    else { const args = f.calls.find(call => call[0] === 'export_investigation')[1]; assert.deepEqual(args, { path: '/tmp/chosen.licase', data: { schemaVersion: 2, active: 'case-a', cases: [before] }, mask }); assert.deepEqual(f.calls.map(call => call[0]), ['save', 'capture', 'sync', 'export_investigation']); }
    assert.equal(f.$('#ws-export-modal').hidden, true);
  }
});

test('Markdown retains its native or legacy report and optional redaction; PDF only opens its generator', async () => {
  for (const nativeCase of [false, true]) for (const mask of [true, false]) {
    const f = fixture({ kind: 'report', nativeCase, mask }); f.open(); await f.save();
    const args = f.calls.find(call => call[0] === 'export_document')[1];
    assert.equal(args.path, '/tmp/chosen.licase'); assert.match(args.content, mask ? /token="\[oculto\]"/ : /token=synthetic-secret/);
    if (nativeCase) assert.match(args.content, /Cronologia preservada.*Preserved occurrence/s);
    assert.equal(f.calls.some(call => ['native', 'export_events', 'export_investigation'].includes(call[0])), false);
    const pdf = fixture({ kind: 'case-pdf', nativeCase, mask }); pdf.open(); await pdf.save();
    assert.deepEqual(pdf.calls, [['pdf']]); assert.equal(pdf.$('#ws-export-modal').hidden, true); assert.equal(pdf.$('#ws-mask').checked, mask);
  }
});

test('file-dialog cancellation and recoverable backend errors retain the draft, format and protection', async () => {
  for (const nativeCase of [false, true]) for (const kind of ['jsonl', 'csv', 'report', 'case']) {
    const f = fixture({ kind, nativeCase }); f.open(); const text = f.text(), draft = plain(f.control.c), filters = plain(f.state.filters);
    f.control.path = null; f.control.nativeResult = null; await f.save();
    assert.equal(f.$('#ws-export-modal').hidden, false); assert.equal(f.messages.length, 0); assert.equal(f.control.busy, 0);
    assert.equal(f.calls.some(call => ['export_events', 'export_document', 'export_investigation'].includes(call[0])), false);
    f.calls.length = 0; f.control.path = '/tmp/chosen.licase'; f.control.fail = true; await f.save();
    assert.equal(f.$('#ws-export-modal').hidden, false); assert.equal(f.$('#ws-export-save').disabled, false); assert.equal(f.control.busy, 0);
    assert.equal(f.$('#ws-export-kind').value, kind); assert.equal(f.$('#ws-mask').checked, true); assert.equal(f.text(), text);
    assert.deepEqual(plain(f.control.c), draft); assert.deepEqual(plain(f.state.filters), filters); assert.equal(f.state.page, 2);
    assert.equal(f.messages.some(([, type]) => type === 'ok'), false);
    if (nativeCase && kind === 'case') assert.match(f.messages.at(-1)[0], /Backend refused protected content/);
    f.$('#ws-export-close').onclick(); f.open(); assert.equal(f.text(), text); assert.equal(f.$('#ws-mask').checked, true);
  }
});
