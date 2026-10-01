import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../../frontend/explorer-timeline.js', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
const settle = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const sparseStats = () => ({ buckets: [[1000, 3], [3000, 3]], bucketMs: 1000 });
const total = buckets => ({ count: buckets.reduce((a, b) => a + b, 0), buckets });
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture({ scope = 'dataset', filters = [] } = {}) {
  const requests = [], cancelled = [], plots = [], messages = [], captured = [], callsToSave = [], nodes = new Map();
  const state = { loaded: true, datasetRevision: 1, filters, sourcePublication: { generation: 7 },
    currentArtifact: { id: 'source-one', loadedAt: 10 }, rows: [{ id: 'keep-this-row' }],
    analysis: { caseId: 'case-a', analysisId: 'analysis-a', configRevision: 2, visibilityRevision: 3 } };
  let caseVersion = 'evidence-1', casePreparation = null, resumeCount = 0;
  function node(tag = 'div', className = '', textContent = '') {
    const classes = new Set(className.split(/\s+/).filter(Boolean));
    return { tag, className, textContent, style: {}, attrs: {}, children: [], hidden: false, clientWidth: 700,
      classList: { add: name => classes.add(name), remove: name => classes.delete(name), contains: name => classes.has(name),
        toggle(name, on) { if (on ?? !classes.has(name)) classes.add(name); else classes.delete(name); } },
      append(...items) { this.children.push(...items); }, prepend(...items) { this.children.unshift(...items); },
      replaceChildren(...items) { this.children = items; }, setAttribute(name, value) { this.attrs[name] = value; },
      getAttribute(name) { return this.attrs[name]; }, scrollIntoView() {} };
  }
  const $ = selector => { if (!nodes.has(selector)) nodes.set(selector, node()); return nodes.get(selector); };
  const capture = () => ({ caseId: state.analysis.caseId, instance: 1, identity: structuredClone(state.analysis),
    sourceGeneration: state.sourcePublication.generation,
    sourceKey: JSON.stringify([state.sourcePublication.generation, state.currentArtifact.id, state.currentArtifact.loadedAt]) });
  const explorerKey = () => JSON.stringify([scope, capture(), state.datasetRevision, state.filters, caseVersion]);
  const explorerAnalytics = new Map();
  const context = vm.createContext({
    state, structuredClone, chart: null, $,
    el: node, document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => '#2368dc' }),
    colLabel: field => field, fmtNum: String, trunc: (text, length) => text.length > length ? `${text.slice(0, length)}…` : text,
    isLight: () => false, workspaceScope: () => scope, caseSig: () => caseVersion, backendFilters: () => state.filters,
    caseEvents: () => [{ id: 'evidence-row' }], explorerKey, explorerAnalytics,
    addFilter: filter => state.filters.push(filter), saveCases: async () => { callsToSave.push(true); },
    toast: message => messages.push(message),
    window: { AnalysisContexts: { capture, isCurrent: owner => JSON.stringify(owner) === JSON.stringify(capture()) },
      Tasks: { cancelLatest: key => cancelled.push(key) }, WorkspaceContext: { capture: () => captured.push(true) } },
    caseArgs: async args => {
      if (casePreparation) await casePreparation(args);
      if (!args.caseEvents) return args;
      const { caseEvents, ...rest } = args;
      return { ...rest, caseKey: caseVersion };
    },
    api: (cmd, args, opts) => {
      const pending = deferred();
      requests.push({ cmd, args, opts, scope, ...pending });
      return pending.promise;
    },
    uPlot: class {
      constructor(options, data, host) { Object.assign(this, { options, data, host, destroyed: false, updates: [], visibility: [] }); plots.push(this); }
      setData(data) { this.data = data; this.updates.push(data); }
      setSeries(index, value) { this.visibility.push({ index, ...value }); }
      destroy() { this.destroyed = true; }
    },
  });
  vm.runInContext(source, context, { filename: 'explorer-timeline.js' });
  const timeline = context.window.ExplorerTimeline;
  const find = className => {
    const walk = item => item.className?.split(/\s+/).includes(className) ? item : item.children?.map(walk).find(Boolean);
    return walk($('.hist-panel'));
  };
  function response(request = requests.at(-1), overrides = {}) {
    const buckets = [3, 0, 3];
    if (request.args.grid.bucketCount !== 3) buckets.splice(0, buckets.length, ...Array(request.args.grid.bucketCount).fill(2));
    const first = buckets.map((value, index) => index === 0 ? value : 0), remaining = buckets.map((value, index) => index === 0 ? 0 : value);
    return { field: request.args.field, grid: plain(request.args.grid), total: total(buckets),
      series: [{ key: 'host-one', ...total(first) }], other: total(remaining), missing: total(buckets.map(() => 0)),
      untimed: 2, outsideGrid: 1, limit: request.args.limit, selection: 'top',
      context: { analysis: plain(request.args.analysisContext), sourceGeneration: request.scope === 'case' ? null : request.args.sourceGeneration, caseKey: request.args.caseKey ?? null }, ...overrides };
  }
  function summary(status = 'done', stats = sparseStats()) {
    const entry = { status, stats, resume() { resumeCount++; this.status = 'queued'; } };
    explorerAnalytics.set(explorerKey(), entry);
    timeline.summary(entry);
    return entry;
  }
  return { context, timeline, state, requests, cancelled, plots, messages, captured, callsToSave, $, find, response, summary,
    expected: request => ({ field: request.args.field, grid: request.args.grid, owner: request.opts.analysisOwner,
      caseKey: request.args.caseKey ?? null, scope: request.scope, limit: request.args.limit }),
    prepare: handler => { casePreparation = handler; }, setScope: value => { scope = value; }, setCaseVersion: value => { caseVersion = value; },
    get resumeCount() { return resumeCount; }, get plot() { return context.chart; } };
}

