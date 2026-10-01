import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const barSource = readFileSync(new URL('../../frontend/query-bar.js', import.meta.url), 'utf8');
const workspaceSource = readFileSync(new URL('../../frontend/workspace-context.js', import.meta.url), 'utf8');
const timers = new Map(), nodes = new Map(); let timerId = 0, nativeCalls = 0, saves = 0;
const document = { body: { dataset: {}, append() {} }, activeElement: null, listeners: {},
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); },
  querySelector(selector) { return node(selector); },
};
function element(tag = 'div', className = '') {
  const classes = new Set(className.split(' '));
  return { tag, className, value: '', hidden: false, innerHTML: '', textContent: '', style: {}, dataset: {}, listeners: {}, selectionStart: 0, selectionEnd: 0, selectionDirection: 'none',
    attributes: {}, classList: { add: c => classes.add(c), contains: c => classes.has(c), toggle(c, on) { if (on ?? !classes.has(c)) classes.add(c); else classes.delete(c); } },
    setAttribute(key,value) { this.attributes[key]=String(value); }, removeAttribute(key) { delete this.attributes[key]; }, append() {}, after() {}, closest() { return node('.search-box'); }, getBoundingClientRect() { return { left: 5, bottom: 15, width: 300 }; },
    addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); },
    dispatchEvent(event) { for (const fn of this.listeners[event.type] || []) fn(event); },
    setSelectionRange(start, end, direction = 'none') { Object.assign(this, { selectionStart: start, selectionEnd: end, selectionDirection: direction }); },
    focus() { document.activeElement = this; this.dispatchEvent({ type: 'focus' }); },
  };
}
function node(selector) { if (!nodes.has(selector)) nodes.set(selector, element()); return nodes.get(selector); }
const state = { filters: [], quick: 'legacy applied', rows: [], columns: ['request_path'], derivedFields: [], datasetRevision: 1, treeAgg: {}, treeAggSig: {} };
let activeScope = 'dataset', source = 'source-a', caseId = 'a', ruleRequest = null;
const context = vm.createContext({ console, window: {}, document, state, structuredClone, Map, Set, Promise, innerWidth: 1000,
  $: node, el: (tag, cls = '') => { const result = element(tag, cls); if (cls) nodes.set(`.${cls}`, result); return result; },
  esc: text => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;'), colLabel: String, fmtNum: String,
  workspaceScope: () => activeScope, backendFilters: () => state.filters, cellValue: (event, field) => event[field],
  setTimeout: fn => { timers.set(++timerId, fn); return timerId; }, clearTimeout: id => timers.delete(id),
  Event: class { constructor(type) { this.type = type; } }, api: () => { nativeCalls++; throw Error('A draft must not query'); },
  commitQuickSearch: () => true, saveCases: () => { saves++; },
});
context.window.Workspace = { sourceKey: () => source, capture: () => ({}), restore() {} };
context.window.QueryLang = { resolve: field => ({ name: field }), fieldValue: (event, field) => event[field.name], validate: value => value.endsWith(':') ? 'Incomplete expression' : null };
context.window.Security = { rules: () => ruleRequest };
node('#quick-search').value='request_path:';node('#quick-search').setSelectionRange(3,8,'backward');
node('#btn-add-search').disabled=true;
vm.runInContext(barSource, context);
assert.equal(node('#quick-search').value,'request_path:');assert.equal(node('#btn-add-search').disabled,false,'prefilled startup draft enables Add without an input event');
assert.equal(node('#quick-search').attributes['aria-invalid'],'true','prefilled startup draft is validated');
assert.equal(node('#quick-search').selectionStart,3);assert.equal(node('#quick-search').selectionEnd,8);
assert.equal(document.activeElement,null,'startup synchronization does not steal focus');assert.equal(nativeCalls,0);assert.equal(saves,0);

