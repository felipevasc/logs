import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value)),owner={storeId:'store',caseId:'c',analysisId:'analysis'},stamp=revision=>({storeId:'store',epoch:'epoch',revision:String(revision)});
const snapshot={schemaVersion:1,caseId:'c',analysisId:'analysis',configRevision:0,visibilityRevision:0,config:{derivedFields:[],references:[]},migrationDiagnostics:[]};
async function fixture(){
 const refs=['a','b'].map(itemId=>({kind:'item',itemId})),trail={id:'trail',itemRefs:structuredClone(refs)},c={id:'c',analysisContext:snapshot,items:[{id:'a'},{id:'b'}],caseTrails:[trail]};
 let store={evidenceViewVersion:1,store:stamp(1),active:'c',diagnostics:[],caseEvidence:[{state:'ready',owner,evidenceSignature:'sig',preservedCount:0}],cases:[c]},id=0,revision=1,lose=false;const calls=[],saved=new Map();
 const context=vm.createContext({window:{},structuredClone,TextEncoder});for(const file of ['case-evidence.js','case-evidence-session.js','case-evidence-items.js'])vm.runInContext(read(file),context);context.window.CaseEvidence.active=true;
 const client=context.window.CaseEvidence.create({enabled:true,requestId:()=>`request-${++id}`,invoke:async(command,{request}={})=>{
  if(command==='cases_load_view')return structuredClone(store);
  assert.equal(command,'cases_save_view');calls.push(plain(request));let receipt=saved.get(request.requestId);if(!receipt){receipt={requestId:request.requestId,committedStore:stamp(++revision),currentStore:stamp(revision),evidence:[],analysisContexts:[snapshot],caseEvidence:store.caseEvidence,replayed:false,reconcileRequired:false};saved.set(request.requestId,receipt);}else receipt={...receipt,replayed:true};
  if(lose){lose=false;throw Error('lost acknowledgement after commit');}return structuredClone(receipt);
 }});await client.load();const session=context.window.CaseEvidenceSession.create({client,getStore:()=>store,setStore:value=>{store=value;}});context.nativeEvidenceServices=()=>({session});const save=async()=>{try{await session.save();return true;}catch{return false;}};
 return{c,trail,refs,client,session,items:context.window.CaseEvidenceItems,save,calls,lose:()=>{lose=true;}};
}

test('lost acknowledgement of native association edit retains the intended draft through exact retry',async()=>{
 const f=await fixture();f.lose();await assert.rejects(f.items.edit(f.c,f.trail,[f.refs[1],f.refs[0]],{save:f.save}),error=>/rascunho foi mantido/.test(error.message)&&!/ordem anterior foi mantida/.test(error.message));
 assert.deepEqual(plain(f.trail.itemRefs),[f.refs[1],f.refs[0]]);assert.equal(f.session.status(),'retry_required');await assert.rejects(f.items.edit(f.c,f.trail,[],{save:f.save}),/Confirme/);assert.equal(f.calls.length,1);
 await f.session.retry();assert.deepEqual(plain(f.trail.itemRefs),plain(f.client.document().cases[0].caseTrails[0].itemRefs));assert.deepEqual(f.calls[0],f.calls[1]);assert.equal(f.session.status(),'ready');assert.equal(f.session.isClean(),true);
});

test('lost acknowledgement of native association Undo retains the restored draft through exact retry',async()=>{
 const f=await fixture(),receipt=await f.items.edit(f.c,f.trail,[f.refs[1],f.refs[0]],{save:f.save});f.lose();await assert.rejects(f.items.undo(receipt,{save:f.save}),error=>/rascunho foi mantido/.test(error.message)&&!/ordem anterior foi mantida/.test(error.message));assert.deepEqual(plain(f.trail.itemRefs),f.refs);
 await f.session.retry();assert.deepEqual(plain(f.trail.itemRefs),plain(f.client.document().cases[0].caseTrails[0].itemRefs));assert.deepEqual(f.calls[1],f.calls[2]);assert.equal(f.session.isClean(),true);
});

test('known association failure before a native ticket is sent still restores the previous order',async()=>{
 const f=await fixture(),before=f.trail.itemRefs;await assert.rejects(f.items.edit(f.c,f.trail,[f.refs[1],f.refs[0]],{save:async()=>{throw Error('validation failed before sending');}}),/validation/);assert.equal(f.trail.itemRefs,before);assert.equal(f.calls.length,0);assert.equal(f.session.isClean(),true);
});
