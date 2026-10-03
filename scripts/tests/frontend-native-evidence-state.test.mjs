import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

// Contract fixtures deliberately contain descriptors and previews, never Event
// authority. The production feature remains uninstalled until the native gate.
const source=readFileSync(new URL('../../frontend/case-evidence.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
// Test-only stand-in for the native decoder; production never parses its save string.
const wireDocument=ticket=>JSON.parse(ticket.request.documentJson);
const uuid=n=>`00000000-0000-4000-8000-${String(n).padStart(12,'0')}`;
const store=(revision='9007199254740993',epoch=uuid(2))=>({storeId:uuid(1),epoch,revision});
const owner=(caseId='case-a',analysisId=uuid(3))=>({storeId:uuid(1),caseId,analysisId});
const analysis=(caseId='case-a',analysisId=uuid(3))=>({schemaVersion:1,caseId,analysisId,
  configRevision:2,visibilityRevision:3,config:{derivedFields:[],references:[]},migrationDiagnostics:[]});
const reference=(id=10,count=2,authority=owner())=>({kind:'native_evidence',schemaVersion:1,
  owner:authority,containerId:uuid(id),manifestId:uuid(id+100),manifestSha256:'a'.repeat(64),memberCount:count});
const member=ref=>({containerId:ref.containerId,manifestId:ref.manifestId,occurrenceId:uuid(999)});
const container=(id=10,count=2,authority=owner())=>({kind:'native_evidence_container',reference:reference(id,count,authority),
  preservedCount:count,preview:null});
const preview=ref=>({kind:'evidence_preview',member:member(ref),cells:[{state:'preview',text:'preview-only-text',incomplete:true}]});
const summary=(count=2,authority=owner(),signature='opaque-evidence-signature-a')=>({state:'ready',owner:authority,evidenceSignature:signature,preservedCount:count});
const identity=(caseId='case-a',analysisId=uuid(3))=>({caseId,analysisId,configRevision:2,visibilityRevision:3});
const openOptions=extra=>({caseId:'case-a',analysisContext:identity(),stationId:null,...extra});
const ready=(token='native-publication-1',count=2,analyticalCount=1,signature='opaque-evidence-signature-a')=>({state:'ready',publication:{
  caseKey:'native-case-key-a',caseContentToken:token,caseEvidenceSignature:'opaque-evidence-signature-a',evidenceSignature:signature,preservedCount:count,analyticalCount,
}});
const document=()=>({evidenceViewVersion:1,store:store(),active:'case-a',diagnostics:[],caseEvidence:[summary()],cases:[{
  id:'case-a',name:'Preserved Case',analysisContext:analysis(),notes:'Original notes',
  rows:container(10,3),events:container(11,4),
  items:[
    {id:'duplicate-legacy-item',label:'First',stationId:'station-b',artifactId:'artifact-b',origin:'original-b',rows:container(12,2),events:container(13,1)},
    {id:'duplicate-legacy-item',label:'Second',stationId:'station-a',artifactId:'artifact-a',origin:'original-a',rows:container(14,0),events:container(15,5)},
  ],
  authored:{rows:['narrative rows remain authored JSON'],unknown:{keep:'exact metadata'}},
}]});
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const reached=async(predicate,message)=>{for(let i=0;i<50;i++){if(predicate())return;await Promise.resolve();}assert.fail(message);};

function moduleFixture(){
  const context=vm.createContext({window:{},JSON:{stringify:JSON.stringify,parse(){throw Error('Production must not parse back the save string');}},structuredClone,TextEncoder,TextDecoder,URL,crypto:{randomUUID:()=>uuid(9000)}});
  vm.runInContext(source,context,{filename:'case-evidence.js'});
  return context.window.CaseEvidence;
}

function clientFixture({enabled=true,loaded=document()}={}){
  const calls=[],handlers=new Map();let nextId=8000,current=structuredClone(loaded);
  const evidence=doc=>doc.cases.flatMap(item=>[item.rows,item.events,...(item.items||[]).flatMap(entry=>[entry.rows,entry.events])])
    .filter(Boolean).map(view=>view.reference);
  const receipt=request=>{const document=JSON.parse(request.documentJson);return {requestId:request.requestId,committedStore:store(String(BigInt(request.expectedStore.revision)+1n),request.expectedStore.epoch),
    currentStore:store(String(BigInt(request.expectedStore.revision)+1n),request.expectedStore.epoch),
    evidence:evidence(document),analysisContexts:document.cases.map(item=>item.analysisContext),caseEvidence:document.caseEvidence,replayed:false,reconcileRequired:false};};
  const invoke=async(command,args)=>{
    calls.push({command,args:plain(args)});
    if(handlers.has(command))return handlers.get(command)(args);
    if(command==='cases_load_view')return structuredClone(current);
    if(command==='cases_save_view')return receipt(args.request);
    throw Error(`Unexpected transport ${command}`);
  };
  const api=moduleFixture(),client=api.create({invoke,enabled,requestId:()=>uuid(nextId++)});
  return{api,client,calls,receipt,on:(command,handler)=>handlers.set(command,handler),setDocument:value=>{current=structuredClone(value);}};
}

test('native evidence controllers load before application bootstrap without a legacy Event adapter',()=>{
  const html=readFileSync(new URL('../../frontend/index.html',import.meta.url),'utf8');
  const app=html.indexOf('<script src="app.js"></script>');
  for(const name of ['case-evidence.js','case-evidence-session.js','case-evidence-analysis.js','case-evidence-preserved.js','case-evidence-items.js','case-evidence-actions.js','case-evidence-detail.js','case-evidence-timeline.js','case-evidence-display.js','case-evidence-transfer.js','case-evidence-recovery.js'])assert.ok(html.indexOf(`<script src="${name}"></script>`)>=0&&html.indexOf(`<script src="${name}"></script>`)<app);
  assert.equal(moduleFixture().active,true);
});

test('native evidence requires explicit synthetic opt-in before invoking any transport',async()=>{
  const f=clientFixture({enabled:false});
  assert.equal(f.client.status(),'inactive');
  await assert.rejects(async()=>f.client.load());
  await assert.rejects(async()=>f.client.prepareSave(document()));
  assert.equal(f.calls.length,0);
  let invoked=false;
  const implicit=f.api.create({invoke:async()=>{invoked=true;return document();}});
  await assert.rejects(async()=>implicit.load());
  assert.equal(invoked,false,'omitting enabled cannot activate the feature');
});

test('load preserves every tagged recognized container and original authored item order',async()=>{
  const f=clientFixture(),before=document();await f.client.load();
  assert.deepEqual(f.calls,[{command:'cases_load_view',args:{}}]);
  assert.equal(f.client.status(),'ready');
  assert.deepEqual(plain(f.client.document()),before);
  const locations=f.client.containers('case-a');
  assert.deepEqual(plain(locations.map(({caseId,itemIndex,field,view})=>({caseId,itemIndex,field,containerId:view.reference.containerId}))),[
    {caseId:'case-a',itemIndex:null,field:'rows',containerId:uuid(10)},
    {caseId:'case-a',itemIndex:null,field:'events',containerId:uuid(11)},
    {caseId:'case-a',itemIndex:0,field:'rows',containerId:uuid(12)},
    {caseId:'case-a',itemIndex:0,field:'events',containerId:uuid(13)},
    {caseId:'case-a',itemIndex:1,field:'rows',containerId:uuid(14)},
    {caseId:'case-a',itemIndex:1,field:'events',containerId:uuid(15)},
  ]);
  assert.ok(locations.every(location=>location.view.kind==='native_evidence_container'&&!Array.isArray(location.view)));
  assert.equal(f.client.preservedCount('case-a'),15,'management counts retain duplicate occurrences across every recognized container');
  assert.equal(f.client.document().caseEvidence[0].preservedCount,2,'native analytical scope starts with items[].rows only');
  assert.deepEqual(plain(f.client.document().cases[0].authored),before.cases[0].authored,'unrecognized nested rows remain authored metadata');
});

test('reordering items preserves container UUIDs, duplicate legacy item IDs and station provenance',async()=>{
  const f=clientFixture();await f.client.load();
  const reordered=plain(f.client.document());reordered.cases[0].items.reverse();
  const ticket=f.client.prepareSave(reordered),sent=wireDocument(ticket).cases[0];
  assert.deepEqual(plain(sent.items.map(item=>item.id)),['duplicate-legacy-item','duplicate-legacy-item']);
  assert.deepEqual(plain(sent.items.map(item=>item.rows.reference.containerId)),[uuid(14),uuid(12)]);
  assert.deepEqual(plain(sent.items.map(item=>[item.stationId,item.artifactId,item.origin])),[
    ['station-a','artifact-a','original-a'],['station-b','artifact-b','original-b'],
  ]);
  assert.equal(sent.rows.reference.containerId,uuid(10));assert.equal(sent.events.reference.containerId,uuid(11));
  assert.deepEqual(plain(sent.items.map(item=>item.events.reference.containerId)),[uuid(15),uuid(13)]);
});

test('empty authority is a tagged zero-count Case instead of an empty-array normalization',async()=>{
  const empty=document();empty.cases[0].rows=container(10,0);delete empty.cases[0].events;
  empty.cases[0].items=[{id:'empty-item',rows:container(12,0)}];
  empty.caseEvidence=[summary(0)];
  const f=clientFixture({loaded:empty});await f.client.load();
  assert.equal(f.client.preservedCount('case-a'),0);assert.equal(f.client.document().active,'case-a');
  assert.deepEqual(plain(f.client.document()),empty);
  const ticket=f.client.prepareSave();
  assert.equal(wireDocument(ticket).cases[0].items[0].rows.kind,'native_evidence_container');
  assert.equal(wireDocument(ticket).cases[0].items[0].rows.reference.memberCount,0);
  assert.equal(f.calls.length,1,'preserving an empty Case never asks for Dataset evidence');
});

test('store revisions stay exact canonical decimal u64 strings without Number coercion',async()=>{
  for(const revision of ['0','9007199254740993','18446744073709551615']){
    const doc=document();doc.store.revision=revision;
    const f=clientFixture({loaded:doc});await f.client.load();
    assert.equal(f.client.stamp().revision,revision);
    assert.equal(f.client.prepareSave().request.expectedStore.revision,revision);
  }
  for(const revision of [0,9007199254740992,'','00','01','+1','-1','1.0','1e3','18446744073709551616']){
    const doc=document();doc.store.revision=revision;
    const f=clientFixture({loaded:doc});await assert.rejects(f.client.load(),undefined,`revision ${String(revision)} must not be rounded or normalized`);
  }
});

test('loaded reference ownership must match store, Case and non-recycled analysis identity',async()=>{
  for(const field of ['storeId','caseId','analysisId']){
    const doc=document();doc.cases[0].items[0].rows.reference.owner[field]=field==='caseId'?'case-b':uuid(700);
    const f=clientFixture({loaded:doc});await assert.rejects(f.client.load(),undefined,`mismatched ${field}`);
  }
  const missing=document();delete missing.store.epoch;
  await assert.rejects(clientFixture({loaded:missing}).client.load());
  const recycled=document();recycled.cases[0].analysisContext.analysisId=uuid(701);
  await assert.rejects(clientFixture({loaded:recycled}).client.load());
});

test('copying one native container into two locations is rejected even when legacy item IDs are duplicated',async()=>{
  const duplicate=document();duplicate.cases[0].items[1].rows=structuredClone(duplicate.cases[0].items[0].rows);
  await assert.rejects(clientFixture({loaded:duplicate}).client.load());
  const f=clientFixture();await f.client.load();
  const draft=plain(f.client.document());draft.cases[0].events=structuredClone(draft.cases[0].rows);
  assert.throws(()=>f.client.prepareSave(draft));
  assert.equal(f.calls.length,1);
});

test('malformed and inline recognized containers fail visibly instead of becoming []',async()=>{
  for(const replacement of [[],[preview(reference())],[{id:0,raw:'not native authority'}],{kind:'native_evidence',preservedCount:0}]){
    const doc=document();doc.cases[0].items[0].rows=replacement;
    await assert.rejects(clientFixture({loaded:doc}).client.load());
  }
  const f=clientFixture();await f.client.load();const original=plain(f.client.document()),invalid=plain(original);
  invalid.cases[0].rows=[];f.setDocument(invalid);
  await assert.rejects(f.client.load());
  assert.deepEqual(plain(f.client.document()),original,'an invalid response cannot erase already loaded authority');
});

test('non-record null metadata is preserved without inventing an empty evidence container',async()=>{
  const doc=document();doc.cases[0].rows=null;
  const f=clientFixture({loaded:doc});await f.client.load();
  assert.equal(f.client.document().cases[0].rows,null);
  assert.equal(wireDocument(f.client.prepareSave()).cases[0].rows,null);
  assert.equal(f.client.containers('case-a').length,5);
  assert.equal(f.client.preservedCount('case-a'),12);
});

test('save strips preview pages while preserving references, counts and authored metadata',async()=>{
  const doc=document(),view=doc.cases[0].items[0].rows;
  view.preview={kind:'evidence_preview_page',columns:['message'],rows:[preview(view.reference)],total:view.preservedCount,nextCursor:'opaque-next-preview'};
  const f=clientFixture({loaded:doc});await f.client.load();
  const ticket=f.client.prepareSave(),saved=wireDocument(ticket).cases[0].items[0].rows;
  assert.equal(saved.preview,null);assert.deepEqual(plain(saved.reference),view.reference);assert.equal(saved.preservedCount,2);
  assert.equal(f.client.document().cases[0].items[0].rows.preview.rows[0].kind,'evidence_preview','preparing a save does not destroy the management preview');
  assert.doesNotMatch(JSON.stringify(ticket.request),/evidence_preview|opaque-next-preview|preview-only-text/);
  await f.client.save(ticket);
  assert.deepEqual(f.calls.at(-1),{command:'cases_save_view',args:{request:plain(ticket.request)}});
});

test('preview rows and projected Event-shaped cells cannot become authoritative save records',async()=>{
  const f=clientFixture();await f.client.load();
  for(const bad of [[preview(reference())],preview(reference()),[{id:0,raw:'',fields:{message:'bounded preview'}}]]){
    const draft=plain(f.client.document());draft.cases[0].items[0].rows=bad;
    assert.throws(()=>f.client.prepareSave(draft));
  }
  assert.equal(f.calls.filter(call=>call.command==='cases_save_view').length,0);
});

test('visible manifest member-count limit is independent from native encoded-byte admission',async()=>{
  const boundary=document();boundary.cases[0].items[0].rows=container(12,100_000);
  boundary.caseEvidence=[summary(100_000)];
  const f=clientFixture({loaded:boundary});await f.client.load();
  assert.equal(f.client.containers('case-a')[2].view.reference.memberCount,100_000);
  const original=plain(f.client.document()),ticket=f.client.prepareSave();
  const nativeError=Object.assign(Error('EVIDENCE_MANIFEST_BYTES: encoded manifest exceeds 8388608 bytes'),{code:'EVIDENCE_MANIFEST_BYTES'});
  f.on('cases_save_view',()=>{throw nativeError;});
  await assert.rejects(f.client.save(ticket),error=>error.code==='EVIDENCE_MANIFEST_BYTES'||String(error).includes('EVIDENCE_MANIFEST_BYTES'));
  assert.equal(f.calls.filter(call=>call.command==='cases_save_view').length,1,'byte admission is delegated to native, without a guessed member-count estimate');
  assert.deepEqual(plain(f.client.document()),original);
  assert.ok(f.calls.every(call=>['cases_load_view','cases_save_view'].includes(call.command)));
  for(const count of [-1,1.5,100_001,Number.MAX_SAFE_INTEGER]){
    const invalid=document();invalid.cases[0].items[0].rows=container(12,count);
    await assert.rejects(clientFixture({loaded:invalid}).client.load());
  }
  const mismatch=document();mismatch.cases[0].items[0].rows.preservedCount++;
  await assert.rejects(clientFixture({loaded:mismatch}).client.load());
});

test('lost-ack save replay retains original CAS/body and never resurrects a later removed item',async()=>{
  const f=clientFixture();await f.client.load();
  const draft=plain(f.client.document());draft.cases[0].notes='The first save';
  const ticket=f.client.prepareSave(draft),originalRequest=plain(ticket.request),committed=f.receipt(ticket.request);
  f.on('cases_save_view',()=>{throw Error('LOST_ACK: committed response unavailable');});
  await assert.rejects(f.client.save(ticket),/LOST_ACK/);
  draft.cases[0].notes='Caller edited its draft after the failed response';
  const later=document();later.store.revision='9007199254740995';later.cases[0].notes='A later successful edit';later.cases[0].items.splice(0,1);
  later.caseEvidence=[summary(0,owner(),'opaque-evidence-signature-after-removal')];
  f.setDocument(later);await f.client.load();
  f.on('cases_save_view',()=>({...committed,currentStore:later.store,replayed:true,reconcileRequired:true}));
  await f.client.save(ticket);
  const saves=f.calls.filter(call=>call.command==='cases_save_view');
  assert.equal(saves.length,2);assert.deepEqual(saves[0].args.request,originalRequest);assert.deepEqual(saves[1].args.request,originalRequest);
  assert.equal(saves[1].args.request.expectedStore.revision,'9007199254740993','retry may not upgrade its original CAS');
  assert.deepEqual(plain(f.client.document()),later,'replayed receipt never reinstalls old metadata or membership');
  assert.deepEqual(plain(f.client.stamp()),later.store);assert.equal(f.client.status(),'reconcile_required');
});

test('delayed older successful save receipts never roll back a newer loaded revision or edits',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();
  const ticket=f.client.prepareSave(),receipt=f.receipt(ticket.request);
  f.on('cases_save_view',()=>gate.promise);const saving=f.client.save(ticket);
  await reached(()=>f.calls.some(call=>call.command==='cases_save_view'),'save was dispatched');
  const later=document();later.store.revision='9007199254740998';later.cases[0].notes='Loaded while prior save was pending';
  f.setDocument(later);await f.client.load();gate.resolve(receipt);await saving;
  assert.deepEqual(plain(f.client.stamp()),later.store);assert.deepEqual(plain(f.client.document()),later);
});

test('repeated save clicks share one in-flight immutable request instead of appending again',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();
  const ticket=f.client.prepareSave(),original=plain(ticket.request),receipt=f.receipt(ticket.request);
  f.on('cases_save_view',()=>gate.promise);
  const first=f.client.save(ticket),second=f.client.save(ticket);
  await reached(()=>f.calls.some(call=>call.command==='cases_save_view'),'one immutable save was dispatched');
  assert.equal(f.calls.filter(call=>call.command==='cases_save_view').length,1);
  gate.resolve(receipt);await Promise.all([first,second]);
  assert.deepEqual(f.calls.find(call=>call.command==='cases_save_view').args.request,original);
  assert.equal(f.client.stamp().revision,receipt.currentStore.revision);
});