const bar = context.window.QueryBar, input = node('#quick-search'), list = node('.query-suggest');
assert.match(input.attributes['aria-label'],/Enter/);
bar.status('Incomplete expression');assert.equal(input.attributes['aria-invalid'],'true');assert.equal(input.attributes['aria-errormessage'],'quick-search-error');
bar.status(null);assert.equal(input.attributes['aria-invalid'],'false');assert.equal(input.attributes['aria-errormessage'],undefined);
const tick = async () => { await Promise.resolve(); await Promise.resolve(); };
const flush = async () => { const pending = [...timers.values()]; timers.clear(); for (const fn of pending) fn(); await tick(); };
const type = async text => { input.value = text; input.setSelectionRange(text.length, text.length); document.activeElement = input; input.dispatchEvent({ type: 'input' }); await flush(); };

// An early empty result is invalidated by new page rows and then completed facets.
await type('request_path:'); assert.equal(list.hidden, true);
state.rows = [{ request_path: '/one' }, { request_path: '/one' }];
await type('request_path:'); assert.match(list.innerHTML, /\/one/); assert.match(list.innerHTML, /2 nesta página/);
state.rows = [{ request_path: '/two' }];
await type('request_path:'); assert.match(list.innerHTML, /\/two/); assert.doesNotMatch(list.innerHTML, /\/one/);
state.treeAgg.dataset = { request_path: [['/all', 400]] };
state.treeAggSig.dataset = `dataset|1|[]|[]|request_path|source-a`;
await type('request_path:'); assert.match(list.innerHTML, /\/all/); assert.match(list.innerHTML, /400 no painel de campos/);
state.filters = [{ column: 'source', op: 'equals_exact', value: 'nginx' }]; source = 'source-a-filtered';
await type('request_path:'); assert.match(list.innerHTML, /\/two/); assert.doesNotMatch(list.innerHTML, /\/all/, 'old facet counts cannot masquerade as current selection counts');
state.datasetRevision++;
state.rows = [{ request_path: '/three' }];
await type('request_path:'); assert.match(list.innerHTML, /\/three/);

// Slow suggestions are invalid from the first subsequent keystroke or context switch.
let resolveRules; ruleRequest = new Promise(resolve => { resolveRules = resolve; });
await type('detection:a');
input.value = 'detection:b'; input.setSelectionRange(11, 11); input.dispatchEvent({ type: 'input' });
resolveRules([{ enabled: true, id: 'alpha', name: 'Alpha' }]); await tick();
assert.equal(list.hidden, true, 'obsolete asynchronous suggestion cannot repaint during the debounce');
bar.restoreDraft({ value: 'request_path:/restored', start: 3, end: 8, direction: 'backward' });
await flush(); assert.equal(list.hidden, true); assert.equal(input.value, 'request_path:/restored');
assert.equal(input.selectionStart, 3); assert.equal(input.selectionEnd, 8); assert.equal(input.selectionDirection, 'backward');
assert.equal(node('#btn-add-search').disabled, false);
const focusBefore = document.activeElement = node('#case-select');
bar.restoreDraft({ value: '', start: -8, end: 900 });
assert.equal(document.activeElement, focusBefore, 'restoring a draft does not steal keyboard focus'); assert.equal(node('#btn-add-search').disabled, true);

// Composition must not submit or show a half-written completion.
await type('request_'); assert.equal(list.hidden, false);
input.dispatchEvent({ type: 'compositionstart' }); assert.equal(list.hidden, true);
input.dispatchEvent({ type: 'input' }); await flush(); assert.equal(list.hidden, true);
input.dispatchEvent({ type: 'compositionend' }); await flush(); assert.equal(list.hidden, false);

// Accessible active options and bounded labels; oversized values are never silently shortened into filters.
state.filters=[];state.treeAggSig={};state.rows=[{request_path:'visible '+ '😀'.repeat(300)},{request_path:'x'.repeat(1000000)}];
await type('request_path:');assert.equal(input.attributes['aria-expanded'],'true');
assert.equal(input.attributes['aria-controls'],'quick-search-suggestions');assert.match(input.attributes['aria-activedescendant'],/^quick-search-suggestions-0$/);
assert.match(list.innerHTML,/Valores longos foram omitidos/);assert.ok(list.innerHTML.length<2000,'the popup never renders the full value');
const beforeApply=input.value;input.listeners.keydown[0]({key:'Tab',preventDefault(){}});assert.notEqual(input.value,beforeApply);assert.ok(input.value.includes('😀'.repeat(300)),'a bounded label still accepts the full eligible value');
input.value='request_path:';input.setSelectionRange(13,13);input.dispatchEvent({type:'input'});
input.listeners.keydown[0]({key:'Escape',preventDefault(){},stopPropagation(){}});await flush();assert.equal(list.hidden,true,'Escape cancels a pending debounce too');
assert.equal(input.attributes['aria-expanded'],'false');assert.equal(input.attributes['aria-activedescendant'],undefined);

