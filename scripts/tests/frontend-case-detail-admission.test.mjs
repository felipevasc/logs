import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const section=(start,end)=>source.slice(source.indexOf(start),source.indexOf(end,source.indexOf(start)));
const plain=value=>JSON.parse(JSON.stringify(value));
const settle=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
function fixture(scope='case'){
  const nodes=new Map(),requests=[],shown=[],copied=[],messages=[],cancelled=[];
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
    caseArgs:async args=>{if(preparation)await preparation;const{caseEvents,...rest}=args;return{...rest,caseKey:'captured-case'};},
    api:(cmd,args,opts)=>{const pending=deferred();requests.push({cmd,args,opts,...pending});return pending.promise;},
    navigator:{clipboard:{writeText:async text=>copied.push(text)}},activeStation:()=>null,activeCase:()=>null,
    debounceTimer:null,filterCountsTimer:null,explorerAnalytics:new Map(),filterCountsCache:new Map(),explorerIntent:'',explorerCursorKey:'',explorerCursors:[],
    treeAggVersion:{dataset:0,case:0},cubeState:{requestVersion:0,results:new Map()},chart:null,
  });
  vm.runInContext(section('let detailRequest =','// abre o drawer imediatamente'),context);
  vm.runInContext(section('function closeDrawer()','function switchDetailTab('),context);
  vm.runInContext(section('function openRightInspector()','// ------------------------------------------------------------------ códigos'),context);
  vm.runInContext(section('function invalidateAnalysisComputedData(','function showQueryEngine('),context);
  return{state,context,$,requests,shown,copied,messages,cancelled,capture,evidence:()=>evidence,setEvidence:value=>{evidence=value;},scope:value=>{scope=value;},prepare:value=>{preparation=value;}};
}

test('Case analysis detail uses one admitted ID lookup and displays only the returned current overlay',async()=>{
  const f=fixture(),before=structuredClone(f.evidence()),read=f.context.openDetail(0);await settle();
  assert.equal(f.requests.length,1);const request=f.requests[0];
  assert.equal(request.cmd,'query_page');assert.equal(request.args.caseKey,'captured-case');assert.equal(request.args.caseEvents,undefined);
  assert.equal(request.opts.caseEvents,f.evidence());assert.equal(request.opts.latest,'event-detail');
  assert.deepEqual(plain(request.args.analysisContext),f.state.owner.identity);assert.equal(request.args.sourceGeneration,7);
  assert.deepEqual(plain(request.args.filters),[{column:'id',op:'equals_exact',value:'0',value2:null}]);assert.equal(request.args.limit,1);
  const event={id:0,event_ref:'source:original',fields:{overlay:'current-B'},raw:'full body',evidence_provenance:{source:'proof'}};
  request.resolve({rows:[event]});await read;assert.equal(f.shown[0],event);assert.deepEqual(f.evidence(),before);
  assert.equal(f.state.detailAdmission.scope,'case');await f.context.copyDetail();
  assert.deepEqual(JSON.parse(f.copied[0]),event);assert.equal(f.requests.length,1,'copy preserves displayed Case identity without a Dataset refetch');
});

test('hidden, absent, malformed and failed detail responses never fall back to saved evidence',async()=>{
  for(const result of [{rows:[]},{rows:[{id:99,fields:{secret:'wrong'}}]},{},new Error('visibility admission failed')]){
    const f=fixture(),read=f.context.openDetail(0);await settle();
    if(result instanceof Error)f.requests[0].reject(result);else f.requests[0].resolve(result);await read;
    assert.equal(f.shown.length,0);assert.equal(f.state.currentDetailEv,null);assert.doesNotMatch(f.$('#pane-overview').textContent,/saved-A|preserved body/);
    assert.match(f.$('#pane-overview').textContent,/não está disponível|Não foi possível/);
  }
});

test('Case detail rejects late Case, source, revision, evidence, area, close and replacement responses',async()=>{
  for(const change of ['case','source','revision','evidence','scope','close','replace']){
    const f=fixture(),read=f.context.openDetail(0);await settle();
    if(change==='case')f.state.owner.instance++;if(change==='source')f.state.owner.sourceKey='source-8';
    if(change==='revision')f.state.owner.identity.visibilityRevision++;if(change==='evidence')f.setEvidence([]);
    if(change==='scope')f.scope('dataset');if(change==='close')f.context.closeDrawer();
    let next;if(change==='replace'){next=f.context.openDetail(1);await settle();}
    f.requests[0].resolve({rows:[{id:0,message:'obsolete'}]});await read;
    assert.equal(f.shown.length,0,change);assert.doesNotMatch(f.$('#pane-overview').textContent,/obsolete/);
    if(next){f.requests[1].resolve({rows:[]});await next;}
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
  const f=fixture(),read=f.context.openDetail(0);await settle();f.requests[0].resolve({rows:[{id:0,fields:{overlay:'B'}}]});await read;
  f.context.closeDrawer();f.context.openRightInspector();await settle();assert.equal(f.requests.length,2);assert.equal(f.shown.length,1);
  f.requests[1].resolve({rows:[]});await settle();assert.equal(f.shown.length,1);assert.match(f.$('#pane-overview').textContent,/não está disponível/);
  const next=f.context.openDetail(0);await settle();f.requests[2].resolve({rows:[{id:0,fields:{overlay:'B'}}]});await next;
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