async function groupedFixture(options = {}) {
  const f = fixture(options);
  f.timeline.render(sparseStats()); f.timeline.select('source'); await settle();
  f.requests[0].resolve(f.response()); await settle();
  return f;
}

test('sparse statistics preserve the exact temporal grid with explicit zero buckets', () => {
  const f = fixture();
  assert.deepEqual(plain(f.timeline.gridFor(sparseStats())), { start: 1000, bucketMs: 1000, bucketCount: 3 });
  f.timeline.render(sparseStats());
  assert.deepEqual(plain(f.plot.data), [[1, 2, 3], [3, 0, 3]]);
  assert.equal(f.requests.length, 0, 'plain total reuses the existing histogram data');
  assert.equal(f.plot.options.series[1].label, 'Total no período');
  assert.deepEqual(plain(f.timeline.gridFor({ buckets: [[1000, 5]], bucket_ms: 500 })), { start: 1000, bucketMs: 500, bucketCount: 1 });
});

test('empty statistics display an empty timeline, and invalid temporal grids are rejected', () => {
  const f = fixture();
  f.timeline.render({ buckets: [] });
  assert.equal(f.plot, null);
  assert.match(f.$('#chart').children[0].textContent, /Sem dados temporais/);
  for (const value of [
    { buckets: [[1000.5, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1]], bucketMs: 0 },
    { buckets: [[1000, 1], [241000, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1], [2500, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1], [1500, 1], [3000, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1], [3000.5, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1], [1000, 2], [3000, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1], [3000, 1], [2000, 1]], bucketMs: 1000 },
    { buckets: [[1000, 1.5]], bucketMs: 1000 },
    { buckets: [[1000, -1]], bucketMs: 1000 },
  ]) assert.throws(() => f.timeline.gridFor(value), /Grade temporal inválida/);
});

test('field selection adds no filter and forwards exact grouped_timeline ownership and grid for Dataset and Case', async () => {
  for (const scope of ['dataset', 'case']) {
    const filter = { column: 'message', op: 'contains', value: 'needle', value2: null };
    const f = fixture({ scope, filters: [filter] });
    f.timeline.render(sparseStats());
    const filters = f.state.filters;
    f.timeline.menuItem('host.name').onClick(); await settle();
    assert.equal(f.state.filters, filters);
    assert.deepEqual(f.state.filters, [filter]);
    assert.equal(f.requests.length, 1);
    const request = f.requests[0];
    assert.equal(request.cmd, 'grouped_timeline');
    assert.deepEqual(plain(request.args), {
      field: 'host.name', grid: { start: 1000, bucketMs: 1000, bucketCount: 3 }, filters: [filter], limit: 12,
      analysisContext: f.state.analysis, sourceGeneration: 7, ...(scope === 'case' ? { caseKey: 'evidence-1' } : {}),
    });
    assert.equal(request.opts.latest, 'explore-timeline');
    assert.equal(request.opts.silent, true);
    assert.deepEqual(plain(request.opts.analysisOwner.identity), f.state.analysis);
    request.resolve(f.response(request)); await settle();
    assert.match(f.find('explorer-timeline-note').textContent, /grupos de maior volume/);
  }
});

