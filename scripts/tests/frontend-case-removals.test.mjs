import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const removalSource=readFileSync(new URL('../../frontend/case-removals.js',import.meta.url),'utf8');
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const timeline=readFileSync(new URL('../../frontend/case-timeline.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const record=(id,event_ref)=>({id,event_ref,timestamp:100+id,source:'source',code:event_ref,message:'original',raw:'untouched raw',
  fields:{number:1.5,negativeZero:-0,object:{'10':'ten','2':'two'},unknown:[null,false]},evidence_provenance:{proof:'original'}});
const example=()=>({id:'case-a',items:[{id:'item-a',label:'A',rows:[record(7,'source:a')]},{id:'item-b',label:'B',rows:[record(0,'source:b')]}],
  timeline:{groups:[],annotations:[{id:'note-a',anchor:'e:item-a:source:a',text:'A note'},{id:'note-b',anchor:'e:item-b:source:b',text:'B note'}],edits:{},layout:{}}});
function fixture(c=example()){
  let current=c,revision=1,saveResult=true;const messages=[],notices=[],menus=[],calls=[];
  const element=(tag,cls,text='')=>({tag,cls,textContent:text,children:[],disabled:false,appendChild(child){this.children.push(child);},remove(){this.removed=true;}});
  const area={appendChild:notice=>notices.push(notice)};
  const context=vm.createContext({window:{AnalysisContexts:{capture:()=>({caseId:current?.id,revision}),isCurrent:owner=>owner.caseId===current?.id&&owner.revision===revision,owns:owner=>owner.caseId===current?.id},WorkspaceContext:{refreshMembership(){}}},
    c,activeCase:()=>current,state:{stationAnalyticsId:null},caseSig:()=>String(revision),el:element,$:()=>area,setTimeout(){},
    toast:(text,type)=>messages.push({text,type}),updateAnalysisBadge(){},renderAnalysis(){},filtersChanged(){},
    saveCases:async()=>{calls.push('save');if(saveResult instanceof Error)throw saveResult;return saveResult;},
    showCtxMenu:(x,y,items)=>menus.push(items),
  });
  vm.runInContext(removalSource,context);
  vm.runInContext(app.slice(app.indexOf('function caseRecordKey('),app.indexOf('function caseEventsCompute(')),context);
  vm.runInContext(app.slice(app.indexOf('let lastCaseRemoval ='),app.indexOf('function openSendToTrailModal(')),context);
  const api=context.window.CaseRemovals,key=(row,item)=>context.caseRecordKey(row,item.artifactId,item.origin);
  return{c,context,api,key,messages,notices,menus,calls,save:value=>{saveResult=value;},switch:next=>{current=next;revision++;},stale:()=>revision++};
}

test('an exact reference never falls back to a colliding raw numeric ID',async()=>{
  const f=fixture(),a=f.c.items[0].rows[0],b=f.c.items[1].rows[0];
  const matches=f.api.resolve(f.c,{id:0,event_ref:'source:a'},{key:f.key});
  assert.equal(matches.length,1);assert.equal(matches[0].row,a);
  assert.equal(f.api.resolve(f.c,{id:0,event_ref:'absent'},{key:f.key}).length,0);
  assert.equal(await f.context.removeEventFromCase({id:0,event_ref:'source:a'}),true);
  assert.equal(f.c.items.length,1);assert.equal(f.c.items[0].rows[0],b);assert.equal(f.calls.length,1);
  assert.equal(a.fields.number,1.5);assert.equal(Object.is(a.fields.negativeZero,-0),true);assert.equal(a.raw,'untouched raw');
  assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);assert.equal(f.c.items[0].rows[0],a);assert.equal(f.c.items[1].rows[0],b);
});

test('legacy no-reference selection resolves the Case dense ID instead of matching a raw ID',()=>{
  const c=example();for(const item of c.items)for(const row of item.rows)delete row.event_ref;
  const f=fixture(c),matches=f.api.resolve(c,{id:0,event_ref:''},{key:f.key});
  assert.equal(matches.length,1);assert.equal(matches[0].item,c.items[0]);assert.equal(matches[0].row.id,7);
  c.items[0].stationId='other';c.items[1].stationId='selected';
  assert.equal(f.api.resolve(c,{id:0},{key:f.key,stationId:'selected'})[0].row,c.items[1].rows[0]);
});

