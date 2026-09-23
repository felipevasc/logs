/* Scope contract checks against the browser mock; native regression is compiled separately. */
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import assert from 'node:assert/strict';
const window={addEventListener(){}};
runInNewContext(readFileSync('scripts/preview/mock-tauri.js','utf8'),{window,localStorage:{getItem:()=>null,setItem(){}},setTimeout,clearTimeout,console,TextEncoder,URL});
const invoke=window.__TAURI__.core.invoke;
const baseline=await invoke('dataset_overview',{filters:[]});
assert(baseline.total>100);
const caseEvents=[[31,0,'Erro'],[72,100,'Aviso'],[95,null,'Informação']].map(([id,timestamp,level])=>({id,event_ref:`saved:${id}`,timestamp,level,source:'Caso',message:`saved event ${id}`,raw:'preserved raw',name:'',description:'',code:'',fields:{password:'synthetic-example'}}));
const filters=[{column:'source',op:'equals_exact',value:'Caso'}],scope={filters,caseEvents};
const overview=await invoke('dataset_overview',scope);
assert.equal(overview.total,3);assert.equal(overview.undated,1);assert.equal(overview.start,0);assert.equal(overview.end,100);
const query=await invoke('query_events',{...scope,sortColumn:'id',sortDir:'desc',offset:1,limit:1});
assert.equal(query.total,3);assert.equal(query.rows[0].id,72);assert.equal(query.rows[0].event_ref,'saved:72');
const snapshot=await invoke('explore_snapshot',{...scope,sortColumn:'id',sortDir:'asc',offset:0,limit:10});
assert.equal(snapshot.query.total,3);assert.equal(snapshot.query.rows[0].id,31);
assert.equal(snapshot.stats.levels.reduce((sum,[,n])=>sum+n,0),3);
assert.equal(snapshot.stats.buckets.reduce((sum,[,n])=>sum+n,0),2);
const undatedStats=await invoke('stats_events',{filters:[],caseEvents:[caseEvents[2]]});
assert.equal(undatedStats.buckets.length,0);assert.equal(undatedStats.levels[0][1],1);
const timeline=await invoke('timeline_range',{...scope,start:0,end:100,bucketCount:240});
assert.equal(timeline.total,2);assert.equal(timeline.errors,1);assert(timeline.buckets.every(b=>b.timestamp<=100));
const periods={before:{start:0,end:49},after:{start:50,end:150}};
const comparison=await invoke('compare_periods',{...scope,...periods});
assert.equal(comparison.before_total,1);assert.equal(comparison.after_total,1);
assert.equal(await invoke('export_events',{...scope,format:'jsonl',mask:true}),3);
assert.equal((await invoke('query_events',{filters:[{column:'_all',op:'contains',value:'preserved raw'}],caseEvents,limit:10})).total,3);
for(const command of ['dataset_overview','query_events','explore_snapshot','timeline_range','compare_periods','export_events']){
  const result=await invoke(command,{...scope,caseEvents:[],start:0,end:100,bucketCount:240,...periods,limit:10});
  assert.equal(command==='explore_snapshot'?result.query.total:command==='compare_periods'?result.before_total+result.after_total:command==='export_events'?result:result.total,0,`${command} must not fall back from empty case`);
}
assert.equal((await invoke('dataset_overview',{filters:[]})).total,baseline.total);
console.log(JSON.stringify({dataset:baseline.total,case:3,scopeIsolation:true,emptyCase:true,sorting:true,rawSearch:true,datedTimeline:true,comparison:true,exportSelection:true}));