// Comment edits mutate local values without replacing the page; invalidate suggestions.
const appSource = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const commentCase = {comments:{source:{1:'old comment'}}};
state.currentArtifact={id:'source'};state.visibleCols=['comentario'];state.columns.push('comentario');state.rows=[{id:1}];
Object.assign(context,{ensureCase:()=>commentCase,fillColumnControls(){},saveVisibleCols(){},renderTable(){},renderExploreTree(){},
  cellValue:(event,field)=>field==='comentario'?commentCase.comments.source[event.id]:event[field]});
vm.runInContext(appSource.slice(appSource.indexOf('function setEventComment('),appSource.indexOf('let commentEv =')),context);
await type('comentario:');assert.match(list.innerHTML,/old comment/);
context.setEventComment(state.rows[0],'new comment');assert.equal(list.hidden,true);assert.equal(saves,1,'the explicit comment edit still persists');
await type('comentario:');assert.match(list.innerHTML,/new comment/);assert.doesNotMatch(list.innerHTML,/old comment/);
assert.equal(saves,1,'suggesting the updated comment does not autosave');saves=0;

// Use real workspace capture/sanitize/apply, preserving draft separately from applied quick.
const c = { id: caseId, workspace: {} };
Object.assign(context, { activeCase: () => c, AGG_FUNCS: [['count', 'Count']], defaultCaseWorkspace: () => ({}), cubeState: { collapsed: new Set(), requestVersion: 0 }, treeAggVersion: { dataset: 0, case: 0 },
  caseEvents: () => [], caseEventsCache: { summary: { columns: [] } }, fillColumnControls() {}, renderChips() {}, renderExploreTree() {}, updateContextBar() {}, restoreVisiblePreferences() {},
});
vm.runInContext(workspaceSource.slice(workspaceSource.indexOf('  let scope ='), workspaceSource.indexOf('  function updateToggle()')), context);
Object.assign(state, vm.runInContext('defaults().values', context), { columns: ['timestamp', 'request_path'], treeCollapsed: new Set(), treeAgg: {}, treeAggSig: {}, treeAggError: {}, quick: 'legacy applied' });
bar.restoreDraft({ value: 'request_path:"unfinished', start: 7, end: 10 });
const dataset = context.capture(); assert.equal(dataset.values.quick, 'legacy applied'); assert.equal(dataset.queryDraft.value, 'request_path:"unfinished');
vm.runInContext('scope = "case";', context); activeScope = 'case'; context.apply(context.sanitize(null));
assert.equal(input.value, '', 'new case workspace starts with its own draft');
bar.restoreDraft({ value: 'case draft', start: 2, end: 2 }); const evidence = context.capture();
vm.runInContext('scope = "dataset";', context); activeScope = 'dataset'; context.apply(context.sanitize(JSON.parse(JSON.stringify(dataset))));
assert.equal(input.value, 'request_path:"unfinished'); assert.equal(input.selectionStart, 7); assert.equal(input.selectionEnd, 10);
assert.equal(state.quick, 'legacy applied', 'draft restoration never changes applied saved search');
assert.equal(node('.query-error').textContent, ''); // Test validator only treats a trailing colon as incomplete.
vm.runInContext('scope = "case";', context); activeScope = 'case'; context.apply(context.sanitize(evidence)); assert.equal(input.value, 'case draft');
context.apply(context.sanitize({ values: { quick: 'old saved search' } })); assert.equal(input.value, 'old saved search', 'old snapshots retain their applied quick-search display');
c.id = 'new-case'; context.apply(context.sanitize(null)); assert.equal(input.value, '', 'another case cannot inherit a draft');
assert.equal(nativeCalls, 0); assert.equal(saves, 0, 'typing/capture helpers never initiate a whole-case save');
console.log('Draft/context isolation, caret restoration, local suggestion freshness, sample labels and IME passed');
