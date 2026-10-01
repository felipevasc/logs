import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'),owner={storeId:'store',caseId:'case',analysisId:'analysis'};
const container=id=>({kind:'native_evidence_container',reference:{kind:'native_evidence',schemaVersion:1,owner,containerId:id,manifestId:`manifest-${id}`,manifestSha256:'a'.repeat(64),memberCount:2},preservedCount:2,preview:null});
const record=(id,containerId)=>({id,label:`Item ${containerId}`,rows:container(containerId)});
const plain=value=>JSON.parse(JSON.stringify(value));
function fixture(){const c={id:'case',items:[record('duplicate','a'),{id:'note',label:'Narrative',note:'text'},record('duplicate','b')],caseTrails:[{id:'trail',itemIds:['duplicate','note'],title:'Trail',summary:'original',updatedAt:1}]},opens=[];
  const context=vm.createContext({window:{CaseEvidenceDetail:{open:(reference,member,options)=>{opens.push({reference,member,options});return Promise.resolve(true);}}},structuredClone,TextEncoder,activeCase:()=>c,showDetail(){assert.fail('native container must not become a legacy Event');}});
  for(const name of ['case-evidence.js','case-evidence-items.js'])vm.runInContext(read(name),context);context.window.CaseEvidence.active=true;return{c,trail:c.caseTrails[0],items:context.window.CaseEvidenceItems,opens,context};}
const deferred=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return{promise,resolve};};

test('duplicate legacy trail item IDs remain ambiguous without choosing or rewriting evidence',()=>{
  const f=fixture(),before=structuredClone(f.c),associations=f.items.associations(f.c,f.trail);assert.equal(associations[0].state,'ambiguous');assert.equal(associations[0].item,null);assert.equal(associations[1].item,f.c.items[1]);assert.deepEqual(f.c,before);
});

test('native container bindings distinguish duplicate item IDs and preserve mixed authored order',async()=>{
  const f=fixture(),first=f.items.forItem(f.c,f.c.items[2]),note=f.items.forItem(f.c,f.c.items[1]),last=f.items.forItem(f.c,f.c.items[0]),beforeRows=f.c.items.map(item=>item.rows);
  assert.deepEqual(plain(first),{kind:'container',containerId:'b'});assert.deepEqual(plain(note),{kind:'item',itemId:'note'});
  await f.items.edit(f.c,f.trail,[first,note,last],{save:async()=>true});assert.deepEqual(f.trail.itemIds,['duplicate','note']);assert.deepEqual(plain(f.trail.itemRefs),[first,note,last].map(plain));
  assert.deepEqual(f.items.associations(f.c,f.trail).map(entry=>entry.item),[f.c.items[2],f.c.items[1],f.c.items[0]]);assert.ok(f.c.items.every((item,index)=>item.rows===beforeRows[index]));
});

test('reordering native/legacy associations supports durable Undo while leaving original record containers untouched',async()=>{
  const f=fixture(),before=f.trail.itemIds,refs=[f.items.forItem(f.c,f.c.items[0]),f.items.forItem(f.c,f.c.items[1])];let saves=0;
  const receipt=await f.items.edit(f.c,f.trail,refs,{save:async()=>{saves++;return true;}});f.trail.summary='new authored note';await f.items.undo(receipt,{save:async()=>{saves++;return true;}});
  assert.equal(f.trail.itemIds,before);assert.equal(Object.hasOwn(f.trail,'itemRefs'),false);assert.equal(f.trail.summary,'new authored note');assert.equal(saves,2);assert.equal(f.items.associations(f.c,f.trail)[0].state,'ambiguous');
});

test('failed association save restores only its owned order and preserves concurrent narrative edits',async()=>{
  const f=fixture(),gate=deferred(),before=f.trail.itemIds,refs=[f.items.forItem(f.c,f.c.items[2])];const editing=f.items.edit(f.c,f.trail,refs,{save:()=>gate.promise});f.trail.summary='typed while saving';f.trail.updatedAt=999;
  await assert.rejects(f.items.edit(f.c,f.trail,[],{save:async()=>true}),/Aguarde/);gate.resolve(false);await assert.rejects(editing,/ordem anterior/);
  assert.equal(f.trail.itemIds,before);assert.equal(f.trail.summary,'typed while saving');assert.equal(f.trail.updatedAt,999);assert.equal(Object.hasOwn(f.trail,'itemRefs'),false);
});

