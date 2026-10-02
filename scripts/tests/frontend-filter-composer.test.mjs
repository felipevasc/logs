import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const app = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const nodes = new Map(), applied = [], messages = []; let scope = 'dataset', caseId = 'case-a', changed = 0, stopped = 0;
const document = { activeElement: null, listeners: {}, addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); } };
const node = id => {
  if (!nodes.has(id)) nodes.set(id, { id, value: '', hidden: true, options: [], listeners: {}, isConnected: true, attributes: {}, setAttribute(key,value){this.attributes[key]=String(value);},
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
const pop=node('#filter-pop'), first=node('#fp-val'), second=node('#fp-val2'), origin=node('#btn-add-filter');origin.hidden=false;
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

// Non-modal accessible controls and focus recovery from non-focusable/disconnected origins.
assert.equal(pop.attributes.role,'dialog');assert.equal(pop.attributes['aria-modal'],'false');
assert.equal(node('#fp-col').attributes['aria-label'],'Campo do filtro');
assert.equal(node('#fp-op').attributes['aria-label'],'Operador do filtro');
assert.match(first.attributes['aria-label'],/Valor/);assert.match(second.attributes['aria-label'],/Fim/);
document.activeElement={isConnected:true,hidden:false,matches:()=>false,focus(){throw Error('Body cannot be the return target');}};
context.openFilterPop();context.closeFilterPop();assert.equal(document.activeElement,origin);
origin.hidden=true;const drawerClose=node('#dr-close');drawerClose.hidden=false;
context.openFilterPop();context.closeFilterPop();assert.equal(document.activeElement,drawerClose,'hidden toolbar falls back to the visible inspector control');
origin.hidden=false;drawerClose.hidden=true;const transient=node('#temporary-trigger');transient.hidden=false;
context.openFilterPop(transient);transient.isConnected=false;context.closeFilterPop();assert.equal(document.activeElement,origin);
console.log('Composer accessible names and visible/focusable return targets passed');

// Textareas preserve LF; CR-containing values use a visible JSON-string view
// so the browser cannot normalize a native CR/CRLF preset before applying it.
for(const value of ['alpha\nbravo','alpha\r\nbravo','alpha\rbravo','alpha\r\nbravo\ncharlie\rdelta','',' \r\n ']){
  context.openValueFilter('message',value,origin,'equals_exact');
  assert.equal(first.value,value.includes('\r')?JSON.stringify(value):value);
  assert.equal(first.attributes['data-value-format'],value.includes('\r')?'json-string':'text');
  assert.equal(context.applyFilterPop(),true);
  assert.equal(applied.at(-1).value,value,'exact empty, whitespace and mixed line-ending literals survive the real composer');
}
const raw='alpha\r\nbravo\ncharlie\rdelta';
context.openValueFilter('message',raw,origin,'equals_exact');
const before=applied.length;first.value='"broken';assert.equal(context.applyFilterPop(),false);
assert.equal(applied.length,before);assert.equal(pop.hidden,false);assert.equal(first.value,'"broken');assert.match(messages.at(-1),/texto JSON entre aspas/);
first.value=JSON.stringify(raw);first.selectionStart=first.selectionEnd=first.value.length-1;
key(first,{shiftKey:true});assert.equal(JSON.parse(first.value),raw+'\n');assert.equal(applied.length,before);
const edited=first.value;key(first,{shiftKey:true,repeat:true});key(first,{shiftKey:true,isComposing:true});assert.equal(first.value,edited);
node('#fp-cancel').onclick();assert.equal(applied.length,before);assert.equal(document.activeElement,origin);
context.openValueFilter('message','one\ntwo',origin,'equals_exact');
let prevented=0;key(first,{shiftKey:true,preventDefault(){prevented++;}});assert.equal(prevented,0,'plain Shift+Enter retains the textarea newline action');assert.equal(applied.length,before);
node('#fp-cancel').onclick();
const saved={column:'message',op:'equals_exact',value:raw,value2:null};state.filters=[saved];
context.openFilterPop(origin,0);assert.equal(first.value,JSON.stringify(raw));assert.equal(context.applyFilterPop(),true);assert.equal(state.filters[0].value,raw);
context.openValueFilter('code',' ',origin,'gte');const numericBefore=applied.length;assert.equal(context.applyFilterPop(),undefined);assert.equal(applied.length,numericBefore,'numeric operators retain their blank-value guard');
context.openValueFilter('message',raw,origin,'between');first.value='"broken';second.value='"also broken';node('#fp-op').value='empty';node('#fp-op').onchange();
assert.equal(context.applyFilterPop(),true);assert.equal(applied.at(-1).op,'empty');assert.equal(applied.at(-1).value,'');assert.equal(applied.at(-1).value2,null,'irrelevant hidden draft values do not block a valueless operator');
console.log('Literal multiline composer preserves CR/LF, saved filters, edits, cancellation, IME and range/numeric behavior');