test('restore epoch or recycled analysis identity prevents stale ticket reuse',async()=>{
  for(const change of ['epoch','analysis']){
    const f=clientFixture();await f.client.load();const ticket=f.client.prepareSave(),replacement=document();
    if(change==='epoch')replacement.store.epoch=uuid(702);
    else{
      replacement.cases[0].analysisContext.analysisId=uuid(703);
      replacement.caseEvidence[0].owner.analysisId=uuid(703);
      for(const view of [replacement.cases[0].rows,replacement.cases[0].events,...replacement.cases[0].items.flatMap(item=>[item.rows,item.events])])view.reference.owner.analysisId=uuid(703);
    }
    f.setDocument(replacement);await f.client.load();await assert.rejects(f.client.save(ticket));
    assert.equal(f.calls.filter(call=>call.command==='cases_save_view').length,0,`${change} change blocks the old owner before native dispatch`);
    assert.deepEqual(plain(f.client.document()),replacement);
  }
});

test('foreign or malformed analysis identities in save receipts cannot replace committed state',async()=>{
  const mutations=[
    context=>{context.caseId='case-b';},
    context=>{context.analysisId=uuid(704);},
    context=>{delete context.caseId;},
    context=>{context.analysisId='';},
    context=>{context.configRevision=-1;},
    context=>{context.configRevision=1;},
    context=>{context.visibilityRevision=2;},
    context=>{context.configRevision='2';},
    context=>{context.visibilityRevision=Number.MAX_SAFE_INTEGER+1;},
  ];
  for(const mutate of mutations){
    const f=clientFixture();await f.client.load();const before=plain(f.client.document()),ticket=f.client.prepareSave(),bad=f.receipt(ticket.request);
    mutate(bad.analysisContexts[0]);f.on('cases_save_view',()=>bad);
    await assert.rejects(f.client.save(ticket));
    assert.deepEqual(plain(f.client.document()),before);assert.deepEqual(plain(f.client.stamp()),before.store);
  }
});

