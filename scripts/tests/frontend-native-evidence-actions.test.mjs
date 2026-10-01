import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const owner={storeId:'store',caseId:'case',analysisId:'analysis'},identity={caseId:'case',analysisId:'analysis',configRevision:1,visibilityRevision:1};
const ref=(id='a',version='1',count=2)=>({kind:'native_evidence',schemaVersion:1,owner,containerId:id,manifestId:`manifest-${id}-${version}`,manifestSha256:'a'.repeat(64),memberCount:count});
const view=reference=>({kind:'native_evidence_container',reference,preservedCount:reference.memberCount,preview:null});
const member=(reference,n)=>({containerId:reference.containerId,manifestId:reference.manifestId,occurrenceId:`occurrence-${n}`});
function fixture(){
  const c={id:'case',analysisContext:identity,items:[{id:'i',label:'saved',rows:view(ref())}]},store={store:{storeId:'store',epoch:'epoch',revision:'1'},caseEvidence:[{state:'ready',owner}],cases:[c]},calls=[],discarded=[];let active=c,status='ready',sourceCurrent=true,saveResult=true,sequence=0,prepareGate=null;
  const client={captureSource:async request=>({...request,catalogSignature:'b'.repeat(64),catalogEpoch:1}),openCase:async()=>({state:'ready',publication:{caseKey:'case-key',caseContentToken:'token'}}),findMembers:async request=>{calls.push(['find',plain(request)]);return{rows:[]};},prepareCapture:request=>({kind:'add',request}),prepareMembership:request=>({kind:'members',request}),
    prepare:async ticket=>{calls.push(['prepare',plain(ticket)]);if(prepareGate)await prepareGate;const before=ticket.kind==='add'?ref('new','1',ticket.request.rows.length):ticket.request.reference,reference={...before,kind:'pending_native_evidence',manifestId:`prepared-${++sequence}`,token:`token-${sequence}`,requestId:`request-${sequence}`,bytes:100,expiresAt:"soon",memberCount:ticket.kind==='add'?ticket.request.rows.length:ticket.request.action.kind==='restore'?ticket.request.action.target.memberCount:before.memberCount-ticket.request.action.members.length,purpose:{kind:'new_container'}};return ticket.kind==='add'?reference:{reference,undo:before,changedCount:1,remainingCount:reference.memberCount};},discard:async ticket=>{discarded.push(ticket);return true;}};
  const context=vm.createContext({window:{},structuredClone,TextEncoder});for(const file of ['case-evidence.js','case-evidence-actions.js'])vm.runInContext(read(file),context);
  const save=async()=>{calls.push(['save']);if(!saveResult){status='retry_required';return false;}for(const item of c.items){if(item.rows?.reference.kind==='pending_native_evidence'){const value={...item.rows.reference,kind:'native_evidence'};delete value.token;delete value.purpose;delete value.requestId;delete value.bytes;delete value.expiresAt;item.rows=view(value);}}return true;};
  const actions=context.window.CaseEvidenceActions.create({client,session:{status:()=>status},getStore:()=>store,currentCase:()=>active,save,selectionOwner:()=>({identity,sourceGeneration:8}),selectionCurrent:()=>sourceCurrent,scope:()=> 'dataset',station:()=>null});
  return {actions,c,store,client,calls,discarded,setActive:value=>{active=value;},setSource:value=>{sourceCurrent=value;},failSave:()=>{saveResult=false;},gate:value=>{prepareGate=value;}};
}

test('native capture forwards exact handles and attaches only the returned reference',async()=>{
 const f=fixture(),selected=await f.actions.selection([{id:9,event_ref:'exact',message:'preview-only',raw:'must not transport'}]);const item=await f.actions.add(selected,{id:'new',label:'capture'});
 assert.deepEqual(f.calls[0][1].request.rows,[{id:9,eventRef:'exact'}]);assert.equal(item.rows.kind,'native_evidence_container');assert.equal(item.rows.reference.kind,'native_evidence');assert.equal(Object.hasOwn(item,'raw'),false);assert.equal(f.c.items.length,2);
});

