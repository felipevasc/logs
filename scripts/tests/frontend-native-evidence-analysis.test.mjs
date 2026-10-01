import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const owner={storeId:'store',caseId:'case',analysisId:'analysis'},identity={caseId:'case',analysisId:'analysis',configRevision:2,visibilityRevision:3};
const snapshot={schemaVersion:1,...identity,config:{derivedFields:[],references:[]},migrationDiagnostics:[]};
const container=(id='a',count=2)=>({kind:'native_evidence_container',reference:{kind:'native_evidence',schemaVersion:1,owner,containerId:id,manifestId:`manifest-${id}`,manifestSha256:'a'.repeat(64),memberCount:count},preservedCount:count,preview:null});
const document=()=>({evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},active:'case',cases:[{id:'case',analysisContext:snapshot,notes:'original',items:[{id:'item',rows:container(),stationId:'station'}]}],caseEvidence:[{state:'ready',owner,evidenceSignature:'native-signature',preservedCount:2}],diagnostics:[]});
const ready=(token='token-1',count=2)=>({state:'ready',publication:{caseKey:'native-key',caseContentToken:token,caseEvidenceSignature:'native-signature',evidenceSignature:'native-station-signature',preservedCount:count,analyticalCount:count}});
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const reached=async predicate=>{for(let i=0;i<40;i++){if(predicate())return;await Promise.resolve();}assert.fail('pending request not reached');};
async function fixture(doc=document()){
  const calls=[];let native=structuredClone(doc),live,station='station',status='ready',serial=0,open=()=>ready(),query=()=>({ok:true});
  const context=vm.createContext({window:{},state:{},structuredClone,TextEncoder,explorerAnalytics:new Map(),toast(){},invoke:async(command,args)=>{
    calls.push({command,args});if(command==='cases_load_view')return structuredClone(native);
    if(command==='case_evidence_open')return open(args.request);
    if(command==='cases_save_view'){
      const doc=JSON.parse(args.request.documentJson),store={...doc.store,revision:String(BigInt(doc.store.revision)+1n)};
      return {requestId:args.request.requestId,committedStore:store,currentStore:store,evidence:doc.cases.flatMap(c=>(c.items||[]).flatMap(i=>i.rows?[i.rows.reference]:[])),analysisContexts:doc.cases.map(c=>c.analysisContext),caseEvidence:doc.caseEvidence,replayed:false,reconcileRequired:false};
    }
    if(command==='case_sync')throw Error('native scopes must never serialize Event arrays');
    return query(command,args);
  }});
  for(const name of ['case-evidence.js','case-evidence-analysis.js'])vm.runInContext(read(name),context);
  context.window.CaseEvidence.active=true;const client=context.window.CaseEvidence.create({invoke:context.invoke,enabled:true,requestId:()=>`request-${++serial}`});live=await client.load();
  const scope=context.window.CaseEvidenceAnalysis,session={status:()=>status};
  const capture=()=>scope.capture({client,session,store:live,item:live.cases[0],stationId:station,currentStore:()=>live,currentCase:()=>live.cases[0],currentStation:()=>station});
  const app=read('app.js');vm.runInContext(app.slice(app.indexOf('const caseTransport ='),app.indexOf('// ------------------------------------------------------------------ helpers de espera')),context);
  Object.assign(context,{activeCase:()=>live.cases[0],nativeEvidenceServices:()=>({client,session}),STANDARD:['timestamp','source','message'],caseTreeProfilesPeek:()=>[]});context.state.cases=live;context.state.stationAnalyticsId=station;
  vm.runInContext(app.slice(app.indexOf('// Track replacement/reordering of immutable evidence'),app.indexOf('function dashboardCharts(')),context);
  return{context,client,scope,calls,capture,live:()=>live,replace:value=>{live=value;context.state.cases=value;},station:value=>{station=value;context.state.stationAnalyticsId=value;},status:value=>{status=value;},native:value=>{native=value;},open:fn=>{open=fn;},query:fn=>{query=fn;}};
}

