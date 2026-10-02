import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'),app=read('app.js'),workspace=read('workspace-context.js'),pageSource=read('workspace.js');
const part=(source,start,end)=>source.slice(source.indexOf(start),source.indexOf(end,source.indexOf(start)));
const stub=()=>({kind:'preserved_case_unavailable',id:'preserved',code:'CASE_EVIDENCE_UNSUPPORTED_METADATA'});
const contextSnapshot={schemaVersion:1,caseId:'normal',analysisId:'normal-analysis',configRevision:0,visibilityRevision:0,config:{derivedFields:[],references:[]},migrationDiagnostics:[]};
const normal=()=>({id:'normal',name:'Normal',notes:'old',items:[],analysisContext:structuredClone(contextSnapshot)});
const documentValue=()=>{const unavailable={state:'unavailable',caseId:'preserved',owner:null,code:stub().code,message:'Metadados não representáveis; originais preservados',preservedCount:7,readiness:'preserved_only'};return{evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},active:'preserved',cases:[stub(),normal()],caseEvidence:[unavailable,{state:'ready',owner:{storeId:'store',caseId:'normal',analysisId:'normal-analysis'},evidenceSignature:'normal-signature',preservedCount:0}],diagnostics:[Object.fromEntries(Object.entries(unavailable).filter(([key])=>key!=='state'))]};};
function fixture(){
  const calls=[],nodes=new Map(),timers=new Map(),events=[],routes=[];let timer=0,request=0;
  const node=(key,text='')=>{if(!nodes.has(key))nodes.set(key,{textContent:text,innerHTML:'',value:'',hidden:false,children:[],dataset:{},classList:{contains:()=>false,add(){},remove(){},toggle(){}},setAttribute(){},append(...values){this.children.push(...values);},appendChild(value){this.append(value);return value;},replaceChildren(...values){this.children=values;},dispatchEvent(){},focus(){}});return nodes.get(key);};
  const state={cases:documentValue(),treeCollapsed:new Set(),rows:[],derivedFields:[],caseTreeProfiles:{},stationAnalyticsId:null,refreshVersion:0,filters:[],quick:''};
  const ctx=vm.createContext({window:{crypto:{randomUUID:()=>`request-${++request}`}},state,structuredClone,TextEncoder,fmtNum:String,countLabel:(n,text)=>`${n} ${text}`,
    document:{readyState:'complete',body:{dataset:{}},documentElement:{dataset:{}},querySelector:selector=>selector==='.shell'?node('shell'):null,querySelectorAll:()=>[],dispatchEvent:event=>events.push(event)},CustomEvent:class{constructor(type,options){this.type=type;this.detail=options?.detail;}},Event:class{},
    $:node,el:(_tag,_class,text)=>node(Symbol(),text),toast:text=>calls.push(['toast',text]),renderStationShortcuts(){},renderArtifactBar(){},closeDrawer(){},setAnalysisView(){},defaultCaseWorkspace:()=>({}),workspaceScope:()=> 'case',
    setTimeout:fn=>{const id=++timer;timers.set(id,fn);return id;},clearTimeout:id=>timers.delete(id),
    api:async(command,args)=>{calls.push([command,args]);if(command==='cases_load_view')return documentValue();if(command==='cases_save_view'){
      const doc=JSON.parse(args.request.documentJson),store={...doc.store,revision:String(BigInt(doc.store.revision)+1n)};
      return{requestId:args.request.requestId,committedStore:store,currentStore:store,evidence:[],analysisContexts:[contextSnapshot],caseEvidence:doc.caseEvidence,replayed:false,reconcileRequired:false};
    }assert.fail(`unavailable Case issued ordinary command ${command}`);},
  });
  for(const file of ['case-evidence.js','case-evidence-session.js','analysis-context.js'])vm.runInContext(read(file),ctx);ctx.window.CaseEvidence.active=true;
  vm.runInContext(part(app,'function activeCase()','function newCase('),ctx);
  vm.runInContext(part(app,'function savedFilters()','function sameFilters('),ctx);vm.runInContext(part(app,'function caseStations()','// Chain of custody:'),ctx);vm.runInContext(part(app,'function registerCurrentArtifact(','\nfunction '),ctx);
  vm.runInContext(part(app,'function caseItems()','function renderCaseBar('),ctx);vm.runInContext(part(app,'function saveCaseWorkspace(','function normalizeCaseStore('),ctx);
  vm.runInContext(part(app,'function updateContextBar()','const esc ='),ctx);Object.assign(ctx,{STANDARD:[],caseTreeProfilesPeek:()=>[]});vm.runInContext(part(app,'function caseAnalysisSummary(','function caseEvents('),ctx);
  vm.runInContext(part(app,'// Native evidence remains opt-in','let casesSaveQueue ='),ctx);vm.runInContext(part(app,'function normalizeCaseStore(','function activeCase('),ctx);vm.runInContext(part(app,'let casesSaveQueue =','function defaultCaseWorkspace('),ctx);
  return{ctx,calls,nodes,routes,node,flush(){const values=[...timers.values()];timers.clear();values.forEach(fn=>fn());},stub:()=>ctx.state.cases.cases.find(c=>c.id==='preserved')};
}
const exact=f=>{assert.deepEqual(Object.keys(f.stub()).sort(),['code','id','kind']);f.ctx.window.CaseEvidence.validate.document(f.ctx.state.cases);};
function contextRoutes(f){
  const ctx=f.ctx;Object.assign(ctx,{stateKeys:[],runtimeKeys:[],scrollSelectors:[],states:new Map(),runtime:new Map(),copy:structuredClone,record:value=>value||{},cubeState:{collapsed:new Set()},defaults:()=>({page:'summary',values:{},workspace:{}}),key:value=>`${ctx.state.cases.active}:${value||'case'}`,
    updateToggle(){},renderCaseBar(){},resetCaseSourceState(){},loadDerivedFields:async()=>f.calls.push(['loadDerivedFields',ctx.activeCase()?.id]),syncActiveCaseArtifacts:async()=>f.calls.push(['syncActiveCaseArtifacts',ctx.activeCase()?.id]),refresh:async()=>f.calls.push(['refresh',ctx.activeCase()?.id])});
  ctx.window.Workspace={showPage:async page=>f.routes.push([ctx.activeCase()?.id,page]),capture:()=>({})};ctx.window.Tasks={cancelStaleAnalysis(){}};
  vm.runInContext('let scope="case",changing=false,generation=0,restoringCase=false,initialized=false,caseGeneration=0,sourceBusy=0,sourceQueue=Promise.resolve(),caseReturnScope="dataset",detailRequest=0;',ctx);
  vm.runInContext(part(workspace,'  function capture()','  // Loaded source'),ctx);vm.runInContext(part(workspace,'  function sourceRuntime(','  function stored('),ctx);
  vm.runInContext(part(workspace,'  async function showUnavailable()','  async function changeCase('),ctx);const actualScope=ctx.setScope;
  ctx.setScope=async(...args)=>{if(ctx.activeCase()?.kind==='preserved_case_unavailable')return actualScope(...args);ctx.nextScope=args[0];vm.runInContext('scope=nextScope;',ctx);};
  vm.runInContext(part(workspace,'  async function changeCase(','  function beforeCaseCreation('),ctx);
  vm.runInContext(part(workspace,'  async function initialize()','  let membershipSignature'),ctx);
  ctx.window.WorkspaceContext={initialize:ctx.initialize,refreshMembership(){},ready:true};return ctx;
}

