import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'),app=read('app.js');
const part=(start,end)=>app.slice(app.indexOf(start),app.indexOf(end,app.indexOf(start)));
const owner={storeId:'store',caseId:'case',analysisId:'analysis'},stamp=revision=>({storeId:'store',epoch:'epoch',revision});
const contextSnapshot={schemaVersion:1,caseId:'case',analysisId:'analysis',configRevision:0,visibilityRevision:0,config:{derivedFields:[],references:[]},migrationDiagnostics:[]};
const document=()=>({evidenceViewVersion:1,store:stamp('1'),active:'case',cases:[{id:'case',analysisContext:contextSnapshot,name:'Case',notes:'original',items:[{id:'item',rows:{kind:'native_evidence_container',reference:{kind:'native_evidence',schemaVersion:1,owner,containerId:'container',manifestId:'manifest',manifestSha256:'a'.repeat(64),memberCount:2},preservedCount:2,preview:null}}],unknown:{fraction:1.2345678901234567}}],caseEvidence:[{state:'ready',owner,evidenceSignature:'signature',preservedCount:2}],diagnostics:[]});
function fixture(){
  const timers=new Map(),calls=[],toasts=[];let timer=0,request=0,mode='ok',loaded=document();
  const context=vm.createContext({window:{crypto:{randomUUID:()=>`request-${++request}`}},state:{cases:{active:null,cases:[]}},structuredClone,TextEncoder,
    JSON:{stringify:JSON.stringify,parse(){throw Error('native hooks must never parse records or the save string');}},
    document:{dispatchEvent(){}},CustomEvent:class{constructor(type,options){this.type=type;this.detail=options.detail;}},
    setTimeout:callback=>{const id=++timer;timers.set(id,callback);return id;},clearTimeout:id=>timers.delete(id),toast:text=>toasts.push(text),
    api:async(command,args={})=>{calls.push({command,args:structuredClone(args)});
      if(command==='cases_load_view'){if(mode==='load-failure')throw Error('native unavailable');return structuredClone(loaded);}
      if(command==='cases_save_view'){
        if(mode==='save-failure')throw Error('disk failure');const doc=JSON.parse(args.request.documentJson),store=stamp(String(BigInt(args.request.expectedStore.revision)+1n));
        return{requestId:args.request.requestId,committedStore:store,currentStore:store,evidence:doc.cases.flatMap(c=>(c.items||[]).map(i=>i.rows.reference)),analysisContexts:[contextSnapshot],caseEvidence:doc.caseEvidence,replayed:false,reconcileRequired:false};
      }
      throw Error(`unexpected legacy transport: ${command}`);
    }});
  for(const name of ['case-evidence.js','case-evidence-session.js'])vm.runInContext(read(name),context);
  context.window.CaseEvidence.active=true;
  context.window.AnalysisContexts={activate(){},capture:id=>({instance:context.state.cases.cases.find(c=>c.id===id)}),adopt:(snapshot,{owner})=>{if(context.state.cases.cases.includes(owner.instance))owner.instance.analysisContext=structuredClone(snapshot);}};
  vm.runInContext(part('// Native evidence remains opt-in','let casesSaveQueue ='),context);
  vm.runInContext(part('function normalizeCaseStore(','function activeCase('),context);
  vm.runInContext(part('let casesSaveQueue =','function defaultCaseWorkspace('),context);
  return{context,calls,toasts,mode:value=>{mode=value;},loaded:value=>{loaded=value;},flush:()=>{const pending=[...timers.values()];timers.clear();pending.forEach(fn=>fn());}};
}

test('installed native load helper retains every descriptor and opaque metadata without a legacy fallback',async()=>{
  const f=fixture(),doc=document(),loaded=await f.context.loadCaseStore();
  assert.deepEqual(JSON.parse(JSON.stringify(loaded)),doc);assert.equal(f.context.state.cases,loaded);
  assert.deepEqual(f.calls.map(c=>c.command),['cases_load_view']);
  f.mode('load-failure');await assert.rejects(f.context.loadCaseStore(),/native unavailable/);
  assert.equal(f.context.state.cases,loaded);assert.ok(f.calls.every(c=>c.command==='cases_load_view'));
});

test('installed saveCases coalesces native metadata and reports durable failure without JS record serialization',async()=>{
  const f=fixture();await f.context.loadCaseStore();const c=f.context.state.cases.cases[0];c.notes='authored';
  const one=f.context.saveCases(),two=f.context.saveCases();f.flush();assert.deepEqual(await Promise.all([one,two]),[true,true]);
  const saves=f.calls.filter(c=>c.command==='cases_save_view');assert.equal(saves.length,1);
  assert.equal(typeof saves[0].args.request.documentJson,'string');assert.equal(Object.hasOwn(saves[0].args.request,'document'),false);
  assert.equal(JSON.parse(saves[0].args.request.documentJson).cases[0].items[0].rows.kind,'native_evidence_container');
  assert.equal(f.context.state.cases.cases[0],c);assert.equal(c.notes,'authored');
  f.mode('save-failure');c.notes='unsaved draft';const failed=f.context.saveCases();f.flush();assert.equal(await failed,false);
  assert.equal(c.notes,'unsaved draft');assert.match(f.toasts.at(-1),/rascunho foi mantido/);
});