test('a changed selection is rejected before any native preparation or append',async()=>{
 const f=fixture(),selected=await f.actions.selection([{id:9,event_ref:'exact'}]);f.setSource(false);await assert.rejects(f.actions.add(selected,{id:'new'}),/VIEW_CHANGED/);assert.equal(f.calls.length,0);assert.equal(f.c.items.length,1);
});

test('exact native removal and Undo restore membership through prepared CAS and preserve authored notes',async()=>{
 const f=fixture(),item=f.c.items[0],before=item.rows.reference;const receipt=await f.actions.remove(f.c,[{item,member:member(before,1)}]);assert.equal(item.rows.preservedCount,1);assert.equal(f.actions.canUndo(receipt),true);item.label='later note';await f.actions.undo(receipt);assert.equal(item.rows.preservedCount,2);assert.equal(item.label,'later note');assert.equal(f.calls[2][1].request.action.kind,'restore');assert.deepEqual(f.calls[2][1].request.action.target,before);assert.equal(f.actions.canUndo(receipt),false);
});

test('native removal targets distinguish identical numeric IDs through container/member tuples',async()=>{
 const f=fixture(),a=f.c.items[0],b={id:'i',rows:view(ref('b'))};f.c.items.push(b);await f.actions.remove(f.c,[{item:b,member:member(b.rows.reference,1)}]);assert.equal(a.rows.preservedCount,2);assert.equal(b.rows.preservedCount,1);assert.equal(f.calls[0][1].request.reference.containerId,'b');
});

test('late preparation after switching Cases is discarded without changing the original draft',async()=>{
 const f=fixture();let resolve;f.gate(new Promise(yes=>resolve=yes));const item=f.c.items[0],before=item.rows;const work=f.actions.remove(f.c,[{item,member:member(before.reference,1)}]);f.setActive({id:'other'});resolve();await assert.rejects(work,/VIEW_CHANGED/);assert.equal(item.rows,before);assert.equal(f.discarded.length,1);assert.equal(f.calls.filter(call=>call[0]==='save').length,0);
});

test('a lost save acknowledgement keeps the exact pending draft and blocks further mutations',async()=>{
 const f=fixture(),item=f.c.items[0];f.failSave();await assert.rejects(f.actions.remove(f.c,[{item,member:member(item.rows.reference,1)}]),/SAVE_RETRY_REQUIRED/);assert.equal(item.rows.reference.kind,'pending_native_evidence');assert.equal(item.rows.preservedCount,1);assert.equal(f.discarded.length,0);await assert.rejects(f.actions.detach(f.c,item),/SAVE_RETRY_REQUIRED/);assert.equal(f.c.items.length,1);
});

test('whole-item Undo reattaches exact immutable history only while absent and preserves later items',async()=>{
 const f=fixture(),item=f.c.items[0],before=item.rows,receipt=await f.actions.detach(f.c,item);const later={id:'later'};f.c.items.push(later);assert.equal(f.actions.canUndo(receipt),true);await f.actions.undo(receipt);assert.equal(f.c.items[0],item);assert.equal(item.rows,before);assert.equal(f.c.items[1],later);
 const second=await f.actions.detach(f.c,item);f.c.items.push({id:'replacement',rows:view(ref('a','later'))});assert.equal(f.actions.canUndo(second),false);await assert.rejects(f.actions.undo(second),/VIEW_CHANGED/);
});

test('membership requests use source authority and row handles without transporting Event values',async()=>{
 const f=fixture();await f.actions.membership([{id:7,event_ref:'row',message:'not sent'}]);assert.deepEqual(f.calls[0][1].rows,[{id:7,eventRef:'row'}]);assert.deepEqual(f.calls[0][1].owner,owner);assert.equal(f.calls[0][1].source.sourceGeneration,8);
});