test('Undo rejects later association changes and unavailable or duplicated new bindings',async()=>{
  const f=fixture(),ref=f.items.forItem(f.c,f.c.items[0]);for(const values of [[{kind:'container',containerId:'missing'}],[ref,ref],[{kind:'item',itemId:'duplicate'}]])assert.throws(()=>f.items.edit(f.c,f.trail,values,{save:async()=>true}),/ambíguos/);
  const receipt=await f.items.edit(f.c,f.trail,[ref],{save:async()=>true});await f.items.edit(f.c,f.trail,[],{save:async()=>true});assert.throws(()=>f.items.undo(receipt,{save:async()=>true}),/associação mudou/);assert.deepEqual(plain(f.trail.itemRefs),[]);
});

test('captured item opening forwards only its native reference and checks the original item/container',async()=>{
  const f=fixture(),item=f.c.items[2];assert.equal(f.items.count(item),2);await f.items.open(item);assert.equal(f.opens[0].reference,item.rows.reference);assert.equal(f.opens[0].member,null);assert.equal(f.opens[0].options.guard(),true);
  item.rows=structuredClone(item.rows);assert.equal(f.opens[0].options.guard(),true,'a notes-only receipt can renew the same container descriptor');item.rows=container('replacement');assert.equal(f.opens[0].options.guard(),false);assert.throws(()=>f.items.count({rows:[]}),/NATIVE_CONTAINER_REQUIRED/);
});

function uiFixture(){
  const f=fixture(),ctx=f.context,saved=[],messages=[],edited=[];let saveResult=true;
  const all=node=>node.children.flatMap(child=>[child,...all(child)]);
  function element(tag='div',className='',text=''){
    let own=String(text);const node={tag,className,children:[],attrs:{},dataset:{},isConnected:true,hidden:false,disabled:false,parentElement:null,offsetParent:{},scrollTop:0,
      get textContent(){return own+this.children.map(child=>child.textContent).join('');},set textContent(value){own=String(value);this.children=[];},set innerHTML(value){own=value;this.children=[];},get innerHTML(){return own;},
      append(...children){for(const child of children){child.parentElement=this;this.children.push(child);}},replaceChildren(...children){own='';this.children=[];this.append(...children);},remove(){if(this.parentElement)this.parentElement.children=this.parentElement.children.filter(child=>child!==this);this.isConnected=false;},
      setAttribute(key,value){this.attrs[key]=String(value);},getAttribute(key){return this.attrs[key];},focus(){ctx.document.activeElement=this;},
      querySelectorAll(selector){return all(this).filter(child=>selector.startsWith('.')?child.className.split(' ').includes(selector.slice(1)):child.tag===selector);},querySelector(selector){return this.querySelectorAll(selector)[0]||null;}};return node;
  }
  const body=element('body');body.dataset.page='case-trails';const root=element('section');body.append(root);const misc=new Map();
  Object.assign(ctx,{document:{body,activeElement:element('button'),addEventListener(){},querySelector:selector=>body.querySelector(selector)},el:element,state:{cases:{cases:[f.c],active:f.c.id}},workspaceScope:()=> 'case',fmtNum:String,nid:()=> 'new',updateAnalysisBadge(){},saveCases:async()=>{saved.push(structuredClone(f.trail));return saveResult;},toast:text=>messages.push(text),$:selector=>{if(!misc.has(selector))misc.set(selector,element());return misc.get(selector);},Workspace:{showPage:async()=>{},page:()=> 'case-trails'}});
  ctx.window.CaseContent={narrative:item=>({summary:item.summary||'',details:item.note||''}),editItem:async item=>edited.push(item)};ctx.CaseContent=ctx.window.CaseContent;ctx.window.Workspace=ctx.Workspace;
  vm.runInContext(read('case-trails.js'),ctx);ctx.window.CaseTrails.render(root,f.c);
  const button=(label,parent=body)=>parent.querySelectorAll('button').find(node=>node.textContent===label||node.attrs['aria-label']===label);
  return{...f,root,body,button,saved,messages,edited,save:value=>{saveResult=value;},render:()=>ctx.window.CaseTrails.render(root,f.c)};
}