test('returned documents are detached drafts and cannot mutate committed authority or an in-flight save',async()=>{
  const f=clientFixture(),gate=deferred(),loaded=await f.client.load();
  loaded.cases[0].notes='Mutated load return';
  assert.equal(f.client.document().cases[0].notes,'Original notes','load() also returns a detached document');
  const draft=f.client.document(),original=plain(draft);
  draft.cases[0].notes='Draft prepared for saving';
  const ticket=f.client.prepareSave(draft),request=plain(ticket.request),receipt=f.receipt(ticket.request);
  f.on('cases_save_view',()=>gate.promise);const saving=f.client.save(ticket);
  await reached(()=>f.calls.some(call=>call.command==='cases_save_view'),'immutable save is in flight');
  draft.cases[0].notes='Later unsaved narrative';draft.cases[0].items.splice(0,1);
  draft.store.revision='1';draft.cases[0].rows.reference.owner.analysisId=uuid(705);
  assert.deepEqual(plain(f.client.document()),original,'caller edits belong to its draft, never the committed view');
  gate.resolve(receipt);await saving;
  assert.deepEqual(f.calls.find(call=>call.command==='cases_save_view').args.request,request);
  assert.equal(f.client.document().cases[0].notes,'Draft prepared for saving');
  assert.equal(f.client.document().cases[0].items.length,2);
  assert.equal(draft.cases[0].notes,'Later unsaved narrative','receipt adoption never mutates the caller’s newer unsaved draft');
  assert.equal(draft.cases[0].items.length,1);
});

test('opening an authoritative empty analytical Case returns an explicit zero publication without promoting other containers',async()=>{
  const doc=document();doc.cases[0].items[0].rows=container(12,0);doc.caseEvidence=[summary(0)];
  const f=clientFixture({loaded:doc});await f.client.load();f.on('case_evidence_open',()=>ready('empty-publication',0,0));
  const result=await f.client.openCase(openOptions());
  assert.deepEqual(plain(result),ready('empty-publication',0,0));assert.equal(f.client.preservedCount('case-a'),13);
  assert.deepEqual(f.calls.at(-1),{command:'case_evidence_open',args:{request:{
    store:{storeId:uuid(1),epoch:uuid(2)},analysisContext:identity(),stationId:null,evidenceSignature:doc.caseEvidence[0].evidenceSignature,
  }}});
  assert.ok(f.calls.every(call=>['cases_load_view','case_evidence_open'].includes(call.command)));
  assert.equal(Object.hasOwn(f.calls.at(-1).args.request,'caseEvents'),false);
  assert.equal(Object.hasOwn(f.calls.at(-1).args.request.store,'revision'),false,'note-only body revision is not analytical authority');
});

test('low-profile unavailable retains preserved counts and retries natively without Dataset or empty-publication fallback',async()=>{
  const doc=document();doc.cases[0].items[0].rows=container(12,10_000);doc.caseEvidence=[summary(10_000)];
  const f=clientFixture({loaded:doc});await f.client.load();const before=plain(f.client.document());
  const unavailable={state:'unavailable',code:'CASE_MATERIALIZATION_LIMIT',preservedCount:10_000,materializationLimit:16*1024*1024};
  f.on('case_evidence_open',()=>unavailable);
  for(let i=0;i<2;i++){
    const result=await f.client.openCase(openOptions());assert.deepEqual(plain(result),unavailable);
    assert.equal(Object.hasOwn(result,'publication'),false);
  }
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,2,'unavailable is never cached as an authoritative empty result');
  assert.equal(f.client.preservedCount('case-a'),10_013);assert.deepEqual(plain(f.client.document()),before);
  assert.ok(f.calls.every(call=>['cases_load_view','case_evidence_open'].includes(call.command)));
  f.on('case_evidence_open',()=>ready('smaller-station-publication',1,1));
  const scoped=await f.client.openCase(openOptions({stationId:'station-a'}));
  assert.equal(scoped.state,'ready');assert.equal(f.calls.at(-1).args.request.stationId,'station-a');
  assert.equal(f.client.preservedCount('case-a'),10_013,'successful smaller analysis cannot rewrite the preserved management total');
});

