import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const workspace=readFileSync(new URL('../../frontend/workspace.js',import.meta.url),'utf8');
const nodes=new Map(),restored=[];let refreshes=0;
const node=id=>{if(!nodes.has(id))nodes.set(id,{value:'old',disabled:false});return nodes.get(id);};
const state={filters:[],quick:'',page:8};
const context=vm.createContext({state,window:{QueryBar:{restoreDraft:value=>restored.push(value.value)}},$:node,
  CURRENT_FILTER_ID:'current',savedFilters:()=>[{id:'current',quick:'legacy',filters:[{column:'code',op:'equals_exact',value:'404'}]}],
  filtersChanged:()=>{refreshes++;},renderChips(){},syncCurrentSavedFilter(){},rememberSelection(){},showPage(){},refresh:()=>{refreshes++;},history:[],structuredClone,
});
vm.runInContext(app.slice(app.indexOf('function setQuickSearchDraft('),app.indexOf('function commitQuickSearch(')),context);
vm.runInContext(app.slice(app.indexOf('function restoreCurrentSavedFilter('),app.indexOf('function syncCurrentSavedFilter(')),context);
vm.runInContext(app.slice(app.indexOf('function applySavedFilter('),app.indexOf('function renderFilterTabs(')),context);
const start=app.indexOf('  $("#btn-clear-filters").onclick');vm.runInContext(app.slice(start,app.indexOf('\n\n  // dashboard',start)),context);
context.restoreCurrentSavedFilter();assert.equal(restored.at(-1),'legacy');assert.equal(refreshes,0);
context.applySavedFilter({quick:'saved expression',filters:[]});assert.equal(restored.at(-1),'saved expression');assert.equal(refreshes,1);
node('#btn-clear-filters').onclick();assert.equal(restored.at(-1),'');assert.equal(state.quick,'');assert.equal(refreshes,2);
vm.runInContext(workspace.slice(workspace.indexOf('  function search('),workspace.indexOf('  function applyRange(')),context);
context.search('explicit entity search');assert.equal(restored.at(-1),'explicit entity search');assert.equal(state.quick,'explicit entity search');
context.window.QueryBar=undefined;context.setQuickSearchDraft('');assert.equal(node('#quick-search').value,'');assert.equal(node('#btn-add-search').disabled,true);
context.setQuickSearchDraft('ready');assert.equal(node('#btn-add-search').disabled,false,'startup fallback synchronizes the Add control too');
assert.ok(!workspace.includes('$("#quick-search").value ='),'workspace search/undo/clear use the same draft restoration helper');
console.log('Saved filters, Clear and programmatic searches synchronize draft validation/suggestions/Add state');

assert.ok(!app.includes('$("#quick-search").value = ""'), "source publication, replacement, clear and MCP refresh use synchronized draft reset");
