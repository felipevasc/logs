import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const owner={storeId:'store',caseId:'case',analysisId:'analysis'},reference={kind:'native_evidence',schemaVersion:1,owner,containerId:'container',manifestId:'manifest',manifestSha256:'a'.repeat(64),memberCount:300};
const member={containerId:'container',manifestId:'manifest',occurrenceId:'occurrence'},identity={caseId:'case',analysisId:'analysis',configRevision:2,visibilityRevision:3};
const doc=()=>({evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},active:'case',cases:[{id:'case',analysisContext:{schemaVersion:1,...identity,config:{derivedFields:[],references:[]},migrationDiagnostics:[]},items:[{id:'legacy',rows:{kind:'native_evidence_container',reference:structuredClone(reference),preservedCount:300,preview:null}}]}],caseEvidence:[{state:'ready',owner,evidenceSignature:'signature',preservedCount:300}],diagnostics:[]});
const detail=()=>({kind:'preserved_member_detail',member,fields:[{column:'number',type:'number',text:'1.0',complete:true},{column:'structured',type:'object',text:'{"b":1.0,"a":18446744073709551615}',complete:true}],fieldsTotal:300,nextCursor:'field-page-2',raw:{text:'original raw',complete:true},envelope:{text:'{"fields":{"number":1.0}}',complete:true},javaTraceAvailable:true});
const field=(column='number',value='1.0',type='number')=>({kind:'preserved_field_text',member,column,present:true,type,text:value,complete:true});
const entry=(index=0)=>({entryId:`n:container:occurrence${index||''}`,member:{...member,occurrenceId:`occurrence${index||''}`},containerId:'container',itemIndex:0,itemId:'legacy',originalRowIndex:index,timestampMs:index,timing:'timed',title:{text:'short title',complete:false},detail:{text:'summary',complete:true},source:{text:'source',complete:true},laneKey:'container',laneLabel:{text:'lane',complete:true},groupingKey:'native-full-value-hash',groupingUsesTitle:false,eventRef:`stable:${index}`});
const page=()=>({kind:'preserved_timeline',owner,evidenceSignature:'signature',authoredViewSignature:'c'.repeat(64),preservedCount:300,scopeCount:250,timedCount:249,untimedCount:1,unavailableCount:0,minTimestampMs:0,maxTimestampMs:1000,entries:[entry()],nextCursor:'page-2',aliases:[{alias:'e:legacy:stable:0',state:'unique',entryId:'n:container:occurrence'}]});
const deferred=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return{promise,resolve};};
async function fixture(){
  const context=vm.createContext({window:{},structuredClone,TextEncoder}),calls=[];let current=doc(),next=detail(),guard=true;
  for(const name of ['case-evidence.js','case-evidence-preserved.js'])vm.runInContext(read(name),context);
  const client=context.window.CaseEvidence.create({enabled:true,invoke:async()=>doc(),requestId:()=>''});await client.load();
  const reader=context.window.CaseEvidencePreserved.create({client,getStore:()=>current,currentCase:()=>current.cases[0],invoke:async(command,args)=>{calls.push({command,args:structuredClone(args)});return typeof next==='function'?next():structuredClone(next);}});
  return{reader,client,calls,doc:()=>current,replace:value=>{current=value;},reply:value=>{next=value;},guard:value=>{guard=value;},capture:()=>reader.capture({reference:current.cases[0].items[0].rows.reference,member,isCurrent:()=>guard}),history:extra=>reader.captureHistory({aliases:['e:legacy:stable:0'],...extra})};
}

test('preserved member detail is a distinct native text DTO and never an Event or parsed record',async()=>{
  const f=await fixture(),ticket=f.capture(),result=await f.reader.detail(ticket);assert.equal(result.kind,'preserved_member_detail');assert.equal(result.fields[0].text,'1.0');assert.equal(result.fields[1].text,'{"b":1.0,"a":18446744073709551615}');
  assert.equal(result.fieldsTotal,300);assert.equal(result.nextCursor,'field-page-2');assert.equal(Object.hasOwn(result,'id'),false);assert.equal(Object.hasOwn(result,'event_ref'),false);
  assert.deepEqual(f.calls[0],{command:'case_evidence_member_detail',args:{request:{store:{storeId:'store',epoch:'epoch'},reference,member,cursor:null}}});
  await f.reader.detail(ticket,{cursor:'field-page-2'});assert.equal(f.calls[1].args.request.cursor,'field-page-2');
});

for(const [name,change] of [['Case instance',f=>f.replace(structuredClone(f.doc()))],['epoch',f=>{f.doc().store.epoch='replacement';}],['owner',f=>{f.doc().cases[0].analysisContext.analysisId='replacement';}],['manifest',f=>{f.doc().cases[0].items[0].rows.reference.manifestId='replacement';}],['closed detail',f=>f.guard(false)]])test(`preserved member reply rejects a changed ${name}`,async()=>{
  const f=await fixture(),gate=deferred();f.reply(()=>gate.promise);const pending=f.reader.detail(f.capture());change(f);gate.resolve(detail());await assert.rejects(pending,/VIEW_CHANGED/);
});