test('a missing native Case signature is explicit unavailability and never triggers an alternate data source',async()=>{
  const doc=document();doc.caseEvidence=[];
  const f=clientFixture({loaded:doc});await f.client.load();
  await assert.rejects(f.client.openCase(openOptions()),/EVIDENCE_UNAVAILABLE/);
  assert.equal(f.calls.length,1);assert.equal(f.client.preservedCount('case-a'),15);
  assert.deepEqual(plain(f.client.document()),doc);
});

test('native publication cache is independent from note-only body revision while station scopes stay separate',async()=>{
  const f=clientFixture();await f.client.load();f.on('case_evidence_open',()=>ready());
  assert.deepEqual(plain(await f.client.openCase(openOptions())),ready());
  const draft=f.client.document();draft.cases[0].notes='Only narrative changed';await f.client.save(f.client.prepareSave(draft));
  assert.equal(f.client.stamp().revision,'9007199254740994');
  await f.client.openCase(openOptions());
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,1,'a note save leaves the unchanged publication reusable');
  await f.client.openCase(openOptions({stationId:'station-a'}));
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,2);
  const returned=await f.client.openCase(openOptions());returned.publication.caseContentToken='caller-tampered-token';
  assert.equal((await f.client.openCase(openOptions())).publication.caseContentToken,'native-publication-1','cached receipts are detached values');
});

test('simultaneous opens share native work and a later cache-miss retry reuses the already refreshed token',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();f.on('case_evidence_open',()=>gate.promise);
  const first=f.client.openCase(openOptions()),second=f.client.openCase(openOptions());
  await reached(()=>f.calls.filter(call=>call.command==='case_evidence_open').length===1,'one native open is pending');
  gate.resolve(ready());await Promise.all([first,second]);
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,1);
  f.on('case_evidence_open',()=>ready('native-publication-2'));
  const refresh=await f.client.openCase(openOptions({force:true,previousToken:'native-publication-1'}));
  const lateRetry=await f.client.openCase(openOptions({force:true,previousToken:'native-publication-1'}));
  assert.equal(refresh.publication.caseContentToken,'native-publication-2');assert.deepEqual(plain(lateRetry),plain(refresh));
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,2,'the second retry must not invalidate another caller’s fresh publication');
});

test('frontend publication descriptors retain no more than three native scopes',async()=>{
  const f=clientFixture();await f.client.load();
  f.on('case_evidence_open',()=>ready(`publication-${f.calls.filter(call=>call.command==='case_evidence_open').length}`));
  for(const stationId of ['station-a','station-b','station-c','station-d'])await f.client.openCase(openOptions({stationId}));
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,4);
  const reopened=await f.client.openCase(openOptions({stationId:'station-a'}));
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,5,'an evicted scope is reopened natively, never reconstructed from preview rows');
  assert.equal(reopened.publication.caseContentToken,'publication-5');
});

test('late native open responses reject changed store, analysis owner, signature and caller context',async()=>{
  for(const change of ['epoch','analysis','signature','caller']){
    const f=clientFixture(),gate=deferred();let current=true;await f.client.load();f.on('case_evidence_open',()=>gate.promise);
    const pending=f.client.openCase(openOptions({isCurrent:()=>current}));
    const rejected=assert.rejects(pending,/EVIDENCE_VIEW_CHANGED/);
    await reached(()=>f.calls.some(call=>call.command==='case_evidence_open'),'native open is in flight');
    if(change==='caller')current=false;
    else{
      const replacement=document();
      if(change==='epoch')replacement.store.epoch=uuid(706);
      if(change==='signature')replacement.caseEvidence[0].evidenceSignature='changed-evidence-signature';
      if(change==='analysis'){
        replacement.cases[0].analysisContext.analysisId=uuid(707);replacement.caseEvidence[0].owner.analysisId=uuid(707);
        for(const view of [replacement.cases[0].rows,replacement.cases[0].events,...replacement.cases[0].items.flatMap(item=>[item.rows,item.events])])view.reference.owner.analysisId=uuid(707);
      }
      f.setDocument(replacement);await f.client.load();
    }
    gate.resolve(ready());await rejected;
    assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,1);
  }
});

test('native station-subset signatures may differ from the complete Case signature but must remain valid',async()=>{
  const f=clientFixture();await f.client.load();
  const subset=ready('native-station-publication',1,1,'native-station-subset-signature');
  f.on('case_evidence_open',()=>subset);
  const result=await f.client.openCase(openOptions({stationId:'station-a'}));
  assert.deepEqual(plain(result),subset);
  assert.equal(f.calls.at(-1).args.request.evidenceSignature,'opaque-evidence-signature-a','request admission retains the complete Case signature');
  assert.deepEqual(plain(await f.client.openCase(openOptions({stationId:'station-a'}))),subset);
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_open').length,1,'the client preserves the native subset signature without recomputing it');
  for(const signature of ['',null,123]){
    const invalid=clientFixture();await invalid.client.load();invalid.on('case_evidence_open',()=>ready('invalid-signature-token',2,1,signature));
    await assert.rejects(invalid.client.openCase(openOptions()));
  }
});


test('unavailable Case stubs retain native readiness and never masquerade as an empty analytical Case',async()=>{
  const doc=document(),diagnostic={caseId:'case-a',owner:owner(),code:'EVIDENCE_METADATA_LIMIT',message:'Preserved original unavailable for editing',preservedCount:42,readiness:'preserved_only'};
  doc.cases=[{kind:'preserved_case_unavailable',id:'case-a',code:diagnostic.code}];doc.diagnostics=[diagnostic];doc.caseEvidence=[{state:'unavailable',...diagnostic}];
  const f=clientFixture({loaded:doc});await f.client.load();
  assert.equal(f.client.preservedCount('case-a'),42);assert.equal(f.client.caseState('case-a').state,'unavailable');
  assert.deepEqual(plain(wireDocument(f.client.prepareSave()).cases),doc.cases);
  await assert.rejects(f.client.openCase(openOptions()),/EVIDENCE_UNAVAILABLE/);assert.equal(f.calls.length,1);
  for(const change of ['metadata','ordinary','fabricated']){
    const draft=f.client.document();
    if(change==='metadata')draft.cases[0].notes='Cannot edit an unavailable stub';
    if(change==='ordinary')draft.cases[0]={id:'case-a',items:[]};
    if(change==='fabricated'){draft.cases[0].code='OTHER';draft.diagnostics[0].code='OTHER';draft.caseEvidence[0].code='OTHER';}
    assert.throws(()=>f.client.prepareSave(draft));
  }
  doc.diagnostics[0].preservedCount=null;doc.caseEvidence[0].preservedCount=null;f.setDocument(doc);await f.client.load();
  assert.equal(f.client.preservedCount('case-a'),null,'unknown preserved size cannot become zero');
});

test('save receipts adopt the committed native Case signature and reject a mismatched complete-signature publication',async()=>{
  const f=clientFixture();await f.client.load();const ticket=f.client.prepareSave(),result=f.receipt(ticket.request);
  result.caseEvidence=[summary(2,owner(),'new-native-signature')];f.on('cases_save_view',()=>result);await f.client.save(ticket);
  assert.equal(f.client.caseState('case-a').evidenceSignature,'new-native-signature');
  f.on('case_evidence_open',()=>ready());await assert.rejects(f.client.openCase(openOptions()),/Publicação/);
  f.on('case_evidence_open',()=>{const response=ready();response.publication.caseEvidenceSignature='new-native-signature';return response;});
  assert.equal((await f.client.openCase(openOptions())).state,'ready');
});

