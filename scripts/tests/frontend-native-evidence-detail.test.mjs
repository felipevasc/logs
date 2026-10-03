import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'),owner={storeId:'store',caseId:'case',analysisId:'analysis'};
const member={containerId:'container',manifestId:'manifest',occurrenceId:'member'},reference={kind:'native_evidence',schemaVersion:1,owner,containerId:'container',manifestId:'manifest',manifestSha256:'a'.repeat(64),memberCount:1};
const identity={caseId:'case',analysisId:'analysis',configRevision:1,visibilityRevision:0};
const documentValue=()=>({evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},active:'case',cases:[{id:'case',items:[{id:'item',rows:{kind:'native_evidence_container',reference,preservedCount:1,preview:null}}],analysisContext:{schemaVersion:1,...identity,config:{derivedFields:[],references:[]},migrationDiagnostics:[]}}],caseEvidence:[{state:'ready',owner,evidenceSignature:'signature',preservedCount:1}],diagnostics:[]});
const data=()=>({kind:'preserved_member_detail',member,fields:[{column:'number',type:'number',text:'1',complete:false},{column:'message',type:'string',text:'message',complete:true}],fieldsTotal:2,nextCursor:null,raw:{text:'decoded raw',complete:true},envelope:{text:'{"fields":{"number":1.0,"u64":18446744073709551615}}',complete:true},javaTraceAvailable:true});
const deferred=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return{promise,resolve};};
const settle=async()=>{for(let index=0;index<30;index++)await Promise.resolve();};
function descendants(node){return node.children.flatMap(child=>[child,...descendants(child)]);}
async function fixture(){
  const roots=new Map(),calls=[],copied=[],menus=[],filters=[],messages=[],cancelled=[],java=[],inspections=[];let live=documentValue(),next=data(),exact='1.0',clipboard=async text=>copied.push(text),selection=null,previewOverride=null,inspectorOpen=false;
  const events=new Map();let context;
  function element(tag='div',className='',initial=''){
    let own=String(initial);const node={tag,className,children:[],dataset:{},hidden:false,disabled:false,isConnected:true,parentNode:null,style:{},attrs:{},
      get textContent(){return own+this.children.map(child=>child.textContent).join('');},set textContent(value){own=String(value??'');this.children=[];},
      get innerHTML(){return own;},set innerHTML(value){own=String(value);this.children=[];},
      append(...children){for(const child of children){child.parentNode=this;this.children.push(child);}},prepend(...children){for(const child of [...children].reverse()){child.parentNode=this;this.children.unshift(child);}},appendChild(child){this.append(child);return child;},replaceChildren(...children){own='';this.children=[];this.append(...children);},
      setAttribute(key,value){this.attrs[key]=String(value);},getAttribute(key){return this.attrs[key]??null;},focus(){context.document.activeElement=this;},contains(other){return other===this||descendants(this).includes(other);},
      remove(){if(this.parentNode)this.parentNode.children=this.parentNode.children.filter(child=>child!==this);for(const [key,value]of roots)if(value===this)roots.delete(key);},
      before(child){this.parentNode?.append(child);if(child.id)roots.set('#'+child.id,child);},after(child){this.before(child);},
      querySelectorAll(selector){return descendants(this).filter(child=>selector.startsWith('.')?child.className.split(' ').includes(selector.slice(1)):child.tag===selector);},querySelector(selector){return this.querySelectorAll(selector)[0]||null;},classList:{add(){},remove(){},toggle(){}}};return node;
  }
  const $=key=>{const id=key.startsWith('#')?key.slice(1):null;const found=id&&[...roots.values()].flatMap(node=>[node,...descendants(node)]).find(node=>node.id===id);if(found)return found;if(!roots.has(key))roots.set(key,element());return roots.get(key);};$('#detail-value-modal').hidden=true;$('#drawer').hidden=true;
  const tabs=['overview','json','raw'].map(pane=>{const node=element('button');node.dataset.pane=pane;return node;});
  context=vm.createContext({window:{getSelection:()=>selection,ValueInspector:{close(){inspectorOpen=false;},open(value,options){if(options.isCurrent?.()===false)return;inspectorOpen=true;inspections.push({value,options});}},Tasks:{cancelLatest:name=>cancelled.push(name)},JavaTrace:{renderPreserved:(value,options)=>{java.push({value,options});return element('details','java-trace');}}},
    document:{activeElement:null,querySelectorAll:selector=>selector==='#drawer .dtab'?tabs:[],addEventListener:(name,fn)=>{if(!events.has(name))events.set(name,[]);events.get(name).push(fn);}},
    structuredClone,TextEncoder,JSON:{stringify:JSON.stringify,parse(){throw Error('display must never parse native canonical JSON');}},state:{cases:live,currentDetailEv:null},
    $ ,el:element,colLabel:String,filterFocusTarget:anchor=>anchor,toast:(text,type)=>messages.push({text,type}),showCtxMenu:(_x,_y,items)=>menus.push(items),
    openValueFilter:(...args)=>filters.push({kind:'composer',args}),addFilter:filter=>filters.push({kind:'apply',filter}),switchDetailTab(){},
    navigator:{clipboard:{writeText:text=>clipboard(text)}},DetailFields:{format(){assert.fail('native field text must not be reparsed/formatted');}},activeCase:()=>live.cases[0],
    api:async(command,args,options)=>{calls.push({command,args:structuredClone(args),options});
      if(command==='case_evidence_member_detail')return typeof next==='function'?next(args.request):structuredClone(next);
      if(command==='case_evidence_member_field_text')return typeof exact==='function'?exact(args.request):{kind:'preserved_field_text',member,column:args.request.column,present:true,type:'number',text:exact,complete:true};
      if(command==='case_evidence_preview'){if(previewOverride)return previewOverride();return{kind:'evidence_preview_page',columns:[],rows:[{kind:'evidence_preview',member,cells:[]}],total:1,nextCursor:null};}
      if(command==='case_evidence_member_java_trace')return{member,state:'unavailable',trace:null,reason:'not_java'};
      throw Error(command);
    }});
  for(const name of ['evidence-ui.js','case-evidence.js','case-evidence-preserved.js'])vm.runInContext(read(name),context);
  const client=context.window.CaseEvidence.create({enabled:true,invoke:async()=>documentValue()});await client.load();const services={client};context.nativeEvidenceServices=()=>services;
  context.window.AnalysisContexts={capture:()=>structuredClone(live.cases[0].analysisContext),isCurrent:captured=>['caseId','analysisId','configRevision','visibilityRevision'].every(key=>captured[key]===live.cases[0].analysisContext[key])};
  const app=read('app.js'),part=(start,end)=>app.slice(app.indexOf(start),app.indexOf(end,app.indexOf(start)));
  vm.runInContext(part('let detailRequest =','function detailAdmissionCurrent('),context);vm.runInContext(part('let detailValueReturnFocus =','function openDetailValue('),context);
  vm.runInContext(part('function showDetailLoading()','function openContextInspector('),context);vm.runInContext(part('function closeDrawer()','function switchDetailTab('),context);
  vm.runInContext(part('  $("#detail-value-copy").onclick =','  $("#dr-prev").onclick ='),context);
  vm.runInContext(read('case-evidence-detail.js'),context);
  const controller=context.window.CaseEvidenceDetail;
  return{context,controller,calls,copied,menus,filters,messages,cancelled,java,inspections,inspectorOpen:()=>inspectorOpen,$,element,events,open:()=>controller.open(reference,member),reply:value=>{next=value;},preview:fn=>{previewOverride=fn;},exact:value=>{exact=value;},clipboard:fn=>{clipboard=fn;},selection:value=>{selection=value;},replace(){live=structuredClone(live);context.state.cases=live;},live:()=>live,
    valueButtons:()=>$('#pane-overview').querySelectorAll('.detail-tree-value'),button:label=>$('#pane-overview').querySelectorAll('button').find(node=>node.textContent===label)};
}
function domEvent(node){return{target:node,currentTarget:node,clientX:1,clientY:2,preventDefault(){},stopPropagation(){}};}

