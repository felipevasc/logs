import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import {installCanonicalFields} from './helpers/canonical-fields-fixture.mjs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const evidence=readFileSync(new URL('../../frontend/evidence-ui.js',import.meta.url),'utf8');
const escape=text=>String(text).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const node=(tag,cls='',text='')=>({tag,className:cls,textContent:text,innerHTML:'',children:[],dataset:{},style:{},classList:{add(){}},append(...children){this.children.push(...children);},appendChild(child){this.children.push(child);return child;}});
const highlighted=[],copied=[],details=[],menus=[];
const state={quick:'',visibleCols:['message','path','zero','flag','structured'],selectedEventRows:new Map(),detailId:null};
const context=vm.createContext({window:{},state,el:node,esc:escape,escRe:text=>String(text).replace(/[.*+?^${}()|[\]\\]/g,'\\$&'),
  workspaceScope:()=> 'dataset',levelColor:()=> '#000',fmtTs:String,eventComment:()=> '',colLabel:String,trunc:value=>String(value).slice(0,32),
  showCtxMenu:(x,y,items)=>menus.push(items),openDetail:id=>details.push(id),updateRowSelectionStyles(){},sendVisibleToCase(){},toast(){},
  navigator:{clipboard:{writeText:text=>{copied.push(text);return Promise.resolve();}}},
});
installCanonicalFields(context);
vm.runInContext(evidence.slice(evidence.indexOf('  const isSensitiveKey ='),evidence.indexOf('  const eventContext =')),context);
context.window.EvidenceUI={redact:context.redact};
context.window.EntityMenu={highlight:text=>{highlighted.push(text);return null;}};
vm.runInContext(app.slice(app.indexOf('function cellValue('),app.indexOf('\nfunction autoVisibleCols(')),context);
vm.runInContext(app.slice(app.indexOf('function ensureSelectionOwner('), app.indexOf('function toggleRowSelect(')), context);
vm.runInContext(app.slice(app.indexOf('function eventCellMenu('),app.indexOf('\nasync function removeEventFromCase(')),context);
vm.runInContext(app.slice(app.indexOf('function tableValuePreview('),app.indexOf('\nfunction renderTable(')),context);
for(const value of ['',0,false,'plain','😀'.repeat(3000),'x'.repeat(4085)+'😀'+'x'.repeat(100),'<script>unsafe</script>'.repeat(1000)]){
  const p=context.tableValuePreview(value);assert.ok(p.text.length+p.marker.length<=4096);assert.equal(p.text.isWellFormed(),true);assert.equal(p.truncated,String(value).length>4096);
  if(!p.truncated)assert.equal(p.text,String(value));else assert.equal(String(value).startsWith(p.text),true);
}
const message='GET /api '+ 'https://example.test/'.repeat(60000);
const event=Object.freeze({id:7,event_ref:'fixture:7',message,source:'nginx',level:'Informação',fields:Object.freeze({path:'/very-long/'+ '😀'.repeat(100000),zero:0,flag:false,structured:Object.freeze({body:'z'.repeat(100000)})})});
state.rows=[event];state.selectedEventRows.set(7,event);
const row=context.buildEventRow(event);
for(const cell of row.children){
  assert.ok(cell.textContent.length+cell.children.map(child=>child.textContent.length).reduce((a,b)=>a+b,0)<=4096);
  assert.ok(cell.title.length<500,'tooltip never duplicates the full oversized value');
}
assert.equal(highlighted.length,0,'clipped tokens never become misleading entity links');
assert.equal(row.children[0].dataset.previewTruncated,'true');assert.match(row.children[0].title,/valor completo/);
assert.equal(row.children[2].textContent,'0');assert.equal(row.children[3].textContent,'false');
row.children[0].oncontextmenu({preventDefault(){},clientX:1,clientY:1});
await menus.at(-1).find(item=>item.label==='Copiar valor').onClick();assert.equal(copied.at(-1),message);
let preset;context.openValueFilter=(...args)=>{preset=args;};
await menus.at(-1).find(item=>item.label==='Criar filtro: message').onClick();assert.equal(preset[1],message);
row.onclick({target:{closest:()=>null}});assert.equal(details.at(-1),7);assert.equal(state.selectedEventRows.get(7),event);
assert.equal(state.rows[0],event);assert.equal(event.fields.structured.body.length,100000,'Case/export Event data stays intact');

// Dynamic JSON null is the native col_ref text "null", distinct from absence
// and an explicit empty string. The actual cell menu receives that same value.
const nullable=Object.freeze({id:10,event_ref:'fixture:10',fields:Object.freeze({explicit_null:null,empty:''})});
state.rows=[nullable];
const nullableRow=context.buildEventRow(nullable,['explicit_null','missing','empty']);
assert.deepEqual(nullableRow.children.map(cell=>cell.textContent),['null','','']);
nullableRow.children[0].oncontextmenu({preventDefault(){},clientX:1,clientY:1});
await menus.at(-1).find(item=>item.label==='Criar filtro: explicit_null').onClick();assert.equal(preset[1],'null');
await menus.at(-1).find(item=>item.label==='Copiar valor').onClick();assert.equal(copied.at(-1),'null');
nullableRow.children[1].oncontextmenu({preventDefault(){},clientX:1,clientY:1});
await menus.at(-1).find(item=>item.label==='Criar filtro: missing').onClick();assert.equal(preset[1],null);assert.equal(preset[3],'empty');
assert.equal(nullable.fields.explicit_null,null);assert.equal(Object.hasOwn(nullable.fields,'missing'),false);

// Values are shown as recorded: a long secret is clipped like any other text, never hidden.
const secret=Object.freeze({id:8,message:'start password="'+'s'.repeat(100000)+'" end',fields:{}});
const shown=context.buildEventRow(secret,['message']).children[0];
assert.match(shown.textContent,/^start password="s{100}/);assert.doesNotMatch(shown.textContent,/\[oculto\]/);assert.equal(shown.dataset.previewTruncated,'true');
assert.equal(secret.message.length,100000+'start password="" end'.length,'the record itself is untouched');

// Highlighting runs only over bounded escaped preview text and cannot introduce HTML from a log.
state.quick='needle';const html=context.buildEventRow({id:9,message:'<img src=x onerror=bad()>needle'.repeat(10000),fields:{}},['message']).children[0];
assert.doesNotMatch(html.innerHTML,/<img/);assert.match(html.innerHTML,/&lt;img/);assert.match(html.innerHTML,/<mark>needle<\/mark>/);
assert.ok(html.innerHTML.length<10000,'escaping/highlight expansion is bounded by preview input');
console.log('Bounded Unicode-safe table previews show recorded values, copy/filter/details and full evidence values');
