import assert from 'node:assert/strict';
import vm from 'node:vm';
import {installCanonicalFields} from './helpers/canonical-fields-fixture.mjs';
import { readFileSync } from 'node:fs';
const read = file => readFileSync(new URL(`../../frontend/${file}`, import.meta.url), 'utf8');
const app = read('app.js'), analysis = read('analysis-workbench.js'), discovery = read('discovery.js');
const part = (source, start, end) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
let scope = 'dataset', shown = [], drafts = [], applies = 0, scopeChanges = 0, caseId = 'case-a', evidenceRevision = 1; const notices=[];
const anchor = { isConnected: true }, event = { preventDefault() {}, stopPropagation() {}, target: anchor, currentTarget: anchor, clientX: 12, clientY: 34 };
const context = vm.createContext({
  state: { columns: ['timestamp', 'request_path'], visibleCols: [], filters: [], analyticsScope: 'dataset', page: 0 }, window: {},
  colLabel: String, trunc: String, workspaceScope: () => scope, activeCase: () => ({id:caseId}), caseSig: () => `${caseId}:${evidenceRevision}`, toast: text => notices.push(text),
  showCtxMenu: (x,y,items) => { shown = items; }, openValueFilter: (...args) => drafts.push(args),
  addFilter: () => { applies++; }, filtersChanged: () => { applies++; },
  toggleDetailColumn() {}, detailFieldFilterValue: node => String(node.filterValue ?? ''), openDetailValue() {},
  showFieldInspector() {}, cubeAdd() {}, addGroupToAnalysis() {}, switchView() {}, switchTab() {},
  navigator: { clipboard: { writeText: async () => {} } },
});
installCanonicalFields(context);context.$=()=>({hidden:false});context.detailRequest=0;context.detailAdmissionCurrent=()=>true;
context.state.detailAdmission={scope:'dataset'};context.state.currentDetailEv={id:1,event_ref:'fixture:1',timestamp:1700000000123,fields:{}};
vm.runInContext(part(app,'function detailCanonicalAction(', '\nfunction toggleDetailColumn('),context);
context.window.WorkspaceContext = { setScope: async next => { scopeChanges++; scope = next; } };
context.window.Discovery = { applySelection: () => { applies++; } };
vm.runInContext(part(app, 'function valueFilterMenuItem(', '\nfunction commitQuickSearch('), context);
vm.runInContext(part(app, 'function showChartValueActions(', '\nfunction renderHBars('), context);
vm.runInContext(part(app, 'function showCubeValueActions(', '\nfunction legacyRenderCubeTable('), context);
vm.runInContext(part(app, 'function showCubeFieldActions(', '\nfunction beginCubeDrag('), context);
vm.runInContext(part(app, 'function showDetailNameMenu(', '\nfunction showDetailValueMenu('), context);
const open = async column => {
  const action = shown.find(item => item.label === `Criar filtro: ${column}`); assert.ok(action, `composer for ${column}`); await action.onClick(); return drafts.at(-1);
};
for (const value of [null, '', 0, false, '(vazio)', '/api?a=1&b=2', '  literal  ']) {
  context.showChartValueActions(event, { field: 'request_path', chart: 'terms' }, value, 'dataset');
  const draft = await open('request_path');
  assert.equal(draft[0], 'request_path'); assert.equal(draft[1], value, 'raw values are not replaced by formatted labels');
  assert.equal(draft[2], anchor); assert.equal(draft[3], value == null ? 'empty' : 'equals_exact');
}
context.showCubeValueActions(event, 'request_path', '/cube'); assert.equal((await open('request_path'))[1], '/cube');
context.showCubeFieldActions(event, 'request_path'); assert.equal((await open('request_path'))[3], 'contains');
context.showDetailNameMenu(event, { path: 'timestamp', hasValue: true, filterValue: 1700000000123 });
assert.equal(Date.parse((await open('timestamp'))[1]), 1700000000123);
context.showDetailNameMenu(event, { path: 'request_path', hasValue: false }); assert.equal((await open('request_path'))[3], 'empty');
context.showDetailNameMenu(event, { path: 'unknown.child', hasValue: true, filterValue: 'x' });
assert.ok(!shown.some(item => item.label?.startsWith('Criar filtro:')), 'a display-only child is not invented as a queryable column');
context.showChartValueActions(event, { field: 'request_path', chart: 'terms' }, '/case', 'case');
await open('request_path'); assert.equal(scope, 'case'); assert.equal(scopeChanges, 1);
context.window.WorkspaceContext.setScope = async () => {};
scope = 'dataset'; context.showChartValueActions(event, { field: 'request_path', chart: 'terms' }, '/blocked', 'case');
const before = drafts.length; await open('request_path'); assert.equal(drafts.length, before, 'refused area switch cannot target the wrong dataset');

