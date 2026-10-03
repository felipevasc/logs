import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const owner={storeId:'s',caseId:'c',analysisId:'a'},text=value=>({text:value,complete:true});
const ref=id=>({kind:'native_evidence',schemaVersion:1,owner,containerId:id,manifestId:`manifest-${id}`,manifestSha256:'a'.repeat(64),memberCount:2});
const item=id=>({id:'duplicate',label:`Item ${id}`,rows:{kind:'native_evidence_container',reference:ref(id),preservedCount:2,preview:null}});
const entry=(id,index,n,time=1000)=>({entryId:`n:${id}:${n}`,containerId:id,itemIndex:index,itemId:'duplicate',member:{containerId:id,manifestId:`manifest-${id}`,occurrenceId:n},timestampMs:time,timing:time===null?'untimed':'timed',title:text('Record'),detail:text('preserved text'),source:text(`source ${id}`),groupingUsesTitle:false,groupingKey:'pattern',originalRowIndex:Number(n),laneKey:id,laneLabel:text(`lane ${id}`),eventRef:'same-ref'});
const page=entries=>({kind:'preserved_timeline',owner,evidenceSignature:'signature',authoredViewSignature:'c'.repeat(64),preservedCount:4,scopeCount:entries.length,timedCount:entries.filter(e=>e.timestampMs!==null).length,untimedCount:entries.filter(e=>e.timestampMs===null).length,unavailableCount:0,minTimestampMs:1000,maxTimestampMs:1000,entries,aliases:[],nextCursor:null});
function fixture(){const c={id:'c',items:[item('a'),item('b')],timeline:{groups:[],annotations:[],edits:{},layout:{}}},requests=[];let active=c,reply=page([entry('a',0,'0'),entry('b',1,'0')]),valid=true;
 const history={captureHistory:request=>{requests.push(request);return request;},timeline:async(ticket,{cursor=null}={})=>typeof reply==='function'?reply(ticket,cursor):structuredClone(reply),current:ticket=>valid&&ticket.isCurrent()};
 const context=vm.createContext({window:{},state:{analysisView:'timeline',stationAnalyticsId:null},structuredClone,TextEncoder,DOMException,Intl,Date,Map,Set,activeCase:()=>active,document:{addEventListener(){},body:{dataset:{page:'case-timeline'}}},nativeEvidenceServices:()=>({client:{},session:{pending:async()=>{},status:()=> 'ready'},history}),fmtNum:String});
 for(const file of ['case-evidence.js','case-evidence-timeline.js','case-timeline.js'])vm.runInContext(read(file),context);context.window.CaseEvidence.active=true;
 return{c,context,api:context.window.CaseEvidenceTimeline,timeline:context.window.CaseTimeline,requests,reply:value=>{reply=value;},active:value=>{active=value;},invalidate:()=>{valid=false;}};
}

test('native historical entries retain exact tuple identities despite repeated raw/item IDs',()=>{
 const f=fixture(),data=page([entry('a',0,'0'),entry('b',1,'0')]),before=structuredClone(f.c);const result=f.api.collect(f.c,data,{complete:true});
 assert.equal(result.entries.length,2);assert.deepEqual(plain(result.entries.map(e=>e.id)),['n:a:0','n:b:0']);assert.equal(result.entries[1].occurrence.item,f.c.items[1]);assert.equal(result.entries[1].rows[0].kind,'preserved_member');assert.equal(Object.hasOwn(result.entries[1].rows[0],'raw'),false);assert.deepEqual(f.c,before);
});

test('historical unique aliases resolve notes and groups while ambiguous bindings remain untouched',()=>{
 const f=fixture();f.c.timeline.edits['e:old:1']={title:'authored title'};f.c.timeline.annotations=[{id:'note',anchor:'e:old:1',text:'authored note'},{id:'ambiguous',anchor:'e:duplicate:1',text:'preserved ambiguity'}];
 const data=page([entry('a',0,'0'),entry('b',1,'0')]);data.aliases=[{alias:'e:old:1',state:'unique',entryId:'n:a:0'},{alias:'e:duplicate:1',state:'ambiguous',entryId:null}];const before=structuredClone(f.c);const result=f.api.collect(f.c,data,{complete:true});assert.equal(result.entries[0].title,'authored title');assert.deepEqual(plain(result.entries[0].aliases),['e:old:1']);assert.deepEqual(f.c,before);
});

test('a group spanning a native page never silently becomes a smaller authored group',()=>{
 const f=fixture();f.c.timeline.groups=[{id:'group',ids:['n:a:0','n:a:1'],name:'authored group'}];const before=structuredClone(f.c.timeline.groups),result=f.api.collect(f.c,page([entry('a',0,'0')]),{complete:false});assert.equal(result.entries[0].type,'event');assert.deepEqual(f.c.timeline.groups,before);
});

