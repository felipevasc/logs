import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const sources=['case-evidence.js','case-evidence-session.js'].map(name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'));
const clone=structuredClone,plain=value=>JSON.parse(JSON.stringify(value));
const authority={storeId:'store',caseId:'case',analysisId:'analysis'},stamp=revision=>({storeId:'store',epoch:'epoch',revision});
const snapshot={schemaVersion:1,caseId:'case',analysisId:'analysis',configRevision:0,visibilityRevision:0,config:{derivedFields:[],references:[]},migrationDiagnostics:[]};
const ref=(manifest='one',count=2)=>({kind:'native_evidence',schemaVersion:1,owner:authority,containerId:'container',manifestId:manifest,manifestSha256:'a'.repeat(64),memberCount:count});
const container=reference=>({kind:'native_evidence_container',reference,preservedCount:reference.memberCount,preview:null});
const document=()=>({evidenceViewVersion:1,store:stamp('1'),active:'case',cases:[{id:'case',analysisContext:clone(snapshot),name:'Case',notes:'old',items:[{id:'item',note:'item note',rows:container(ref())}]}],caseEvidence:[{state:'ready',owner:authority,evidenceSignature:'signature',preservedCount:2}],diagnostics:[]});
const gate=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const reached=async fn=>{for(let n=0;n<100;n++){if(fn())return;await Promise.resolve();}assert.fail('expected async boundary');};
function fixture(){
  const context=vm.createContext({window:{},structuredClone,TextEncoder,JSON:{stringify:JSON.stringify,parse(){throw Error('no production parse-back');}}});
  for(const source of sources)vm.runInContext(source,context);
  let live={active:null,cases:[]},native=document(),nextId=0;const calls=[],handlers=new Map(),notices=[];
  const receipt=request=>{const doc=JSON.parse(request.documentJson);return{requestId:request.requestId,committedStore:stamp(String(BigInt(request.expectedStore.revision)+1n)),currentStore:stamp(String(BigInt(request.expectedStore.revision)+1n)),evidence:doc.cases.flatMap(c=>(c.items||[]).map(i=>i.rows?.reference).filter(Boolean)),analysisContexts:[clone(snapshot)],caseEvidence:clone(doc.caseEvidence),replayed:false,reconcileRequired:false};};
  const client=context.window.CaseEvidence.create({enabled:true,requestId:()=>`request-${++nextId}`,invoke:async(command,args)=>{
    calls.push({command,args:clone(args)});if(handlers.has(command))return handlers.get(command)(args);
    if(command==='cases_load_view')return clone(native);if(command==='cases_save_view')return receipt(args.request);throw Error(command);
  }});
  const session=context.window.CaseEvidenceSession.create({client,getStore:()=>live,setStore:value=>{live=value;},captureOwner:c=>c,
    adoptContext:async(value,{owner})=>{const c=live.cases.find(c=>c.id===value.caseId);if(c===owner)c.analysisContext=clone(value);},changed:value=>notices.push(value.kind)});
  return{client,session,calls,notices,receipt,get live(){return live;},setLive:value=>{live=value;},setNative:value=>{native=value;},on:(cmd,fn)=>handlers.set(cmd,fn)};
}

test('native session loads a tagged document without invoking legacy save or normalizing records',async()=>{
  const f=fixture();await f.session.load();assert.equal(f.live.evidenceViewVersion,1);assert.equal(f.live.cases[0].items[0].rows.kind,'native_evidence_container');
  assert.deepEqual(f.calls.map(c=>c.command),['cases_load_view']);assert.equal(f.session.status(),'ready');
});

test('durable metadata save preserves live Case identity and notes written while the request is pending',async()=>{
  const f=fixture();await f.session.load();const owner=f.live.cases[0],item=owner.items[0],wait=gate();owner.notes='first intent';
  f.on('cases_save_view',()=>wait.promise);const saving=f.session.save();await reached(()=>f.calls.some(c=>c.command==='cases_save_view'));
  const request=f.calls.at(-1).args.request;owner.notes='newer draft';item.note='newer item note';wait.resolve(f.receipt(request));await saving;
  assert.equal(f.live.cases[0],owner);assert.equal(f.live.cases[0].items[0],item);assert.equal(owner.notes,'newer draft');assert.equal(item.note,'newer item note');
  assert.equal(f.live.store.revision,'2');assert.equal(JSON.parse(request.documentJson).cases[0].notes,'first intent');
});

test('queued new saves prepare against the acknowledged revision and retain newer metadata',async()=>{
  const f=fixture();await f.session.load();const wait=gate();let calls=0;
  f.on('cases_save_view',({request})=>++calls===1?wait.promise:f.receipt(request));
  f.live.cases[0].notes='first';const first=f.session.save();await reached(()=>calls===1);
  f.live.cases[0].notes='latest';const second=f.session.save();assert.equal(f.calls.filter(c=>c.command==='cases_save_view').length,1);
  wait.resolve(f.receipt(f.calls.at(-1).args.request));await Promise.all([first,second]);
  const requests=f.calls.filter(c=>c.command==='cases_save_view').map(c=>c.args.request);
  assert.equal(requests[0].expectedStore.revision,'1');assert.equal(requests[1].expectedStore.revision,'2');
  assert.equal(JSON.parse(requests[1].documentJson).cases[0].notes,'latest');assert.equal(f.live.store.revision,'3');
});

test('a failed save retains its immutable request for retry and blocks an accidental fresh save',async()=>{
  const f=fixture();await f.session.load();f.live.cases[0].notes='first';
  f.on('cases_save_view',()=>{throw Error('lost ack');});await assert.rejects(f.session.save(),/lost ack/);
  const original=f.calls.at(-1).args.request;f.live.cases[0].notes='unsaved newer';
  await assert.rejects(f.session.save(),/RETRY_REQUIRED/);assert.equal(f.calls.filter(c=>c.command==='cases_save_view').length,1);
  f.on('cases_save_view',({request})=>f.receipt(request));await f.session.retry();
  assert.deepEqual(f.calls.at(-1).args.request,original);assert.equal(f.live.cases[0].notes,'unsaved newer');assert.equal(f.live.store.revision,'2');
  await f.session.save();assert.equal(JSON.parse(f.calls.at(-1).args.request.documentJson).cases[0].notes,'unsaved newer');
});

test('receipt adoption cannot resurrect a container removed from the live draft while saving',async()=>{
  const f=fixture();await f.session.load();const wait=gate();f.on('cases_save_view',()=>wait.promise);
  const saving=f.session.save();await reached(()=>f.calls.some(c=>c.command==='cases_save_view'));const request=f.calls.at(-1).args.request;
  f.live.cases[0].items=[];wait.resolve(f.receipt(request));await saving;assert.equal(f.live.cases[0].items.length,0);assert.equal(f.live.store.revision,'2');
});

test('reconciliation returns authority separately and never overwrites the authored draft',async()=>{
  const f=fixture();await f.session.load();f.live.cases[0].notes='local draft';
  f.on('cases_save_view',({request})=>({...f.receipt(request),currentStore:stamp('5'),replayed:true,reconcileRequired:true}));
  await assert.rejects(f.session.save(),/RECONCILE_REQUIRED/);assert.equal(f.live.cases[0].notes,'local draft');assert.equal(f.live.store.revision,'1');
  const native=document();native.store=stamp('5');native.cases[0].notes='external authority';f.setNative(native);
  const result=await f.session.reconcile();assert.equal(result.authoritative.cases[0].notes,'external authority');assert.equal(result.draft.cases[0].notes,'local draft');
  assert.equal(f.live.cases[0].notes,'local draft');assert.ok(result.failedRequest.documentJson);
});

test('load and late save receipts cannot replace a newly selected store instance',async()=>{
  const f=fixture(),wait=gate();f.on('cases_load_view',()=>wait.promise);const loading=f.session.load(),replacement={active:null,cases:[]};f.setLive(replacement);wait.resolve(document());
  await assert.rejects(loading,/VIEW_CHANGED/);assert.equal(f.live,replacement);
  const saved=fixture();await saved.session.load();const reply=gate();saved.on('cases_save_view',()=>reply.promise);const pending=saved.session.save();
  await reached(()=>saved.calls.some(c=>c.command==='cases_save_view'));const request=saved.calls.at(-1).args.request,next=document();next.store.epoch='new';saved.setLive(next);
  reply.resolve(saved.receipt(request));await assert.rejects(pending,/VIEW_CHANGED/);assert.equal(saved.live,next);
});

test('a queued save blocks reload even before its native invocation starts',async()=>{
  const f=fixture();await f.session.load();const saving=f.session.save();await assert.rejects(f.session.load(),/DRAFT_PENDING/);await saving;
  assert.equal(f.calls.filter(c=>c.command==='cases_load_view').length,1);
});

async function prepareMembership(f,manifest,count,base='one') {
  const reference=ref(base, count+1),ticket=f.client.prepareMembership({reference,action:{kind:'remove',members:[{containerId:'container',manifestId:base,occurrenceId:'removed'}]}});
  f.on('case_evidence_prepare_membership',({request})=>({reference:{...ref(manifest,count),kind:'pending_native_evidence',purpose:{kind:'replace_container',baseManifestId:base,baseManifestSha256:'a'.repeat(64)},token:`token-${manifest}`,requestId:request.requestId,bytes:10,expiresAt:'2026-10-01T17:00:00Z'},undo:reference,changedCount:1,remainingCount:count}));
  return (await f.client.prepare(ticket)).reference;
}

test('matching prepared references become committed while item reordering and new notes remain authored',async()=>{
  const f=fixture();await f.session.load();const c=f.live.cases[0],item=c.items[0],wait=gate();item.rows=container(await prepareMembership(f,'two',1));c.items.push({id:'narrative',note:'not a record container'});
  f.on('cases_save_view',()=>wait.promise);const saving=f.session.save();await reached(()=>f.calls.some(call=>call.command==='cases_save_view'));
  const request=f.calls.at(-1).args.request,result=f.receipt(request);result.evidence=[ref('two',1)];result.caseEvidence[0].evidenceSignature='signature-two';result.caseEvidence[0].preservedCount=1;
  c.items.reverse();item.note='edited while pending';wait.resolve(result);await saving;
  assert.equal(c.items[1],item);assert.equal(item.note,'edited while pending');assert.equal(item.rows.reference.kind,'native_evidence');
  assert.equal(item.rows.reference.manifestId,'two');assert.equal(item.rows.preservedCount,1);assert.equal(f.live.caseEvidence[0].evidenceSignature,'signature-two');
});

test('a newer prepared membership is never overwritten by the previous save receipt',async()=>{
  const f=fixture();await f.session.load();const item=f.live.cases[0].items[0],wait=gate();item.rows=container(await prepareMembership(f,'two',1));
  f.on('cases_save_view',()=>wait.promise);const saving=f.session.save();await reached(()=>f.calls.some(call=>call.command==='cases_save_view'));
  const request=f.calls.at(-1).args.request,result=f.receipt(request);result.evidence=[ref('two',1)];
  const next=container(await prepareMembership(f,'three',0,'two'));item.rows=next;wait.resolve(result);await saving;
  assert.equal(item.rows,next);assert.equal(item.rows.reference.kind,'pending_native_evidence');assert.equal(item.rows.reference.manifestId,'three');assert.equal(item.rows.preservedCount,0);
});

test('an in-place note edit during native load rejects the old document instead of replacing the draft',async()=>{
  const f=fixture();await f.session.load();const wait=gate();f.on('cases_load_view',()=>wait.promise);
  const loading=f.session.load(),current=f.live;current.cases[0].notes='typed during reload';wait.resolve(document());
  await assert.rejects(loading,/VIEW_CHANGED/);assert.equal(f.live,current);assert.equal(f.live.cases[0].notes,'typed during reload');
});

test('a dirty signal before the outer save debounce blocks reload immediately',async()=>{
  const f=fixture();await f.session.load();f.live.cases[0].notes='draft';f.session.markDirty();
  await assert.rejects(f.session.load(),/DRAFT_PENDING/);assert.equal(f.calls.filter(c=>c.command==='cases_load_view').length,1);
  await f.session.save();f.setNative(clone(f.live));await f.session.load();
});

test('receipt evidence states exclude deleted Cases while preserving newly authored Cases',async()=>{
  const f=fixture();await f.session.load();const wait=gate();let count=0;
  f.on('cases_save_view',({request})=>++count===1?wait.promise:{...f.receipt(request),analysisContexts:[]});
  const first=f.session.save();await reached(()=>count===1);const request=f.calls.at(-1).args.request;
  f.live.cases=[];f.live.active=null;f.live.caseEvidence=[];const second=f.session.save();wait.resolve(f.receipt(request));await Promise.all([first,second]);
  assert.deepEqual(plain(f.live.caseEvidence),[]);assert.deepEqual(plain(f.live.cases),[]);assert.equal(f.live.store.revision,'3');
});

test('native load can defer installation to the existing workspace source-queue boundary',async()=>{
  const f=fixture();await f.session.load();const before=f.live,next=document();next.store=stamp('2');next.cases[0].notes='external';f.setNative(next);
  const loaded=await f.session.load({install:false});assert.equal(f.live,before);assert.equal(f.live.cases[0].notes,'old');
  assert.equal(loaded.cases[0].notes,'external');assert.equal(loaded.store.revision,'2');
});

test('a deferred load rejects notes changed while awaiting the workspace installation boundary',async()=>{
  const f=fixture();await f.session.load();const next=document();next.store=stamp('2');f.setNative(next);
  const loaded=await f.session.load({install:false});f.live.cases[0].notes='changed while waiting for source queue';
  assert.throws(()=>f.session.assertLoadCurrent(loaded),/VIEW_CHANGED/);assert.equal(f.live.cases[0].notes,'changed while waiting for source queue');
});
