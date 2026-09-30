import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../../frontend/app.js', import.meta.url), 'utf8');
const core = readFileSync(new URL('../../frontend/performance-core.js', import.meta.url), 'utf8');
const tick = () => new Promise(resolve => setTimeout(resolve, 5));
const context = vm.createContext({ window: {}, performance, setTimeout, clearTimeout, console });
vm.runInContext(core, context);
const { queue, estimate } = context.window.PerformanceTools;
let e = estimate(null, {operationId:'a',phaseId:'read',completed:0,total:100,unit:'bytes'}, 0);
e = estimate(e, {operationId:'a',phaseId:'read',completed:20,total:100,unit:'bytes'}, 1000);
assert.equal(e.eta, null, 'two samples are insufficient for ETA');
e = estimate(e, {operationId:'a',phaseId:'read',completed:40,total:100,unit:'bytes'}, 2000);
assert.equal(e.eta, 3);
for (const payload of [
  {operationId:'a',phaseId:'commit',completed:40,total:100,unit:'bytes'},
  {operationId:'b',phaseId:'read',completed:40,total:100,unit:'bytes'},
  {operationId:'a',phaseId:'read',completed:40,total:100,unit:'records'},
  {operationId:'a',phaseId:'read',completed:1,total:100,unit:'bytes'},
  {operationId:'a',phaseId:'read',completed:50,total:0,unit:'bytes'},
]) assert.equal(estimate(e, payload, 3000).eta, null, 'phase/job/unit/reset/unknown total clears ETA');
assert.equal(estimate(null,{completed:50,total:0},0).percent,null);
assert.equal(estimate(null,{completed:100,total:100,state:'indexing'},0).percent,null,'counters complete does not imply commit complete');
assert.equal(estimate(null,{completed:100,total:100,state:'ready'},0).percent,100);
let running = 0, maxRunning = 0, done = 0;
const q = queue(1);
await Promise.all(Array.from({length:5},()=>q.add(async()=>{maxRunning=Math.max(maxRunning,++running);await tick();running--;done++;})));
assert.equal(maxRunning,1); assert.equal(done,5);
await assert.rejects(q.add(()=>{throw Error('must not start');},()=>false), /substituída/);

// Execute the actual query orchestration with delayed analytics, without a browser or backend.
const nodes = new Map();
const node = key => { if (!nodes.has(key)) nodes.set(key, {textContent:'',hidden:false,disabled:false,attrs:{},setAttribute(k,v){this.attrs[k]=v;},getAttribute(k){return this.attrs[k];},replaceChildren(){},before(){},append(){}});return nodes.get(key); };
let resolveCount, calls = [], rendered = [], treeCalls = 0, countUpdates = 0;
const state = { loaded:true, rows:[], total:0, currentArtifact:{id:'source',loadedAt:1}, cases:{active:'case'}, derivedFields:[], sortCol:'timestamp',sortDir:'desc',pageSize:100,page:0,refreshVersion:0,activeDatasetTab:'table',filters:[] };
Object.assign(context, { state, $:node, el:()=>node('new'), chart:null, workspaceScope:()=> 'dataset', activeCase:()=>({id:'case'}), caseSig:()=>'', backendFilters:()=>state.filters, caseEvents:()=>[], fmtNum:String, renderTable:qr=>rendered.push(qr), renderChips(){}, updateContextBar(){}, renderChart(){}, refreshTreeAggs:async()=>{ treeCalls++; }, document:{createTextNode:String}, api:async (cmd,args,opts)=> {
  calls.push({cmd,args,opts});
  if(cmd==='query_page') return {rows:[{id:args.offset}],total:null,hasMore:true,nextCursor:`cursor-${args.offset+100}`,engine:'columnar',warning:null};
  if(cmd==='count_filtered') return new Promise(resolve=>{resolveCount=resolve;});
  if(cmd==='stats_events') return {buckets:[],levels:[]};
  if(cmd==='engine_status') return null;
} });
context.window.Tasks={cancelLatest(){}};
context.window.Workspace={onRefresh(){},onCountChanged(){countUpdates++;}};
const queryCode = source.slice(source.indexOf('const explorerAnalytics ='),source.indexOf('\nfunction scheduleRefresh()'));
vm.runInContext(queryCode,context);
assert.equal(await context.refresh(),true);
assert.equal(rendered.length,1,'first rows rendered before count settles');
assert.equal(state.total,null,'unknown total remains unknown');
await tick(); assert.equal(calls.filter(c=>c.cmd==='count_filtered').length,1);
state.page=1; await context.refresh();
assert.equal(calls.filter(c=>c.cmd==='query_page').at(-1).args.cursor,'cursor-100');
assert.equal(calls.filter(c=>c.cmd==='count_filtered').length,1,'paging shares count');
resolveCount(50000000); await tick();
assert.equal(state.total,50000000); assert.equal(countUpdates,1,'exact count refreshes workspace subtitle');
const facetsBefore=treeCalls;await context.refresh();await tick();assert.ok(treeCalls>facetsBefore,'cached filter still refreshes independently keyed facets');
node('#events-table').setAttribute('aria-busy','true');context.updatePager();assert.equal(node('#pg-next').disabled,true,'local column render cannot reopen pagination during a query');
node('#events-table').setAttribute('aria-busy','false');state.page=9;context.updatePager({...state.pageResult,pageIndex:1});assert.match(node('#pg-label').textContent,/^2 /,'old rows retain their successful page label');
state.filters=[{column:'level',op:'equals',value:'error'}];state.page=9;
await context.refresh();
assert.equal(state.page,0,'new filters reset cursor stack');
assert.equal(calls.filter(c=>c.cmd==='query_page').at(-1).args.cursor,null);