function membershipUI(){
 const code=read('workspace-context.js'),requests=[],nodes=[];let owner='a';
 const node=()=>({classList:{values:new Set(),toggle(key,on){on?this.values.add(key):this.values.delete(key);}},title:'',dataset:{eventId:'1'},hidden:false,removeAttribute(){}});
 const table=node(),drawer=node(),save=node();nodes.push(table);const c={id:'case'},state={cases:{store:{epoch:'epoch',revision:'1'},caseEvidence:[{state:'ready',owner,evidenceSignature:'signature'}]},rows:[{id:1,event_ref:'row-a'}],currentDetailEv:null};
 const context=vm.createContext({window:{CaseEvidence:{active:true},AnalysisContexts:{capture:()=>({source:owner})}},state,activeCase:()=>c,caseSig:()=> 'case',scope:'dataset',document:{querySelectorAll:()=>nodes,addEventListener(){}},$:selector=>selector==='#drawer'?drawer:save,updateToggle(){},nativeEvidenceServices:()=>({actions:{membership:async(rows,options)=>new Promise(resolve=>requests.push({rows,options,resolve}))}})});
 const start=code.indexOf('  let nativeMembership ='),end=code.indexOf('  const oldDetail =',start);vm.runInContext(code.slice(start,end),context);
 return{context,state,table,save,requests,owner:value=>{owner=value;},included:row=>{context.target=row;return vm.runInContext('isIncluded(target)',context);}};
}
const drain=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
test('actual native membership badge confirms exact handles and shows ambiguous occurrences explicitly',async()=>{
 const f=membershipUI();assert.equal(f.included(f.state.rows[0]),false);await drain();assert.equal(f.requests.length,1);assert.deepEqual(plain(f.requests[0].rows),[{id:1,eventRef:'row-a'}]);
 f.requests[0].resolve({rows:[{row:{id:1,eventRef:'row-a'},state:'ambiguous',matches:[{},{}]}]});await drain();assert.equal(f.included(f.state.rows[0]),true);assert.equal(f.table.classList.values.has('event-in-case'),true);assert.match(f.table.title,/2 ocorrências/);
});

test('actual membership badge discards late replies after replacing the visible page/source',async()=>{
 const f=membershipUI();f.included(f.state.rows[0]);await drain();f.owner('b');f.state.rows=[{id:1,event_ref:'row-b'}];assert.equal(f.included(f.state.rows[0]),false);await drain();assert.equal(f.requests.length,2);
 f.requests[0].resolve({rows:[{row:{id:1,eventRef:'row-a'},state:'unique',matches:[{}]}]});await drain();assert.equal(f.table.classList.values.has('event-in-case'),false);
 f.requests[1].resolve({rows:[{row:{id:1,eventRef:'row-b'},state:'missing',matches:[]}]});await drain();assert.equal(f.included(f.state.rows[0]),false);assert.equal(f.table.title,'');
});

test('composed native adapter session and actions consume bare capture references and commit their authority',async()=>{
 const snapshot={schemaVersion:1,...identity,config:{derivedFields:[],references:[]},migrationDiagnostics:[]};
 let document={evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},active:'case',cases:[{id:'case',analysisContext:snapshot,items:[]}],caseEvidence:[{state:'ready',owner,evidenceSignature:'empty',preservedCount:0}],diagnostics:[]},live;const calls=[];let id=0;
 const context=vm.createContext({window:{},structuredClone,TextEncoder,crypto:{randomUUID:()=>`request-${++id}`}});for(const file of ['case-evidence.js','case-evidence-session.js','case-evidence-actions.js'])vm.runInContext(read(file),context);
 const client=context.window.CaseEvidence.create({enabled:true,requestId:()=>`request-${++id}`,invoke:async(command,{request}={})=>{calls.push([command,plain(request??{})]);
  if(command==='cases_load_view')return structuredClone(document);
  if(command==='case_evidence_source_receipt')return{...request,catalogSignature:'b'.repeat(64),catalogEpoch:1};
  if(command==='case_evidence_prepare')return{...ref('capture','1',request.rows.length),kind:'pending_native_evidence',requestId:request.requestId,token:'token',bytes:100,expiresAt:'future',purpose:{kind:'new_container'}};
  if(command==='cases_save_view'){document=JSON.parse(request.documentJson);document.store.revision='2';const pending=document.cases[0].items[0].rows.reference,committed={kind:'native_evidence',schemaVersion:1,owner:pending.owner,containerId:pending.containerId,manifestId:pending.manifestId,manifestSha256:pending.manifestSha256,memberCount:pending.memberCount};document.cases[0].items[0].rows=view(committed);document.caseEvidence[0]={state:'ready',owner,evidenceSignature:'captured',preservedCount:1};return{requestId:request.requestId,committedStore:document.store,currentStore:document.store,evidence:[committed],analysisContexts:[snapshot],caseEvidence:document.caseEvidence,replayed:false,reconcileRequired:false};}
  throw Error(`Unexpected ${command}`);
 }});
 const session=context.window.CaseEvidenceSession.create({client,getStore:()=>live,setStore:value=>{live=value;}});await session.load();
 const actions=context.window.CaseEvidenceActions.create({client,session,getStore:()=>live,currentCase:()=>live.cases[0],save:()=>session.save(),selectionOwner:()=>({identity,sourceGeneration:4}),selectionCurrent:()=>true,scope:()=> 'dataset',station:()=>null});
 const selection=await actions.selection([{id:3,event_ref:'exact-ref',raw:'not sent'}]);await actions.add(selection,{id:'new',label:'Captured'});
 assert.equal(live.cases[0].items[0].rows.reference.kind,'native_evidence');assert.equal(live.cases[0].items[0].rows.preservedCount,1);assert.deepEqual(calls.map(call=>call[0]),['cases_load_view','case_evidence_source_receipt','case_evidence_prepare','cases_save_view']);assert.equal(JSON.stringify(calls).includes('not sent'),false);assert.equal(session.status(),'ready');
});

