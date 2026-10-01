import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const core = readFileSync(new URL('../../frontend/performance-core.js', import.meta.url), 'utf8');
const code = app.slice(app.indexOf('const explorerAnalytics ='), app.indexOf('\nfunction scheduleRefresh()'));
const tick = () => new Promise(resolve => setTimeout(resolve, 10));
function setup({ knownTotal = null, rejectCancelled = true } = {}) {
  const nodes = new Map(), calls = [], pending = [], cancelled = [], bars = [];
  const node = id => {
    if (!nodes.has(id)) { const classes = new Set(); nodes.set(id, { classList: { add: c => classes.add(c), contains: c => classes.has(c), toggle(c,on) { if (on) classes.add(c); else classes.delete(c); } }, closest: () => node(".hist-panel"), attrs: {}, children: [], hidden: false, textContent: '', setAttribute(k,v) { this.attrs[k]=v; }, getAttribute(k) { return this.attrs[k]; }, replaceChildren(...children) { this.children = children; }, append(child) { this.children.push(child); }, before() {} }); }
    return nodes.get(id);
  };
  const state = { loaded: true, rows: [], total: null, currentArtifact: { id: 'source', loadedAt: 1 }, datasetRevision: 1, filters: [], derivedFields: [], sortCol: 'timestamp', sortDir: 'desc', pageSize: 100, page: 0, refreshVersion: 0, activeDatasetTab: 'table' };
  const context = vm.createContext({ window: {}, performance, setTimeout, clearTimeout, console, state, $: node, el: (tag,cls,text) => ({ tag, className: cls, textContent: text }), document: { createTextNode: String, body: { dataset: { page: "explore" } } }, chart: null, setWorkbar: (...args) => bars.push(args),
    workspaceScope: () => 'dataset', activeCase: () => null, caseSig: () => '', backendFilters: () => state.filters, caseEvents: () => [], fmtNum: String,
    renderTable() {}, renderChips() {}, updateContextBar() {}, renderChart() {}, refreshTreeAggs: async () => {},
    api: async (cmd,args,opts) => {
      calls.push({cmd,args,opts});
      if (cmd === 'query_page') return { rows: [{id:0}], total: knownTotal, hasMore: true, nextCursor: 'next' };
      if (cmd === 'engine_status') return null;
      return new Promise((resolve,reject) => pending.push({cmd,resolve,reject,latest:opts?.latest}));
    }
  });
  vm.runInContext(core, context);
  context.window.Tasks = { cancelLatest(key) { cancelled.push(key); if (rejectCancelled) for (const task of pending.filter(p => p.latest === key)) task.reject(Error('Operação cancelada.')); } };
  context.window.Workspace = { onRefresh() {}, onCountChanged() {} };
  vm.runInContext(code, context);
  vm.runInContext(app.slice(app.indexOf("function updateContextBar()"),app.indexOf("const esc =")),context);
  vm.runInContext(app.match(/function currentCountLabel\([^\n]+/)[0],context);
  return { context, state, calls, pending, node, cancelled, bars, entry: () => context.loadExplorerAnalytics(context.explorerKey(), 'dataset', state.filters) };
}

// Pause keeps visible rows; resume repeats only the unfinished stage.
{
  const f = setup(); await f.context.refresh(); await tick();
  const entry = f.entry(); assert.equal(entry.status, 'count');
  const rows = f.state.rows; entry.pause(); await tick();
  assert.equal(entry.status, 'paused'); assert.equal(f.state.rows, rows);
  assert.match(f.node('#result-count').textContent, /total pausado/);
  assert.deepEqual(f.cancelled.slice(-3), ['explore-count','explore-stats','explore-tree']);
  entry.resume(); await tick();
  f.pending.filter(p => p.cmd === 'count_filtered').at(-1).resolve(77); await tick();
  assert.equal(entry.status, 'stats'); assert.equal(f.state.total, 77);
  entry.pause(); await tick(); entry.resume(); await tick();
  assert.equal(f.calls.filter(c => c.cmd === 'count_filtered').length, 2, 'completed count is reused after pausing histogram');
  f.pending.filter(p => p.cmd === 'stats_events').at(-1).resolve({buckets:[],levels:[]}); await entry.promise;
  assert.equal(entry.status, 'done'); assert.equal(f.node('#query-analytics-status').hidden, true);
  assert.equal(f.calls.filter(c => c.cmd === 'query_page').length, 1, 'summary controls never reload event rows');
}

// An exact total already proven by the page is not counted a second time.
{
  const f = setup({knownTotal: 2}); await f.context.refresh(); await tick();
  assert.equal(f.calls.filter(c => c.cmd === 'count_filtered').length, 0);
  assert.equal(f.state.total, 2);
  f.pending.at(-1).resolve({buckets:[],levels:[]}); await f.entry().promise;
}

// A→B→A while cancellation is slow must not poison or remove the resumed entry.
{
  const f = setup({rejectCancelled:false});
  f.state.filters = [{column:'source',op:'equals_exact',value:'A'}];
  await f.context.refresh(); await tick(); const first = f.pending[0], entry = f.entry();
  f.state.filters = [{column:'source',op:'equals_exact',value:'B'}]; await f.context.refresh();
  f.state.filters = [{column:'source',op:'equals_exact',value:'A'}]; await f.context.refresh();
  assert.equal(f.entry(), entry);
  first.reject(Error('Operação cancelada.')); await tick(); await tick();
  assert.equal(f.entry(), entry); assert.equal(entry.status, 'count');
  f.pending.filter(p => p.cmd === 'count_filtered').at(-1).resolve(42); await tick();
  f.pending.filter(p => p.cmd === 'stats_events').at(-1).resolve({buckets:[],levels:[]}); await entry.promise;
  assert.equal(f.state.total, 42); assert.equal(entry.status, 'done');
  assert.equal(f.calls.filter(c => c.cmd === 'count_filtered').length, 2, 'stale B work never starts');
}

// Failures show a resumable state instead of an eternal total-in-progress label.
{
  const f = setup(); await f.context.refresh(); await tick();
  f.pending[0].reject(Error('Insufficient query spill space')); await f.entry().promise;
  assert.equal(f.entry().status, 'failed'); assert.match(f.node('#result-count').textContent, /total não concluído/);
  assert.match(f.node('#query-analytics-status').children[0], /Insufficient query spill space/);
  f.entry().resume(); await tick(); f.pending.at(-1).resolve(1); await tick();
  f.pending.at(-1).resolve({buckets:[],levels:[]}); await f.entry().promise;
  assert.equal(f.entry().status, 'done'); assert.equal(f.state.rows.length, 1);
}
console.log('Summary pause/resume, exact-total reuse, stale generations and failure recovery passed');

// Every surface reflects the same current summary state, and blank charts collapse.
{
  const f = setup(); await f.context.refresh(); await tick(); const entry = f.entry();
  entry.pause(); await tick();
  assert.match(f.node('#context-summary').textContent, /pausado/);
  assert.match(f.context.currentCountLabel('eventos'), /pausado/);
  assert.equal(f.bars.at(-1)[0], 'Resumo pausado');
  assert.equal(f.bars.at(-1)[4], 'paused');
  assert.match(f.node('#chart').children[0].textContent, /Histograma pausado/);
  assert.equal(f.node('.hist-panel').classList.contains('analytics-placeholder'), true);
  entry.status = 'queued'; f.context.showExplorerAnalytics(entry);
  assert.match(f.node('#context-summary').textContent, /aguardando cálculo/);
  assert.equal(f.bars.at(-1)[0], 'Resumo na fila');
  entry.status = 'failed'; entry.error = 'Spill budget'; f.context.showExplorerAnalytics(entry);
  assert.match(f.node('#context-summary').textContent, /não concluído/);
  assert.match(f.node('#chart').children[0].textContent, /não concluído/);
  assert.equal(f.bars.at(-1)[0], 'Resumo não concluído');
  const barsBefore = f.bars.length;
  for (const owner of [{loadOverlay:true},{activeOperation:{kind:'load'}},{progressOperationId:'another-task'}]) {
    Object.assign(f.state,{loadOverlay:false,activeOperation:null,progressOperationId:null},owner);
    f.context.showExplorerAnalytics(entry);
  }
  assert.equal(f.bars.length,barsBefore,'summary state never overwrites another operation or import overlay');
  const chart = {destroy(){throw Error('A completed current histogram must remain visible');}};
  f.context.chart=chart;entry.stats={buckets:[[1,2]]};entry.status='paused';
  const contents=f.node('#chart').children;f.context.showExplorerAnalytics(entry);
  assert.equal(f.context.chart,chart);assert.equal(f.node('#chart').children,contents);
}
// Facet failure is not promoted to a completed summary merely because rows/counts succeeded.
{
  const f=setup();f.context.refreshTreeAggs=async()=>{f.state.treeAggError={dataset:'Grouping payload limit'};};
  await f.context.refresh();await tick();f.pending.at(-1).resolve(12);await tick();
  f.pending.at(-1).resolve({buckets:[],levels:[]});await f.entry().promise;
  assert.equal(f.entry().status,'failed');assert.match(f.entry().error,/payload limit/);
  assert.equal(f.state.total,12);assert.equal(f.context.currentCountLabel('eventos'),'12 eventos','valid completed counts are preserved');
  assert.equal(f.bars.at(-1)[0],'Resumo não concluído');
}
console.log('Header, pager, footer and compact histogram use honest queued/paused/failed summary states');