test('preserved detail uses existing drawer, exposes bounded field pages, and never creates an Event',async()=>{
  const f=await fixture();assert.equal(await f.open(),true);assert.equal(f.context.state.currentDetailEv,null);assert.match(f.$('#drawer-badges').textContent,/Evidência preservada/);
  assert.match(f.$('#pane-overview').textContent,/1–2 de 2 campos/);assert.match(f.valueButtons()[0].textContent,/prévia limitada/);assert.equal(f.calls.length,1);assert.equal(f.calls[0].command,'case_evidence_member_detail');
  await f.context.copyDetail();assert.equal(f.copied[0],data().envelope.text);assert.equal(f.$('#dr-prev').hidden,true);
});

test('whole field menu resolves exact native text instead of filtering the clipped preview',async()=>{
  const f=await fixture();await f.open();const button=f.valueButtons()[0];button.oncontextmenu(domEvent(button));await f.menus.at(-1)[0].onClick();
  assert.equal(f.calls.at(-1).command,'case_evidence_member_field_text');assert.equal(f.filters[0].args[1],'1.0');assert.equal(f.filters[0].args[3],'equals_exact');
});

test('full native number/object text remains verbatim in the existing value modal and copy handler',async()=>{
  const f=await fixture();await f.open();f.exact('{"b":1.0,"a":18446744073709551615}');await f.valueButtons()[0].onclick();
  assert.equal(f.$('#detail-value-content').textContent,'{"b":1.0,"a":18446744073709551615}');assert.match(f.$('#detail-value-type').textContent,/preservada/);
  await f.$('#detail-value-copy').onclick();assert.equal(f.copied[0],'{"b":1.0,"a":18446744073709551615}');
});

