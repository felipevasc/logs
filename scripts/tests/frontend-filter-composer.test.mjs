import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const nodes = new Map(), applied = [], messages = []; let scope = 'dataset', caseId = 'case-a', changed = 0, stopped = 0;
const document = { activeElement: null, listeners: {}, addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); } };
const node = id => {
  if (!nodes.has(id)) nodes.set(id, { id, value: '', hidden: true, options: [], listeners: {}, isConnected: true,
    set innerHTML(value) { this.options = []; }, get innerHTML() { return ''; },
    focus() { document.activeElement = this; }, matches() { return true; }, closest() { return null; },
    appendChild(option) { this.options.push(option); return option; }, querySelector() { return { textContent:'' }; },
    addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); },
  }); return nodes.get(id);
};
const state = { filters: [], columns: ['code','timestamp'], datasetRevision: 1, currentArtifact: {id:'source-a',loadedAt:1}, derivedFields: [], page: 4 };
const context = vm.createContext({ state, document, window: {QueryLang:{validate:() => null}}, $:node, el:() => ({}), colLabel:String, positionPop() {},
  activeCase:() => ({id:caseId}), workspaceScope:() => scope, toast:message => messages.push(message), renderChips(){},
  addFilter:filter => { applied.push(filter); state.filters.push(filter); }, filtersChanged:() => { changed++; },
});
vm.runInContext(app.slice(app.indexOf('const OPS ='),app.indexOf('const OP_SYMBOL =')),context);
vm.runInContext(app.slice(app.indexOf('let currentEditFilterIndex ='),app.indexOf('\nfunction positionPop(')),context);
const bindingStart = app.indexOf('  $("#btn-add-filter").onclick');
vm.runInContext(app.slice(bindingStart,app.indexOf('  $("#np-ok").onclick',bindingStart)),context);
const pop=node('#filter-pop'), first=node('#fp-val'), second=node('#fp-val2'), origin=node('#btn-add-filter');
const key = (target, extras={}) => pop.listeners.keydown[0]({ key:'Enter', target, preventDefault(){}, stopPropagation(){stopped++;}, ...extras });
context.openValueFilter('timestamp', 123, origin); assert.equal(document.activeElement, first);
key(first,{repeat:true});key(first,{isComposing:true});key(first,{keyCode:229});assert.equal(applied.length,0);
pop.listeners.compositionstart[0](); key(first); assert.equal(context.applyFilterPop(),false); assert.equal(applied.length,0);
pop.listeners.compositionend[0](); second.value='456';key(second);assert.equal(applied.length,1);assert.equal(applied[0].value2,'456');
assert.equal(pop.hidden,true);assert.equal(document.activeElement,origin);key(second);assert.equal(applied.length,1,'queued/repeated Enter cannot apply a closed editor again');
context.openValueFilter('code','404',origin); key(first,{key:'Escape'});assert.equal(pop.hidden,true);assert.equal(document.activeElement,origin);assert.equal(applied.length,1);
context.openValueFilter('code','503',origin);pop.listeners.compositionstart[0]();node('#fp-cancel').onclick();
context.openValueFilter('code','500',origin);key(first);assert.equal(applied.length,2,'cancelled composition does not poison the next editor');

// Index movement must never edit another chip, and stale edits keep the draft visible.
const a={column:'code',op:'equals_exact',value:'200'}, b={column:'code',op:'equals_exact',value:'404'};
state.filters=[a,b];context.openFilterPop(origin,1);first.value='503';state.filters.shift();
assert.equal(context.applyFilterPop(),true);assert.equal(state.filters[0].value,'503');assert.equal(changed,1);
state.filters=[a,b];context.openFilterPop(origin,0);first.value='draft kept';state.filters.shift();
assert.equal(context.applyFilterPop(),false);assert.equal(state.filters[0],b);assert.equal(first.value,'draft kept');assert.equal(pop.hidden,false);assert.match(messages.at(-1),/rascunho foi mantido/);
state.filters=[a];context.openFilterPop(origin,0);a.value='changed elsewhere';assert.equal(context.applyFilterPop(),false);
for(const change of [()=>state.datasetRevision++,()=>scope='case',()=>caseId='case-b',()=>state.currentArtifact.loadedAt++]) {
  context.openValueFilter('code','418',origin);const count=applied.length;change();assert.equal(context.applyFilterPop(),false);assert.equal(applied.length,count);assert.equal(first.value,'418');
}

// The same context-menu click that opened the composer must not immediately dismiss it.
const outsideStart=app.indexOf('  document.addEventListener("click", (e) => {\n    if (!$("#filter-pop").hidden');
vm.runInContext(app.slice(outsideStart,app.indexOf('\n\n  $("#page-size")',outsideStart)),context);
context.openValueFilter('code','404',origin);
document.listeners.click[0]({target:{closest:selector=>selector==='.ctx-menu'?{}:null}});assert.equal(pop.hidden,false);
const other=node('#unrelated-input');document.activeElement=other;
document.listeners.click[0]({target:{closest:()=>null}});assert.equal(pop.hidden,true);assert.equal(document.activeElement,other,'outside click keeps its own focus');
assert.ok(stopped>=3);
console.log('Composer keyboard/IME, focus return, stale-editor protection and context-menu opening passed');