test('full group keys, the empty value, missing field and Other remain distinct and conserve counts', async () => {
  const f = fixture(); f.timeline.render(sparseStats()); f.timeline.select('source'); await settle();
  const keyA = `${'long-full-value-'.repeat(5)}A`, keyB = `${'long-full-value-'.repeat(5)}B`;
  const result = f.response(undefined, {
    total: total([6, 0, 5]),
    series: [{ key: keyA, ...total([1, 0, 1]) }, { key: keyB, ...total([1, 0, 1]) }, { key: '', ...total([2, 0, 1]) }],
    other: total([1, 0, 1]), missing: total([1, 0, 1]),
  });
  assert.equal(f.timeline.validate(result, f.expected(f.requests[0])), result);
  f.requests[0].resolve(result); await settle();
  const labels = f.plot.options.series.slice(1).map(item => item.label);
  assert.deepEqual(plain(labels), ['Total no período', keyA, keyB, '(valor vazio)', 'Outros valores', 'Campo ausente']);
  const legend = f.find('explorer-timeline-legend');
  assert.match(legend.children[1].title, new RegExp(`${keyA} · 2`));
  assert.match(legend.children[2].title, new RegExp(`${keyB} · 2`));
  assert.notEqual(legend.children[1].children[0].attrs['aria-label'], legend.children[2].children[0].attrs['aria-label']);
  assert.match(f.find('explorer-timeline-note').textContent, /2 sem horário/);
  assert.match(f.find('explorer-timeline-note').textContent, /1 fora da grade/);
});

test('malformed, duplicate, incomplete and nonconserving grouped responses are rejected', async () => {
  const f = fixture(); f.timeline.render(sparseStats()); f.timeline.select('source'); await settle();
  const expected = f.expected(f.requests[0]);
  const mutations = [
    value => { value.total.count++; },
    value => { value.total.buckets[0]++; value.total.count++; },
    value => { value.series[0].buckets.pop(); },
    value => { value.other.buckets[0] = -1; },
    value => { value.missing.count = 0.5; },
    value => { value.series.push(structuredClone(value.series[0])); },
    value => { value.series[0].key = null; },
    value => { delete value.other; },
    value => { delete value.missing; },
    value => { value.untimed = -1; },
    value => { value.outsideGrid = Number.MAX_SAFE_INTEGER + 1; },
  ];
  for (const mutate of mutations) {
    const result = f.response(); mutate(result);
    assert.throws(() => f.timeline.validate(result, expected));
  }
  f.requests[0].resolve(f.response(undefined, { total: total([999, 0, 999]) })); await settle();
  assert.match(f.find('explorer-timeline-note').textContent, /Agrupamento não concluído/);
  assert.equal(f.plot.options.series.length, 2, 'invalid grouped data never appears in the plot');
});

test('response field, grid, limit, selection and exact context must match the request', async () => {
  for (const scope of ['dataset', 'case']) {
    const f = fixture({ scope }); f.timeline.render(sparseStats()); f.timeline.select('source'); await settle();
    const expected = f.expected(f.requests[0]);
    const mutations = [
      value => { value.field = 'different'; }, value => { value.grid.start++; }, value => { value.grid.bucketMs++; },
      value => { value.grid.bucketCount++; }, value => { value.limit = 24; }, value => { value.selection = 'all'; },
      value => { value.context.analysis.configRevision++; }, value => { value.context.analysis.visibilityRevision++; },
      value => { value.context.analysis.analysisId = 'other'; }, value => { value.context.analysis.caseId = 'other'; },
      value => { value.context.caseKey = 'foreign-key'; }, value => { value.context.sourceGeneration = 999; },
    ];
    for (const mutate of mutations) {
      const result = f.response(); mutate(result);
      assert.throws(() => f.timeline.validate(result, expected), /outro contexto/);
    }
    const valid = f.response();
    assert.equal(valid.context.sourceGeneration, scope === 'case' ? null : 7);
    assert.equal(f.timeline.validate(valid, expected), valid);
  }
});