test('preserved views remain available after current-analysis visibility/config changes and note edits',async()=>{
  const f=await fixture(),ticket=f.capture();f.doc().cases[0].analysisContext.configRevision++;f.doc().cases[0].analysisContext.visibilityRevision++;f.doc().cases[0].notes='new note';
  assert.equal((await f.reader.detail(ticket)).fields[0].text,'1.0');assert.equal(f.reader.current(ticket),true);
  assert.equal(f.calls[0].args.analysisContext,undefined);assert.equal(f.calls[0].args.request.analysisContext,undefined);
});

test('exact preserved field actions retain number/structured/null/missing/CRLF native semantics',async()=>{
  const f=await fixture(),ticket=f.capture();
  for(const [column,value,type]of [['number','1.0','number'],['u64','18446744073709551615','number'],['object','{"b":1,"a":2}','object'],['message',' \r\nline\n\r ','string'],['null','null','null']]){
    f.reply(field(column,value,type));const result=await f.reader.fieldText(ticket,column);assert.equal(result.text,value);assert.equal(result.type,type);assert.equal(result.complete,true);
  }
  f.reply({...field('missing'),present:false,type:null,text:null});assert.equal((await f.reader.fieldText(ticket,'missing')).present,false);
  f.reply({...field('timestamp'),present:false,type:null,text:null});assert.equal((await f.reader.fieldText(ticket,'timestamp')).text,null);
  f.reply(field('timestamp','0','string'));assert.equal((await f.reader.fieldText(ticket,'timestamp')).text,'0');
});

test('mismatched member/column or clipped exact field responses cannot become actions',async()=>{
  const f=await fixture(),ticket=f.capture();for(const result of [{...field(),complete:false},{...field(),column:'other'},{...field(),member:{...member,occurrenceId:'other'}},{...field(),present:false,type:'null',text:'null'}]){
    f.reply(result);await assert.rejects(f.reader.fieldText(ticket,'number'),/HISTORY_INVALID/);
  }
  assert.throws(()=>f.reader.fieldText(ticket,'*'),/HISTORY_INVALID/);
});

test('native Java is lazy and separately owned by the member handle',async()=>{
  const f=await fixture(),ticket=f.capture();await f.reader.detail(ticket);assert.equal(f.calls.some(c=>c.command==='case_evidence_member_java_trace'),false);
  f.reply({member,state:'unavailable',trace:null,reason:'raw_unavailable'});assert.equal((await f.reader.java(ticket)).reason,'raw_unavailable');assert.equal(f.calls.at(-1).command,'case_evidence_member_java_trace');
  f.reply({member:{...member,occurrenceId:'other'},state:'unavailable',trace:null,reason:'not_java'});await assert.rejects(f.reader.java(ticket),/HISTORY_INVALID/);
});

test('detail field/raw/envelope previews enforce bounded cells and explicit pagination',async()=>{
  const f=await fixture(),ticket=f.capture();for(const mutate of [v=>v.fields[0].text='x'.repeat(4097),v=>v.raw.text='x'.repeat(262145),v=>v.envelope.text='x'.repeat(262145),v=>v.fieldsTotal=1,v=>v.fields.push({...v.fields[0]})]){
    const value=detail();mutate(value);f.reply(value);await assert.rejects(f.reader.detail(ticket),/HISTORY_INVALID/);
  }
});

test('history pages carry exact full-scope totals and native occurrence handles without hydrating records',async()=>{
  const f=await fixture(),filters=[{column:'_all',op:'query',value:'status>=400 AND source:API'}],ticket=f.history({filters,fromMs:0,toMs:2000});f.reply(page());filters[0].value='edited draft';
  const result=await f.reader.timeline(ticket);assert.equal(result.scopeCount,250);assert.equal(result.entries.length,1);assert.equal(result.nextCursor,'page-2');assert.equal(result.entries[0].title.complete,false);
  assert.equal(f.calls[0].command,'case_evidence_timeline');assert.equal(f.calls[0].args.request.filters[0].value,'status>=400 AND source:API');assert.equal(f.calls[0].args.request.fromMs,0);
  await f.reader.timeline(ticket,{cursor:'page-2'});assert.equal(f.calls[1].args.request.cursor,'page-2');
});

test('history full-Case alias resolution preserves explicit missing/ambiguous aliases outside the current page',async()=>{
  const f=await fixture(),aliases=['e:duplicate:ref','e:missing:ref','e:offpage:ref'],ticket=f.history({aliases}),value=page();
  value.aliases=[{alias:aliases[0],state:'ambiguous',entryId:null},{alias:aliases[1],state:'missing',entryId:null},{alias:aliases[2],state:'unique',entryId:'n:container:offpage'}];f.reply(value);
  const result=await f.reader.timeline(ticket);assert.deepEqual(result.aliases,value.aliases);assert.equal(result.entries.length,1,'off-page aliases do not invent or delete entries');
});