test('unavailable Case read helpers and badge do not attach defaults or interpret preserved records as empty authority',()=>{
  const f=fixture();f.ctx.updateAnalysisBadge();assert.match(f.node('#analysis-count').textContent,/indisponíveis.*preservados/);
  assert.deepEqual([...f.ctx.caseItems()],[]);assert.deepEqual([...f.ctx.savedFilters()],[]);assert.deepEqual([...f.ctx.caseStations()],[]);assert.deepEqual([...f.ctx.caseArtifacts()],[]);
  f.ctx.saveCaseWorkspace();assert.equal(f.ctx.restoreCaseWorkspace(),false);assert.throws(()=>f.ctx.ensureCase(),/METADATA_UNAVAILABLE/);assert.throws(()=>f.ctx.registerCurrentArtifact(),/METADATA_UNAVAILABLE/);exact(f);
});

test('direct context preparation, refresh and receipt adoption cannot extend the issued stub',async()=>{
  const f=fixture(),contexts=f.ctx.window.AnalysisContexts,owner=contexts.capture();await assert.rejects(contexts.prepare(owner,{metadata:true}),/METADATA_UNAVAILABLE/);await assert.rejects(contexts.refresh('preserved',{owner}),/METADATA_UNAVAILABLE/);
  const result=await contexts.adopt({...contextSnapshot,caseId:'preserved',analysisId:'unavailable-analysis'},{owner});assert.equal(result.reason,'metadata_unavailable');assert.equal(f.calls.length,0);exact(f);
});

test('actual bootstrap selects the preserved management surface without config or artifact requests',async()=>{
  const f=fixture();contextRoutes(f);f.ctx.renderCaseBar=()=>{};
  vm.runInContext(part(app,'window.workspaceBootstrap =','// =========================================================================='),f.ctx);await f.ctx.window.workspaceBootstrap;
  assert.deepEqual(f.calls.map(([name])=>name),['cases_load_view']);assert.deepEqual(f.routes,[['preserved','evidence']]);exact(f);
});