test('native table includes untimed occurrences without inventing epoch timestamps',async()=>{
 const f=fixture();f.reply(page([entry('a',0,'0',null)]));const result=await f.timeline.rows(f.c,()=>true,{includeUndated:true});assert.equal(result.rows[0].start,null);assert.equal(result.rows[0].end,null);assert.equal(result.undated,1);assert.deepEqual(plain(result.rows[0].itemRefs),[{kind:'container',containerId:'a'}]);
});

test('native report reads an exact metadata snapshot while retaining source authority',async()=>{
 const f=fixture(),snapshot=structuredClone(f.c);snapshot.timeline.annotations.push({id:'note',anchor:'n:a:0',text:'snapshot note'});const result=await f.timeline.rows(snapshot,()=>true,{includeUndated:true});assert.equal(result.complete,true);assert.equal(result.rows[0].notes[0].text,'snapshot note');assert.equal(f.c.timeline.annotations.length,0);
});

test('report refuses a truncated preview instead of presenting it as full text',async()=>{
 const f=fixture(),result=page([entry('a',0,'0')]);result.entries[0].detail.complete=false;f.reply(result);await assert.rejects(f.api.all(f.c),/textos completos/);
});

test('report checks counts, repeated occurrences and changing summaries across native pages',async()=>{
 for(const kind of ['count','repeat','summary']){
  const f=fixture(),first=page([entry('a',0,'0')]);first.scopeCount=2;first.timedCount=2;first.nextCursor='next';
  f.reply((_ticket,cursor)=>{if(!cursor)return first;const next={...first,entries:kind==='count'?[]:[entry(kind==='repeat'?'a':'b',kind==='repeat'?0:1,'0')],nextCursor:null};if(kind==='summary')next.scopeCount=3;return next;});
  await assert.rejects(f.api.all(f.c),/incompleta|repetiu|não corresponde/);
 }
});

test('all historical alias batches use the same captured scope without transporting records',async()=>{
 const f=fixture();for(let i=0;i<300;i++)f.c.timeline.edits[`e:old:${i}`]={title:'note'};
 f.reply(ticket=>({...page([entry('a',0,'0')]),aliases:ticket.aliases.map(alias=>({alias,state:'missing',entryId:null}))}));const result=await f.api.all(f.c);assert.equal(result.aliases.length,300);assert.equal(f.requests.length,2);assert.equal(f.requests[0].aliases.length,256);assert.equal(f.requests[1].aliases.length,44);assert.equal(f.requests.every(request=>!Object.hasOwn(request,'events')),true);
});

test('automatic grouping uses full native title hashes instead of colliding clipped previews',()=>{
 const f=fixture(),left=entry('a',0,'0'),right=entry('a',0,'1');left.groupingUsesTitle=right.groupingUsesTitle=true;left.title=right.title={text:'same clipped prefix',complete:false};left.groupingKey='full-title-hash-a';right.groupingKey='full-title-hash-b';const data=page([left,right]);
 assert.equal(f.api.collect(f.c,data,{complete:true}).entries.length,2);
 f.c.timeline.edits={'n:a:0':{title:'same authored title'},'n:a:1':{title:'same authored title'}};assert.equal(f.api.collect(f.c,data,{complete:true}).entries[0].type,'auto');
 f.c.timeline.edits={'n:a:0':{title:'full-title-hash-b'}};assert.equal(f.api.collect(f.c,data,{complete:true}).entries.length,2,'authored and native grouping identities have distinct tags');
});

test('native graphical collection retains fractional authored manual dates',()=>{
 const f=fixture();f.c.manual=[{id:'fraction',name:'Milestone',start:1.25,end:2.75}];const result=f.api.collect(f.c,page([]));assert.equal(result.entries[0].start,1.25);assert.equal(result.entries[0].end,2.75);assert.equal(f.c.manual[0].start,1.25);
});

test('complete historical report collection preserves authored duplicate group slots and eligible subsets',()=>{
 const f=fixture();f.c.timeline.groups=[{id:'g',ids:['n:a:0','n:a:0','n:b:missing'],name:'Duplicate slots'}];const result=f.api.collect(f.c,page([entry('a',0,'0')]),{complete:true});assert.equal(result.entries.length,1);assert.equal(result.entries[0].type,'group');assert.equal(result.entries[0].rows.length,2);assert.equal(result.entries[0].members[0],result.entries[0].members[1]);assert.deepEqual(f.c.timeline.groups[0].ids,['n:a:0','n:a:0','n:b:missing']);
});