test('history rejects unsaved membership and refreshes captured authored views without rewriting them',async()=>{
  const f=await fixture(),ticket=f.history();f.doc().cases[0].timeline={edits:{'e:legacy:stable:0':{title:'authored title'}},annotations:[{text:'unsaved'}]};f.reply(page());await assert.rejects(f.reader.timeline(ticket),/VIEW_CHANGED/);await f.reader.timeline(f.history());
  f.doc().cases[0].items[0].stationId='new station';assert.throws(()=>f.history(),/SAVE_PENDING/);await assert.rejects(f.reader.timeline(ticket),/VIEW_CHANGED/);
});

test('history rejects malformed count/order/member envelopes rather than treating a page as complete evidence',async()=>{
  const f=await fixture(),ticket=f.history();for(const mutate of [v=>v.scopeCount=1,v=>v.entries=[entry(1),entry(0)],v=>v.entries[0].member.manifestId='other',v=>v.entries[0].timestampMs=null,v=>v.entries[0].title.text='x'.repeat(4097),v=>v.aliases=[],v=>v.entries[0].groupingUsesTitle=undefined]){
    const value=structuredClone(page());mutate(value);f.reply(value);await assert.rejects(f.reader.timeline(ticket),/HISTORY_INVALID|VIEW_INVALID/);
  }
});

test('history request limits match native UTF-8 and encoded JSON budgets before any invoke',async()=>{
  const f=await fixture();assert.ok(f.history({filters:Array.from({length:200},()=>({column:'source',op:'eq',value:'API'}))}));
  for(const extra of [{filters:Array.from({length:201},()=>({column:'source',op:'eq',value:'API'}))},{aliases:['界'.repeat(683)]},{filters:[{column:'界'.repeat(342),op:'eq',value:'x'}]},{filters:[{column:'message',op:'x'.repeat(33),value:'x'}]},{filters:[{column:'message',op:'eq',value:'x'.repeat(1000001)}]},{filters:[{column:'message',op:'eq',value:'\0'.repeat(400000)}]}])assert.throws(()=>f.history(extra),/HISTORY_INVALID/);
  await assert.rejects(f.reader.timeline(f.history(),{cursor:'x'.repeat(513)}),/HISTORY_INVALID/);assert.equal(f.calls.length,0);
});

test('whole response accounting rejects encoded overflow even when each preview cell fits',async()=>{
  const f=await fixture(),ticket=f.capture(),value=detail();value.fields=Array.from({length:128},(_,index)=>({column:`field${index}`,type:'string',text:'\0'.repeat(4096),complete:false}));value.fieldsTotal=128;f.reply(value);
  await assert.rejects(f.reader.detail(ticket),/HISTORY_INVALID/);
});

test('untimed preserved records keep an explicit missing timestamp preview and separate exact missing action',async()=>{
  const f=await fixture(),ticket=f.capture(),value=detail();value.fields.unshift({column:'timestamp',type:'missing',text:'',complete:true});f.reply(value);
  assert.equal((await f.reader.detail(ticket)).fields[0].type,'missing');
  f.reply({...field('timestamp'),present:false,type:null,text:null});assert.equal((await f.reader.fieldText(ticket,'timestamp')).present,false);
  f.reply({...field('timestamp'),type:'missing',text:''});await assert.rejects(f.reader.fieldText(ticket,'timestamp'),/HISTORY_INVALID/);
});

test('empty history chips send the native required empty string without fabricating a value filter',async()=>{
  const f=await fixture(),ticket=f.history({filters:[{column:'timestamp',op:'empty'},{column:'message',op:'not_empty'}]});f.reply(page());await f.reader.timeline(ticket);
  assert.deepEqual(f.calls[0].args.request.filters,[{column:'timestamp',op:'empty',value:'',value2:null},{column:'message',op:'not_empty',value:'',value2:null}]);
});

test('history retains sparse original occurrence positions after removal',async()=>{
  const f=await fixture(),value=page();value.entries[0].originalRowIndex=900;f.reply(value);
  assert.equal((await f.reader.timeline(f.history())).entries[0].originalRowIndex,900);
});

test('captured authored view is exact bounded JSON and its native signature cannot change between pages',async()=>{
 const f=await fixture();f.doc().cases[0].timeline={edits:{'n:container:occurrence':{title:'A title',color:'#000000'}},annotations:[]};f.doc().cases[0].manual=[{id:'milestone',start:1000,name:'Manual'}];const ticket=f.history();f.reply(page());await f.reader.timeline(ticket);
 assert.equal(f.calls[0].args.request.authoredViewJson,JSON.stringify({timeline:f.doc().cases[0].timeline,manual:f.doc().cases[0].manual}));
 f.reply({...page(),authoredViewSignature:'d'.repeat(64)});await assert.rejects(f.reader.timeline(ticket,{cursor:'page-2'}),/HISTORY_INVALID/);
 assert.throws(()=>f.history({authoredViewJson:'x'.repeat(1048577)}),/HISTORY_INVALID/);
});