test('tagged preview pages keep columns and occurrence handles separate from Event values',async()=>{
  const doc=document(),view=doc.cases[0].rows;
  view.preview={kind:'evidence_preview_page',columns:['message'],rows:[preview(view.reference)],total:view.preservedCount,nextCursor:null};
  const f=clientFixture({loaded:doc});await f.client.load();
  assert.equal(f.client.document().cases[0].rows.kind,'native_evidence_container');
  for(const mutate of [page=>delete page.kind,page=>page.columns=[],page=>page.columns=['a','a'],page=>page.rows[0].member.manifestId=uuid(888)]){
    const changed=structuredClone(doc);mutate(changed.cases[0].rows.preview);await assert.rejects(clientFixture({loaded:changed}).client.load());
  }
});

const selectionReceipt=()=>({analysisContext:identity(),sourceGeneration:7,caseKey:null,caseContentToken:null,catalogSignature:'b'.repeat(64),catalogEpoch:4});
const pendingReference=(request,{count=1,replace=null}={})=>({kind:'pending_native_evidence',schemaVersion:1,purpose:replace?{kind:'replace_container',baseManifestId:replace.manifestId,baseManifestSha256:replace.manifestSha256}:{kind:'new_container'},
  token:'sealed-native-token',requestId:request.requestId,owner:request.target||request.reference.owner,containerId:replace?.containerId||uuid(80),manifestId:uuid(180),manifestSha256:'c'.repeat(64),memberCount:count,bytes:99,expiresAt:'2026-10-01T16:00:00Z'});

test('capture sends only immutable native receipt and row handles and coalesces repeated preparation',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();
  const input={target:owner(),source:selectionReceipt(),rows:[{id:0,eventRef:'source:0'}]},ticket=f.client.prepareCapture(input),original=plain(ticket.request);
  f.on('case_evidence_prepare',()=>gate.promise);const first=f.client.prepare(ticket),second=f.client.prepare(ticket);
  input.rows[0].eventRef='edited caller';ticket.request.rows[0].eventRef='edited public ticket';
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_prepare').length,1);
  gate.resolve(pendingReference(original));const [a,b]=await Promise.all([first,second]);
  assert.deepEqual(plain(a),plain(b));assert.deepEqual(f.calls.at(-1).args,{request:original});
  assert.equal(a.kind,'pending_native_evidence');assert.equal(f.client.document().cases[0].items.length,2,'prepared capture is not a durable or implicit append');
  a.token='caller mutation';assert.equal((await f.client.prepare(ticket)).token,'sealed-native-token');
  assert.equal(f.calls.filter(call=>call.command==='case_evidence_prepare').length,1);
});

test('capture rejects Events, absent native proof, excessive handles and foreign target owners before IPC',async()=>{
  const f=clientFixture();await f.client.load();
  const make=()=>({target:owner(),source:selectionReceipt(),rows:[{id:0,eventRef:'source:0'}]});
  for(const mutate of [v=>v.rows[0].fields={number:1},v=>v.rows[0].event_ref='wrong-shape',v=>delete v.source.catalogSignature,
    v=>v.source.sourceGeneration=null,v=>v.target.analysisId=uuid(888),v=>v.rows.push(v.rows[0]),v=>v.rows=Array.from({length:10001},(_,id)=>({id,eventRef:String(id)}))]){
    const value=make();mutate(value);assert.throws(()=>f.client.prepareCapture(value));
  }
  assert.equal(f.calls.length,1);
});

test('lost prepare acknowledgement retries the same request and stale owner replies cannot be adopted',async()=>{
  const f=clientFixture();await f.client.load();const ticket=f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows:[{id:0,eventRef:'source:0'}]});
  f.on('case_evidence_prepare',()=>{throw Error('lost response');});await assert.rejects(f.client.prepare(ticket),/lost response/);
  f.on('case_evidence_prepare',({request})=>pendingReference(request));await f.client.prepare(ticket);
  const calls=f.calls.filter(call=>call.command==='case_evidence_prepare');assert.deepEqual(calls[0].args,calls[1].args);
  const gate=deferred(),next=f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows:[{id:1,eventRef:'source:1'}]});
  f.on('case_evidence_prepare',()=>gate.promise);const preparing=f.client.prepare(next);
  const replacement=document();replacement.store.epoch=uuid(999);f.setDocument(replacement);await f.client.load();gate.resolve(pendingReference(next.request));
  await assert.rejects(preparing,/EVIDENCE_VIEW_CHANGED/);
});

test('membership removes exact occurrence handles, retaining its preceding ref for unsaved undo',async()=>{
  const f=clientFixture();await f.client.load();const ref=reference(12,2),chosen=member(ref),ticket=f.client.prepareMembership({reference:ref,action:{kind:'remove',members:[chosen]}});
  f.on('case_evidence_prepare_membership',({request})=>({reference:pendingReference(request,{count:1,replace:ref}),undo:{...ref},changedCount:1,remainingCount:1}));
  const result=await f.client.prepare(ticket);
  assert.deepEqual(f.calls.at(-1).args.request.action,{kind:'remove',members:[chosen]});
  assert.equal(result.reference.containerId,ref.containerId);assert.equal(result.reference.purpose.baseManifestId,ref.manifestId);
  assert.deepEqual(plain(result.undo),ref);assert.equal(result.remainingCount,1);
  assert.equal(f.client.document().cases[0].items[0].rows.reference.memberCount,2,'prepared membership waits for the authored metadata save');
  const reordered={};for(const [key,value]of Object.entries(ref).reverse())reordered[key]=value;
  const again=f.client.prepareMembership({reference:ref,action:{kind:'remove',members:[chosen]}});
  f.on('case_evidence_prepare_membership',({request})=>({reference:pendingReference(request,{count:1,replace:ref}),undo:reordered,changedCount:1,remainingCount:1}));
  await f.client.prepare(again);
});

test('committed undo stages restore against the current manifest rather than swapping an old durable ref',async()=>{
  const f=clientFixture();await f.client.load();const old=reference(12,2),current={...old,manifestId:uuid(778),manifestSha256:'d'.repeat(64),memberCount:1};
  const ticket=f.client.prepareMembership({reference:current,action:{kind:'restore',target:old}});
  f.on('case_evidence_prepare_membership',({request})=>({reference:pendingReference(request,{count:2,replace:current}),undo:current,changedCount:1,remainingCount:2}));
  const result=await f.client.prepare(ticket);
  assert.equal(f.calls.at(-1).args.request.reference.manifestId,current.manifestId);assert.deepEqual(f.calls.at(-1).args.request.action,{kind:'restore',target:old});
  assert.equal(result.reference.purpose.baseManifestId,current.manifestId);assert.equal(result.reference.memberCount,2);
});

test('malformed membership replies never replace a draft or silently drop unmatched occurrences',async()=>{
  const f=clientFixture();await f.client.load();const ref=reference(12,2);
  for(const mutate of [r=>r.changedCount=0,r=>r.remainingCount=0,r=>r.reference.containerId=uuid(888),r=>r.reference.purpose.baseManifestId='wrong',r=>r.undo.manifestId='wrong']){
    const ticket=f.client.prepareMembership({reference:ref,action:{kind:'remove',members:[member(ref)]}});
    f.on('case_evidence_prepare_membership',({request})=>{const result={reference:pendingReference(request,{count:1,replace:ref}),undo:structuredClone(ref),changedCount:1,remainingCount:1};mutate(result);return result;});
    await assert.rejects(f.client.prepare(ticket));
  }
  assert.equal(f.client.document().cases[0].items[0].rows.reference.memberCount,2);
});

test('management preview is bounded and tied to the exact manifest and requested column bytes',async()=>{
  const f=clientFixture();await f.client.load();const ref=reference(12,2);
  f.on('case_evidence_preview',({request})=>({kind:'evidence_preview_page',columns:request.columns,rows:[preview(ref)],total:2,nextCursor:'next'}));
  const result=await f.client.preview(ref,{limit:1,columns:[' exact child '],cursor:'prior'});
  assert.equal(result.rows[0].kind,'evidence_preview');assert.deepEqual(f.calls.at(-1).args.request,{store:{storeId:uuid(1),epoch:uuid(2)},reference:ref,cursor:'prior',limit:1,columns:[' exact child ']});
  const calls=f.calls.length;await assert.rejects(f.client.preview(ref,{limit:17,columns:[]}));assert.equal(f.calls.length,calls);
  f.on('case_evidence_preview',()=>({kind:'evidence_preview_page',columns:['wrong'],rows:[preview(ref)],total:2,nextCursor:null}));
  await assert.rejects(f.client.preview(ref,{columns:[' exact child ']}));
});