test('native normalization preserves an issued unavailable stub and rejects inline record arrays',()=>{
  const f=fixture(),doc=document(),diagnostic={caseId:'case',owner,code:'UNSUPPORTED_METADATA',message:'preserved',preservedCount:2,readiness:'preserved_only'};
  doc.cases=[{kind:'preserved_case_unavailable',id:'case',code:diagnostic.code}];doc.caseEvidence=[{state:'unavailable',...diagnostic}];doc.diagnostics=[diagnostic];
  const loaded=f.context.normalizeCaseStore(doc);assert.deepEqual(JSON.parse(JSON.stringify(loaded.cases)),doc.cases);
  assert.equal(Object.hasOwn(loaded.cases[0],'items'),false,'an unavailable Case is never normalized into an empty Case');
  const bad=document();bad.cases[0].items[0].rows=[{id:0,fields:{unsafe:1}}];assert.throws(()=>f.context.normalizeCaseStore(bad));
  assert.throws(()=>f.context.normalizeCaseStore({cases:[],active:null}),/VIEW_REQUIRED/);
});

test('actual native bootstrap loads authority without scheduling a legacy record save',async()=>{
  const f=fixture(),bootstrap=app.slice(app.indexOf('window.workspaceBootstrap ='),app.indexOf('// ==========================================================================\n// DASHBOARD'));
  f.context.document.readyState='complete';Object.assign(f.context,{renderCaseBar(){},updateAnalysisBadge(){},
    activeCase:()=>f.context.state.cases.cases[0],loadDerivedFields:async()=>true,setAnalysisView(){},syncActiveCaseArtifacts:async()=>{}});
  f.context.window.WorkspaceContext={initialize:async()=>{}};
  vm.runInContext(bootstrap,f.context);await f.context.window.workspaceBootstrap;f.flush();await Promise.resolve();
  assert.deepEqual(f.calls.map(call=>call.command),['cases_load_view']);
  assert.equal(f.context.state.cases.cases[0].items[0].rows.kind,'native_evidence_container');
});

test('actual saveCases intent protects the draft before its debounce timer reaches the session',async()=>{
  const f=fixture();await f.context.loadCaseStore();const current=f.context.state.cases;current.cases[0].notes='pending timer draft';
  const saving=f.context.saveCases();await assert.rejects(f.context.loadCaseStore(),/DRAFT_PENDING/);
  assert.equal(f.context.state.cases,current);assert.equal(current.cases[0].notes,'pending timer draft');f.flush();assert.equal(await saving,true);
});

test('native MCP reload installs after the actual sourceChanged capture and preserves remote workspace state',async()=>{
  const f=fixture();await f.context.loadCaseStore();const old=f.context.state.cases,remote=document();
  remote.cases[0].workspace={contextStates:{dataset:{queryDraft:{value:'remote saved query'}}},activeScope:'dataset'};f.loaded(remote);
  const workspace=read('workspace-context.js'),section=(start,end)=>workspace.slice(workspace.indexOf(start),workspace.indexOf(end,workspace.indexOf(start)));
  const ctx=f.context;Object.assign(ctx,{stateKeys:[],runtimeKeys:[],scrollSelectors:[],states:new Map(),runtime:new Map(),copy:structuredClone,
    cubeState:{collapsed:new Set()},record:value=>value||{},defaultCaseWorkspace:()=>({}),activeCase:()=>ctx.state.cases.cases[0],key:()=>`case:dataset`,
    $:()=>({value:'old workspace query'}),renderCaseBar(){},updateAnalysisBadge(){},loadDerivedFields:async()=>{},syncActiveCaseArtifacts:async()=>{},initialize:async()=>{}});
  ctx.state.treeCollapsed=new Set();ctx.document.body={dataset:{page:'explore',density:'comfortable',wrap:'false'}};
  ctx.document.querySelector=selector=>selector==='.shell'?{classList:{contains:()=>false}}:null;
  vm.runInContext('let scope="dataset", sourceBusy=0,sourceQueue=Promise.resolve(),restoringCase=false,initialized=true,caseGeneration=0,generation=0,detailRequest=0;',ctx);
  vm.runInContext(section('  function capture()', '  // Loaded source'),ctx);
  vm.runInContext(section('  function sourceChanged(', '  let membershipSignature'),ctx);
  ctx.window.WorkspaceContext={ready:true,replaceCases:ctx.replaceCases};
  vm.runInContext(part('async function mcpReloadCases()', '// ------------------------------------------------------------------ teclado'),ctx);
  let release;const paused=new Promise(resolve=>release=resolve),sourceJob=ctx.sourceChanged(()=>paused);await Promise.resolve();const reload=ctx.mcpReloadCases();
  for(let i=0;i<30;i++)await Promise.resolve();assert.equal(ctx.state.cases,old,'native read does not install before the source job ends');
  release();await Promise.all([sourceJob,reload]);
  assert.equal(ctx.state.cases.cases[0].workspace.contextStates.dataset.queryDraft.value,'remote saved query');
  assert.equal(f.toasts.length,0);
});