test('actual caseArgs uses an immutable native scope and never posts preview or Event values',async()=>{
  const f=await fixture(),scope=f.capture(),filters=[{column:'number',op:'eq',value:'1.0'}];
  const args=await f.context.caseArgs({caseEvents:scope,analysisContext:identity,sourceGeneration:99,filters},{},{},{canonical:true});
  assert.equal(args.caseEvents,undefined);assert.equal(args.caseKey,'native-key');assert.equal(args.caseContentToken,'token-1');assert.equal(args.sourceGeneration,null);assert.equal(args.filters,filters);
  assert.equal(f.calls[1].command,'case_evidence_open');assert.deepEqual(JSON.parse(JSON.stringify(f.calls[1].args.request)),{store:{storeId:'store',epoch:'epoch'},analysisContext:identity,stationId:'station',evidenceSignature:'native-signature'});
  assert.equal(Array.isArray(scope),false);assert.ok(Object.isFrozen(scope));assert.equal('rows' in scope,false);
  await assert.rejects(f.context.caseArgs({caseEvents:[],analysisContext:identity}),/NATIVE_SCOPE_REQUIRED/);
  await assert.rejects(f.context.caseArgs({caseEvents:{...scope},analysisContext:identity}),/NATIVE_SCOPE_REQUIRED/);
});

test('ordinary native Case queries omit the canonical token but keep authoritative empty scope',async()=>{
  const doc=document();doc.cases[0].items[0].rows=container('a',0);doc.caseEvidence[0].preservedCount=0;const f=await fixture(doc);f.open(()=>ready('empty-token',0));
  const args=await f.context.caseArgs({caseEvents:f.capture(),analysisContext:identity,sourceGeneration:12});
  assert.equal(args.caseKey,'native-key');assert.equal(args.sourceGeneration,null);assert.equal(args.caseContentToken,undefined);
  assert.equal(f.calls.filter(c=>c.command==='case_evidence_open').length,1);
});

for(const [name,change] of [
  ['Case replacement',f=>f.replace(structuredClone(f.live()))],
  ['store epoch',f=>{f.live().store.epoch='new-epoch';}],
  ['station selection',f=>f.station('other')],
  ['configuration revision',f=>{f.live().cases[0].analysisContext={...snapshot,configRevision:3};}],
  ['visibility revision',f=>{f.live().cases[0].analysisContext={...snapshot,visibilityRevision:4};}],
  ['container membership',f=>{f.live().cases[0].items[0].rows=container('replacement');}],
  ['station metadata',f=>{f.live().cases[0].items[0].stationId='other';}],
  ['artifact metadata',f=>{f.live().cases[0].items[0].artifactId='other';}],
])test(`pending native publication rejects a changed ${name}`,async()=>{
  const f=await fixture(),gate=deferred();f.open(()=>gate.promise);const result=f.context.caseArgs({caseEvents:f.capture(),analysisContext:identity});
  await reached(()=>f.calls.some(c=>c.command==='case_evidence_open'));change(f);gate.resolve(ready());await assert.rejects(result,/VIEW_CHANGED/);
});

test('notes and compatibility preview changes do not replace the native analytical binding',async()=>{
  const f=await fixture(),gate=deferred();f.open(()=>gate.promise);const captured=f.capture(),result=f.context.caseArgs({caseEvents:captured,analysisContext:identity});
  await reached(()=>f.calls.some(c=>c.command==='case_evidence_open'));f.live().cases[0].notes='unsaved note';f.live().cases[0].items[0].label='new label';
  gate.resolve(ready());assert.equal((await result).caseKey,'native-key');assert.equal(f.scope.token(captured),'token-1');
});

test('unsaved analytical container edits are rejected before native publication',async()=>{
  const f=await fixture();f.live().cases[0].items[0].origin='unsaved-origin';assert.throws(f.capture,/SAVE_PENDING/);assert.equal(f.calls.length,1);
});

test('unavailable native Case and materialization budget failures never become Dataset queries',async()=>{
  const f=await fixture();f.open(()=>({state:'unavailable',code:'CASE_MATERIALIZATION_LIMIT',preservedCount:2,materializationLimit:1}));
  await assert.rejects(f.context.api('query_page',{caseEvents:f.capture(),analysisContext:identity},{silent:true}),/2 ocorrências.*preservadas/);
  assert.deepEqual(f.calls.map(c=>c.command),['cases_load_view','case_evidence_open']);
  f.live().caseEvidence[0]={state:'unavailable',caseId:'case',owner,message:'budget',preservedCount:2};assert.throws(f.capture,/EVIDENCE_UNAVAILABLE/);
});

test('failed metadata persistence blocks a native Case open without replacing preserved records',async()=>{
  const f=await fixture(),scope=f.capture();f.status('retry_required');await assert.rejects(f.context.caseArgs({caseEvents:scope,analysisContext:identity}),/RECONCILE_REQUIRED/);assert.equal(f.calls.length,1);
});