test('capture and membership request bytes are bounded independently from handle counts',async()=>{
  const f=clientFixture();await f.client.load();
  const rows=Array.from({length:1000},(_,id)=>({id,eventRef:String(id)+':'+('a'.repeat(3000))}));
  assert.throws(()=>f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows}),/2 MiB/);
  const ref=reference(12,1000),members=Array.from({length:1000},(_,id)=>({...member(ref),occurrenceId:String(id)+':'+('a'.repeat(3000))}));
  assert.throws(()=>f.client.prepareMembership({reference:ref,action:{kind:'remove',members}}),/2 MiB/);
  assert.equal(f.calls.length,1);
});

test('a replaced management preview suppresses its late response without changing preserved state',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();const ref=reference(12,2);let current=true;
  f.on('case_evidence_preview',()=>gate.promise);
  const pending=f.client.preview(ref,{columns:['message'],isCurrent:()=>current});current=false;
  gate.resolve({kind:'evidence_preview_page',columns:['message'],rows:[preview(ref)],total:2,nextCursor:null});
  await assert.rejects(pending,/EVIDENCE_VIEW_CHANGED/);assert.equal(f.client.preservedCount('case-a'),15);
});


test('restore cannot accept a receipt with a different preserved count than its exact target',async()=>{
  const f=clientFixture();await f.client.load();const target=reference(12,2),current={...target,manifestId:uuid(778),memberCount:1};
  for(const count of [0,1,3]){
    const ticket=f.client.prepareMembership({reference:current,action:{kind:'restore',target}});
    f.on('case_evidence_prepare_membership',({request})=>({reference:pendingReference(request,{count,replace:current}),undo:current,changedCount:1,remainingCount:count}));
    await assert.rejects(f.client.prepare(ticket),/restauração/);
  }
});

test('capture handle identity is independent of JSON property insertion order',async()=>{
  const f=clientFixture();await f.client.load();
  assert.throws(()=>f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows:[{id:0,eventRef:'same'},{eventRef:'same',id:0}]}),/repete/);
  assert.equal(f.calls.length,1);
});


test('management preview remains bound to captured owner, manifest and columns across the await',async()=>{
  for(const change of ['owner','manifest','columns']){
    const f=clientFixture(),gate=deferred();await f.client.load();const ref=reference(12,2),columns=['message'],captured=structuredClone(ref);
    f.on('case_evidence_preview',()=>gate.promise);const pending=f.client.preview(ref,{columns});
    if(change==='owner')ref.owner.analysisId=uuid(777);
    if(change==='manifest')ref.manifestId=uuid(778);
    if(change==='columns')columns[0]='different';
    gate.resolve({kind:'evidence_preview_page',columns:['message'],rows:[preview(captured)],total:2,nextCursor:null});
    await assert.rejects(pending,/EVIDENCE_VIEW_CHANGED/);
    assert.deepEqual(f.calls.at(-1).args.request.reference,captured);assert.deepEqual(f.calls.at(-1).args.request.columns,['message']);
  }
});


test('a malformed committed reference forces reconciliation even when the receipt header is valid',async()=>{
  const f=clientFixture();await f.client.load();const before=plain(f.client.document()),ticket=f.client.prepareSave(),result=f.receipt(ticket.request);
  result.evidence[0]={...result.evidence[0],owner:owner('foreign-case')};
  f.on('cases_save_view',()=>result);await assert.rejects(f.client.save(ticket));
  assert.equal(f.client.status(),'reconcile_required');assert.deepEqual(plain(f.client.document()),before);
});

test('missing confirmation of a pending reference requires reconciliation rather than retaining a prepared ref as saved',async()=>{
  const f=clientFixture();await f.client.load();const draft=f.client.document(),preparation=f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows:[{id:0,eventRef:'source:0'}]});
  f.on('case_evidence_prepare',({request})=>pendingReference(request));const prepared=await f.client.prepare(preparation);
  draft.cases[0].items.push({id:'prepared',rows:{kind:'native_evidence_container',reference:prepared,preservedCount:1,preview:null}});
  const ticket=f.client.prepareSave(draft),result=f.receipt(ticket.request);result.evidence=result.evidence.filter(ref=>ref.kind==='native_evidence');
  f.on('cases_save_view',()=>result);await assert.rejects(f.client.save(ticket),/Preparação/);
  assert.equal(f.client.status(),'reconcile_required');assert.equal(f.client.document().cases[0].items.length,2);
});


test('source receipts come from native admission and preserve the captured Dataset or Case identity',async()=>{
  for(const scope of ['dataset','case']){
    const f=clientFixture();await f.client.load();const request={analysisContext:identity(),sourceGeneration:scope==='dataset'?7:null,caseKey:scope==='case'?'native-key':null,caseContentToken:scope==='case'?'native-token':null};
    const receipt={...request,catalogSignature:'b'.repeat(64),catalogEpoch:4};f.on('case_evidence_source_receipt',()=>receipt);
    assert.deepEqual(plain(await f.client.captureSource(request)),receipt);
    assert.deepEqual(f.calls.at(-1),{command:'case_evidence_source_receipt',args:{request}});
    assert.equal(Object.hasOwn(f.calls.at(-1).args.request,'catalogSignature'),false,'the frontend never manufactures catalog proof');
  }
});

test('late or mismatched source receipt cannot relabel an old row selection',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();let current=true;
  const request={analysisContext:identity(),sourceGeneration:7,caseKey:null,caseContentToken:null};
  f.on('case_evidence_source_receipt',()=>gate.promise);const pending=f.client.captureSource(request,{isCurrent:()=>current});
  current=false;request.sourceGeneration=8;gate.resolve(selectionReceipt());await assert.rejects(pending,/EVIDENCE_VIEW_CHANGED/);
  assert.equal(f.calls.at(-1).args.request.sourceGeneration,7);
  f.on('case_evidence_source_receipt',()=>selectionReceipt());await assert.rejects(f.client.captureSource(request),/EVIDENCE_VIEW_CHANGED/);
});

test('fractional metadata crosses the save boundary only as the immutable JSON string and survives a lost-ack retry',async()=>{
  const f=clientFixture();await f.client.load();const draft=f.client.document();
  draft.cases[0].authored.fraction=1.2345678901234567;draft.cases[0].authored.exponent=1.2345678901234567e-80;
  const ticket=f.client.prepareSave(draft),original=plain(ticket.request),reply=f.receipt(ticket.request);
  assert.deepEqual(Object.keys(original).sort(),['documentJson','expectedStore','requestId']);
  assert.equal(typeof original.documentJson,'string');assert.match(original.documentJson,/"fraction":1\.2345678901234567/);
  assert.match(original.documentJson,/"exponent":1\.2345678901234567e-80/);
  assert.equal(Object.hasOwn(original,'document'),false);assert.doesNotMatch(original.documentJson,/evidence_preview/);
  f.on('cases_save_view',()=>{throw Error('lost acknowledgement');});await assert.rejects(f.client.save(ticket));
  draft.cases[0].authored.fraction=5;ticket.request.documentJson='{"cases":[]}';
  f.on('cases_save_view',()=>reply);await f.client.save(ticket);
  const calls=f.calls.filter(call=>call.command==='cases_save_view');assert.equal(calls.length,2);
  assert.equal(calls[0].args.request.documentJson,original.documentJson);assert.deepEqual(calls[1].args.request,original);
  assert.equal(f.client.document().cases[0].authored.fraction,1.2345678901234567);
  assert.equal(f.client.document().cases[0].authored.exponent,1.2345678901234567e-80);
  assert.equal(f.client.document().cases[0].rows.kind,'native_evidence_container');
});


