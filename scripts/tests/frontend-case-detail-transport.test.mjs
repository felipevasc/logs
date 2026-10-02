import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const transport=app.slice(app.indexOf('const caseTransport ='),app.indexOf('// ------------------------------------------------------------------ helpers de espera'));
const mock=readFileSync(new URL('../preview/mock-tauri.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const identity=value=>Object.fromEntries(['caseId','analysisId','configRevision','visibilityRevision'].map(key=>[key,value[key]]));
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const reached=async(predicate,message)=>{for(let i=0;i<50;i++){if(predicate())return;await Promise.resolve();}assert.fail(message);};

function transportFixture(detailCommand='event_detail'){
  const calls={sync:[],detail:[],prepared:[]},hooks={};
  const owner={caseId:'case-a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3};
  const evidence=[{id:0,event_ref:'captured:0',raw:'complete captured body',fields:{input:'captured'}}];
  const context=vm.createContext({window:{},state:{},TextEncoder,explorerAnalytics:new Map(),toast(){},
    invoke:async(command,args)=>{
      if(command==='case_sync'){
        calls.sync.push(args);return hooks.sync?hooks.sync(args):{caseContentToken:`publication-${calls.sync.length}`};
      }
      assert.equal(command,detailCommand);calls.detail.push(args);
      return hooks.detail?hooks.detail(args):{...evidence[0],fields:{input:'admitted'}};
    },
  });
  vm.runInContext(transport,context,{filename:'app.js:Case detail transport'});
  const args={id:0,eventRef:'captured:0',analysisContext:owner,sourceGeneration:7,operationId:'detail-test'};
  const options={silent:true,caseEvents:evidence,onCasePrepared:receipt=>calls.prepared.push(receipt)};
  return{context,calls,hooks,evidence,owner,args,options,
    prepare:()=>context.caseArgs({...args,caseEvents:evidence},false,null,{canonical:true})};
}

test('installed Case detail transport dispatches only its synchronized key and mandatory token',async()=>{
  const f=transportFixture(),prepared=await f.prepare();
  assert.equal(prepared.caseContentToken,'publication-1');assert.equal(prepared.caseEvents,undefined);
  assert.equal(prepared.caseKey,f.calls.sync[0].key);assert.equal(f.calls.sync[0].events,f.evidence);
  const result=await f.context.api('event_detail',prepared,f.options);
  assert.equal(result.raw,'complete captured body');assert.equal(f.calls.sync.length,1);assert.equal(f.calls.detail.length,1);
  const sent=f.calls.detail[0];assert.equal(Object.hasOwn(sent,'caseEvents'),false);assert.equal(sent.caseContentToken,'publication-1');
  assert.equal(sent.analysisContext,f.owner);assert.equal(sent.sourceGeneration,7);assert.equal(sent.eventRef,'captured:0');
  assert.equal(sent.operationId,'detail-test');
  assert.deepEqual(plain(f.calls.prepared),[{caseKey:prepared.caseKey,caseContentToken:'publication-1'}]);
});

test('installed Case detail transport refuses missing or malformed synchronization tokens before dispatch',async()=>{
  for(const token of [undefined,null,'',[], 'x'.repeat(129),'é'.repeat(65)]){
    const f=transportFixture();f.hooks.sync=()=>({caseContentToken:token});
    await assert.rejects(f.context.api('event_detail',{...f.args,caseEvents:f.evidence},f.options),/confirmar a versão exata das evidências/);
    assert.equal(f.calls.sync.length,1);assert.equal(f.calls.detail.length,0);assert.equal(f.calls.prepared.length,0);
    const g=transportFixture();
    await assert.rejects(g.context.api('event_detail',{...g.args,caseKey:'known-key',caseContentToken:token},g.options),/confirmar a versão exata das evidências/);
    assert.equal(g.calls.sync.length,0);assert.equal(g.calls.detail.length,0);
  }
});

test('Case detail cache-miss retry retains captured evidence and dispatches the refreshed token once',async()=>{
  const f=transportFixture(),prepared=await f.prepare(),replacement=[{id:0,event_ref:'replacement:0',raw:'wrong'}];
  f.context.state.rows=replacement;
  f.hooks.detail=()=>{if(f.calls.detail.length===1)throw Error('CASE_CACHE_MISS');return f.evidence[0];};
  assert.equal(await f.context.api('event_detail',prepared,f.options),f.evidence[0]);
  assert.equal(f.calls.sync.length,2);assert.equal(f.calls.detail.length,2);
  for(const sync of f.calls.sync){assert.equal(sync.events,f.evidence);assert.equal(sync.analysisContext,f.owner);assert.equal(sync.sourceGeneration,7);}
  assert.equal(f.calls.detail[0].caseKey,f.calls.detail[1].caseKey);
  assert.deepEqual(f.calls.detail.map(args=>args.caseContentToken),['publication-1','publication-2']);
  assert.deepEqual(f.calls.prepared.map(receipt=>receipt.caseContentToken),['publication-1','publication-2']);
  assert.ok(f.calls.detail.every(args=>!Object.hasOwn(args,'caseEvents')));
  const missing=transportFixture();missing.hooks.detail=()=>{throw Error('CASE_CACHE_MISS');};
  await assert.rejects(missing.context.api('event_detail',{...missing.args,caseEvents:missing.evidence},missing.options),/CASE_CACHE_MISS/);
  assert.equal(missing.calls.sync.length,2);assert.equal(missing.calls.detail.length,2,'a second miss terminates instead of looping');
});

test('changed Case detail evidence is refused without a cache-miss resynchronization',async()=>{
  const f=transportFixture();f.hooks.detail=()=>{throw Error('CASE_CACHE_CHANGED: evidence replaced');};
  await assert.rejects(f.context.api('event_detail',{...f.args,caseEvents:f.evidence},f.options),/CASE_CACHE_CHANGED/);
  assert.equal(f.calls.sync.length,1);assert.equal(f.calls.detail.length,1);
});

test('opt-in Java detail uses the same mandatory Case token and captured cache-miss retry transport',async()=>{
  const f=transportFixture('java_trace_detail');
  f.hooks.detail=()=>{if(f.calls.detail.length===1)throw Error('CASE_CACHE_MISS');return{state:'available',trace:{schemaVersion:1},reason:null,row:{id:0,eventRef:'captured:0'}};};
  const response=await f.context.api('java_trace_detail',{...f.args,caseEvents:f.evidence},f.options);
  assert.equal(response.state,'available');assert.equal(f.calls.sync.length,2);
  assert.deepEqual(f.calls.detail.map(args=>args.caseContentToken),['publication-1','publication-2']);
  assert.ok(f.calls.detail.every(args=>!Object.hasOwn(args,'caseEvents')));
  assert.ok(f.calls.sync.every(args=>args.events===f.evidence));
  const missing=transportFixture('java_trace_detail');missing.hooks.sync=()=>({});
  await assert.rejects(missing.context.api('java_trace_detail',{...missing.args,caseEvents:missing.evidence},missing.options),/confirmar a versão exata/);
  assert.equal(missing.calls.detail.length,0);
  const changed=transportFixture('java_trace_detail');changed.hooks.detail=()=>{throw Error('CASE_CACHE_CHANGED');};
  await assert.rejects(changed.context.api('java_trace_detail',{...changed.args,caseEvents:changed.evidence},changed.options),/CASE_CACHE_CHANGED/);
  assert.equal(changed.calls.sync.length,1);
});

test('caller guards and cancellation stop initial and retried Case detail dispatch after synchronization',async()=>{
  for(const stage of ['initial','retry'])for(const guard of ['onCasePrepared','cancelled']){
    const f=transportFixture(),gate=deferred();let current=true;
    const target=stage==='initial'?1:2;
    f.hooks.sync=()=>f.calls.sync.length===target?gate.promise:{caseContentToken:'publication-1'};
    f.hooks.detail=()=>{throw Error('CASE_CACHE_MISS');};
    const options={...f.options,cancelled:()=>guard==='cancelled'&&!current,
      onCasePrepared:receipt=>{if(guard==='onCasePrepared'&&!current)throw Error('caller detail guard expired');f.calls.prepared.push(receipt);}};
    const request=f.context.api('event_detail',{...f.args,caseEvents:f.evidence},options);
    const rejected=assert.rejects(request,guard==='cancelled'?/cancelada/:/caller detail guard expired/);
    await reached(()=>f.calls.sync.length===target,`${stage}: synchronization waits`);current=false;
    gate.resolve({caseContentToken:`publication-${target}`});await rejected;
    assert.equal(f.calls.detail.length,stage==='initial'?0:1,`${stage}/${guard}: no dispatch follows an expired caller`);
    assert.equal(f.calls.prepared.length,stage==='initial'?0:1);
  }
});

async function defaultMock(){
  const storage=new Map(),timers=[];let holdTimers=false;
  const context=vm.createContext({window:{addEventListener(){}},TextEncoder,structuredClone,btoa,atob,
    localStorage:{getItem:key=>storage.get(key)||null,setItem:(key,value)=>storage.set(key,value)},
    setTimeout:callback=>{if(holdTimers)timers.push(callback);else queueMicrotask(callback);return timers.length;},
  });
  vm.runInContext(mock,context,{filename:'mock-tauri.js'});
  const invoke=context.window.__TAURI__.core.invoke,store=await invoke('cases_load');
  let owner=identity(store.cases[0].analysisContext);
  const source=await invoke('source_snapshot');
  return{context,invoke,timers,owner:()=>owner,sourceGeneration:source.generation,
    saveOverlay:async()=>{owner=identity((await invoke('save_derived_field',{name:'overlay',source:'input',rules:[{pattern:'^(.*)$',template:'current-$1'}],steps:[],analysisContext:owner})).analysisContext);},
    holdDetail:()=>{holdTimers=true;context.window.__mockLatency={event_detail:25};},
    release:()=>{holdTimers=false;for(const callback of timers.splice(0))callback();},
  };
}

const fullEvent=()=>({id:0,event_ref:'case-full:0',timestamp:1234,source:'evidence.jsonl',level:'Informação',code:'200',name:'Saved event',
  message:'complete message',raw:'BEGIN\r\n'+'👣'.repeat(20_000)+'\nEND',
  fields:{input:'payload',overlay:'historical-A',typed:{enabled:false,list:[0,null]}},
  derived_originals:{overlay:{state:'present',value:'original'}},
  evidence_provenance:{source:{version:'evidence-v1',recordSpace:'jsonl-v1',label:'evidence.jsonl',eventRefPrefix:'case-full'},locator:{byte_offset:0}}});

async function syncFullEvent(f,key='full-case',event=fullEvent()){
  const receipt=await f.invoke('case_sync',{key,events:[event],analysisContext:f.owner(),sourceGeneration:f.sourceGeneration});
  return{event,args:{id:event.id,eventRef:event.event_ref,caseKey:key,caseContentToken:receipt.caseContentToken,analysisContext:f.owner(),sourceGeneration:f.sourceGeneration}};
}

test('default mock Case full detail preserves large raw evidence and current overlays while query_page drops raw',async()=>{
  const f=await defaultMock();await f.saveOverlay();const event=fullEvent(),before=structuredClone(event);
  assert.ok(Buffer.byteLength(event.raw,'utf8')>70*1024);
  const {args}=await syncFullEvent(f,'full-case',event),result=await f.invoke('event_detail',args);
  assert.equal(result.raw,event.raw);assert.equal(result.fields.overlay,'current-payload');assert.equal(result.event_ref,event.event_ref);
  assert.deepEqual(plain(result.fields.typed),event.fields.typed);assert.deepEqual(plain(result.evidence_provenance),event.evidence_provenance);
  assert.deepEqual(plain(result.derived_originals.overlay),{state:'present',value:'original'});assert.deepEqual(event,before,'admission never mutates preserved evidence');
  const page=await f.invoke('query_page',{caseKey:args.caseKey,analysisContext:f.owner(),filters:[],limit:1});
  assert.equal(page.rows.length,1);assert.equal(page.rows[0].raw,'');assert.equal(page.rows[0].fields.overlay,'current-payload');
  assert.equal((await f.invoke('event_detail',args)).raw,event.raw,'a projected page cannot replace the full synchronized event');
  assert.equal(await f.invoke('event_detail',{...args,id:99,eventRef:'absent:99'}),null);
  assert.equal((await f.invoke('event_detail',{...args,eventRef:undefined})).raw,event.raw,'an optional reference does not truncate detail');
});

test('default mock Case detail rejects old or missing tokens, inline evidence and mismatched references',async()=>{
  const f=await defaultMock(),{event,args}=await syncFullEvent(f);
  for(const token of [undefined,null,'',[], 'x'.repeat(129),'é'.repeat(65)]){
    await assert.rejects(f.invoke('event_detail',{...args,caseContentToken:token}),/ANALYSIS_FIELD_ADMISSION/);
  }
  await assert.rejects(f.invoke('event_detail',{...args,eventRef:'different:0'}),/referência solicitada/);
  for(const inline of [[event],[],{},'malformed',0,false]){
    await assert.rejects(f.invoke('event_detail',{...args,caseEvents:inline}),/ANALYSIS_DETAIL_ADMISSION/);
    await assert.rejects(f.invoke('event_detail',{id:0,analysisContext:f.owner(),caseEvents:inline}),/ANALYSIS_DETAIL_ADMISSION/);
  }
  const replacement=await syncFullEvent(f,args.caseKey,{...event,raw:'replacement body'});
  await assert.rejects(f.invoke('event_detail',args),/CASE_CACHE_CHANGED/);
  assert.equal((await f.invoke('event_detail',replacement.args)).raw,'replacement body');
  await assert.rejects(f.invoke('event_detail',{...replacement.args,caseKey:'evicted-key'}),/CASE_CACHE_MISS/);
});

test('default mock Case detail rechecks the content token after artificial latency',async()=>{
  const f=await defaultMock(),{event,args}=await syncFullEvent(f);f.holdDetail();
  await assert.rejects(f.invoke('event_detail',{...args,caseContentToken:null}),/ANALYSIS_FIELD_ADMISSION/);
  assert.equal(f.timers.length,0,'invalid admission fails before delay or field access');
  const pending=f.invoke('event_detail',args),rejected=assert.rejects(pending,/CASE_CACHE_CHANGED/);
  await reached(()=>f.timers.length===1,'the admitted detail is paused before handler execution');
  await syncFullEvent(f,args.caseKey,{...event,raw:'replacement after admission'});f.release();await rejected;
});