test('actual saved-trail association picker resolves duplicate IDs through explicit containers and can Undo',async()=>{
  const f=uiFixture(),prior=f.trail.itemIds;assert.match(f.root.textContent,/Associação ambígua/);assert.equal(f.opens.length,0);
  f.button('Associar itens').onclick();const picker=f.body.querySelector('.case-trail-picker'),labels=picker.querySelectorAll('label');
  const ghost=labels.find(node=>node.textContent.includes('Associação ambígua')),target=labels.find(node=>node.textContent.includes('Item b'));
  const uncheck=ghost.querySelector('input');uncheck.checked=false;uncheck.onchange();const choose=target.querySelector('input');choose.checked=true;choose.onchange();await f.button('Aplicar',picker).onclick();
  assert.deepEqual(plain(f.trail.itemRefs),[{kind:'item',itemId:'note'},{kind:'container',containerId:'b'}]);assert.equal(f.saved.length,1);
  await f.button('Primeiro registro').onclick();assert.equal(f.opens.at(-1).reference.containerId,'b');
  await f.button('Desfazer associação ou ordem').onclick();assert.equal(f.trail.itemIds,prior);assert.match(f.root.textContent,/Associação ambígua/);assert.equal(f.saved.length,2);
});

test('actual trail reorder preserves exact selected item, rolls back failed save, and retains later note edits',async()=>{
  const f=uiFixture();f.trail.itemRefs=[{kind:'container',containerId:'b'},{kind:'item',itemId:'note'},{kind:'container',containerId:'a'}];delete f.trail.itemIds;f.render();const before=f.trail.itemRefs;
  const card=f.root.querySelectorAll('.case-trail-item').find(node=>node.textContent.includes('Item a'));await f.button('Editar item',card).onclick();assert.equal(f.edited[0],f.c.items[0]);
  f.save(false);await f.button('Mover item acima',card).onclick();assert.equal(f.trail.itemRefs,before);assert.match(f.messages.at(-1),/ordem anterior/);
  f.save(true);await f.button('Mover item acima',card).onclick();assert.deepEqual(plain(f.trail.itemRefs),[{kind:'container',containerId:'b'},{kind:'container',containerId:'a'},{kind:'item',itemId:'note'}]);
  f.trail.summary='authored after reorder';await f.button('Desfazer associação ou ordem').onclick();assert.equal(f.trail.itemRefs,before);assert.equal(f.trail.summary,'authored after reorder');
});

test('actual item editor accepts the captured object and refuses ambiguous legacy IDs',async()=>{
  const f=fixture(),edited=[],messages=[];Object.assign(f.context,{edit:async item=>{edited.push(item);return false;},saveCases:async()=>true,toast:text=>messages.push(text)});
  const code=read('case-content.js');vm.runInContext(code.slice(code.indexOf('  async function editItem('),code.indexOf('  return {narrative')),f.context);
  assert.equal(await f.context.editItem('duplicate'),false);assert.equal(edited.length,0);assert.match(messages[0],/ambígua/);
  await f.context.editItem(f.c.items[2]);assert.equal(edited[0],f.c.items[2]);
});

test('actual Evidence card handler opens its captured array-position item through native member authority',async()=>{
  const f=fixture(),code=read('workspace.js');let click;Object.assign(f.context,{content:{addEventListener:(_event,fn)=>{click=fn;}},toast(){}});
  vm.runInContext(code.slice(code.indexOf('  const savedRecordCount ='),code.indexOf('  function rememberSelection()')),f.context);
  const start=code.indexOf('  content.addEventListener("click"'),end=code.indexOf('    if (action === "reopen-evidence")',start);vm.runInContext(code.slice(start,end)+'\n});',f.context);
  const target={dataset:{action:'evidence-event',index:'2'}};await click({target:{closest:selector=>selector==='[data-action]'?target:null}});
  assert.equal(f.opens.length,1);assert.equal(f.opens[0].reference.containerId,'b');assert.equal(vm.runInContext('savedRecordCount(activeCase().items[2])',f.context),2);
});

test('actual Workspace item navigation refuses ambiguous legacy links and accepts an explicit native binding',async()=>{
  const f=fixture(),code=read('workspace.js'),messages=[];f.context.toast=message=>messages.push(message);
  vm.runInContext(code.slice(code.indexOf('  const savedRecordCount ='),code.indexOf('  function rememberSelection()')),f.context);
  const start=code.indexOf('    openItem: itemId =>'),end=code.indexOf('\n    page:',start);vm.runInContext('const openItem = '+code.slice(start,end).trim().slice('openItem:'.length).replace(/,$/,';'),f.context);
  vm.runInContext('openItem("duplicate")',f.context);assert.equal(f.opens.length,0);assert.match(messages[0],/ambígua/);
  await vm.runInContext('openItem({kind:"container",containerId:"b"})',f.context);assert.equal(f.opens[0].reference.containerId,'b');
});