test('cancellation after a native publication suppresses the following query',async()=>{
  const f=await fixture(),gate=deferred();let cancelled=false;f.open(()=>gate.promise);
  const result=f.context.api('query_page',{caseEvents:f.capture(),analysisContext:identity},{silent:true,cancelled:()=>cancelled});
  await reached(()=>f.calls.some(c=>c.command==='case_evidence_open'));cancelled=true;gate.resolve(ready());await assert.rejects(result,/VIEW_CHANGED/);
  assert.equal(f.calls.some(c=>c.command==='query_page'),false);
});

test('actual API retry resynchronizes captured native scope once and late misses reuse the newer token',async()=>{
  const f=await fixture(),reads=[],capture=f.capture();let opens=0;f.open(()=>ready(`token-${++opens}`));f.query((command,args)=>{
    assert.equal(command,'event_detail');assert.equal(args.caseEvents,undefined);if(reads.length<2){const gate=deferred();reads.push({gate,id:args.id});return gate.promise;}return{token:args.caseContentToken};
  });
  const args={caseEvents:capture,analysisContext:identity,id:0,eventRef:'original'},a=f.context.api('event_detail',args,{silent:true}),b=f.context.api('event_detail',{...args,id:1},{silent:true});
  await reached(()=>reads.length===2);assert.equal(opens,1);reads.find(r=>r.id===0).gate.reject(Error('CASE_CACHE_MISS'));assert.equal((await a).token,'token-2');reads.find(r=>r.id===1).gate.reject(Error('CASE_CACHE_MISS'));assert.equal((await b).token,'token-2');assert.equal(opens,2);
});

test('preprepared canonical calls retain native scope for missing-cache retry and reject changed-cache errors',async()=>{
  const f=await fixture(),capture=f.capture();let opens=0,reads=0;f.open(()=>ready(`token-${++opens}`));f.query(()=>{if(++reads===1)throw Error('CASE_CACHE_MISS');return{ok:true};});
  const args=await f.context.caseArgs({caseEvents:capture,analysisContext:identity},{},{},{canonical:true});assert.equal((await f.context.api('analysis_field_text',args,{silent:true,caseEvents:capture})).ok,true);assert.equal(opens,2);
  f.query(()=>{throw Error('CASE_CACHE_CHANGED');});await assert.rejects(f.context.api('event_detail',args,{silent:true,caseEvents:capture}),/CASE_CACHE_CHANGED/);assert.equal(opens,2);
});

test('a new empty Case opens only after its durable native owner receipt and cannot rebind later',async()=>{
  const empty={...document(),active:null,cases:[],caseEvidence:[]},f=await fixture(empty),item={id:'case',items:[],notes:'new'};
  f.live().cases.push(item);f.live().active='case';const capture=f.capture();
  await assert.rejects(f.context.caseArgs({caseEvents:capture,analysisContext:identity}),/VIEW_CHANGED/);assert.equal(f.calls.length,1);
  const ack={...document(),cases:[{...item,analysisContext:snapshot}],caseEvidence:[{state:'ready',owner,evidenceSignature:'native-signature',preservedCount:0}]};
  f.native(ack);await f.client.load();item.analysisContext=snapshot;f.live().caseEvidence=structuredClone(ack.caseEvidence);f.open(()=>ready('token-empty',0));
  assert.equal((await f.context.caseArgs({caseEvents:capture,analysisContext:identity})).caseKey,'native-key');
  item.analysisContext={...snapshot,analysisId:'replacement'};f.live().caseEvidence[0].owner.analysisId='replacement';
  await assert.rejects(f.context.caseArgs({caseEvents:capture,analysisContext:item.analysisContext}),/VIEW_CHANGED/);
});

test('a metadata-only save may advance store revision without republishing unchanged native evidence',async()=>{
  const f=await fixture(),capture=f.capture();await f.context.caseArgs({caseEvents:capture,analysisContext:identity});
  f.live().cases[0].notes='saved note';const result=await f.client.save(f.client.prepareSave(f.live()));f.live().store=structuredClone(result.currentStore);
  assert.equal((await f.context.caseArgs({caseEvents:capture,analysisContext:identity})).caseKey,'native-key');
  assert.equal(f.calls.filter(c=>c.command==='case_evidence_open').length,1);
});

