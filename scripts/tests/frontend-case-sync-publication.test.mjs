import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const code=app.slice(app.indexOf('const caseTransport ='),app.indexOf('// ------------------------------------------------------------------ helpers de espera'));
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const settle=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
const owner=id=>({caseId:id,analysisId:`analysis-${id}`,configRevision:1,visibilityRevision:0});
function fixture(invoke){
  const context=vm.createContext({state:{},window:{},explorerAnalytics:new Map(),toast(){},invoke});
  vm.runInContext(code,context);return context;
}

test('concurrent first Case reads share one backend publication',async()=>{
  const gate=deferred(),calls=[],events=[{id:1,event_ref:'original:1'}];
  const context=fixture((cmd,args)=>{calls.push({cmd,args});return gate.promise;});
  const args={caseEvents:events,analysisContext:owner('a')};
  const first=context.caseArgs(args),second=context.caseArgs(args);await settle();
  assert.equal(calls.length,1);assert.equal(calls[0].cmd,'case_sync');
  assert.equal(calls[0].args.events,events);
  gate.resolve();const [a,b]=await Promise.all([first,second]);
  assert.equal(a.caseKey,b.caseKey);assert.equal(a.caseEvents,undefined);
  await context.caseArgs(args);assert.equal(calls.length,1);
  assert.equal(vm.runInContext('caseTransport.pending.size',context),0);
});

test('failed shared synchronization clears its promise and permits a fresh retry',async()=>{
  const gate=deferred(),calls=[],events=[];
  const context=fixture((cmd,args)=>{calls.push({cmd,args});return calls.length===1?gate.promise:Promise.resolve();});
  const args={caseEvents:events,analysisContext:owner('a')};
  const pending=Promise.allSettled([context.caseArgs(args),context.caseArgs(args)]);await settle();gate.reject(Error('sync failed'));
  assert.deepEqual((await pending).map(result=>result.status),['rejected','rejected']);
  assert.equal(vm.runInContext('caseTransport.pending.size',context),0);
  assert.equal(vm.runInContext('caseTransport.synced.size',context),0);
  await context.caseArgs(args);assert.equal(calls.length,2);
});

test('identical frontend keys never share synchronization across Case owners',async()=>{
  const gates=[deferred(),deferred()],calls=[],events=[{id:1}];
  const context=fixture((cmd,args)=>{calls.push({cmd,args});return gates[calls.length-1].promise;});
  const first=context.caseArgs({caseEvents:events,analysisContext:owner('a')});
  const second=context.caseArgs({caseEvents:events,analysisContext:owner('b')});await settle();
  assert.equal(calls.length,2);assert.equal(calls[0].args.key,calls[1].args.key);
  assert.notEqual(calls[0].args.analysisContext.caseId,calls[1].args.analysisContext.caseId);
  gates.forEach(g=>g.resolve());await Promise.all([first,second]);
});

test('late cache misses reuse a newer completed resync instead of republishing it',async()=>{
  const events=[{id:1,event_ref:'original:1'}],reads=[],gate=deferred();let syncs=0;
  const context=fixture((cmd,args)=>{
    if(cmd==='case_sync'){syncs++;return syncs===2?gate.promise:Promise.resolve();}
    assert.equal(cmd,'count');
    if(reads.length<2){const read=deferred();reads.push(read);return read.promise;}
    return Promise.resolve({publication:syncs,key:args.caseKey});
  });
  const args={caseEvents:events,analysisContext:owner('a')};
  const first=context.api('count',args,{silent:true}),second=context.api('count',args,{silent:true});await settle();
  assert.equal(syncs,1);assert.equal(reads.length,2);
  reads[0].reject(Error('CASE_CACHE_MISS'));await settle();assert.equal(syncs,2);
  gate.resolve();const a=await first;assert.equal(a.publication,2);
  reads[1].reject(Error('CASE_CACHE_MISS'));const b=await second;
  assert.equal(b.publication,2);assert.equal(syncs,2);assert.equal(a.key,b.key);
});
