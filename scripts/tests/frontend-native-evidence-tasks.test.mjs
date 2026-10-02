import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/tasks.js',import.meta.url),'utf8');
const deferred=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return{promise,resolve};};
function fixture(){
  const calls=[],gate=deferred(),node={before(){},classList:{toggle(){}},querySelector(){return this;}};
  const context=vm.createContext({window:{PerformanceTools:{queue:()=>({add:run=>run()})},AnalysisContexts:{capture(){throw Error('native request admission must not borrow active Case');},validSnapshot:()=>false}},
    state:{cases:{active:'other-case'},derivedFields:[],activeDatasetTab:'table'},document:{body:{dataset:{page:'explore'}},documentElement:{dataset:{zone:'analysis'}}},performance,
    $:()=>node,el:()=>({...node}),setTimeout(){},clearTimeout(){},setInterval(){},clearInterval(){},requestAnimationFrame(){},
    api:async(command,args)=>{calls.push({command,args});if(['cancel_task','case_evidence_discard'].includes(command))return{discarded:true};return gate.promise;}});
  vm.runInContext(source,context);return{context,calls,gate,tasks:context.window.Tasks};
}
for(const command of ['case_evidence_prepare','case_evidence_prepare_membership'])test(`late cancelled ${command} releases its exact captured token`,async()=>{
  const f=fixture(),request={expectedStore:{storeId:'old-store',epoch:'old-epoch',revision:'9'},target:{caseId:'original'}};
  const result=f.context.api(command,{request},{silent:true,latest:'preserve'});assert.equal(typeof f.calls[0].args.operationId,'string');
  f.tasks.cancelLatest('preserve');f.context.state.cases.active='replacement';const ref={kind:'pending_native_evidence',token:'original-token',owner:{caseId:'original'}};
  f.gate.resolve(command==='case_evidence_prepare'?ref:{reference:ref});await assert.rejects(result,/cancelada/);
  const discard=f.calls.find(c=>c.command==='case_evidence_discard');assert.deepEqual(JSON.parse(JSON.stringify(discard.args)),{request:{store:{storeId:'old-store',epoch:'old-epoch'},reference:ref}});
  assert.equal(f.calls[0].args.analysisContext,undefined);assert.equal(f.calls[0].args.sourceGeneration,undefined);
});

test('a completed native import receipt survives late cancellation for durable reconciliation',async()=>{
  const f=fixture(),result=f.context.api('case_import_native',{request:{store:{storeId:'store',epoch:'epoch',revision:'1'},path:'local.licase',requestId:'import'}},{silent:true,latest:'import'});
  f.tasks.cancelLatest('import');const receipt={kind:'native_case_import',importedCaseId:'imported',receipt:{requestId:'import'}};f.gate.resolve(receipt);assert.equal(await result,receipt);
  assert.equal(f.calls.some(c=>c.command==='case_evidence_discard'),false);
});

test('native preserved-member reads retain captured admission and have an individually cancellable task',async()=>{
  const f=fixture(),request={store:{storeId:'store',epoch:'epoch'},reference:{owner:{caseId:'original'}},member:{occurrenceId:'exact'}};
  const result=f.context.api('case_evidence_member_detail',{request},{silent:true,latest:'member-detail'});assert.equal(f.calls[0].args.request,request);
  assert.equal(f.calls[0].args.analysisContext,undefined);assert.ok(f.tasks.operationFor('member-detail'));
  f.tasks.cancelLatest('member-detail');f.gate.resolve({kind:'preserved_member_detail'});await assert.rejects(result,/cancelada/);
});