test('installed Case query capture exposes only native scope while raw local readers fail closed',async()=>{
  const f=await fixture();assert.throws(()=>f.context.caseEvents(),/NATIVE_SCOPE_REQUIRED/);assert.throws(()=>f.context.caseEventsCompute(true),/NATIVE_SCOPE_REQUIRED/);
  const evidence=f.context.caseEvents('analysis');assert.equal(f.scope.isCapture(evidence),true);assert.equal(evidence.stationId,'station');
  assert.equal((await f.context.caseArgs({caseEvents:evidence,analysisContext:identity})).caseKey,'native-key');
  const full=f.context.caseEvents('analysis-all');assert.equal(full.stationId,null);f.station('another');assert.equal((await f.context.caseArgs({caseEvents:full,analysisContext:identity})).caseKey,'native-key');
  assert.equal(f.calls.some(c=>c.command==='case_sync'),false);
});

test('installed Case cache signature follows native metadata authority without hashing notes or preview cells',async()=>{
  const f=await fixture(),before=f.context.caseSig();f.live().cases[0].notes='new note';assert.equal(f.context.caseSig(),before);
  f.live().cases[0].items[0].rows.preview={kind:'evidence_preview_page',columns:[],rows:[],total:2,nextCursor:null};assert.equal(f.context.caseSig(),before);
  f.live().cases[0].items[0].origin='new origin';assert.notEqual(f.context.caseSig(),before);
});

test('installed Case counts identify preserved occurrences and keep empty versus unavailable explicit',async()=>{
  const f=await fixture();assert.equal(f.context.caseAnalysisSummary().preservedCount,2);assert.equal(f.context.scopeHasEvents('case'),true);
  f.live().cases[0].items.push({id:'other',stationId:'other',rows:container('b',3)});f.live().caseEvidence[0].preservedCount=5;
  assert.equal(f.context.caseAnalysisSummary().preservedCount,2);assert.equal(f.context.caseAnalysisSummary(true).preservedCount,5);
  f.live().caseEvidence[0]={state:'unavailable',caseId:'case',owner,preservedCount:5,message:'native preserved-only'};
  assert.equal(f.context.caseAnalysisSummary(true).ready,false);assert.equal(f.context.caseAnalysisSummary(true).preservedCount,5);assert.equal(f.context.scopeHasEvents('case'),false);
  assert.equal(f.context.caseAnalysisSummary(true).message,'native preserved-only');
});

test('actual whole-Case threat capture refreshes its owner after config changes while reusing native records',async()=>{
  const f=await fixture(),security=read('security.js');f.context.workspaceScope=()=> 'case';
  vm.runInContext(security.slice(security.indexOf('  let fullCaseKey ='),security.indexOf('  const two =')),f.context);
  const first=f.context.fullRequest();assert.equal(first.caseEvents.stationId,null);await f.context.caseArgs({...first,analysisContext:identity});
  const newer={...snapshot,configRevision:3};f.live().cases[0].analysisContext=newer;const second=f.context.fullRequest();assert.notEqual(first.caseEvents,second.caseEvents);
  assert.equal((await f.context.caseArgs({...second,analysisContext:newer})).caseKey,'native-key');assert.equal(f.calls.filter(c=>c.command==='case_evidence_open').length,1);
});

function unavailable(f) {f.live().caseEvidence[0]={state:'unavailable',caseId:'case',owner,preservedCount:2,message:'Originais preservados; análise indisponível'};}
function controls() {const nodes=new Map(),make=()=>({textContent:'',innerHTML:'',children:[],dataset:{},setAttribute(){},replaceChildren(...values){this.children=values;},append(...values){this.children.push(...values);},appendChild(value){this.children.push(value);},classList:{toggle(){}}});return{nodes,make,get:key=>{if(!nodes.has(key))nodes.set(key,make());return nodes.get(key);}};}