test('legend visibility and clearing grouping reuse loaded data without making queries or changing filters', async () => {
  const f = await groupedFixture();
  const before = f.requests.length, filters = f.state.filters, plot = f.plot;
  const checkbox = f.find('explorer-timeline-legend').children[1].children[0];
  checkbox.checked = false; checkbox.onchange();
  assert.deepEqual(plot.visibility, [{ index: 2, show: false }]);
  assert.deepEqual(plain(f.timeline.capture().hidden), [JSON.stringify(['value', 'source', 'host-one'])]);
  checkbox.checked = true; checkbox.onchange();
  assert.equal(f.timeline.capture().hidden.length, 0);
  const clear = f.find('explorer-timeline-heading').children.find(item => item.textContent === 'Limpar agrupamento');
  clear.onclick(); await settle();
  assert.equal(f.requests.length, before);
  assert.equal(f.state.filters, filters);
  assert.equal(f.timeline.capture().field, null);
  assert.deepEqual(plain(f.plot.data), [[1, 2, 3], [3, 0, 3]]);
  assert.equal(f.find('explorer-timeline-legend').children.length, 1);
});

test('same-context field changes retain prior series with their previous field named until new data arrives', async () => {
  const f = await groupedFixture();
  const previous = plain(f.plot.data);
  f.timeline.select('host'); await settle();
  assert.equal(f.requests.length, 2);
  assert.deepEqual(plain(f.plot.data), previous);
  assert.match(f.find('explorer-timeline-note').textContent, /Séries anteriores: source/);
  assert.match(f.find('explorer-timeline-note').textContent, /Calculando host/);
  f.requests[1].resolve(f.response(f.requests[1], { series: [{ key: 'host-two', ...total([3, 0, 0]) }] })); await settle();
  assert.doesNotMatch(f.find('explorer-timeline-note').textContent, /Séries anteriores/);
  assert.equal(f.plot.options.series[2].label, 'host-two');
});

test('filter, source, revision and grid changes discard old series and late responses', async () => {
  for (const change of ['filter', 'source', 'config', 'visibility', 'dataset-revision', 'grid']) {
    const f = await groupedFixture();
    f.timeline.select('host'); await settle();
    const old = f.requests[1];
    let stats = sparseStats();
    if (change === 'filter') f.state.filters = [{ column: 'message', op: 'contains', value: 'new' }];
    if (change === 'source') { f.state.sourcePublication.generation++; f.state.currentArtifact.id = 'source-two'; }
    if (change === 'config') f.state.analysis.configRevision++;
    if (change === 'visibility') f.state.analysis.visibilityRevision++;
    if (change === 'dataset-revision') f.state.datasetRevision++;
    if (change === 'grid') stats = { buckets: [[4000, 3], [6000, 3]], bucketMs: 1000 };
    f.timeline.render(stats); await settle();
    assert.equal(f.plot.options.series.length, 2, `${change}: only current total remains until a current grouped response`);
    assert.doesNotMatch(f.find('explorer-timeline-note').textContent, /Séries anteriores/, change);
    const current = f.requests.at(-1);
    current.resolve(f.response(current, { series: [{ key: `current-${change}`, ...total([3, 0, 0]) }] })); await settle();
    old.resolve(f.response(old, { series: [{ key: 'obsolete', ...total([3, 0, 0]) }] })); await settle();
    assert.equal(f.plot.options.series[2].label, `current-${change}`);
    assert.equal(f.find('explorer-timeline-note').textContent.includes('obsolete'), false);
  }
});

test('field selection after a context change cannot retain old series while waiting for the next summary', async () => {
  for (const change of ['filter', 'source', 'revision']) {
    const f = await groupedFixture();
    if (change === 'filter') f.state.filters = [{ column: 'message', op: 'contains', value: 'new' }];
    if (change === 'source') f.state.sourcePublication.generation++;
    if (change === 'revision') f.state.analysis.visibilityRevision++;
    // The new summary has not arrived yet; the field action is already owned by the new context.
    f.timeline.menuItem('host').onClick(); await settle();
    assert.ok(!f.plot || f.plot.options.series.length <= 2, `${change}: old grouped series must be removed immediately`);
    assert.doesNotMatch(f.find('explorer-timeline-note').textContent, /Séries anteriores/, change);
    assert.equal(f.requests.length, 1, 'new grouped work must wait for current histogram data');
  }
});