// Column UI persistence must neither clone case evidence nor call cases_save.
const store=new Map();let saves=0,toasts=0;
Object.assign(context,{localStorage:{setItem:(k,v)=>store.set(k,v),getItem:k=>store.get(k)},currentCaseArtifact:()=>context.artifact,artifact:{},toast(){toasts++;},saveCases(){saves++;}});
state.columns=['timestamp','message','host'];state.visibleCols=['timestamp','host'];state.colWidths={host:120};
vm.runInContext(source.slice(source.indexOf('function visiblePreferenceKey()'),source.indexOf('\nasync function removeArtifact')),context);
context.saveVisibleCols();assert.equal(saves,0);
state.currentArtifact.loadedAt=2;state.visibleCols=['timestamp'];context.restoreVisiblePreferences();
assert.deepEqual(Array.from(state.visibleCols),['timestamp','host'],'reopening same source preserves preferences');
assert.deepEqual(Array.from(context.artifact.visibleCols),['timestamp','host'],'restored preferences also appear in exported artifact metadata');
context.localStorage.getItem=()=>'{corrupt';assert.doesNotThrow(()=>context.restoreVisiblePreferences());
context.localStorage.setItem=()=>{throw Error('full');};assert.doesNotThrow(()=>context.saveVisibleCols());assert.equal(toasts,1);
assert.ok(!source.slice(source.indexOf('window.addEventListener("resize"'),source.indexOf('  bindKeyboard();',source.indexOf('window.addEventListener("resize"'))).includes('refresh()'),'resize is layout only');
console.log('Frontend responsiveness: queue, ETA isolation, first rows, cursor/count cache, local preferences and resize passed');

// Real Tasks wrapper: cancellation is isolated; shared consumers keep useful work alive.
const taskSource=readFileSync(new URL('../../frontend/tasks.js',import.meta.url),'utf8');
const taskCalls=[], pendingTasks=new Map();
const taskNode={before(){},querySelector(){return this;},classList:{toggle(){}},setAttribute(){},append(){}};
const taskContext=vm.createContext({window:{PerformanceTools:context.window.PerformanceTools},state:{currentArtifact:{id:'a',loadedAt:1},cases:{active:'c'},derivedFields:[]},performance,
  document:{body:{dataset:{page:'explore'}},documentElement:{dataset:{zone:'analysis'}}}, $:()=>taskNode,el:()=>({...taskNode}),toast(){},esc:String,fmtNum:String,
  setTimeout:(fn,delay)=>delay<100?setTimeout(fn,delay):0,clearInterval(){},setInterval(){},requestAnimationFrame(){},
  api:(cmd,args)=>{taskCalls.push({cmd,args});if(cmd==='cancel_task'){pendingTasks.get(args.operationId)?.reject(Error('Operação cancelada.'));return Promise.resolve(true);}return new Promise((resolve,reject)=>pendingTasks.set(args.operationId,{resolve,reject}));}
});
vm.runInContext(taskSource,taskContext);
const first=taskContext.api('query_page',{limit:1},{latest:'records'}).catch(String);
const unrelated=taskContext.api('query_page',{limit:2},{latest:'other'});
const firstId=taskCalls[0].args.operationId;
const otherId=taskCalls[1].args.operationId;
const newer=taskContext.api('query_page',{limit:3},{latest:'records'});
assert.equal(taskCalls.filter(c=>c.cmd==='cancel_task').length,1);
assert.equal(taskCalls.find(c=>c.cmd==='cancel_task').args.operationId,firstId);
assert.ok(!taskCalls.some(c=>c.cmd==='cancel_operation'));
pendingTasks.get(otherId).resolve('other');
pendingTasks.get(taskCalls.filter(c=>c.cmd==='query_page').at(-1).args.operationId).resolve('new');
assert.match(await first,/cancelada/);assert.equal(await unrelated,'other');assert.equal(await newer,'new');
const shared=taskContext.api('count_filtered',{filters:[]},{});
const joined=taskContext.api('count_filtered',{filters:[]},{latest:'count'});
assert.equal(shared,joined,'identical readers share one invocation');
const cancelsBefore=taskCalls.filter(c=>c.cmd==='cancel_task').length;
taskContext.window.Tasks.cancelLatest('count');
assert.equal(taskCalls.filter(c=>c.cmd==='cancel_task').length,cancelsBefore,'superseded subscriber preserves another consumer');
pendingTasks.get(taskCalls.at(-1).args.operationId).resolve(7);assert.equal(await shared,7);
console.log('Task-scoped cancellation and subscriber-aware deduplication passed');