test('actual grouping shows unavailable originals distinctly from a genuine empty Case',async()=>{
  const f=await fixture(),ui=controls();unavailable(f);Object.assign(f.context,{fmtNum:String,groupShell(){},clearTimeout(){},groupTimer:0,groupView:{version:0,result:null,computedKey:''},workspaceScope:()=> 'case',$:ui.get});
  const workbench=read('analysis-workbench.js');vm.runInContext(workbench.slice(workbench.indexOf('  runGroup = async function'),workbench.indexOf('  function filterGroup')),f.context);
  await f.context.runGroup();assert.match(ui.get('#aw-group-summary').textContent,/2 ocorrências preservadas.*indisponível/);assert.doesNotMatch(ui.get('#aw-group-summary').textContent,/Adicione/);
  f.live().cases[0].items[0].rows=container('a',0);f.live().caseEvidence[0]={state:'ready',owner,evidenceSignature:'empty',preservedCount:0};await f.context.runGroup();assert.match(ui.get('#aw-group-summary').textContent,/Adicione registros/);
});

test('actual Discovery and pivot entry points show native unavailability without authoring empty specs',async()=>{
  const f=await fixture(),ui=controls();unavailable(f);Object.assign(f.context,{fmtNum:String,$:ui.get,mount(){},clearCharts(){},scopeKey:()=> 'captured',empty:(node,message)=>{node.textContent=message;},cubeState:{requestVersion:0},activeCube(){assert.fail('unavailable Case must not create a default cube');}});
  vm.runInContext('let generation=0,mode="custom";',f.context);const discovery=read('discovery.js'),start=discovery.indexOf('  renderDashboard = async function');
  vm.runInContext(discovery.slice(start,discovery.indexOf('    if (mode ===',start))+'\n};',f.context);await f.context.renderDashboard('case');assert.match(ui.get('#dash-grid').textContent,/preservadas.*indisponível/);
  const app=read('app.js'),cube=app.indexOf('async function runCube({');vm.runInContext(app.slice(cube,app.indexOf('  const cube = activeCube(scope);',cube))+'\n}',f.context);
  const result=await f.context.runCube();assert.equal(result.status,'unavailable');assert.match(result.error,/preservadas/);assert.equal(f.live().cases[0].caseCube,undefined);
});

test('actual field-profile failure is latched per admission and retries only on a changed scope or explicit control',async()=>{
  const f=await fixture(),ctx=f.context,app=read('app.js'),ui=controls();unavailable(f);
  Object.assign(ctx.state,{caseTreeProfiles:{},caseProfilesLoading:false,filters:[],treeAgg:{case:null},treeAggSig:{},treeAggError:{},datasetRevision:0,derivedFields:[]});
  const box=ui.make();box.dataset.treeScope='case';box.closest=()=>null;
  Object.assign(ctx,{workspaceScope:()=> 'case',treeGroupProfiles:()=>({categories:[],ranges:[]}),backendFilters:()=>[],treeSpin(){},setTimeout(){return 1;},clearTimeout(){},el:(_tag,_class,text)=>({...ui.make(),textContent:text||''}),document:{querySelector:()=>null,querySelectorAll:()=>[box]}});
  vm.runInContext(app.slice(app.indexOf('function caseTreeProfiles()'),app.indexOf('// ------------------------------------------------------------------ matcher local')),ctx);
  vm.runInContext(app.slice(app.indexOf('const treeAggVersion ='),app.indexOf('function includeDiscoveredFields(')),ctx);
  const start=app.indexOf('function renderExploreTreeInto('),end=app.indexOf('  includeDiscoveredFields(profiles, scope);',start);vm.runInContext(app.slice(start,end)+'\n}',ctx);
  let attempts=0;const original=ctx.caseEvents;ctx.caseEvents=(...args)=>{attempts++;assert.ok(attempts<12,'a synchronous refusal must not spin');return original(...args);};
  ctx.renderExploreTree();for(let n=0;n<100;n++)await Promise.resolve();assert.equal(attempts,2);assert.match(ctx.state.caseTreeProfiles.case.error,/UNAVAILABLE/);assert.equal(ctx.state.caseProfilesLoading,false);
  const failure=box.children.find(node=>node.children?.some(child=>child.textContent==='Tentar perfil novamente'));assert.ok(failure);failure.children[0].onclick();for(let n=0;n<100;n++)await Promise.resolve();assert.equal(attempts,4);
  f.live().caseEvidence[0]={state:'ready',owner,evidenceSignature:'native-signature',preservedCount:2};f.query(()=>[]);const admitted=ctx.api;ctx.api=(command,args,options)=>admitted(command,{...args,analysisContext:identity},options);ctx.renderExploreTree();for(let n=0;n<100;n++)await Promise.resolve();assert.equal(ctx.state.caseTreeProfiles.case.error,undefined);assert.ok(f.calls.some(c=>c.command==='profile_fields'));
});