test('ordered unresolved aliases and repeated native slots remain intact through reorder and Undo', async()=>{
  const f=fixture(),refs=[{kind:'unresolved_legacy_item',itemId:'old',state:'ambiguous'},{kind:'container',containerId:'b'},{kind:'unresolved_legacy_item',itemId:null,state:'unsupported'},{kind:'unresolved_native_container',containerId:'absent',state:'missing'},{kind:'container',containerId:'b'}];f.trail.itemRefs=refs;
  const legacy=f.trail.itemIds,next=[refs[4],refs[0],refs[2],refs[3],refs[1]];const receipt=await f.items.edit(f.c,f.trail,next,{save:async()=>true});
  assert.deepEqual(plain(f.trail.itemRefs),next);assert.equal(f.trail.itemIds,legacy);assert.deepEqual(f.items.associations(f.c,f.trail).map(entry=>entry.state),['unique','ambiguous','unsupported','missing','unique']);
  await f.items.undo(receipt,{save:async()=>true});assert.equal(f.trail.itemRefs,refs);assert.equal(f.trail.itemIds,legacy);
  assert.throws(()=>f.items.edit(f.c,f.trail,[...refs,refs[0]],{save:async()=>true}),/ambíguos/);
});

test('authoritative legacy alias blocks prevent later item deletion from resolving old ambiguity',()=>{
  const f=fixture();f.c.items.splice(0,1);const bindings=f.items.index(f.c,{legacyItemAliases:[{alias:'duplicate',state:'ambiguous'}]});
  const result=bindings.associations(f.trail);assert.equal(result[0].state,'ambiguous');assert.equal(result[0].item,null);assert.deepEqual(plain(result[0].reference),{kind:'unresolved_legacy_item',itemId:'duplicate',state:'ambiguous'});
});

test('hypothesis links use captured container identities, preserve original items and show unresolved report positions',async()=>{
  const f=fixture(),messages=[];f.c.intel={indicators:[],hypotheses:[{id:'h',text:'Hypothesis',status:'aberta',items:['duplicate','note']}]};
  Object.assign(f.context,{saveCases:async()=>true,toast:value=>messages.push(value),document:{body:{dataset:{page:'other'}}}});vm.runInContext(read('case-intel.js'),f.context);
  const h=f.c.intel.hypotheses[0],prior=h.items,menu=f.context.window.CaseIntel.menuItems(f.c.items[2]);await menu[0].onClick();
  assert.equal(h.items,prior);assert.deepEqual(plain(h.itemRefs),[{kind:'unresolved_legacy_item',itemId:'duplicate',state:'ambiguous'},{kind:'item',itemId:'note'},{kind:'container',containerId:'b'}]);
  assert.deepEqual(plain(f.context.window.CaseIntel.synthesis(f.c).hypotheses[0].refs),['Associação ambígua','Narrative','Item b']);
  assert.equal(f.context.window.CaseIntel.menuItems(f.c.items[2]).length,0);assert.equal(f.context.window.CaseIntel.menuItems(f.c.items[0]).length,1);
});

test('hypothesis failed native link rolls back its own references and retains original arrays',async()=>{
  const f=fixture();f.c.intel={indicators:[],hypotheses:[{id:'h',text:'Hypothesis',status:'aberta',items:['note']}]};
  Object.assign(f.context,{saveCases:async()=>false,toast(){},document:{body:{dataset:{page:'other'}}}});vm.runInContext(read('case-intel.js'),f.context);
  await f.context.window.CaseIntel.menuItems(f.c.items[2])[0].onClick();assert.equal(Object.hasOwn(f.c.intel.hypotheses[0],'itemRefs'),false);assert.deepEqual(f.c.intel.hypotheses[0].items,['note']);
});

test('removed typed container associations retain identity and order until whole-item Undo',async()=>{
 const f=fixture(),item=f.c.items.pop(),missing={kind:'container',containerId:'b'},note={kind:'item',itemId:'note'};f.trail.itemRefs=[missing,note];
 await f.items.edit(f.c,f.trail,[note,missing],{save:async()=>true});assert.deepEqual(plain(f.trail.itemRefs),[note,missing]);assert.equal(f.items.associations(f.c,f.trail)[1].state,'missing');
 assert.throws(()=>f.items.edit(f.c,f.trail,[note,missing,{kind:'container',containerId:'forged'}],{save:async()=>true}),/ambíguos/);
 f.c.items.push(item);assert.equal(f.items.associations(f.c,f.trail)[1].item,item);
});

test('malformed associations remain unavailable but can be explicitly removed',async()=>{
 const f=fixture();f.trail.itemRefs=[null];assert.equal(f.items.associations(f.c,f.trail)[0].state,'unavailable');await f.items.edit(f.c,f.trail,[],{save:async()=>true});assert.deepEqual(plain(f.trail.itemRefs),[]);
 assert.throws(()=>f.items.edit(f.c,f.trail,[null],{save:async()=>true}),/ambíguos/);
});