test('normal to stub to normal selection never restores the stub as an analytical Dataset',async()=>{
  const f=fixture(),ctx=contextRoutes(f);ctx.state.cases.active='normal';await ctx.changeCase('preserved');assert.deepEqual(f.routes.at(-1),['preserved','evidence']);exact(f);
  assert.equal(f.calls.some(([name,id])=>['loadDerivedFields','syncActiveCaseArtifacts','refresh'].includes(name)&&id==='preserved'),false);
  await ctx.changeCase('normal');assert.ok(f.calls.some(([name,id])=>name==='loadDerivedFields'&&id==='normal'));exact(f);
});

test('source completion, scope switching and deferred replacement leave exact stub metadata untouched',async()=>{
  const f=fixture(),ctx=contextRoutes(f);await ctx.setScope('dataset',{force:true});await ctx.sourceChanged(async()=>{ctx.state.loaded=true;});ctx.capture();exact(f);
  await ctx.replaceCases(documentValue(),{beforeReplace:()=>exact(f)});exact(f);assert.equal(f.calls.some(([name])=>['loadDerivedFields','syncActiveCaseArtifacts'].includes(name)),false);
});

test('saving another normal Case retains the unchanged unavailable stub while it stays active',async()=>{
  const f=fixture();await f.ctx.loadCaseStore();contextRoutes(f);await f.ctx.initialize();vm.runInContext(part(workspace,'  const originalSave = saveCases;','  updateToggle();'),f.ctx);f.ctx.state.cases.cases[1].notes='authored while preserved Case active';const saving=f.ctx.saveCases();f.flush();assert.equal(await saving,true);exact(f);
  const request=f.calls.find(([name])=>name==='cases_save_view')[1].request,sent=JSON.parse(request.documentJson);assert.deepEqual(sent.cases[0],stub());assert.equal(sent.cases[1].notes,'authored while preserved Case active');assert.equal(f.ctx.state.cases.active,'preserved');
});

test('evidence Timeline and Trails routes render only the nonmutating preserved surface',async()=>{
  const f=fixture(),ctx=f.ctx;const home=f.node('home'),content=f.node('content'),empty=f.node('empty');
  Object.assign(ctx,{home,content,empty,markPage:page=>f.routes.push(page),switchView:view=>assert.equal(view,'workspace'),openExport(){},newCase(){}});vm.runInContext('let navigation=0;',ctx);
  const start=pageSource.indexOf('  async function showPage('),end=pageSource.indexOf('    // Comparar lives',start);vm.runInContext(pageSource.slice(start,end)+'\n}',ctx);
  const render=pageSource.indexOf('  function renderUnavailableCase(');
  const next=pageSource.indexOf('\n  function ',render+5);vm.runInContext(pageSource.slice(render,next),ctx);
  for(const page of ['evidence','case-timeline','case-trails']){await ctx.showPage(page);exact(f);assert.equal(content.children.length,1);assert.match(content.children[0].children[0].textContent,/preservado.*indisponíveis/);}
  const exportButton=content.children[0].children.at(-1).children[0];assert.equal(exportButton.hidden,true);exportButton.onclick();assert.equal(f.calls.length,0);assert.deepEqual(f.routes,['evidence','evidence','evidence']);
});

test('direct and stale Case deletion entry points refuse the exact unavailable stub before any save',async()=>{
  const f=fixture(),ctx=contextRoutes(f),before=structuredClone(ctx.state.cases);
  vm.runInContext(part(workspace,'  async function deleteCase(','  async function initialize('),ctx);vm.runInContext(part(app,'async function deleteActiveCase(','let pendingCaseAdd ='),ctx);
  assert.equal(await ctx.deleteCase(f.stub()),false);assert.equal(await ctx.deleteActiveCase(),false);assert.deepEqual(ctx.state.cases,before);assert.equal(f.calls.some(([name])=>name==='cases_save_view'),false);exact(f);
});

test('saved-view controls are hidden for the stub and a previously opened normal-Case draft cannot write into it',()=>{
  const f=fixture(),ctx=f.ctx;let callback;
  Object.assign(ctx,{updateFilterTabsLayout(){},refreshFilterTabCounts(){},openNamePop:(_button,save)=>{callback=save;}});
  vm.runInContext(part(app,'function renderFilterTabs()','// As abas ficam fixas'),ctx);vm.runInContext(part(app,'function syncCurrentSavedFilter()','function applySavedFilter('),ctx);
  ctx.renderFilterTabs();assert.equal(f.node('#filter-tabs').hidden,true);assert.equal(ctx.syncCurrentSavedFilter(),null);exact(f);
  ctx.state.cases.active='normal';ctx.renderFilterTabs();const add=f.node('#filter-tabs').children.at(-1);add.onclick({stopPropagation(){}});assert.equal(typeof callback,'function');
  ctx.state.cases.active='preserved';callback('Must not be created');exact(f);assert.equal(f.calls.some(([name])=>name==='cases_save_view'),false);assert.doesNotMatch(JSON.stringify(ctx.state.cases.cases[1]),/Must not be created/);
});
