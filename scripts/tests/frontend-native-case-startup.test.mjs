import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../${name}`,import.meta.url),'utf8'),app=read('frontend/app.js'),workspace=read('frontend/workspace-context.js');
const part=(source,start,end)=>source.slice(source.indexOf(start),source.indexOf(end,source.indexOf(start)));
const plain=value=>JSON.parse(JSON.stringify(value)),deferred=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return{promise,resolve};};
async function fixture(){
 const contexts=new Map(),timers=new Map(),events=[],messages=[],order=[];let id=0;const context=vm.createContext({window:{crypto:{randomUUID:()=>`native-request-${++id}`}},structuredClone,TextEncoder,
  state:{cases:{active:null,cases:[]},derivedFields:[],analysisDefinitionsPending:false,artifactSwitchVersion:0,refreshVersion:0},
  document:{readyState:'complete',querySelector:()=>null,dispatchEvent:event=>events.push(event)},CustomEvent:class{constructor(type,options){this.type=type;this.detail=options.detail;}},
  setTimeout:fn=>{const key=++id;timers.set(key,fn);return key;},clearTimeout:key=>timers.delete(key),toast:text=>messages.push(text),
  renderCaseBar(){},updateAnalysisBadge(){},renderExploreTree(){},setAnalysisView(){},syncActiveCaseArtifacts:async()=>{},
 });
 for(const name of ['frontend/case-evidence.js','frontend/case-evidence-session.js','frontend/analysis-context.js','scripts/preview/mock-native-case.js'])vm.runInContext(read(name),context);
 const transport=context.window.createMockNativeCase({analysisContexts:contexts});context.api=async(command,args={})=>{
  if(transport.handlers[command])return transport.handlers[command](args);
  if(command==='list_derived_fields')return structuredClone(contexts.get(args.analysisContext.caseId).config.derivedFields);
  if(command==='analysis_context_snapshot')return structuredClone(contexts.get(args.caseId));
  throw Error(`Unexpected startup transport ${command}`);
 };
 vm.runInContext(part(app,'// Native evidence remains opt-in','let casesSaveQueue ='),context);
 vm.runInContext(part(app,'function normalizeCaseStore(','function ensureCase('),context);
 vm.runInContext(part(app,'let casesSaveQueue =','function defaultCaseWorkspace('),context);
 vm.runInContext(part(app,'async function loadDerivedFields(','function renderExploreTree('),context);
 Object.assign(context,{scope:'dataset',sourceBusy:0,sourceQueue:Promise.resolve(),restoringCase:false,initialized:false,caseGeneration:0,generation:0,detailRequest:0,states:new Map(),runtime:new Map(),
  capture:()=>{order.push('capture');if(context.activeCase())context.activeCase().workspace.captureWitness='source-queue';},setScope:async()=>{throw Error('Unexpected area switch');},initialize:async()=>{context.initialized=true;order.push('initialize');}});
 vm.runInContext(part(workspace,'  function sourceChanged(update)','  let nativeMembership ='),context);
 context.window.WorkspaceContext={sourceChanged:context.sourceChanged,replaceCases:context.replaceCases,initialize:context.initialize,invalidateAnalysis(){},get ready(){return context.initialized;}};
 vm.runInContext(part(app,'async function mcpReloadCases()','// ------------------------------------------------------------------ teclado'),context);
 vm.runInContext(part(app,'window.workspaceBootstrap =','// ==========================================================================\n// DASHBOARD'),context);
 await context.window.workspaceBootstrap;
 return{context,transport,events,messages,order,flush:()=>{const pending=[...timers.values()];timers.clear();pending.forEach(fn=>fn());}};
}

test('production startup loads native authority and its real session adopts the subsequent metadata receipt',async()=>{
 const f=await fixture(),services=f.context.nativeEvidenceServices(),c=f.context.activeCase();assert.equal(f.context.window.CaseEvidence.active,true);assert.equal(f.context.window.WorkspaceContext.ready,true);assert.equal(c.id,'native-startup-case');assert.equal(c.items[0].rows.kind,'native_evidence_container');assert.equal(services.session.isClean(),true);assert.deepEqual(plain(f.transport.calls.map(call=>call.command)),['cases_load_view']);
 f.transport.advanceContextOnNextSave();c.name='Metadata saved after native startup';const saving=f.context.saveCases();f.flush();assert.equal(await saving,true);assert.equal(services.session.isClean(),true);assert.equal(f.context.window.AnalysisContexts.identity().configRevision,1);assert.equal(services.client.document().store.revision,f.context.state.cases.store.revision);assert.equal(f.transport.document().cases[0].name,c.name);assert.ok(f.events.some(event=>event.type==='analysis-context-change'&&event.detail.snapshot.configRevision===1));assert.ok(f.transport.calls.every(call=>call.nativeActive));assert.deepEqual(f.messages,[]);
 const wire=f.transport.calls.find(call=>call.command==='cases_save_view').args.request;assert.equal(typeof wire.documentJson,'string');assert.equal(JSON.parse(wire.documentJson).cases[0].items[0].rows.kind,'native_evidence_container');assert.equal(f.context.nativeEvidenceServices(),services);
});

test('actual native management reload waits for the source queue before installing a loaded document',async()=>{
 const f=await fixture(),before=f.context.state.cases,services=f.context.nativeEvidenceServices(),gate=deferred(),entered=deferred();
 const source=f.context.sourceChanged(async()=>{f.order.push('source-enter');entered.resolve();await gate.promise;f.order.push('source-exit');});await entered.promise;
 f.transport.publishName('Queued native reload');const reload=f.context.mcpReloadCases();for(let i=0;i<30&&f.transport.calls.filter(call=>call.command==='cases_load_view').length<2;i++)await Promise.resolve();
 assert.equal(f.transport.calls.filter(call=>call.command==='cases_load_view').length,2);assert.equal(f.context.sourceBusy,1);assert.equal(f.context.state.cases,before);assert.equal(f.context.activeCase().name,'Native startup after adoption');gate.resolve();await source;await reload;assert.notEqual(f.context.state.cases,before);assert.equal(f.context.activeCase().name,'Queued native reload');assert.equal(f.context.sourceBusy,0);assert.equal(f.context.nativeEvidenceServices(),services);assert.equal(services.session.isClean(),true);assert.ok(f.order.indexOf('source-exit')<f.order.lastIndexOf('initialize'));assert.deepEqual(f.messages,[]);
});

test('queued native reload cannot overwrite a metadata draft authored while the source queue waits',async()=>{
 const f=await fixture(),before=f.context.state.cases,gate=deferred(),entered=deferred();const source=f.context.sourceChanged(async()=>{entered.resolve();await gate.promise;});await entered.promise;f.transport.publishName('New native view');const reload=f.context.mcpReloadCases();for(let i=0;i<30&&f.transport.calls.filter(call=>call.command==='cases_load_view').length<2;i++)await Promise.resolve();f.context.activeCase().name='Keep this draft';gate.resolve();await source;await reload;assert.equal(f.context.state.cases,before);assert.equal(f.context.activeCase().name,'Keep this draft');assert.equal(f.context.nativeEvidenceServices().session.status(),'reconcile_required');assert.match(f.messages.at(-1),/rascunho/);
});

test('native startup browser acceptance uses production scripts and services and is selected by both CI smoke lists',()=>{
 const browser=read('scripts/preview/test-native-case-startup.mjs'),mock=read('scripts/preview/mock-tauri.js'),server=read('scripts/preview/serve.mjs'),html=read('frontend/index.html');
 assert.doesNotMatch(browser,/addScriptTag|CaseEvidence\.active\s*=|nativeEvidenceServices\s*=|state\.cases\s*=(?!=)/);assert.match(browser,/__mockNativeCaseBootstrapEnabled=true/);assert.match(mock,/if \(window.CaseEvidence && !window.__mockNativeCaseBootstrapEnabled\) window.CaseEvidence.active = false/);assert.match(mock,/Object.assign\(handlers, native.handlers\)/);assert.match(server,/__mock-native-case__\.js/);assert.ok(html.indexOf('src="case-evidence-session.js"')<html.indexOf('src="app.js"'));for(const path of ['scripts/preview/run-smoke.mjs','.github/workflows/checks.yml'])assert.match(read(path),/test-native-case-startup\.mjs/);
});