test('hypothesis menu does not rebuild the full item index for every empty hypothesis',()=>{
 const f=fixture();f.c.items=Array.from({length:100},(_,index)=>({id:`note-${index}`,label:'note'}));let examined=0;
 f.c.items=new Proxy(f.c.items,{get(target,name,receiver){if(name==='entries')return function*(){for(let i=0;i<target.length;i++){examined++;yield[i,target[i]];}};return Reflect.get(target,name,receiver);}});
 f.c.intel={indicators:[],hypotheses:Array.from({length:100},(_,index)=>({id:`h-${index}`,text:'hypothesis',items:[]}))};
 Object.assign(f.context,{saveCases:async()=>true,toast(){},document:{body:{dataset:{page:'other'}}}});vm.runInContext(read('case-intel.js'),f.context);
 const result=f.context.window.CaseIntel.menuItems(f.c.items[0]);assert.equal(result.length,6);
 assert.ok(examined<=200,`examined ${examined} item positions for 100 empty hypotheses`);
});

test('native item record disclosure pages exact previews and opens a captured member without Events',async()=>{
 const f=fixture(),nodes=[];function el(tag='div',className='',text=''){const value={tag,className,textContent:text,children:[],isConnected:true,attrs:{},append(...children){this.children.push(...children);},replaceChildren(...children){this.children=children;},setAttribute(name,value){this.attrs[name]=value;},addEventListener(name,handler){this[name]=handler;}};nodes.push(value);return value;}
 const item=f.c.items[0],ref=item.rows.reference,one={containerId:'a',manifestId:ref.manifestId,occurrenceId:'one'},two={...one,occurrenceId:'two'},requests=[];
 Object.assign(f.context,{el,state:{visibleCols:['message']},nativeEvidenceServices:()=>({client:{preview:async(reference,options)=>{requests.push({reference,options});return{kind:'evidence_preview_page',columns:['message'],rows:[{kind:'evidence_preview',member:options.cursor?two:one,cells:[{state:'preview',text:'Short preview',incomplete:true}]}],total:2,nextCursor:options.cursor?null:'next'};}}})});
 const parent=el(),details=f.items.attachRecords(parent,f.c,item);details.open=true;details.toggle();for(let i=0;i<8;i++)await Promise.resolve();assert.equal(requests[0].options.limit,16);
 nodes.find(node=>node.attrs['aria-label']==='Abrir ocorrência preservada 1').onclick();assert.equal(f.opens[0].member,one);await nodes.find(node=>node.tag==='button'&&node.textContent==='Próximas').onclick();assert.equal(requests[1].options.cursor,'next');nodes.find(node=>node.attrs['aria-label']==='Abrir ocorrência preservada 2').onclick();assert.equal(f.opens[1].member,two);
 item.rows=container('replacement');assert.equal(f.opens[1].options.guard(),false);
});

test('native possible-trail conversion preserves the complete bounded selection using only handles',async()=>{
 const f=uiFixture(),ctx=f.context,requests=[],adds=[];ctx.document.removeEventListener=()=>{};ctx.window.AnalysisContexts={capture:()=>({id:'owner'}),isCurrent:()=>true,prepare:async owner=>owner};
 Object.assign(ctx,{analyticsRequest:()=>({}),sourceSpecFromArtifact:()=>({}),sourceSpecFromControls:()=>({}),caseItemBase:()=>({id:'captured'}),api:async(command,args)=>{requests.push({command,args});return{rows:[{id:7,event_ref:'source-ref',raw:'not-authority'}],total:1,hasMore:false,nextCursor:null};},nativeEvidenceServices:()=>({actions:{source:async()=>({owner:{},receipt:{}}),handles:rows=>rows.map(row=>({id:row.id,eventRef:row.event_ref})),add:async(selected,metadata,options)=>{adds.push(selected);const item={...metadata,rows:container('capture')};f.c.items.push(item);options.attach(item);return item;}}})});
 await ctx.window.CaseTrails.fromJourney({scope:'case',field:'user',value:'alice',filters:[]});assert.equal(requests[0].command,'query_page');assert.match(f.body.textContent,/1 ocorrências serão preservadas/);await f.button('Preservar e guardar').onclick();assert.deepEqual(plain(adds[0].rows),[{id:7,eventRef:'source-ref'}]);assert.deepEqual(plain(f.c.caseTrails.at(-1).itemRefs),[{kind:'container',containerId:'capture'}]);
});