test('duplicate appearances require an explicit occurrence choice and retain surviving shared anchors',async()=>{
  const c=example(),first=c.items[0].rows[0],second={...first,raw:'second preserved occurrence'};
  c.items[0].rows=[first,second];c.items[1].rows=[first];const f=fixture(c);
  assert.equal(await f.context.removeEventFromCase({id:0,event_ref:'source:a'}),false);
  assert.equal(f.calls.length,0);assert.equal(f.menus[0].length,3);
  assert.match(f.menus[0][1].label,/Item 1.*ocorrência 2/);assert.match(f.menus[0][2].label,/Item 2.*ocorrência 1/);
  assert.equal(await f.menus[0][1].onClick(),true);
  assert.equal(c.items[0].rows.length,1);assert.equal(c.items[0].rows[0],first);assert.equal(c.items[1].rows[0],first);
  assert.ok(c.timeline.annotations.some(note=>note.id==='note-a'),'an anchor still used by another occurrence is retained');
  assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);assert.equal(c.items[0].rows[1],second);
});

test('stale duplicate menus and stale exact locators do not remove any occurrence',async()=>{
  const c=example();c.items[1].rows=[{...c.items[0].rows[0]}];const f=fixture(c),before=plain(c);
  await f.context.removeEventFromCase({id:0,event_ref:'source:a'});f.stale();await f.menus[0][0].onClick();
  assert.equal(f.calls.length,0);assert.deepEqual(plain(c),before);
  const loc=f.api.resolve(c,{id:0,event_ref:'source:a'},{key:f.key})[0];c.items[0].rows=[record(9,'replacement')];
  assert.throws(()=>f.api.prepare(c,[loc]),/Caso mudou/);
});

test('failed or rejected persistence rolls back owned rows and annotations without a success notice',async()=>{
  for(const result of [false,new Error('disk unavailable')]){
    const f=fixture(),before=plain(f.c),row=f.c.items[0].rows[0];f.save(result);
    assert.equal(await f.context.removeEventFromCase({id:0,event_ref:'source:a'}),false);
    assert.deepEqual(plain(f.c),before);assert.equal(f.c.items[0].rows[0],row);assert.equal(f.notices.length,0);
    assert.equal(f.messages.some(message=>message.type==='ok'),false);
  }
});

test('undo preserves later authored item notes, original order and values, and failed undo restores removed state',async()=>{
  const c=example(),a=c.items[0].rows[0];c.items[0].rows.push(record(8,'source:c'));c.items[0].includedCount=2;
  const f=fixture(c);await f.context.removeEventFromCase({id:0,event_ref:'source:a'});
  c.items[0].note='authored after removal';f.save(false);
  assert.equal(await f.context.undoCaseOccurrenceRemoval(),false);assert.equal(c.items[0].rows.length,1);assert.equal(c.items[0].rows[0].event_ref,'source:c');
  f.save(new Error('disk unavailable'));assert.equal(await f.context.undoCaseOccurrenceRemoval(),false);
  assert.equal(c.items[0].rows.length,1);assert.equal(c.items[0].rows[0].event_ref,'source:c');
  f.save(true);assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);
  assert.equal(c.items[0].rows[0],a);assert.equal(c.items[0].note,'authored after removal');assert.equal(c.items[0].includedCount,2);
});

test('failed undo keeps metadata authored while its durable save was pending',async()=>{
  const c=example();c.items[0].rows.push(record(8,'source:c'));const f=fixture(c);
  await f.context.removeEventFromCase({id:0,event_ref:'source:a'});
  let release;f.context.saveCases=()=>new Promise(resolve=>{release=resolve;});
  const pending=f.context.undoCaseOccurrenceRemoval();c.items[0].note='authored during undo';release(false);
  assert.equal(await pending,false);assert.equal(c.items[0].rows.length,1);assert.equal(c.items[0].note,'authored during undo');
  f.context.saveCases=async()=>true;assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);
  assert.equal(c.items[0].rows.length,2);assert.equal(c.items[0].note,'authored during undo');
});

test('overlapping removals and undo wait for the owned save result without stacking optimistic changes',async()=>{
  const f=fixture(),before=plain(f.c);let release;
  f.context.saveCases=()=>new Promise(resolve=>{release=resolve;});
  const first=f.context.removeEventFromCase({id:0,event_ref:'source:a'});
  assert.equal(await f.context.removeEventFromCase({id:0,event_ref:'source:b'}),false);
  release(false);assert.equal(await first,false);assert.deepEqual(plain(f.c),before);
  f.context.saveCases=async()=>true;await f.context.removeEventFromCase({id:0,event_ref:'source:a'});
  f.context.saveCases=()=>new Promise(resolve=>{release=resolve;});
  const undo=f.context.undoCaseOccurrenceRemoval();
  assert.equal(await f.context.removeEventFromCase({id:0,event_ref:'source:b'}),false);
  assert.equal(await f.context.undoCaseOccurrenceRemoval(),false);
  release(true);assert.equal(await undo,true);assert.deepEqual(plain(f.c),before);
});

