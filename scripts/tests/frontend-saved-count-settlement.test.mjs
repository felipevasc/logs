import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const timers=new Map(),requests=[];let nextTimer=0;
const tab={isConnected:true,textContent:''};
const state={datasetRevision:1,currentArtifact:{id:'source',loadedAt:1},derivedFields:[]};
const context=vm.createContext({state,Promise,Map,Set,window:{},
  setTimeout:fn=>{timers.set(++nextTimer,fn);return nextTimer;},clearTimeout:id=>timers.delete(id),
  activeCase:()=>({id:'case'}),caseSig:()=> 'evidence-1',caseEvents:()=>[{id:1}],
  savedFilters:()=>[{id:'view'}],savedBackendFilters:()=>[{column:'code',op:'equals_exact',value:'404'}],
  document:{querySelector:()=>tab},fmtNum:String,
  api:(cmd,args,opts)=>new Promise((resolve,reject)=>requests.push({cmd,args,opts,resolve,reject})),
});
vm.runInContext(source.slice(source.indexOf('let filterCountsTimer = null;'),source.indexOf('// ------------------------------------------------------------------ estações')),context);
context.refreshFilterTabCounts();context.refreshFilterTabCounts();
assert.equal(timers.size,1,'normal work remains debounced');assert.equal(requests.length,0);
let finished=false;const settled=context.settleFilterTabCounts().then(()=>{finished=true;});
assert.equal(timers.size,0);assert.equal(requests.length,2,'barrier flushes the actual Dataset and Case requests');
assert.ok(requests.every(r=>r.cmd==='count_filtered'&&r.opts.background));
await Promise.resolve();assert.equal(finished,false,'must await admitted work');
requests[0].resolve(50);requests[1].resolve(3);await settled;
assert.equal(tab.textContent,'3 · 50');assert.equal(finished,true);
context.refreshFilterTabCounts();await context.settleFilterTabCounts();
assert.equal(requests.length,2,'settling an unchanged signature still uses the shared count cache');
state.datasetRevision++;
context.refreshFilterTabCounts();let secondDone=false;
const second=context.settleFilterTabCounts().then(()=>{secondDone=true;});
assert.equal(requests.length,4,'failed-import revision invalidation must finish before a typing baseline');
state.datasetRevision++;
context.refreshFilterTabCounts();
requests[2].resolve(80);requests[3].resolve(4);
for(let i=0;i<8;i++)await Promise.resolve();
assert.equal(requests.length,6,'newly scheduled work during settlement is also flushed');
assert.equal(secondDone,false);requests[4].resolve(90);requests[5].resolve(5);await second;
assert.equal(tab.textContent,'5 · 90');assert.equal(timers.size,0);
const baseline=requests.length;
await context.settleFilterTabCounts();assert.equal(requests.length,baseline,'an idle barrier never creates a query');
state.datasetRevision++;context.refreshFilterTabCounts();const failed=context.settleFilterTabCounts();
requests[6].reject(Error('count failed'));requests[7].resolve(1);await failed;
assert.equal(tab.textContent,'—','failed background counts also settle without unhandled rejections');
console.log('Saved-count settlement includes lazy timers, in-flight/requeued requests, reuse and errors');
