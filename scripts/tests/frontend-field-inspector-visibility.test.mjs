import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const code=app.slice(app.indexOf('async function showFieldInspector('),app.indexOf('\nfunction showStationInspector('));
const plain=value=>JSON.parse(JSON.stringify(value));
const settle=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
function fixture(scope='case'){
  const nodes=new Map(),views=[],requests=[],cancelled=[];
  let evidence=[{id:1,fields:{target:'visible'}},{id:2,fields:{target:'visible'}},{id:3,fields:{target:'excluded-secret'}}],preparation=null;
  const state={rows:[{fields:{target:'admitted-page'}}],filters:[{column:'environment',op:'equals_exact',value:'prod'}],
    owner:{caseId:'a',instance:1,identity:{caseId:'a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-7'}};
  const node=(tag='',className='',textContent='')=>({tag,className,textContent,hidden:false,value:'',children:[],
    append(...children){this.children.push(...children);},appendChild(child){this.children.push(child);},replaceChildren(...children){this.children=children;}});
  const $=selector=>{if(!nodes.has(selector))nodes.set(selector,node());return nodes.get(selector);};
  const capture=()=>structuredClone(state.owner);
  const context=vm.createContext({state,$,el:node,structuredClone,detailRequest:0,
    window:{AnalysisContexts:{capture,isCurrent:owner=>JSON.stringify(owner)===JSON.stringify(state.owner)},Tasks:{cancelLatest:key=>cancelled.push(key)}},
    workspaceScope:()=>scope,backendFilters:()=>state.filters,caseEvents:()=>evidence,caseSig:()=>JSON.stringify(evidence),
    cellValue:(event,column)=>{assert.equal(scope,'dataset','Case values must only be read by the admitted backend');return event.fields[column];},
    fmtNum:String,colLabel:String,trunc:(value,max)=>value.length>max?`${value.slice(0,max)}…`:value,openFilterPop(){},
    addFilter:filter=>state.filters.push(filter),closeDrawer:()=>{context.detailRequest++;$('#drawer').hidden=true;},
    openContextInspector:(title,subtitle,overview)=>{context.detailRequest++;$('#drawer').hidden=false;views.push({title,subtitle,overview});},
    caseArgs:async args=>{if(preparation)await preparation;if(!args.caseEvents)return args;const{caseEvents,...rest}=args;return{...rest,caseKey:'captured-evidence'};},
    api:(cmd,args,opts)=>{const request=deferred();requests.push({cmd,args,opts,...request});return request.promise;}});
  vm.runInContext(code,context);
  const response=(request=requests.at(-1),values=[['visible',2]],extra={})=>({group_values:values.map(([value])=>value),
    rows:values.map(([value,count])=>({[request.args.groupColumn]:value,[request.args.aggs[0].alias]:count})),omitted_records:0,...extra});
  return{context,state,requests,views,cancelled,response,$,capture,evidence:()=>evidence,setEvidence:value=>{evidence=value;},scope:value=>{scope=value;},prepare:gate=>{preparation=gate;}};
}
const textIn=node=>[node.textContent,...node.children.map(textIn)].join('\n');
test('Case inspection renders only admitted exact aggregates and preserves raw evidence',async()=>{
  const f=fixture(),before=structuredClone(f.evidence()),read=f.context.showFieldInspector('target');await settle();
  const request=f.requests[0];assert.equal(request.cmd,'aggregate_events');
  assert.equal(request.args.caseKey,'captured-evidence');assert.equal(request.args.caseEvents,undefined);
  assert.equal(request.opts.caseEvents,f.evidence(),'captured evidence is retained only for request-lifetime resync');
  assert.deepEqual(plain(request.args.analysisContext),f.state.owner.identity);assert.equal(request.args.sourceGeneration,7);
  assert.deepEqual(plain(request.args.filters),[...f.state.filters,{column:'target',op:'not_empty',value:'',value2:null}]);
  assert.equal(request.opts.latest,'field-inspector');assert.equal(request.opts.analysisOwner.instance,1);
  request.resolve(f.response());await read;
  const view=f.views[0].overview;assert.match(textIn(view),/2 valores preenchidos no recorte visível do Caso/);
  assert.match(textIn(view),/visible/);assert.doesNotMatch(textIn(view),/excluded-secret/);
  assert.deepEqual(f.evidence(),before);
  view.children.find(node=>node.className==='kv-row').onclick();
  assert.deepEqual(plain(f.state.filters.at(-1)),{column:'target',op:'equals_exact',value:'visible',value2:null});
});
test('aggregate failure remains explicit and never falls back to preserved values',async()=>{
  for(const response of [{error:'Aggregation budget exceeded'},{rows:[{n:3}],group_values:[]}]){
    const f=fixture(),read=f.context.showFieldInspector('target');await settle();f.requests[0].resolve(response);await read;
    assert.match(textIn(f.views[0].overview),/Não foi possível inspecionar/);assert.doesNotMatch(textIn(f.views[0].overview),/excluded-secret/);
    assert.equal(f.views[0].overview.children.some(node=>node.tag==='button'),false);
  }
});
test('late field counts are rejected after Case, source, filter, evidence, area or drawer changes',async()=>{
  for(const change of ['Case','source','filter','evidence','scope','drawer']){
    const f=fixture(),read=f.context.showFieldInspector('target');await settle();
    if(change==='Case')f.state.owner.instance++;if(change==='source')f.state.owner.sourceKey='source-next';
    if(change==='filter')f.state.filters=[];if(change==='evidence')f.setEvidence([]);if(change==='scope')f.scope('dataset');if(change==='drawer')f.context.closeDrawer();
    f.requests[0].resolve(f.response(undefined,[['late-value',99]]));await read;
    assert.doesNotMatch(textIn(f.views[0].overview),/late-value|99 valores/,change);
  }
});
test('stale Case preparation never dispatches an inspector query',async()=>{
  const f=fixture(),gate=deferred();f.prepare(gate.promise);const read=f.context.showFieldInspector('target');await settle();
  f.state.owner.identity.visibilityRevision++;gate.resolve();await read;
  assert.equal(f.requests.length,0);assert.match(textIn(f.views[0].overview),/contexto mudou/);
});
test('bounded inspection retains exact full keys, accounts for omitted counts and rejects stale clicks',async()=>{
  const f=fixture(),read=f.context.showFieldInspector('target');await settle();const long='key'.repeat(300);
  f.requests[0].resolve(f.response(undefined,[[long,7],['null',3]],{omitted_records:5,omitted_groups:2}));await read;
  const view=f.views[0].overview;assert.match(textIn(view),/15 valores preenchidos/);
  assert.match(textIn(view),/5 registros de 2 grupos adicionais/);assert.equal(view.children.filter(node=>node.className==='kv-row').length,2,'omitted counts do not become a literal Other filter');
  const row=view.children.find(node=>node.className==='kv-row');assert.ok(row.children[0].textContent.length<200);assert.ok(row.title.length<520);
  f.state.owner.identity.visibilityRevision++;row.onclick();assert.equal(f.state.filters.length,1,'stale inspector cannot create a value filter');
});
test('literal empty-label keys remain literal while missing group keys never become value filters',async()=>{
  const f=fixture(),read=f.context.showFieldInspector('target');await settle();f.requests[0].resolve(f.response(undefined,[['(vazio)',2],['null',1]]));await read;
  f.views[0].overview.children.find(node=>node.className==='kv-row').onclick();
  assert.equal(f.state.filters.at(-1).value,'(vazio)');assert.equal(f.state.filters.at(-1).op,'equals_exact');
  const missing=fixture(),request=missing.context.showFieldInspector('target');await settle();missing.requests[0].resolve(missing.response(undefined,[[null,1]]));await request;
  assert.match(textIn(missing.views[0].overview),/Contagens de campo inválidas/);assert.equal(missing.views[0].overview.children.some(node=>node.tag==='button'),false);
});
test('Dataset inspection stays query-free and uses only its already admitted page',async()=>{
  const f=fixture('dataset');await f.context.showFieldInspector('target');
  assert.equal(f.requests.length,0);assert.match(textIn(f.views[0].overview),/1 valores observados na página atual/);
  assert.doesNotMatch(textIn(f.views[0].overview),/excluded-secret/);
});

test('Case cube and dashboard captions never count preserved evidence as visible records',async()=>{
  const nodes=new Map(),$=key=>{if(!nodes.has(key))nodes.set(key,{textContent:'',children:[],innerHTML:'',classList:{toggle(){}},setAttribute(){},dispatchEvent(){}});return nodes.get(key);};
  const state={analyticsScope:'case',filters:[],stationAnalyticsId:null};
  const context=vm.createContext({state,$,window:{},Event:class{},renderCubeFields(){},pivotShell(){},fieldNames:()=>[],
    caseEvents:()=>{throw Error('Preserved evidence must not provide visible metric labels');},backendFilters:()=>[],
    dashboardCharts:()=>[{id:'one'}],dashCharts:{},scopeHasEvents:()=>false});
  const workbench=readFileSync(new URL('../../frontend/analysis-workbench.js',import.meta.url),'utf8');
  vm.runInContext(workbench.slice(workbench.indexOf('  const originalFields ='),workbench.indexOf('  const originalZones =')),context);
  context.renderCubeFields();assert.match($('#cube-info').textContent,/Registros visíveis do Caso/);
  vm.runInContext(app.slice(app.indexOf('async function renderDashboard('),app.indexOf('async function renderChartCard(')),context);
  await context.renderDashboard('case');assert.match($('#dash-info').textContent,/1 gráfico · registros visíveis do Caso/);
});
test('journey key preference uses a matching admitted count or the backend suggestion order',()=>{
  const source=readFileSync(new URL('../../frontend/journeys.js',import.meta.url),'utf8'),cache=new Map();
  const context=vm.createContext({view:{scope:'case'},explorerAnalytics:cache,explorerKey:()=> 'matching-visible-scope',
    caseEvents:()=>{throw Error('Do not scan preserved evidence for a default');}});
  vm.runInContext(source.slice(source.indexOf('  function preferred('),source.indexOf('  const duration =')),context);
  const fields=[{field:'trace-unique',suggested:true,distinct:2,coverage:1},{field:'trace-repeated',suggested:true,distinct:1,coverage:1}];
  assert.equal(context.preferred(fields).field,'trace-unique');
  cache.set('matching-visible-scope',{total:2});assert.equal(context.preferred(fields).field,'trace-repeated');
});