test('failed removal restores its record and anchor while retaining notes edited during the save',async()=>{
  for (const outcome of [false,new Error('disk unavailable')]) {
    const f=fixture(),row=f.c.items[0].rows[0];let finish;
    f.context.saveCases=()=>new Promise((resolve,reject)=>{finish=()=>outcome===false?resolve(false):reject(outcome);});
    const removing=f.context.removeEventFromCase({id:0,event_ref:'source:a'});
    f.c.timeline.annotations.find(note=>note.id==='note-b').text='newly authored note';finish();
    assert.equal(await removing,false);assert.equal(f.c.items[0].rows[0],row);
    assert.equal(f.c.timeline.annotations.find(note=>note.id==='note-b').text,'newly authored note');
    assert.equal(f.c.timeline.annotations.find(note=>note.id==='note-a').anchor,'e:item-a:source:a');
    assert.equal(f.notices.length,0);
  }
});

test('failed undo retains a concurrent surviving-note edit through the later successful undo',async()=>{
  const f=fixture();await f.context.removeEventFromCase({id:0,event_ref:'source:a'});let release;
  f.context.saveCases=()=>new Promise(resolve=>{release=resolve;});
  const undo=f.context.undoCaseOccurrenceRemoval();f.c.timeline.annotations.find(note=>note.id==='note-b').text='edited during undo';
  release(false);assert.equal(await undo,false);assert.equal(f.c.items.length,1);
  assert.equal(f.c.timeline.annotations.find(note=>note.id==='note-b').text,'edited during undo');
  f.context.saveCases=async()=>true;assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);
  assert.equal(f.c.items.length,2);assert.equal(f.c.timeline.annotations.find(note=>note.id==='note-b').text,'edited during undo');
});

test('undo refuses intervening membership, annotation or Case changes rather than overwriting them',async()=>{
  for(const change of ['membership','annotation','case']){
    const f=fixture();await f.context.removeEventFromCase({id:0,event_ref:'source:a'});const saves=f.calls.length;
    if(change==='membership')f.c.items.push({id:'new',rows:[record(5,'new')]});
    if(change==='annotation')f.c.timeline.annotations.push({id:'new-note',text:'new',anchor:'e:item-b:source:b'});
    if(change==='case')f.switch({id:'other',items:[]});
    const before=plain(f.c);assert.equal(await f.context.undoCaseOccurrenceRemoval(),false);
    assert.equal(f.calls.length,saves);assert.deepEqual(plain(f.c),before);
  }
});

function timelineFixture({save=true}={}){
  const f=fixture();f.save(save);
  Object.assign(f.context,{COLORS:['red'],display:value=>value,valid:Number.isFinite,callbacks:null,
    box:{scrollTop:9,querySelector:()=>({scrollTop:4,scrollLeft:7,dispatchEvent(){}}),querySelectorAll:()=>[]},
    shell:{querySelector:()=>({scrollTop:4,scrollLeft:7})},document:{activeElement:null},requestAnimationFrame:callback=>{f.frame=callback;}});
  vm.runInContext(timeline.slice(timeline.indexOf('  function collect('),timeline.indexOf('  function noteMap(')),f.context);
  const savedStart=app.indexOf('  save: () => { const saved = saveCases();'),savedEnd=app.indexOf(',\n  removeOccurrences:',savedStart);
  vm.runInContext('saveInstalled = '+app.slice(savedStart+8,savedEnd),f.context);
  f.context.callbacks={save:f.context.saveInstalled,removeOccurrences:f.context.removeCaseOccurrences,
    notify:text=>f.messages.push({text})};
  const wrapStart=timeline.indexOf('    const callbacksSave = () => {'),wrapEnd=timeline.indexOf('    if (horizontal) shell.querySelectorAll',wrapStart);
  vm.runInContext(timeline.slice(wrapStart,wrapEnd)+'\nthis.timelineSave = callbacksSave;',f.context);
  const collected=f.context.collect(f.c);f.context.entry=collected.entries.find(entry=>entry.id==='e:item-a:source:a');f.context.config=collected.config;
  f.context.callbacks.removeOccurrences=(c,targets,entryIds,save)=>f.context.removeCaseOccurrences(c,targets,{entryIds,save});
  const label=timeline.indexOf('label: "Remover do caso"'),start=timeline.indexOf('onClick: async () => {',label)+9,end=timeline.indexOf('\n            }\n          });',start)+14;
  vm.runInContext('removeInstalled = '+timeline.slice(start,end),f.context);return f;
}

test('actual Timeline occurrence handler removes the record before cleaning its note and supports undo',async()=>{
  const f=timelineFixture(),a=f.c.items[0].rows[0],b=f.c.items[1].rows[0],before=plain(f.c);
  assert.equal(f.context.entry.id,'e:item-a:source:a');assert.equal(f.context.entry.occurrence.row,a);
  await f.context.removeInstalled();assert.equal(f.c.items.length,1);assert.equal(f.c.items[0].rows[0],b);
  assert.deepEqual(Array.from(f.c.timeline.annotations,note=>note.id),['note-b']);assert.equal(f.notices.length,1);
  assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);assert.deepEqual(plain(f.c),before);assert.equal(f.c.items[0].rows[0],a);
});