test('actual group capture discloses the bounded first page and keeps matched count distinct from preserved count',async()=>{
 for(const total of [501,null]){
  const c={id:'case',items:[]},saved=[],finishes=[],rows=Array.from({length:500},(_,id)=>({id,event_ref:`row-${id}`})),nodes=new Map();
  const context=vm.createContext({window:{WorkspaceContext:{refreshMembership(){}}},pendingCaseAdd:{kind:'group',nativeCase:c,nativeGuard:()=>true,column:'message',value:'match',op:'equals_exact'},state:{currentArtifact:null},ensureCase:()=>c,nativeEvidenceEnabled:()=>true,$:selector=>{if(!nodes.has(selector))nodes.set(selector,{value:'',hidden:true});return nodes.get(selector);},toast(){},startOperation(){},finishOperation:(_title,text)=>finishes.push(text),backendFilters:()=>[],api:async command=>{assert.equal(command,'query_page');return{rows,total,hasMore:true};},nativeEvidenceServices:()=>({actions:{selection:async rows=>rows,add:async(_rows,metadata)=>{saved.push(metadata);return metadata;}}}),caseItemBase:(_kind,_station,foundCount,includedCount)=>({foundCount,includedCount}),chipLabel:()=> 'Group',structuredClone,fmtNum:String,updateAnalysisBadge(){},renderAnalysis(){},renderStations(){}});
  const code=read('app.js'),start=code.indexOf('async function confirmCaseAdd()'),end=code.indexOf('// popover genérico',start);vm.runInContext(code.slice(start,end),context);await context.confirmCaseAdd();
  assert.equal(saved[0].foundCount,total);assert.equal(saved[0].includedCount,500);assert.equal(saved[0].captureScope.hasMore,true);assert.match(finishes[0],total===null?/primeira página.*mais registros/:/500 de 501/);
 }
});

