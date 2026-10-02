import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const code=readFileSync(new URL('../../frontend/exclusion-visibility.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const settle=async()=>{for(let i=0;i<20;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
function fixture(){
  const nodes=new Map(),requests=[],listeners={};let scope='dataset',evidence=[{id:1,event_ref:'saved:1'}],preparation=null;
  const state={owner:{caseId:'a',instance:1,identity:{caseId:'a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-7'}};
  const node=()=>({hidden:false,textContent:'',setAttribute(){},before(){},set id(value){nodes.set(value,this);}});
  const capture=()=>structuredClone(state.owner),identity=value=>JSON.stringify(value);
  const context=vm.createContext({state,window:{workspaceBootstrap:new Promise(()=>{}),AnalysisContexts:{capture,signature:identity,
    assertOwner:owner=>assert.deepEqual(plain(owner),plain(state.owner))}},document:{addEventListener:(name,listener)=>{listeners[name]=listener;}},
    el:node,$:node,fmtNum:String,workspaceScope:()=>scope,caseSig:()=>JSON.stringify(evidence),caseEvents:()=>evidence,
    caseArgs:async args=>{if(preparation)await preparation;if(!args.caseEvents)return args;const{caseEvents,...rest}=args;return{...rest,caseKey:'captured-evidence'};},
    api:(cmd,args,opts)=>{const request=deferred();requests.push({cmd,args,opts,...request});return request.promise;}});
  vm.runInContext(code,context);
  const response=(request=requests.at(-1),overrides={})=>({analysis:structuredClone(request.args.analysisContext),sourceGeneration:request.args.caseKey?null:request.args.sourceGeneration,
    scope:request.args.caseKey?'case':'dataset',sourceAvailable:true,totalRows:100,excludedRows:20,unavailableMembers:request.args.caseKey?null:4,...overrides});
  return{controller:context.window.ExclusionVisibility,state,requests,nodes,response,listeners,capture,
    scope:value=>{scope=value;},evidence:value=>{evidence=value;},prepare:gate=>{preparation=gate;}};
}
test('only explicit admitted unavailable membership produces the shared source and archive notices',async()=>{
  const f=fixture(),read=f.controller.refresh();await settle();const request=f.requests[0];
  assert.equal(request.cmd,'exclusion_visibility');assert.equal(request.opts.latest,'exclusion-visibility');assert.equal(request.opts.silent,true);
  request.resolve(f.response());await read;
  for(const notice of f.nodes.values()){assert.equal(notice.hidden,false);assert.match(notice.textContent,/4 registro.*fonte original está ausente ou mudou/);}
  await f.controller.refresh();assert.equal(f.requests.length,1,'same context reuses the admitted status');
  for(const value of [null,0]){const next=f.controller.refresh({force:true});await settle();f.requests.at(-1).resolve(f.response(undefined,{unavailableMembers:value}));await next;assert.ok([...f.nodes.values()].every(node=>node.hidden));}
});
test('Case evidence is captured for transport and unknown membership is never called unavailable',async()=>{
  const f=fixture();f.scope('case');const read=f.controller.refresh();await settle();const request=f.requests[0];
  assert.equal(request.args.caseKey,'captured-evidence');assert.equal(request.args.caseEvents,undefined);assert.deepEqual(plain(request.opts.caseEvents),[{id:1,event_ref:'saved:1'}]);
  request.resolve(f.response());await read;assert.ok([...f.nodes.values()].every(node=>node.hidden));
  assert.throws(()=>f.controller.validate(f.response(undefined,{unavailableMembers:80}),{owner:f.capture(),scope:'case'}),/inválido/);
});
test('malformed counts, incomplete echoes and foreign source/analysis summaries never become notices',async()=>{
  for(const alter of [value=>{value.analysis.caseId='other';},value=>{value.analysis.visibilityRevision++;},value=>{value.sourceGeneration++;},
    value=>{value.scope='case';},value=>{value.unavailableMembers=-1;},value=>{value.excludedRows=101;},value=>{delete value.unavailableMembers;},value=>{value.sourceAvailable=false;}]){
    const f=fixture(),read=f.controller.refresh();await settle();const value=f.response();alter(value);f.requests[0].resolve(value);await read;
    assert.ok([...f.nodes.values()].every(node=>node.hidden));
  }
});
test('late reads are rejected after owner, source, area or captured evidence changes',async()=>{
  for(const change of ['Case','source','scope','evidence']){
    const f=fixture();if(change==='evidence')f.scope('case');const read=f.controller.refresh();await settle();const request=f.requests[0];
    if(change==='Case')f.state.owner.instance++;
    if(change==='source'){f.state.owner.sourceGeneration++;f.state.owner.sourceKey='new-source';}
    if(change==='scope')f.scope('case');if(change==='evidence')f.evidence([{id:2,event_ref:'different'}]);
    request.resolve(f.response(request));await read;assert.ok([...f.nodes.values()].every(node=>node.hidden),change);
  }
});
test('stale preparation never dispatches; failed status refresh never asserts zero or erases a valid warning',async()=>{
  const stale=fixture(),gate=deferred();stale.prepare(gate.promise);const blocked=stale.controller.refresh();await settle();stale.state.owner.sourceKey='replaced';gate.resolve();await blocked;assert.equal(stale.requests.length,0);
  const f=fixture(),read=f.controller.refresh();await settle();f.requests[0].resolve(f.response());await read;
  const retry=f.controller.refresh({force:true});await settle();f.requests.at(-1).reject(Error('status unavailable'));await retry;
  assert.ok([...f.nodes.values()].every(node=>!node.hidden));
  f.state.owner.identity.visibilityRevision++;f.controller.render();assert.ok([...f.nodes.values()].every(node=>node.hidden),'a warning from the prior receipt is hidden immediately');
});