test('both installed Timeline save wrappers propagate false and trigger exact rollback',async()=>{
  const f=timelineFixture({save:false}),before=plain(f.c);
  assert.equal(await f.context.timelineSave(),false,'renderer wrapper returns actual app save outcome');
  await f.context.removeInstalled();assert.deepEqual(plain(f.c),before);assert.equal(f.notices.length,0);
  assert.equal(typeof f.frame,'function','scroll/focus restoration remains scheduled');
});

test('group removals clean selected anchors, retarget surviving links and undo restores original metadata',()=>{
  const c=example();c.items[0].rows.push(record(8,'source:c'));
  c.timeline.groups=[{id:'g',ids:['e:item-a:source:a','e:item-b:source:b'],name:'Authored group'}];
  c.timeline.annotations.push({id:'linked',anchor:'e:item-a:source:a',links:[{targetId:'e:item-b:source:b',arrow:true}],text:'Linked'});
  const f=fixture(c),before=plain(c),targets=f.api.resolve(c,{id:0,event_ref:'source:a'},{key:f.key}),tx=f.api.prepare(c,targets);
  tx.apply();assert.equal(c.timeline.groups.length,0);assert.equal(c.timeline.annotations.find(note=>note.id==='linked').anchor,'e:item-b:source:b');
  tx.undo();assert.deepEqual(plain(c),before);
});


test('failure rollback restores removed links while preserving concurrently added links and edited properties',()=>{
  const c=example(),a='e:item-a:source:a',b='e:item-b:source:b';
  c.timeline.annotations=[{id:'links',anchor:a,text:'original',arrow:'forward',links:[{targetId:b,arrow:'both',color:'red'},{targetId:'C',arrow:'forward'}]}];
  const f=fixture(c),tx=f.api.prepare(c,f.api.resolve(c,{id:0,event_ref:'source:a'},{key:f.key}));tx.apply();
  const note=c.timeline.annotations[0];assert.equal(note.anchor,b);
  note.links[0].color='edited C';note.links.push({targetId:'D',arrow:'back'});note.text='concurrent edit';tx.rollback();
  assert.equal(c.timeline.annotations[0].anchor,a);assert.equal(c.timeline.annotations[0].text,'concurrent edit');
  assert.deepEqual(plain(c.timeline.annotations[0].links),[{targetId:b,arrow:'both',color:'red'},{targetId:'C',arrow:'forward',color:'edited C'},{targetId:'D',arrow:'back'}]);
});

test('failure rollback restores the removed group member while preserving a concurrent group addition',()=>{
  const c=example(),a='e:item-a:source:a',b='e:item-b:source:b';
  c.timeline.groups=[{id:'group',name:'group',ids:[a,b,'C']}];
  const f=fixture(c),tx=f.api.prepare(c,f.api.resolve(c,{id:0,event_ref:'source:a'},{key:f.key}));tx.apply();
  c.timeline.groups[0].ids.push('D');c.timeline.groups[0].name='renamed';tx.rollback();
  assert.deepEqual(plain(c.timeline.groups),[{id:'group',name:'renamed',ids:[a,b,'C','D']}]);
});


test('failed removal restores only its membership while preserving a concurrently added container',async()=>{
  const f=fixture(),original=f.c.items[0].rows[0];let release;
  f.context.saveCases=()=>new Promise(resolve=>{release=resolve;});
  const pending=f.context.removeEventFromCase({id:0,event_ref:'source:a'});
  const added={id:'later',rows:[record(5,'source:new')]};f.c.items.push(added);f.c.items[0].note='concurrent unrelated note';release(false);
  assert.equal(await pending,false);assert.equal(f.c.items.length,3);assert.equal(f.c.items[0].rows[0],original);
  assert.equal(f.c.items[1].note,'concurrent unrelated note');assert.equal(f.c.items[2],added);
});


test('failed undo preserves a concurrently appended container through a later successful undo',async()=>{
  const f=fixture();await f.context.removeEventFromCase({id:0,event_ref:'source:a'});let release;
  f.context.saveCases=()=>new Promise(resolve=>{release=resolve;});const pending=f.context.undoCaseOccurrenceRemoval();
  const added={id:'later',rows:[record(5,'source:new')]};f.c.items.push(added);release(false);
  assert.equal(await pending,false);assert.equal(f.c.items.length,2);assert.equal(f.c.items[1],added);
  f.context.saveCases=async()=>true;assert.equal(await f.context.undoCaseOccurrenceRemoval(),true);
  assert.equal(f.c.items.length,3);assert.equal(f.c.items[2],added);assert.equal(f.c.items[0].rows[0].event_ref,'source:a');
});