test('the complete document JSON uses the exact 16 MiB UTF-8 ceiling without encoding a second full buffer',async()=>{
  const f=clientFixture();await f.client.load();assert.equal(f.api.limits.documentBytes,16777216);
  for(const text of ['plain','ação','😀','\ud800','x😀é'])for(let maximum=0;maximum<=12;maximum++){
    assert.equal(f.api.validate.utf8Within(text,maximum),new TextEncoder().encode(text).length<=maximum);
  }
  const draft=f.client.document();draft.cases[0].authored.padding='';
  const overhead=new TextEncoder().encode(f.client.prepareSave(draft).request.documentJson).length;
  draft.cases[0].authored.padding='é'.repeat(Math.floor((16777216-overhead)/2))+'x'.repeat((16777216-overhead)%2);
  const exact=f.client.prepareSave(draft);assert.equal(new TextEncoder().encode(exact.request.documentJson).length,16777216);
  draft.cases[0].authored.padding+='x';assert.throws(()=>f.client.prepareSave(draft),/16 MiB/);
  assert.equal(f.calls.length,1,'oversized metadata never reaches save admission');
});

const captureTicket=f=>f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows:[{id:0,eventRef:'source:0'}]});
const committedRef=ref=>Object.fromEntries(['kind','schemaVersion','owner','containerId','manifestId','manifestSha256','memberCount'].map(key=>[key,key==='kind'?'native_evidence':ref[key]]));
const attachPrepared=(f,ref)=>{const doc=f.client.document();doc.cases[0].items.push({id:'new',rows:{kind:'native_evidence_container',reference:ref,preservedCount:ref.memberCount,preview:null}});return f.client.prepareSave(doc);};

test('Cancel discards only the captured pending token and epoch after the live store changes',async()=>{
  const f=clientFixture();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',({request})=>pendingReference(request));
  const ref=await f.client.prepare(ticket),expected=plain(ref);ref.token='caller mutation';ticket.request.expectedStore.epoch=uuid(700);
  const replacement=document();replacement.store.epoch=uuid(701);f.setDocument(replacement);await f.client.load();
  f.on('case_evidence_discard',()=>({discarded:true}));assert.equal(await f.client.discard(ticket),true);
  assert.deepEqual(f.calls.at(-1),{command:'case_evidence_discard',args:{request:{store:{storeId:uuid(1),epoch:uuid(2)},reference:expected}}});
  await assert.rejects(f.client.prepare(ticket),/cancelada|substituída/);
});

test('Cancel before preparation completion releases the late native token without adopting it',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',()=>gate.promise);f.on('case_evidence_discard',()=>({discarded:true}));
  const preparing=f.client.prepare(ticket);assert.equal(await f.client.discard(ticket),false);assert.equal(f.calls.some(c=>c.command==='case_evidence_discard'),false);
  gate.resolve(pendingReference(ticket.request));await assert.rejects(preparing,/VIEW_CHANGED/);
  assert.equal(f.calls.filter(c=>c.command==='case_evidence_discard').length,1);assert.equal(f.client.document().cases[0].items.length,2);
});

test('a successful stale preparation is discarded with its original owner rather than the replacement',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',()=>gate.promise);f.on('case_evidence_discard',()=>({discarded:true}));
  let current=true;const preparing=f.client.prepare(ticket,{isCurrent:()=>current});current=false;gate.resolve(pendingReference(ticket.request));await assert.rejects(preparing,/VIEW_CHANGED/);
  assert.deepEqual(f.calls.at(-1).args.request.reference.owner,owner());assert.equal(f.calls.at(-1).args.request.store.epoch,uuid(2));
});

test('discard failure is silent and never turns a pending token into committed evidence',async()=>{
  const f=clientFixture();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',({request})=>pendingReference(request));await f.client.prepare(ticket);
  f.on('case_evidence_discard',()=>{throw Error('transport unavailable');});assert.equal(await f.client.discard(ticket),false);assert.equal(f.client.status(),'ready');
  f.on('case_evidence_discard',()=>({discarded:true}));assert.equal(await f.client.discard(ticket),true);
});

test('a committing or already committed token is never discarded by a late Cancel',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',({request})=>pendingReference(request));const ref=await f.client.prepare(ticket),save=attachPrepared(f,ref);
  f.on('cases_save_view',()=>gate.promise);const saving=f.client.save(save);assert.equal(await f.client.discard(ticket),false);
  const receipt=f.receipt(save.request);receipt.evidence=receipt.evidence.map(committedRef);gate.resolve(receipt);await saving;
  assert.equal(await f.client.discard(ticket),false);assert.equal(f.calls.filter(c=>c.command==='case_evidence_discard').length,0);
  assert.equal(f.client.document().cases[0].items.at(-1).rows.reference.kind,'native_evidence');
});

test('lost save acknowledgement keeps a token protected until its exact retry resolves',async()=>{
  const f=clientFixture();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',({request})=>pendingReference(request));const ref=await f.client.prepare(ticket),save=attachPrepared(f,ref);
  f.on('cases_save_view',()=>{throw Error('lost acknowledgement');});await assert.rejects(f.client.save(save),/lost acknowledgement/);assert.equal(await f.client.discard(ticket),false);
  f.on('cases_save_view',({request})=>{const receipt=f.receipt(request);receipt.evidence=receipt.evidence.map(committedRef);return receipt;});await f.client.save(save);
  assert.equal(await f.client.discard(ticket),false);assert.equal(f.calls.filter(c=>c.command==='case_evidence_discard').length,0);
});

test('a cancelled prepared token is rejected before any attempted metadata commit',async()=>{
  const f=clientFixture();await f.client.load();const ticket=captureTicket(f);f.on('case_evidence_prepare',({request})=>pendingReference(request));const ref=await f.client.prepare(ticket),save=attachPrepared(f,ref);
  f.on('case_evidence_discard',()=>({discarded:true}));assert.equal(await f.client.discard(ticket),true);
  await assert.rejects(f.client.save(save),/PREPARATION_REQUIRED/);assert.equal(f.calls.some(c=>c.command==='cases_save_view'),false);
});

test('expired or failed-discard preparation cycles retain at most the eight newest ready authorities',async()=>{
  const f=clientFixture();await f.client.load();const refs=[];
  f.on('case_evidence_prepare',({request})=>({...pendingReference(request),token:`token-${request.requestId}`,containerId:`container-${request.requestId}`}));
  for(let index=0;index<16;index++)refs.push(await f.client.prepare(captureTicket(f)));
  for(const ref of refs.slice(0,-8))assert.throws(()=>attachPrepared(f,ref),/PREPARATION_REQUIRED/);
  for(const ref of refs.slice(-8))assert.ok(attachPrepared(f,ref));
  f.on('case_evidence_discard',()=>{throw Error('cleanup offline');});
  for(let index=0;index<8;index++){
    const ticket=captureTicket(f),ref=await f.client.prepare(ticket),save=attachPrepared(f,ref);assert.equal(await f.client.discard(ticket),false);
    assert.throws(()=>attachPrepared(f,ref),/PREPARATION_REQUIRED/);await assert.rejects(f.client.save(save),/PREPARATION_REQUIRED/);
  }
  assert.equal(f.calls.some(c=>c.command==='cases_save_view'),false);
});

const memberMatch=(ref,itemIndex=0,itemId='duplicate-legacy-item',occurrenceId=uuid(999))=>({containerId:ref.containerId,itemIndex,itemId,member:{containerId:ref.containerId,manifestId:ref.manifestId,occurrenceId}});
const membershipReply=(rows,matches)=>({kind:'native_case_members',caseEvidenceSignature:'opaque-evidence-signature-a',rows:rows.map((row,index)=>({row,state:matches[index].length===0?'missing':matches[index].length===1?'unique':'ambiguous',matches:matches[index]}))});

test('native membership reads bind source proof and exact row handles without Event or preview values',async()=>{
  const f=clientFixture();await f.client.load();const rows=[{id:0,eventRef:'source:0'},{id:1,eventRef:'source:1'}],source=selectionReceipt(),result=membershipReply(rows,[[],[memberMatch(reference(12,2))]]);
  f.on('case_evidence_find_members',()=>result);assert.deepEqual(plain(await f.client.findMembers({owner:owner(),source,rows})),result);
  assert.deepEqual(f.calls.at(-1).args,{request:{store:{storeId:uuid(1),epoch:uuid(2)},owner:owner(),caseEvidenceSignature:'opaque-evidence-signature-a',source,rows,stationId:null}});
});