// Only explicit Cancel All may use the engine-wide cancellation command.
for (const file of ['app.js','workspace.js','journeys.js','remote-sources.js']) {
  assert.ok(!readFileSync(new URL(`../../frontend/${file}`,import.meta.url),'utf8').includes('api("cancel_operation"'), `${file} cancels only its own task`);
}
for (const cmd of ['journey_fields','journey_index','journey_events','remote_test','remote_import','load_bundle','set_ts_config']) {
  const promise=taskContext.api(cmd,{},{latest:cmd}).catch(String);
  const request=taskCalls.at(-1);assert.ok(request.args.operationId,`${cmd} has an isolated ID`);
  taskContext.window.Tasks.cancelLatest(cmd);assert.match(await promise,/cancelada/);
}
console.log('Journey/remote/bundle named cancellation and explicit-only global cancel passed');

const staleDetail=taskContext.window.Tasks.detail({progress:{phase:'Sincronizando checkpoint',completed:100,total:100,unit:'registros'},estimate:{updated:performance.now()-11000,eta:5,rate:20}});
assert.match(staleDetail,/Aguardando atualização/);assert.doesNotMatch(staleDetail,/nesta etapa|registros\/s/,'stale rate and ETA disappear');
const freshDetail=taskContext.window.Tasks.detail({progress:{phase:'Indexando',completed:50,total:100,unit:'registros'},estimate:{updated:performance.now(),eta:5,rate:20}});
assert.match(freshDetail,/nesta etapa/);
assert.match(readFileSync(new URL('../../frontend/workspace.js',import.meta.url),'utf8'), /if \(kind === "case"\) \{ window\.WorkspaceContext\?\.capture\(\); syncVisiblePreferenceMetadata\(\); \}/);
// Changed source can only be reopened; it must never offer resume against stale bytes.
context.api=async()=>({state:'stale',baseReady:false,canResume:false,phase:'Fonte alterada; reabra o arquivo',completedRows:0,totalRows:50,error:'Fonte mudou'});
let retryButtons=0;context.el=tag=>{if(tag==='button')retryButtons++;return node(`created-${tag}`);};
await context.refreshEngineStatus();assert.equal(retryButtons,0);assert.equal(node('#engine-readiness').hidden,false);
console.log('Export preferences, live elapsed/stale estimates and stale-source recovery semantics passed');

// Aggregation budget errors are not empty successes and may not poison facet caches.
Object.assign(context,{colLabel:String,treeSpin(){},treeGroupProfiles:()=>({categories:[],ranges:[]}),renderExploreTree(){},document:{querySelector:()=>({classList:{contains:()=>false}})}});
state.treeAgg={dataset:{level:[['old',7]]},case:null};state.treeAggSig={dataset:'old',case:null};state.treeAggError={dataset:null,case:null};state.datasetProfiles=[];
const previousFacets=state.treeAgg.dataset;
context.api=async()=>[['level',{rows:[],error:'Budget exceeded; narrow the period'}]];
vm.runInContext(source.slice(source.indexOf('const treeAggVersion ='),source.indexOf('function renderExploreTreeInto(')),context);
await context.refreshTreeAggs('dataset',{force:true});
assert.equal(state.treeAgg.dataset,previousFacets,'keep previous useful facets');assert.equal(state.treeAggSig.dataset,null,'never cache empty budget failure');assert.match(state.treeAggError.dataset,/Budget exceeded/);
const legacyGroup=source.slice(source.indexOf('async function runGroup()'),source.indexOf('function drillDown(',source.indexOf('async function runGroup()')));
assert.match(legacyGroup,/if \(res.error\) throw new Error\(res.error\)/);
console.log('Aggregation budget errors remain explicit and uncached');