// The real group menu handler uses canonical group values, including literal '(vazio)'.
Object.assign(context, { tr: anchor, field: 'request_path', value: '(vazio)', filterGroup() {}, openNamePop() {} });
vm.runInContext(analysis.match(/  const exactFilter = .*;/)[0], context);
const groupHandler = part(analysis, '      tr.oncontextmenu = event =>', '\n      for (const [index, key]');
vm.runInContext(groupHandler, context); anchor.oncontextmenu(event);
assert.equal((await open('request_path'))[1], '(vazio)'); assert.equal(drafts.at(-1)[3], 'equals_exact');

// Pivot menus expose source dimensions, not the aggregated metric cell value.
context.emptyFilter = (field, value) => ({ column: field, op: value === '(vazio)' ? 'empty' : 'equals', value: value === '(vazio)' ? '' : String(value) });
vm.runInContext(part(analysis, '  function pivotFilters(', '  renderCubeTable ='), context);
context.pivotMenu(event, { rows: ['request_path', 'status'], cols: ['host'] }, ['/checkout', 503], 'formatted', ['nginx-a'], true);
for (const [column, value] of [['request_path','/checkout'], ['status','503'], ['host','nginx-a']]) {
  assert.equal((await open(column))[1], value);
}
assert.equal(shown.filter(item => item.label?.startsWith('Criar filtro:')).length, 3);

// Discovery's actual ranking handler uses raw x_values, even with a formatted label.
const rows = [], box = { append: row => rows.push(row) };
Object.assign(context, { button: () => ({}), rawCategory: (raw, label) => raw === undefined ? label : raw,
  valueFilter: (field, value) => ({column:field, op:value == null || value === '' ? 'empty' : 'equals_exact', value}),
  applySelection() { applies++; }, esc: String, fmtNum: String, addExplanation() {},
});
vm.runInContext(part(discovery, '  function renderRanking(', '  function renderInteractiveLine('), context);
context.renderRanking(box, { x:['display label'], x_values:['raw value'], series:[{points:[7]}] }, {field:'request_path'}, 'dataset');
rows[0].oncontextmenu(event); assert.equal((await open('request_path'))[1], 'raw value');
assert.equal(applies, 0, 'opening any composer never applies a filter or runs data work');
console.log('Chart, cube, grouped, detail-name and Discovery menus preserve editable raw filter values');

// A delayed workspace transition must not reopen an old value over a new Case/source.
context.state.currentArtifact={id:'source-a',loadedAt:1};context.state.datasetRevision=1;context.state.derivedFields=[];
for(const mutate of [()=>caseId='case-b',()=>context.state.datasetRevision++,()=>context.state.currentArtifact.id='source-b',()=>context.state.currentArtifact.loadedAt++,()=>context.state.sourceIdentityUnconfirmed=true,()=>context.state.derivedFields.push({name:'changed'}),()=>evidenceRevision++]) {
  scope='dataset';caseId='case-a';context.state.sourceIdentityUnconfirmed=false;let release;
  context.window.WorkspaceContext.setScope=async next=>{await new Promise(resolve=>release=resolve);scope=next;};
  const action=context.valueFilterMenuItem('source','value-from-old-context',null,{scope:'case'});
  const before=drafts.length,pending=action.onClick();mutate();release();await pending;
  assert.equal(drafts.length,before,'old menu must not populate an editor after an awaited context replacement');
  assert.match(notices.at(-1),/Abra o menu novamente/);
}
let switches=0;scope='dataset';context.window.WorkspaceContext.setScope=async next=>{switches++;scope=next;};
const stale=context.valueFilterMenuItem('source','old',null,{scope:'case'});caseId='case-c';await stale.onClick();
assert.equal(switches,0,'a menu invalidated before click does not even switch areas');
const current=context.valueFilterMenuItem('source','current',null,{scope:'case'});const count=drafts.length;await current.onClick();
assert.equal(drafts.length,count+1);assert.equal(drafts.at(-1)[1],'current','unchanged cross-scope opening remains supported');
console.log('Deferred menu-to-composer transitions reject replaced Case/source/config identities');