test('native membership keeps duplicate item-ID ambiguity and tuple-specific occurrence identities explicit',async()=>{
  const doc=document();doc.cases[0].items[1].rows=container(14,2);doc.caseEvidence[0].preservedCount=4;const f=clientFixture({loaded:doc});await f.client.load();
  const rows=[{id:0,eventRef:'source:0'}],matches=[memberMatch(reference(12,2),0),memberMatch(reference(14,2),1)];f.on('case_evidence_find_members',()=>membershipReply(rows,[matches]));
  const result=await f.client.findMembers({owner:owner(),source:selectionReceipt(),rows});assert.equal(result.rows[0].state,'ambiguous');assert.equal(result.rows[0].matches.length,2);
  assert.equal(result.rows[0].matches[0].itemId,result.rows[0].matches[1].itemId);assert.notEqual(result.rows[0].matches[0].containerId,result.rows[0].matches[1].containerId);
});

test('native membership captures caller inputs and rejects late source or owner changes',async()=>{
  const f=clientFixture(),gate=deferred();await f.client.load();const rows=[{id:0,eventRef:'source:0'}],source=selectionReceipt(),original=structuredClone(rows);let current=true;
  f.on('case_evidence_find_members',()=>gate.promise);const read=f.client.findMembers({owner:owner(),source,rows,stationId:'station-b'},{isCurrent:()=>current});rows[0].eventRef='caller edit';source.catalogSignature='c'.repeat(64);current=false;gate.resolve(membershipReply(original,[[]]));await assert.rejects(read,/VIEW_CHANGED/);
  assert.equal(f.calls.at(-1).args.request.rows[0].eventRef,'source:0');assert.equal(f.calls.at(-1).args.request.source.catalogSignature,'b'.repeat(64));
});

test('native membership rejects wrong signatures, counts, row order, containers and duplicate matches',async()=>{
  const f=clientFixture();await f.client.load();const rows=[{id:0,eventRef:'source:0'}];
  for(const mutate of [v=>v.caseEvidenceSignature='foreign',v=>v.rows=[],v=>v.rows[0].row.id=1,v=>v.rows[0].state='missing',v=>v.rows[0].matches[0].member.manifestId='other',v=>v.rows[0].matches[0].itemIndex=1,v=>v.rows[0].matches.push(structuredClone(v.rows[0].matches[0]))]){
    const result=membershipReply(structuredClone(rows),[[memberMatch(reference(12,2))]]);mutate(result);f.on('case_evidence_find_members',()=>result);await assert.rejects(f.client.findMembers({owner:owner(),source:selectionReceipt(),rows}));
  }
  assert.equal(f.client.status(),'ready');assert.equal(f.client.document().cases[0].items.length,2);
});

test('native membership refuses excessive or duplicate input handles before transport',async()=>{
  const f=clientFixture();await f.client.load();
  for(const rows of [[{id:0,eventRef:'same'},{eventRef:'same',id:0}],Array.from({length:2001},(_,id)=>({id,eventRef:`source:${id}`})),[{id:Number.MAX_SAFE_INTEGER+1,eventRef:'unsafe'}],[{id:0,eventRef:'source',fields:{number:1}}]])await assert.rejects(f.client.findMembers({owner:owner(),source:selectionReceipt(),rows}));
  assert.equal(f.calls.length,1);
});

test('native membership enforces its encoded response ceiling without retaining oversized match lists',async()=>{
  const f=clientFixture();await f.client.load();const rows=[{id:0,eventRef:'source:0'}],matches=Array.from({length:260},(_,index)=>memberMatch(reference(12,2),0,'duplicate-legacy-item',`${index}:`+'x'.repeat(4090)));
  f.on('case_evidence_find_members',()=>membershipReply(rows,[matches]));await assert.rejects(f.client.findMembers({owner:owner(),source:selectionReceipt(),rows}),/1 MiB/);assert.equal(f.client.status(),'ready');
});

test('unsaved capture removal and Undo retain the native new-container purpose and all prepared authority',async()=>{
  const f=clientFixture();await f.client.load();
  f.on('case_evidence_prepare',({request})=>({...pendingReference(request,{count:2}),token:`capture-${request.requestId}`}));
  const capture=f.client.prepareCapture({target:owner(),source:selectionReceipt(),rows:[{id:1,eventRef:'one'},{id:2,eventRef:'two'}]}),original=await f.client.prepare(capture);
  f.on('case_evidence_prepare_membership',({request})=>({reference:{...pendingReference(request,{count:request.action.kind==='remove'?1:2}),containerId:request.reference.containerId,purpose:request.reference.purpose,token:`edit-${request.requestId}`},undo:request.reference,changedCount:1,remainingCount:request.action.kind==='remove'?1:2}));
  const removal=await f.client.prepare(f.client.prepareMembership({reference:original,action:{kind:'remove',members:[member(original)]}}));
  assert.deepEqual(plain(removal.reference.purpose),{kind:'new_container'});assert.deepEqual(plain(removal.undo),plain(original));assert.ok(attachPrepared(f,original));assert.ok(attachPrepared(f,removal.reference));
  const restored=await f.client.prepare(f.client.prepareMembership({reference:removal.reference,action:{kind:'restore',target:original}}));
  assert.deepEqual(plain(restored.reference.purpose),{kind:'new_container'});assert.equal(restored.remainingCount,2);assert.ok(attachPrepared(f,restored.reference));
});

test('chained pending membership keeps its original committed base rather than the intermediate manifest',async()=>{
  const f=clientFixture();await f.client.load();const base=reference(12,3);
  f.on('case_evidence_prepare_membership',({request})=>({reference:{...pendingReference(request,{count:request.reference.memberCount-1,replace:base}),manifestId:`prepared-${request.requestId}`,token:`token-${request.requestId}`},undo:request.reference,changedCount:1,remainingCount:request.reference.memberCount-1}));
  const first=await f.client.prepare(f.client.prepareMembership({reference:base,action:{kind:'remove',members:[member(base)]}}));
  const second=await f.client.prepare(f.client.prepareMembership({reference:first.reference,action:{kind:'remove',members:[member(first.reference)]}}));
  assert.equal(second.reference.purpose.baseManifestId,base.manifestId);assert.notEqual(second.reference.purpose.baseManifestId,first.reference.manifestId);assert.equal(second.remainingCount,1);
  f.on('case_evidence_prepare_membership',({request})=>({reference:pendingReference(request,{count:0,replace:request.reference}),undo:request.reference,changedCount:1,remainingCount:0}));
  await assert.rejects(f.client.prepare(f.client.prepareMembership({reference:second.reference,action:{kind:'remove',members:[member(second.reference)]}})),/ocorrências selecionadas/);
});

test('native readonly legacy item blocks round-trip outside authored Case metadata',async()=>{
  const doc=document();doc.caseEvidence[0].legacyItemAliases=[{alias:'duplicate-legacy-item',state:'ambiguous'},{alias:'removed',state:'missing'}];
  const f=clientFixture({loaded:doc});await f.client.load();assert.deepEqual(plain(f.client.caseState('case-a').legacyItemAliases),doc.caseEvidence[0].legacyItemAliases);
  const ticket=f.client.prepareSave();assert.deepEqual(wireDocument(ticket).caseEvidence,doc.caseEvidence);assert.equal(Object.hasOwn(wireDocument(ticket).cases[0],'legacyItemAliases'),false);
  await f.client.save(ticket);assert.deepEqual(plain(f.client.caseState('case-a').legacyItemAliases),doc.caseEvidence[0].legacyItemAliases);
});

test('native legacy item blocks enforce UTF-8 alias, count, encoded-map and strict shape limits',()=>{
  const api=moduleFixture(),validate=api.validate.legacyItemAliases;
  assert.doesNotThrow(()=>validate([{alias:'😀'.repeat(1024),state:'missing'}]));
  for(const values of [[{alias:'😀'.repeat(1024)+'x',state:'missing'}],[{alias:'a',state:'unique'}],[{alias:'a',state:'missing',reason:'key_collision'}],[{alias:'a',state:'missing'},{alias:'a',state:'ambiguous'}],Array.from({length:10001},(_,i)=>({alias:String(i),state:'missing'})),Array.from({length:300},(_,i)=>({alias:String(i)+'x'.repeat(4000),state:'missing'}))])assert.throws(()=>validate(values));
  assert.doesNotThrow(()=>validate(Array.from({length:10000},(_,i)=>({alias:String(i),state:'ambiguous'}))));
});
