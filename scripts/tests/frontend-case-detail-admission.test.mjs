import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const journeys=readFileSync(new URL('../../frontend/journeys.js',import.meta.url),'utf8');
const section=(start,end)=>source.slice(source.indexOf(start),source.indexOf(end,source.indexOf(start)));
const plain=value=>JSON.parse(JSON.stringify(value));
const settle=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
function fixture(scope='case'){
  const nodes=new Map(),requests=[],preparations=[],shown=[],copied=[],messages=[],cancelled=[];
  let evidence=[{id:0,event_ref:'source:original',fields:{overlay:'saved-A'},raw:'preserved body'}],preparation=null;
  const state={rows:[],detailId:null,currentDetailEv:null,datasetRevision:0,refreshVersion:0,caseProfiles:{},caseTreeProfiles:{},
    owner:{caseId:'a',instance:1,identity:{caseId:'a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-7'}};
  const $=key=>{if(!nodes.has(key))nodes.set(key,{hidden:true,textContent:'',innerHTML:'',classList:{add(){},remove(){}},replaceChildren(){}});return nodes.get(key);};
  const capture=()=>structuredClone(state.owner),context=vm.createContext({state,$,structuredClone,clearTimeout(){},
    window:{AnalysisContexts:{capture,isCurrent:owner=>JSON.stringify(owner)===JSON.stringify(state.owner)},Tasks:{cancelLatest:key=>cancelled.push(key)}},
    workspaceScope:()=>scope,caseEvents:()=>evidence,caseSig:()=>JSON.stringify(evidence),
    document:{querySelectorAll:()=>[]},closeDetailValue(){},toast:message=>messages.push(message),
    showDetailLoading(){state.detailId=null;state.currentDetailEv=null;state.detailAdmission=null;$('#drawer').hidden=false;$('#pane-overview').textContent='loading';},
    showDetail(event,sourceSpec=null,admission=null){vm.runInContext('detailRequest++',context);state.detailId=event.id;state.currentDetailEv=event;state.detailSourceSpec=sourceSpec;state.detailAdmission=admission;shown.push(event);},
    caseArgs:async (args,_retry,_publication,options)=>{preparations.push({args,options});if(preparation)await preparation;const{caseEvents,...rest}=args;return{...rest,caseKey:'captured-case',caseContentToken:'captured-token'};},
    api:(cmd,args,opts)=>{const pending=deferred();requests.push({cmd,args,opts,...pending});return pending.promise;},
    navigator:{clipboard:{writeText:async text=>copied.push(text)}},activeStation:()=>null,activeCase:()=>null,
    debounceTimer:null,filterCountsTimer:null,explorerAnalytics:new Map(),filterCountsCache:new Map(),explorerIntent:'',explorerCursorKey:'',explorerCursors:[],
    treeAggVersion:{dataset:0,case:0},cubeState:{requestVersion:0,results:new Map()},chart:null,
  });
  vm.runInContext(section('let detailRequest =','// abre o drawer imediatamente'),context);
  vm.runInContext(section('function closeDrawer()','function switchDetailTab('),context);
  vm.runInContext(section('function openRightInspector()','// ------------------------------------------------------------------ códigos'),context);
  vm.runInContext(section('function invalidateAnalysisComputedData(','function showQueryEngine('),context);
  return{state,context,$,requests,preparations,shown,copied,messages,cancelled,capture,evidence:()=>evidence,setEvidence:value=>{evidence=value;},scope:value=>{scope=value;},prepare:value=>{preparation=value;}};
}

test('Case analysis detail uses one admitted ID lookup and displays only the returned current overlay',async()=>{
  const f=fixture(),before=structuredClone(f.evidence()),read=f.context.openDetail(0,{eventRef:'source:original'});await settle();
  assert.equal(f.requests.length,1);const request=f.requests[0];
  assert.equal(request.cmd,'event_detail');assert.equal(request.args.caseKey,'captured-case');assert.equal(request.args.caseEvents,undefined);
  assert.equal(request.opts.caseEvents,f.evidence());assert.equal(request.opts.latest,'event-detail');
  assert.deepEqual(plain(request.args.analysisContext),f.state.owner.identity);assert.equal(request.args.sourceGeneration,7);
  assert.equal(request.args.id,0);assert.equal(request.args.eventRef,'source:original');assert.equal(request.args.caseContentToken,'captured-token');assert.equal(f.preparations[0].options.canonical,true);
  const event={id:0,event_ref:'source:original',fields:{overlay:'current-B'},raw:'full body'.repeat(10000),evidence_provenance:{source:'proof'}};
  request.resolve(event);assert.equal(await read,true);assert.equal(f.shown[0],event);assert.deepEqual(f.evidence(),before);
  assert.equal(f.state.detailAdmission.scope,'case');await f.context.copyDetail();
  assert.deepEqual(JSON.parse(f.copied[0]),event);assert.equal(f.requests.length,1,'copy preserves displayed Case identity without a Dataset refetch');
});

test('hidden, absent, malformed and failed detail responses never fall back to saved evidence',async()=>{
  for(const result of [null,{id:99,fields:{secret:'wrong'}},{},[],new Error('visibility admission failed')]){
    const f=fixture(),read=f.context.openDetail(0);await settle();
    if(result instanceof Error)f.requests[0].reject(result);else f.requests[0].resolve(result);assert.equal(await read,false);
    assert.equal(f.shown.length,0);assert.equal(f.state.currentDetailEv,null);assert.doesNotMatch(f.$('#pane-overview').textContent,/saved-A|preserved body/);
    assert.match(f.$('#pane-overview').textContent,/não está disponível|Não foi possível/);
  }
});

test('a supplied detail guard rejects before opening, preparation or cancellation',async()=>{
  const f=fixture();
  assert.equal(await f.context.openDetail(0,{eventRef:'source:original',guard:()=>false}),false);
  assert.equal(f.$('#drawer').hidden,true);assert.equal(f.preparations.length,0);assert.equal(f.requests.length,0);
  assert.equal(f.shown.length,0);assert.equal(f.cancelled.length,0);
});

test('a supplied detail guard invalidates Case preparation and Case or Dataset native replies',async()=>{
  for(const stage of ['Case preparation','Case reply','Dataset reply']){
    const f=fixture(stage==='Dataset reply'?'dataset':'case'),gate=deferred();let current=true;
    if(stage==='Case preparation')f.prepare(gate.promise);
    const read=f.context.openDetail(0,{eventRef:'source:original',guard:()=>current});await settle();current=false;
    if(stage==='Case preparation'){
      assert.equal(f.preparations.length,1);assert.equal(f.requests.length,0);gate.resolve();
    }else{
      assert.equal(f.requests.length,1);
      const event={id:0,event_ref:'source:original',fields:{overlay:'late-overlay'}};
      f.requests[0].resolve(event);
    }
    assert.equal(await read,false,stage);assert.equal(f.shown.length,0,stage);assert.equal(f.state.currentDetailEv,null,stage);
    assert.doesNotMatch(f.$('#pane-overview').textContent,/late-overlay|saved-A|preserved body/);
    if(stage==='Case preparation')assert.equal(f.requests.length,0,'expired journey guard prevents detail dispatch');
  }
});

test('expected detail identity rejects changed or missing event references and wrong IDs in both areas',async()=>{
  for(const scope of ['case','dataset'])for(const event of [
    {id:0,event_ref:'source:replacement'}, {id:0}, {id:99,event_ref:'source:original'},
  ]){
    const f=fixture(scope),read=f.context.openDetail(0,{eventRef:'source:original'});await settle();
    f.requests[0].resolve(event);
    assert.equal(await read,false,`${scope}: ${JSON.stringify(event)}`);assert.equal(f.shown.length,0);
    assert.equal(f.state.currentDetailEv,null);assert.match(f.$('#pane-overview').textContent,/Não foi possível/);
  }
});

test('Case detail rejects late Case, source, revision, evidence, area, close and replacement responses',async()=>{
  for(const change of ['case','source','revision','evidence','scope','close','replace']){
    const f=fixture(),read=f.context.openDetail(0);await settle();
    if(change==='case')f.state.owner.instance++;if(change==='source')f.state.owner.sourceKey='source-8';
    if(change==='revision')f.state.owner.identity.visibilityRevision++;if(change==='evidence')f.setEvidence([]);
    if(change==='scope')f.scope('dataset');if(change==='close')f.context.closeDrawer();
    let next;if(change==='replace'){next=f.context.openDetail(1);await settle();}
    f.requests[0].resolve({id:0,message:'obsolete'});await read;
    assert.equal(f.shown.length,0,change);assert.doesNotMatch(f.$('#pane-overview').textContent,/obsolete/);
    if(next){f.requests[1].resolve(null);await next;}
  }
});

test('stale Case preparation rejects dispatch and Dataset detail also captures ownership',async()=>{
  const f=fixture(),gate=deferred();f.prepare(gate.promise);const read=f.context.openDetail(0);await settle();
  f.state.owner.identity.configRevision++;gate.resolve();await read;assert.equal(f.requests.length,0);assert.match(f.$('#pane-overview').textContent,/contexto mudou/);
  const g=fixture('dataset'),pending=g.context.openDetail(7);await settle();
  assert.equal(g.requests[0].cmd,'event_detail');assert.deepEqual(plain(g.requests[0].args.analysisContext),g.state.owner.identity);
  g.state.owner.sourceKey='other-source';g.requests[0].resolve({id:7,message:'stale dataset'});await pending;assert.equal(g.shown.length,0);
});

test('reopening analysis details re-admits them and revision invalidation removes stale drawer content',async()=>{
  const f=fixture(),read=f.context.openDetail(0);await settle();f.requests[0].resolve({id:0,fields:{overlay:'B'}});await read;
  f.context.closeDrawer();f.context.openRightInspector();await settle();assert.equal(f.requests.length,2);assert.equal(f.shown.length,1);
  f.requests[1].resolve(null);await settle();assert.equal(f.shown.length,1);assert.match(f.$('#pane-overview').textContent,/não está disponível/);
  const next=f.context.openDetail(0);await settle();f.requests[2].resolve({id:0,fields:{overlay:'B'}});await next;
  f.state.owner.identity.visibilityRevision++;await f.context.copyDetail();assert.equal(f.copied.length,0);assert.match(f.messages.at(-1),/contexto mudou/);
  f.context.invalidateAnalysisComputedData({caseId:'a'});assert.equal(f.state.currentDetailEv,null);assert.equal(f.state.detailAdmission,null);assert.equal(f.$('#drawer').hidden,true);
});

test('explicit saved-evidence detail stays separate and copies its complete original event',async()=>{
  const f=fixture(),saved={id:34,raw:'saved full original',fields:{original:null},derived_originals:{original:{state:'present',value:null}}};
  f.context.showDetail(saved,{kind:'saved-evidence'});f.state.owner.identity.visibilityRevision++;
  f.context.invalidateAnalysisComputedData({caseId:'a'});assert.equal(f.state.currentDetailEv,saved,'analysis invalidation does not rewrite an explicit preserved-evidence view');
  f.context.closeDrawer();f.context.openRightInspector();assert.equal(f.requests.length,0);assert.equal(f.state.currentDetailEv,saved);
  await f.context.copyDetail();assert.deepEqual(JSON.parse(f.copied[0]),saved);
  const workspace=readFileSync(new URL('../../frontend/workspace.js',import.meta.url),'utf8');
  assert.match(workspace,/if \(action === "evidence-event"\) \{ showDetail\(item\.rows\[0\], item\.sourceSpec\); \}/);
});

// Exercise the installed journey rendering and row callback with the real
// openDetail admission helper above; replace only DOM rendering and native I/O.
function journeyFixture(){
  const f=fixture(),nodes=[];
  const el=(tag,cls='',text='')=>{
    const node={tag,cls,textContent:text,children:[],style:{setProperty(){}},classList:{add(){}},setAttribute(){},append(...children){this.children.push(...children);}};
    nodes.push(node);return node;
  };
  const detail=el('div','journey-detail'),view={scope:'case',token:1,detailToken:0,field:'correlation',selected:{value:'job-1'},detailPage:0,detailScroll:0};
  Object.assign(f.context,{view,generation:1,drawerSource:null,el,host:{isConnected:true,querySelector:()=>detail},
    globalScope:f.context.workspaceScope,base:()=>({caseEvents:f.evidence(),filters:[]}),timeArgs:()=>({}),heuristic:()=>false,
    busy(){},pager(){},backendFilters:()=>[],fmtNum:String,duration:String,fullTime:String,levelColor:()=>'',
    failure(_node,error){f.messages.push(String(error));},
  });
  f.context.document.body={dataset:{page:'journeys'}};
  vm.runInContext(journeys.slice(journeys.indexOf('  const button ='),journeys.indexOf('  const base =')),f.context);
  vm.runInContext(journeys.slice(journeys.indexOf('  async function loadDetail()'),journeys.indexOf('  async function render(')),f.context);
  return{...f,view,rows:()=>nodes.filter(node=>node.cls==='journey-record')};
}

async function renderJourney(f){
  const read=f.context.loadDetail();await settle();assert.equal(f.requests.length,1);assert.equal(f.requests[0].cmd,'journey_events');
  assert.deepEqual(plain(f.requests[0].opts.analysisOwner),f.capture(),'journey read belongs to its captured Case analysis owner');
  f.requests[0].resolve({total:1,rows:[{id:0,event_ref:'source:original',timestamp:null,fields:{overlay:'preview'}}]});
  await read;assert.equal(f.rows().length,1);return f.rows()[0];
}

test('live Case journey row re-admits its identity and shows only the current detail overlay',async()=>{
  const f=journeyFixture(),before=structuredClone(f.evidence()),row=await renderJourney(f),click=row.onclick();await settle();
  assert.equal(f.requests.length,2,'click performs current visibility and overlay admission');
  assert.equal(f.requests[1].cmd,'event_detail');assert.equal(f.shown.length,0,'neither saved evidence nor the journey preview opens directly');
  const event={id:0,event_ref:'source:original',raw:'complete admitted body',fields:{overlay:'current-overlay'}};
  f.requests[1].resolve(event);await click;
  assert.equal(f.shown[0],event);assert.equal(f.state.detailAdmission.scope,'case');assert.equal(f.context.drawerSource.event,event);
  assert.equal(f.context.drawerSource.scope,'case');assert.deepEqual(f.evidence(),before);
});

test('live Case journey row rejects hidden or replacement records without showing historical evidence',async()=>{
  for(const event of [null,{id:0,event_ref:'source:replacement',fields:{overlay:'unrelated'}}]){
    const f=journeyFixture(),row=await renderJourney(f),click=row.onclick();await settle();
    assert.equal(f.requests.length,2);f.requests[1].resolve(event);await click;
    assert.equal(f.shown.length,0);assert.equal(f.state.currentDetailEv,null);assert.equal(f.context.drawerSource,null);
  }
});

test('journey ownership is captured before loading and guards stale row clicks before detail dispatch',async()=>{
  const changes=[
    ['Case',f=>{f.state.owner.instance++;}],['source',f=>{f.state.owner.sourceKey='source-8';}],
    ['configuration',f=>{f.state.owner.identity.configRevision++;}],['visibility',f=>{f.state.owner.identity.visibilityRevision++;}],
    ['evidence',f=>{f.setEvidence([{id:0,event_ref:'source:replacement'}]);}],
    ['scope',f=>{f.scope('dataset');}],['view',f=>{f.view.detailToken++;}],
  ];
  for(const [name,change] of changes){
    const loading=journeyFixture(),read=loading.context.loadDetail();await settle();change(loading);
    loading.requests[0].resolve({total:1,rows:[{id:0,event_ref:'source:original'}]});await read;
    assert.equal(loading.rows().length,0,`${name}: stale journey response never installs actionable rows`);
    const f=journeyFixture(),row=await renderJourney(f);change(f);await row.onclick();await settle();
    assert.equal(f.requests.length,1,`${name}: stale row never dispatches detail`);assert.equal(f.preparations.length,0,name);
    assert.equal(f.shown.length,0,name);assert.equal(f.$('#drawer').hidden,true,name);
  }
});

test('changing the journey selection during detail admission prevents a late drawer result',async()=>{
  const f=journeyFixture(),row=await renderJourney(f),click=row.onclick();await settle();assert.equal(f.requests.length,2);
  f.view.detailToken++;f.requests[1].resolve({id:0,event_ref:'source:original',fields:{overlay:'obsolete'}});await click;
  assert.equal(f.shown.length,0);assert.equal(f.context.drawerSource,null);assert.equal(f.state.currentDetailEv,null);
});

test('explicit saved-trail callback retains the original historical event without live admission',async()=>{
  const f=fixture(),saved={id:0,event_ref:'source:original',raw:'historical complete body',fields:{overlay:'preserved'}};
  Object.assign(f.context,{caseTimelineCallbacks:{},drawerSource:null});
  const callback=journeys.split('\n').find(line=>line.includes('caseTimelineCallbacks.detail ='));
  assert.ok(callback,'the installed saved-trail detail callback remains available');vm.runInContext(callback,f.context);
  f.state.owner.identity.visibilityRevision++;f.context.caseTimelineCallbacks.detail(saved);
  assert.equal(f.shown[0],saved);assert.equal(f.state.detailAdmission,null);assert.equal(f.requests.length,0);assert.equal(f.preparations.length,0);
  assert.equal(f.context.drawerSource.event,saved);assert.equal(f.context.drawerSource.scope,'case');
  await f.context.copyDetail();assert.deepEqual(JSON.parse(f.copied[0]),saved);
});
