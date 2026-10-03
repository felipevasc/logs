import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/discovery.js',import.meta.url),'utf8');
const makeNode=()=>({title:'',dataset:{},attrs:{},getBoundingClientRect:()=>({right:50,bottom:80}),setAttribute(key,value){this.attrs[key]=value;}});
const actionHeader=makeNode(),headers=['source','mock_payload_b64'].map(column=>Object.assign(makeNode(),{dataset:{column}})),main=makeNode(),more=makeNode(),shown=[],opened=[],renders=[],filters=[];
const row={dataset:{column:'source'},querySelector:selector=>selector==='.field-item'?main:more};
const box={querySelectorAll:()=>[row]};
const context=vm.createContext({
  state:{visibleCols:['source','mock_payload_b64'],filters},document:{querySelectorAll:selector=>selector==='#events-table th[data-column]'?[actionHeader,...headers].filter(node=>node.dataset.column):selector==='#events-table th'?[actionHeader,...headers]:[]},
  workspaceScope:()=> 'dataset',colLabel:field=>field==='source'?'Origem':field,
  renderTable:(...args)=>renders.push(args),renderExploreTreeInto(){},showCtxMenu:(x,y,items)=>shown.push({x,y,items}),
  window:{ExplorerTimeline:{menuItem:field=>({label:`Representar ${field==='source'?'Origem':field} na Timeline`,onClick:()=>opened.push({kind:'timeline',field})})},
    FieldTransforms:{menuItem:(field,{anchor})=>({label:'Transformar campo',onClick:()=>opened.push({kind:'transform',field,anchor})})},
    CaseReferences:{lookupMenuItem:(field,anchor)=>({label:'Criar campo por referência',onClick:()=>opened.push({kind:'reference',field,anchor})})}},
  addFilter:filter=>filters.push(filter),fieldTop(){},applySelection(){},openFilterPop(){},$:()=>({value:'',options:[]}),el:makeNode,cubeAdd(){},switchTab(){},
});
vm.runInContext(source.slice(source.indexOf('  function fieldMenu('),source.indexOf('  const oldDetail=showDetail;')),context);
const qr={rows:[{id:1}]},options={reuseRows:true};context.renderTable(qr,options);
assert.equal(actionHeader.oncontextmenu,undefined,'the record-action column must never become a field menu');
assert.equal(actionHeader.title,'');
assert.equal(renders[0][0],qr);assert.equal(renders[0][1],options,'the installed decorator still forwards row reuse');
const event={clientX:12,clientY:34,preventDefault(){},stopPropagation(){}};
for(const [index,field] of ['source','mock_payload_b64'].entries()){
  headers[index].oncontextmenu(event);const menu=shown.at(-1).items;
  const timeline=menu.find(item=>item.label===`Representar ${field==='source'?'Origem':field} na Timeline`);
  const transform=menu.find(item=>item.label==='Transformar campo');assert.ok(timeline);assert.ok(transform);
  timeline.onClick();transform.onClick();assert.equal(opened.at(-1).field,field);assert.equal(opened.at(-1).anchor,headers[index]);
  const reference=menu.find(item=>item.label==='Criar campo por referência');assert.ok(reference);reference.onClick();assert.equal(opened.at(-1).field,field);assert.equal(opened.at(-1).anchor,headers[index]);
  for(const label of ['Resumir valores','Somente preenchidos','Somente vazios','Cruzar nas linhas','Cruzar nas colunas'])assert.ok(menu.some(item=>item.label===label),label);
}
// The DOM can be reordered before the decorator runs: fields follow their own identifiers.
headers.reverse();context.renderTable(qr,options);
for(const header of headers){header.oncontextmenu(event);shown.at(-1).items.find(item=>item.label==='Transformar campo').onClick();assert.equal(opened.at(-1).field,header.dataset.column);}
assert.equal(actionHeader.oncontextmenu,undefined);
context.renderExploreTreeInto(box,'dataset');
for(const invoke of [()=>row.oncontextmenu(event),()=>more.onclick(event)]){
  invoke();const menu=shown.at(-1).items;
  assert.ok(menu.some(item=>item.label==='Representar Origem na Timeline'));
  const transform=menu.find(item=>item.label==='Transformar campo');assert.ok(transform);transform.onClick();assert.equal(opened.at(-1).field,'source');
  const reference=menu.find(item=>item.label==='Criar campo por referência');assert.ok(reference);reference.onClick();assert.equal(opened.at(-1).field,'source');
  assert.ok(menu.some(item=>item.label==='Filtro avançado'),'field menus keep their existing filtering actions');
}
assert.equal(filters.length,0,'showing menus and selecting Timeline/Transform actions never applies a value filter');
context.window.ExplorerTimeline=null;context.window.FieldTransforms=null;context.window.CaseReferences=null;headers[0].oncontextmenu(event);
assert.ok(shown.at(-1).items.some(item=>item.label==='Resumir valores'),'optional controllers do not break legacy menu actions');
console.log('Installed Discovery decorators expose Timeline and Transform actions on headers and field menus');