test('explicit whitespace selections retain all bytes and avoid a whole-field fetch',async()=>{
  const f=await fixture();await f.open();const button=f.valueButtons()[1];f.selection({anchorNode:button,focusNode:button,toString:()=> ' \r\n  '});button.oncontextmenu(domEvent(button));
  await f.menus.at(-1)[0].onClick();assert.equal(f.filters[0].args[1],' \r\n  ');assert.equal(f.filters[0].args[3],'contains');
  await f.menus.at(-1).at(-1).onClick();assert.equal(f.copied[0],' \r\n  ');assert.equal(f.calls.filter(c=>c.command==='case_evidence_member_field_text').length,0);
});

test('replacement Case and closed drawer suppress pending native detail and action results',async()=>{
  for(const change of ['replace','close']){
    const f=await fixture(),gate=deferred();f.reply(()=>gate.promise);const opening=f.open();await settle();change==='replace'?f.replace():f.context.closeDrawer();gate.resolve(data());assert.equal(await opening,false);assert.equal(f.context.state.currentDetailEv,null);assert.equal(f.$('#drawer-badges').textContent,'');
  }
  const f=await fixture();await f.open();const gate=deferred();f.exact(()=>gate.promise);const button=f.valueButtons()[0];button.oncontextmenu(domEvent(button));const action=f.menus.at(-1)[0].onClick();await settle();f.context.closeDrawer();gate.resolve({kind:'preserved_field_text',member,column:'number',present:true,type:'number',text:'1.0',complete:true});await action;assert.equal(f.filters.length,0);
});

test('native detail pagination uses returned page sizes and rejects incomplete final counts',async()=>{
  const f=await fixture(),first=data();first.fieldsTotal=3;first.nextCursor='next';f.reply(first);await f.open();const second=data();second.fields=[{column:'last',type:'null',text:'null',complete:true}];second.fieldsTotal=3;second.nextCursor=null;f.reply(second);
  await f.button('Próximos campos').onclick();assert.match(f.$('#pane-overview').textContent,/3–3 de 3 campos/);assert.equal(f.calls.at(-1).args.request.cursor,'next');
  const broken=data();broken.fieldsTotal=3;f.reply(broken);await f.button('Campos anteriores').onclick();assert.match(f.messages.at(-1).text,/contagem preservada/);
});

test('first-page failures stop the loading state and clipped envelope is never offered as complete JSON',async()=>{
  const f=await fixture();f.reply(()=>{throw Error('native unavailable');});assert.equal(await f.open(),false);assert.match(f.$('#pane-overview').textContent,/native unavailable/);
  const clipped=data();clipped.envelope.complete=false;f.reply(clipped);await f.open();assert.equal(f.$('#dr-copy').hidden,true);assert.match(f.$('#pane-json').textContent,/Prévia limitada/);await f.context.copyDetail();assert.equal(f.copied.length,0);
});

