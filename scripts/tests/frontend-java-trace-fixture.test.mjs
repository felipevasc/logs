import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const fixture=JSON.parse(readFileSync(new URL('../preview/fixtures/java-traces.json',import.meta.url),'utf8'));
const source=readFileSync(new URL('../preview/mock-tauri.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
async function mock(){
  const storage=new Map(),context=vm.createContext({window:{addEventListener(){},__mockJavaFixturesEnabled:true,__mockJavaFixtures:structuredClone(fixture.cases)},
    structuredClone,btoa,atob,TextEncoder,setTimeout:callback=>{queueMicrotask(callback);return 0;},
    localStorage:{getItem:key=>storage.get(key)||null,setItem:(key,value)=>storage.set(key,value)}});
  vm.runInContext(source,context,{filename:'mock-tauri.js'});
  const invoke=context.window.__TAURI__.core.invoke,store=await invoke('cases_load');
  const analysisContext=Object.fromEntries(['caseId','analysisId','configRevision','visibilityRevision'].map(key=>[key,store.cases[0].analysisContext[key]]));
  const sourceGeneration=(await invoke('source_snapshot')).generation;
  const input=context.window.__mockJavaFixtureRows[0];
  const args={id:input.id,eventRef:input.eventRef,analysisContext,sourceGeneration};
  return{context,invoke,args,rows:context.window.__mockJavaFixtureRows};
}

test('actual-parser fixture structures are opt-in detail responses, never page/Event fields',async()=>{
  const f=await mock();
  assert.match(fixture.parserSourceSha256,/^[a-f0-9]{64}$/);
  assert.match(fixture.generatedBy,/actual/);
  assert.equal(f.context.window.__mockCommandCalls.java_trace_detail,undefined);
  const page=await f.invoke('query_page',{analysisContext:f.args.analysisContext,sourceGeneration:f.args.sourceGeneration,filters:[],limit:100});
  assert.ok(page.rows.length>0);
  for(const row of page.rows){assert.equal(Object.hasOwn(row,'javaTrace'),false);assert.equal(Object.hasOwn(row.fields,'java.trace'),false);}
  for(const [index,sample] of fixture.cases.entries()){
    const ref=f.rows[index],args={...f.args,id:ref.id,eventRef:ref.eventRef};
    const event=await f.invoke('event_detail',args);
    assert.equal(Object.hasOwn(event,'javaTrace'),false);assert.equal(Object.hasOwn(event.fields,'java.trace'),false);
    assert.equal(event.raw,sample.raw);assert.equal(event.fields['java.trace.complete'],sample.detail.trace.complete);
    const before=structuredClone(event),response=await f.invoke('java_trace_detail',args);
    assert.deepEqual(plain(response),{state:'available',trace:sample.detail.trace,reason:null,row:{id:ref.id,eventRef:ref.eventRef}});
    assert.deepEqual(plain(event),plain(before),'reading rich detail does not mutate the Event later saved as evidence');
  }
});

test('Java mock transport preserves keyed evidence ownership and explicit unavailable reasons',async()=>{
  const f=await mock(),event=await f.invoke('event_detail',f.args),before=structuredClone(event);
  const receipt=await f.invoke('case_sync',{key:'java-case',events:[event],analysisContext:f.args.analysisContext,sourceGeneration:f.args.sourceGeneration});
  const args={...f.args,caseKey:'java-case',caseContentToken:receipt.caseContentToken};
  assert.equal((await f.invoke('java_trace_detail',args)).state,'available');
  await assert.rejects(f.invoke('java_trace_detail',{...args,eventRef:'wrong'}),/referência/);
  for(const inline of [[],{},false])await assert.rejects(f.invoke('java_trace_detail',{...args,caseEvents:inline}),/ADMISSION/);
  await assert.rejects(f.invoke('java_trace_detail',{...args,caseContentToken:null}),/ADMISSION/);
  assert.deepEqual(plain(await f.invoke('java_trace_detail',{...args,id:999999,eventRef:'missing'})),{state:'unavailable',trace:null,reason:'record_unavailable',row:null});
  for(const [raw,reason] of [['','raw_unavailable'],['ordinary text','not_java']]){
    const replacement=await f.invoke('case_sync',{key:'java-case',events:[{...event,raw}],analysisContext:f.args.analysisContext});
    await assert.rejects(f.invoke('java_trace_detail',args),/CASE_CACHE_CHANGED/);
    assert.deepEqual(plain(await f.invoke('java_trace_detail',{...args,caseContentToken:replacement.caseContentToken})),
      {state:'unavailable',trace:null,reason,row:{id:event.id,eventRef:event.event_ref}});
  }
  assert.deepEqual(plain(event),plain(before));
});
