import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const context=vm.createContext({window:{addEventListener(){}},structuredClone,btoa,atob,setTimeout});
vm.runInContext(readFileSync(new URL('../../frontend/query-lang.js',import.meta.url),'utf8'),context);
vm.runInContext(readFileSync(new URL('../preview/mock-pivot.js',import.meta.url),'utf8'),context);
const language=context.window.QueryLang;
// These scalar text expectations follow Rust Event::col_ref and QueryLang::Ctx::field.
// Free text deliberately omits Value::Null, and roles retain their existing fallback.
const rows=[{}, {'payload.result':null}, {'payload.result':''}, {'payload.result':'null'}, {'payload.result':false}, {'payload.result':0}]
  .map((fields,id)=>({id,event_ref:`null-fixture:${id}`,message:'',source:'',code:'',name:'',description:'',fields}));
const expectations=[
  ['payload.result:*',[1,3,4,5]],
  ['payload.result:"null"',[1,3]],
  ['payload.result="null"',[1,3]],
  ['payload.result=""',[2]],
  ['payload.result!=""',[0,1,3,4,5]],
  ['NOT payload.result:*',[0,2]],
  ['NOT payload.result:null',[0,2,4,5]],
  ['payload.result:(null OR false)',[1,3,4]],
  ['payload.result:/^null$/',[1,3]],
  ['payload.result>=0',[5]],
  ['payload.result:false',[4]],
  ['payload.result:0',[5]],
  ['null',[3]],
  ['_all:null',[3]],
];
for(const [query,expected] of expectations){
  assert.equal(language.validate(query),null,query);
  assert.deepEqual(rows.filter(row=>language.matches(row,query)).map(row=>row.id),expected,query);
}
assert.equal(language.fieldValue(rows[0],language.resolve('payload.result')),null);
assert.equal(language.fieldValue(rows[1],language.resolve('payload.result')),'null');
assert.equal(language.fieldValue({fields:{'payload.result':[null,false,0]}},language.resolve('payload.result')),'[null,false,0]');
assert.equal(language.fieldValue({fields:{'payload.result':{child:null}}},language.resolve('payload.result')),'{"child":null}');
assert.equal(language.matchFilter(rows[1],{column:'payload.result',op:'in',value:'null,other'}),true);
assert.equal(language.matchFilter(rows[0],{column:'payload.result',op:'in',value:'null,other'}),false);
for(const value of [null,'']){
  const event={source:'10.2.3.4',fields:{'@src_ip':value,src_ip:value}};
  assert.equal(language.fieldValue(event,language.resolve('@src_ip')),'10.2.3.4');
  assert.equal(language.fieldValue(event,language.resolve('src_ip')),'10.2.3.4');
  assert.equal(language.matches(event,'@src_ip:null'),false);
}
// Run those same parser predicates through the actual preview command transport.
const storage=new Map();context.localStorage={getItem:key=>storage.get(key)||null,setItem:(key,value)=>storage.set(key,value)};
vm.runInContext(readFileSync(new URL('../preview/mock-tauri.js',import.meta.url),'utf8'),context);
const invoke=context.window.__TAURI__.core.invoke,store=await invoke('cases_load'),snapshot=store.cases[0].analysisContext;
const identity=Object.fromEntries(['caseId','analysisId','configRevision','visibilityRevision'].map(key=>[key,snapshot[key]]));
await invoke('case_sync',{key:'typed-null-parity',events:rows,analysisContext:identity});
for(const [query,expected] of expectations){
  const result=await invoke('query_events',{analysisContext:identity,caseKey:'typed-null-parity',filters:[{column:'_all',op:'query',value:query}],limit:100});
  assert.deepEqual(Array.from(result.rows,row=>row.id),expected,`transport: ${query}`);
}
for(const [op,value,expected] of [['equals_exact','null',[1,3]],['equals_exact','',[2]],['empty','',[0,2]],['not_empty','',[1,3,4,5]]]){
  const result=await invoke('query_events',{analysisContext:identity,caseKey:'typed-null-parity',filters:[{column:'payload.result',op,value}],limit:100});
  assert.deepEqual(Array.from(result.rows,row=>row.id),expected,`${op}: ${value}`);
}
const aggregate=await invoke('aggregate_events',{analysisContext:identity,caseKey:'typed-null-parity',filters:[{column:'payload.result',op:'not_empty',value:''}],groupColumn:'payload.result',aggs:[{func:'count',column:'*',alias:'n'}]});
assert.deepEqual(Array.from(aggregate.group_values),['null','false','0']);assert.equal(aggregate.rows[0].n,2,'typed null and literal null share native scalar grouping text');
console.log('Typed dynamic null matches native scalar text through the real parser and preview transport; absence, free text and role fallback remain distinct');