test('native preserved Java stays lazy and reuses the exact owned response on repeated expansion',async()=>{
  const f=await fixture();await f.open();assert.equal(f.calls.length,1);assert.equal(f.java[0].value.kind,'preserved_member_detail');
  await f.java[0].options.load();await f.java[0].options.load();assert.equal(f.calls.filter(c=>c.command==='case_evidence_member_java_trace').length,1);
  f.context.closeDrawer();assert.equal(f.java[0].options.isCurrent(),false);assert.equal(f.$('#pane-raw').oncontextmenu,null);
});

test('historical copy ignores current visibility while a filter menu refuses changed analytical admission',async()=>{
  const f=await fixture();await f.open();const button=f.valueButtons()[0];button.oncontextmenu(domEvent(button));const menu=f.menus.at(-1);f.live().cases[0].analysisContext.visibilityRevision++;
  await menu[0].onClick();assert.equal(f.filters.length,0);await menu.at(-1).onClick();assert.equal(f.copied[0],'1.0');
});

test('first-member lookup cannot rebind an open request to a replacement Case instance',async()=>{
  const f=await fixture(),gate=deferred();f.preview(()=>gate.promise);const pending=f.controller.open(reference);await settle();f.replace();
  gate.resolve({kind:'evidence_preview_page',columns:[],rows:[{kind:'evidence_preview',member,cells:[]}],total:1,nextCursor:null});assert.equal(await pending,false);
  assert.equal(f.calls.some(c=>c.command==='case_evidence_member_detail'),false);
});

test('native detail and exact copies follow explicit reveal without parsing or altering preserved numeric text',async()=>{
  const f=await fixture(),value=data();value.fields[0]={column:'token',type:'string',text:'private-token',complete:true};
  value.envelope.text='{"fields":{"token":"private-token","n":9007199254740993,"x":1.0}}';f.reply(value);f.exact('private-token');await f.open();
  assert.doesNotMatch(f.$('#pane-overview').textContent,/private-token/);assert.doesNotMatch(f.$('#pane-json').textContent,/private-token/);
  await f.context.copyDetail();assert.doesNotMatch(f.copied.at(-1),/private-token/);assert.match(f.copied.at(-1),/9007199254740993/);
  await f.valueButtons()[0].onclick();await f.$('#detail-value-copy').onclick();assert.equal(f.copied.at(-1),'[oculto]');
  f.$('#detail-value-inspect').onclick();assert.equal(f.inspections.at(-1).value,'private-token');assert.equal(f.inspections.at(-1).options.revealed,false);
  f.$('#dr-reveal').onclick();assert.equal(f.inspectorOpen(),false);assert.equal(f.$('#detail-value-content').textContent,'');
  assert.match(f.$('#pane-overview').textContent,/private-token/);await f.context.copyDetail();assert.equal(f.copied.at(-1),value.envelope.text);
  await f.valueButtons()[0].onclick();await f.$('#detail-value-copy').onclick();assert.equal(f.copied.at(-1),'private-token');
  f.$('#dr-reveal').onclick();assert.doesNotMatch(f.$('#pane-overview').textContent,/private-token/);assert.equal(f.$('#detail-value-content').textContent,'');
  assert.equal(value.fields[0].text,'private-token');assert.equal(f.calls.filter(call=>call.command==='case_evidence_member_detail').length,1,'toggle is local');
});

test('native reveal and inspector reset on close and old inspection rejects a replaced Case',async()=>{
  const f=await fixture();await f.open();await f.valueButtons()[0].onclick();const inspect=f.$('#detail-value-inspect').onclick;inspect();
  const view=f.inspections.at(-1);assert.equal(view.options.isCurrent(),true);f.replace();assert.equal(view.options.isCurrent(),false);
  inspect();assert.equal(f.inspections.length,1);f.context.closeDrawer();assert.equal(f.inspectorOpen(),false);assert.equal(f.$('#pane-json').textContent,'');
  await f.open();assert.equal(f.$('#dr-reveal').textContent,'Mostrar valores ocultos');
});

test('unavailable full native field does not become an empty value or an inspectable original',async()=>{
  const f=await fixture();await f.open();f.exact(request=>({kind:'preserved_field_text',member,column:request.column,present:false,type:null,text:null,complete:true}));
  await f.valueButtons()[0].onclick();assert.equal(f.$('#detail-value-content').textContent,'(campo ausente)');
  await f.$('#detail-value-copy').onclick();assert.equal(f.copied.length,0);assert.match(f.messages.at(-1).text,/ausente/);assert.equal(f.inspections.length,0);
});