test('composed new-Case first save establishes the issued owner before source receipt and native capture',async()=>{
 const snapshot={schemaVersion:1,...identity,config:{derivedFields:[],references:[]},migrationDiagnostics:[]},calls=[];
 let stored={evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},active:'case',cases:[{id:'case',name:'New Case',items:[]}],caseEvidence:[],diagnostics:[]},id=0;
 const context=vm.createContext({window:{},state:{cases:null,sourcePublication:{generation:4},currentArtifact:{id:'source',loadedAt:1}},structuredClone,TextEncoder,document:{dispatchEvent(){}},CustomEvent:class{constructor(type,options){this.type=type;this.detail=options?.detail;}},crypto:{randomUUID:()=>`request-${++id}`}});
 for(const file of ['case-evidence.js','case-evidence-session.js','case-evidence-actions.js','analysis-context.js'])vm.runInContext(read(file),context);
 const client=context.window.CaseEvidence.create({enabled:true,requestId:()=>`request-${++id}`,invoke:async(command,{request}={})=>{calls.push([command,plain(request??{})]);
  if(command==='cases_load_view')return structuredClone(stored);
  if(command==='case_evidence_source_receipt'){assert.deepEqual(plain(request.analysisContext),identity);assert.equal(stored.cases[0].analysisContext.analysisId,'analysis');return{...request,catalogSignature:'b'.repeat(64),catalogEpoch:1};}
  if(command==='case_evidence_prepare'){assert.deepEqual(plain(request.target),owner);return{...ref('capture','1',request.rows.length),kind:'pending_native_evidence',requestId:request.requestId,token:'token',bytes:100,expiresAt:'future',purpose:{kind:'new_container'}};}
  if(command==='cases_save_view'){
   stored=JSON.parse(request.documentJson);stored.store.revision=String(BigInt(stored.store.revision)+1n);stored.cases[0].analysisContext=snapshot;const evidence=[];
   for(const item of stored.cases[0].items){const pending=item.rows.reference,committed={kind:'native_evidence',schemaVersion:1,owner,containerId:pending.containerId,manifestId:pending.manifestId,manifestSha256:pending.manifestSha256,memberCount:pending.memberCount};item.rows=view(committed);evidence.push(committed);}
   stored.caseEvidence=[{state:'ready',owner,evidenceSignature:stored.cases[0].items.length?'captured':'empty',preservedCount:stored.cases[0].items.length}];return{requestId:request.requestId,committedStore:stored.store,currentStore:stored.store,evidence,analysisContexts:[snapshot],caseEvidence:stored.caseEvidence,replayed:false,reconcileRequired:false};
  }throw Error(`Unexpected ${command}`);
 }});
 const contexts=context.window.AnalysisContexts,session=context.window.CaseEvidenceSession.create({client,getStore:()=>context.state.cases,setStore:value=>{context.state.cases=value;},captureOwner:item=>contexts.capture(item.id),adoptContext:(value,options)=>contexts.adopt(value,options)});
 context.saveCases=()=>session.save();context.api=async()=>{assert.fail('new owner must come from the save receipt');};await session.load();assert.equal(contexts.capture().identity,null);
 const actions=context.window.CaseEvidenceActions.create({client,session,getStore:()=>context.state.cases,currentCase:()=>context.state.cases.cases[0],save:()=>session.save(),selectionOwner:()=>contexts.capture(),prepareSelectionOwner:captured=>contexts.prepare(captured,{metadata:true}),selectionCurrent:captured=>contexts.isCurrent(captured),scope:()=> 'dataset',station:()=>null});
 const selected=await actions.selection([{id:3,event_ref:'original-ref'}]);await actions.add(selected,{id:'new',label:'Preserved'});
 assert.deepEqual(calls.map(call=>call[0]),['cases_load_view','cases_save_view','case_evidence_source_receipt','case_evidence_prepare','cases_save_view']);assert.equal(context.state.cases.cases[0].items[0].rows.reference.kind,'native_evidence');assert.equal(context.state.cases.caseEvidence[0].owner.analysisId,'analysis');assert.equal(session.status(),'ready');
});

test('actual Case-add confirmation respects Cancel and repeated clicks while first-owner preparation is pending',async()=>{
 const code=read('app.js'),start=code.indexOf('async function confirmCaseAdd()'),end=code.indexOf('// popover genérico',start);let resolve;const c={id:'case'},request={nativeCase:c,nativeGuard:()=>true,nativeOwnerReady:new Promise(yes=>resolve=yes)};let starts=0;
 const context=vm.createContext({window:{},pendingCaseAdd:request,ensureCase:()=>c,nativeEvidenceEnabled:()=>true,toast(){},startOperation:()=>{starts++;},$:()=>({value:'',hidden:true})});vm.runInContext(code.slice(start,end),context);
 const first=context.confirmCaseAdd(),second=context.confirmCaseAdd();context.pendingCaseAdd=null;resolve(true);await Promise.all([first,second]);assert.equal(starts,0);
});