test('ownership is captured before async Case preparation and stale preparation never dispatches', async () => {
  const f = fixture({ scope: 'case' }), gate = deferred();
  f.prepare(() => gate.promise);
  f.timeline.render(sparseStats()); f.timeline.select('source'); await settle();
  assert.equal(f.requests.length, 0);
  f.state.analysis.configRevision++;
  gate.resolve(); await settle();
  assert.equal(f.requests.length, 0);
  assert.equal(f.plot.options.series.length, 2);
});

test('summary pause cancels grouped work and resume restarts only the unfinished grouped request', async () => {
  const f = fixture();
  const entry = f.summary();
  f.timeline.select('source'); await settle();
  const first = f.requests[0], rows = f.state.rows;
  entry.status = 'paused'; f.timeline.summary(entry); await settle();
  assert.equal(f.cancelled.at(-1), 'explore-timeline');
  assert.match(f.find('explorer-timeline-note').textContent, /pausado/);
  first.resolve(f.response(first)); await settle();
  assert.equal(f.plot.options.series.length, 2);
  f.timeline.resume(); assert.equal(f.resumeCount, 1);
  entry.status = 'stats'; f.timeline.summary(entry); await settle();
  assert.equal(f.requests.length, 2);
  f.requests[1].resolve(f.response(f.requests[1])); await settle();
  assert.match(f.find('explorer-timeline-note').textContent, /grupos de maior volume/);
  assert.equal(f.state.rows, rows);
  assert.ok(f.requests.every(request => request.cmd === 'grouped_timeline'));
});

test('manual pause, latest field changes and repeated renders cannot revive canceled grouped results', async () => {
  const f = fixture(); f.timeline.render(sparseStats()); f.timeline.select('source'); await settle();
  const old = f.requests[0];
  f.timeline.pause(); f.timeline.render(sparseStats()); await settle();
  assert.equal(f.requests.length, 1, 'manual pause survives an unchanged summary render');
  f.timeline.resume(); await settle();
  assert.equal(f.requests.length, 2);
  const resumed = f.requests[1];
  f.timeline.select('host'); await settle();
  assert.equal(f.requests.length, 3);
  f.timeline.render(sparseStats()); await settle();
  assert.equal(f.requests.length, 3, 'an identical render joins the pending grouped request');
  f.requests[2].resolve(f.response(f.requests[2], { series: [{ key: 'new-host', ...total([3, 0, 0]) }] })); await settle();
  old.resolve(f.response(old)); resumed.reject(Error('Operação cancelada.')); await settle();
  assert.equal(f.plot.options.series[2].label, 'new-host');
  assert.match(f.find('explorer-timeline-note').textContent, /grupos de maior volume/);
  assert.ok(f.cancelled.every(key => key === 'explore-timeline'));
});

test('capture and restore preserve grouping preferences, sanitize input and never carry computed results', async () => {
  const f = await groupedFixture();
  const checkbox = f.find('explorer-timeline-legend').children[1].children[0];
  checkbox.checked = false; checkbox.onchange();
  const saved = f.timeline.capture();
  saved.hidden.push('not-live');
  assert.equal(f.timeline.capture().hidden.includes('not-live'), false, 'capture returns an independent copy');
  const restored = fixture(); restored.timeline.restore({ ...saved, limit: 24 });
  assert.equal(restored.requests.length, 0);
  assert.equal(restored.timeline.capture().limit, 24);
  restored.timeline.render(sparseStats()); await settle();
  assert.equal(restored.requests[0].args.field, 'source');
  assert.equal(restored.requests[0].args.limit, 24);
  assert.equal(restored.plot.options.series.length, 2, 'computed data must be loaded for the restored context');
  restored.requests[0].resolve(restored.response()); await settle();
  assert.equal(restored.plot.options.series[2].show, false);
  restored.timeline.restore({ field: 4, limit: 999, hidden: [true, null, 'total'] });
  assert.deepEqual(plain(restored.timeline.capture()), { field: null, limit: 12, hidden: ['total'] });
});

test('stale context menu actions never select a field or add filters', async () => {
  const f = fixture(); f.timeline.render(sparseStats());
  const item = f.timeline.menuItem('source');
  f.state.analysis.visibilityRevision++;
  item.onClick(); await settle();
  assert.equal(f.timeline.capture().field, null);
  assert.equal(f.requests.length, 0);
  assert.equal(f.state.filters.length, 0);
  assert.match(f.messages[0], /contexto mudou/);
});
