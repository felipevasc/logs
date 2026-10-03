import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'),plain=value=>JSON.parse(JSON.stringify(value));
const owner={storeId:'store',caseId:'case',analysisId:'analysis'},reference={kind:'native_evidence',schemaVersion:1,owner,containerId:'container',manifestId:'manifest',manifestSha256:'a'.repeat(64),memberCount:300};
const preview=text=>({text,complete:true});
const entry=()=>({entryId:'group',type:'group',startMs:1,endMs:300,timing:'timed',title:preview('Complete authored group'),detail:preview('300 eventos'),source:preview('Saved item'),memberCount:300,memberToken:'opaque-group-token',firstMemberEntryId:'n:container:occurrence-0',firstMemberEditKey:null,itemIndex:0,groupIndex:0,manualIndex:null,noteIndices:[0]});
const page=()=>({kind:'native_case_display_page',owner,evidenceSignature:'evidence',authoredViewSignature:'a'.repeat(64),displaySignature:'b'.repeat(64),preservedCount:300,scopeOccurrenceCount:300,unavailableCount:0,displayEntryCount:2,matchingDisplayEntryCount:1,entries:[entry()],nextCursor:null});
const member=n=>({reference,member:{containerId:'container',manifestId:'manifest',occurrenceId:`occurrence-${n}`},itemIndex:0});
function fixture(){const c={id:'case',items:[{id:'item',rows:{kind:'native_evidence_container',reference,preservedCount:300,preview:null}}],timeline:{groups:[{id:'group',ids:['n:container:a','n:container:b'],name:'Complete authored group'}],annotations:[{id:'note',anchor:'group',text:'Full plan attached note'}],edits:{}},manual:[{id:'m',name:'Manual'}]},store={evidenceViewVersion:1,store:{storeId:'store',epoch:'epoch',revision:'1'},cases:[c],caseEvidence:[{state:'ready',owner,evidenceSignature:'evidence',preservedCount:300}]},calls=[];let reply=page(),current=true;
 const context=vm.createContext({window:{},structuredClone,TextEncoder});for(const name of ['case-evidence.js','case-evidence-display.js'])vm.runInContext(read(name),context);
 const reader=context.window.CaseEvidenceDisplay.create({client:{analysisShape:()=>context.window.CaseEvidence.analysisShape(c)},getStore:()=>store,currentCase:()=>c,invoke:async(command,args)=>{calls.push({command,args:plain(args)});return typeof reply==='function'?reply(command,args):structuredClone(reply);}});
 return{c,store,context,reader,calls,capture:options=>reader.capture({displayQuery:'grupo',isCurrent:()=>current,...options}),reply:value=>{reply=value;},invalidate:()=>{current=false;}};
}

test('native full-Case search sends bounded captured authoring and paginates display entries rather than occurrences',async()=>{
 const f=fixture(),ticket=f.capture();const result=await f.reader.page(ticket);assert.equal(result.entries[0].memberCount,300);assert.equal(result.matchingDisplayEntryCount,1);assert.equal(result.scopeOccurrenceCount,300);assert.equal(f.calls[0].command,'case_evidence_display_timeline');assert.equal(f.calls[0].args.request.store.revision,'1');assert.equal(f.calls[0].args.request.displayQuery,'grupo');assert.equal(f.calls[0].args.request.authoredViewJson,JSON.stringify({timeline:f.c.timeline,manual:f.c.manual}));assert.equal(f.reader.notes(ticket,result.entries[0])[0].text,'Full plan attached note');
});

test('display member expansion uses the exact nested scope and preserves authored duplicate slots',async()=>{
 const f=fixture(),ticket=f.capture(),result=await f.reader.page(ticket);f.reply({kind:'native_case_display_members',displaySignature:'b'.repeat(64),entryId:'group',memberCount:300,members:[member(1),member(1)],nextCursor:'members-next'});const expanded=await f.reader.members(ticket,result.entries[0]);assert.equal(expanded.members.length,2);assert.deepEqual(expanded.members[0],expanded.members[1]);
 const request=f.calls[1].args.request;assert.equal(request.scope.displayQuery,'grupo');assert.equal(request.scope.cursor,null);assert.equal(request.cursor,null);assert.equal(request.memberToken,'opaque-group-token');assert.equal(Object.hasOwn(request,'events'),false);assert.equal(Object.hasOwn(request.scope,'events'),false);
});

test('manual-only native matches legitimately contain no preserved member and no fake timestamp',async()=>{
 const f=fixture(),result=page();result.entries=[{...entry(),entryId:'m:m',type:'manual',memberCount:0,memberToken:null,firstMemberEntryId:null,firstMemberEditKey:null,itemIndex:null,groupIndex:null,manualIndex:0,startMs:null,endMs:null,timing:'unavailable'}];f.reply(result);const found=await f.reader.page(f.capture());assert.equal(found.entries[0].memberCount,0);assert.equal(found.entries[0].startMs,null);
});

test('authored edits, metadata saves and later queries invalidate old display pages and expansions',async()=>{
 for(const change of [f=>{f.c.timeline.edits.x={title:'new'};},f=>{f.store.store.revision='2';},f=>f.invalidate()]){
  const f=fixture(),ticket=f.capture(),result=await f.reader.page(ticket);change(f);await assert.rejects(f.reader.page(ticket),/CHANGED/);await assert.rejects(f.reader.members(ticket,result.entries[0]),/CHANGED/);
 }
});

test('display response rejects changed native view signatures and forged member owner or container locators',async()=>{
 const f=fixture(),ticket=f.capture(),result=await f.reader.page(ticket);f.reply({...page(),displaySignature:'c'.repeat(64)});await assert.rejects(f.reader.page(ticket),/INVALID/);
 for(const mutate of [value=>{value.members[0].itemIndex=1;},value=>{value.members[0].reference.owner.caseId='other';},value=>{value.members[0].member.manifestId='other';}]){const value={kind:'native_case_display_members',displaySignature:'b'.repeat(64),entryId:'group',memberCount:300,members:[structuredClone(member(0))],nextCursor:null};mutate(value);f.reply(value);await assert.rejects(f.reader.members(ticket,result.entries[0]),/INVALID|VIEW_INVALID/);}
});

test('display query limits follow UTF-16 input length and the captured metadata byte ceiling before transport',()=>{
 const f=fixture();assert.ok(f.capture({displayQuery:'😀'.repeat(100)}));assert.throws(()=>f.capture({displayQuery:'😀'.repeat(101)}),/INVALID/);f.c.timeline.notes='x'.repeat(1048576);assert.throws(()=>f.capture(),/INVALID/);assert.equal(f.calls.length,0);
});

function uiFixture(){const f=fixture(),all=node=>node.children.flatMap(child=>[child,...all(child)]),timers=[],opens=[];const el=(tag='div',className='',text='')=>{let own=String(text);const node={tag,className,children:[],attrs:{},dataset:{},hidden:false,disabled:false,isConnected:true,parentElement:null,value:'',classList:{add(){}},get textContent(){return own+this.children.map(child=>child.textContent).join('');},set textContent(value){own=String(value);this.children=[];},append(...children){for(const child of children){child.parentElement=this;this.children.push(child);}},replaceChildren(...children){own='';for(const child of this.children)child.isConnected=false;this.children=[];this.append(...children);},setAttribute(name,value){this.attrs[name]=value;},remove(){this.isConnected=false;if(this.parentElement)this.parentElement.children=this.parentElement.children.filter(child=>child!==this);},focus(){}};return node;};const body=el('body');body.dataset.page='case-timeline';const host=el();body.append(host);const state={cases:f.store,stationAnalyticsId:null,analysisView:'timeline-table'};Object.assign(f.context,{document:{body,activeElement:el('button')},el,state,activeCase:()=>f.c,backendFilters:()=>[],fmtNum:String,setTimeout:fn=>{timers.push(fn);return timers.length;},clearTimeout(){},nativeEvidenceServices:()=>({session:{pending:async()=>{},status:()=> 'ready'},display:f.reader})});f.context.window.CaseEvidenceDetail={open:(reference,member,options)=>{opens.push({reference,member,options});return Promise.resolve(true);}};return{...f,body,host,state,opens,all:()=>all(body),search:()=>all(body).find(node=>node.tag==='input'),button:label=>all(body).find(node=>node.tag==='button'&&node.textContent===label),render:()=>f.context.window.CaseEvidenceDisplay.render(host,f.c),flush:async()=>{while(timers.length)await timers.shift()();}};}

test('actual historical table sends whole-Case display queries and renders manual-only results with native entry counts',async()=>{
 const f=uiFixture();await f.render();assert.match(f.host.textContent,/300.*ocorrências preservadas/);assert.equal(f.search().attrs['aria-label'],'Buscar na cronologia completa do Caso');
 f.reply((command,args)=>{assert.equal(command,'case_evidence_display_timeline');assert.equal(args.request.displayQuery,'manual needle');return{...page(),entries:[{...entry(),entryId:'m:m',type:'manual',title:preview('manual needle'),memberCount:0,memberToken:null,firstMemberEntryId:null,firstMemberEditKey:null,itemIndex:null,groupIndex:null,manualIndex:0}],matchingDisplayEntryCount:1};});
 f.search().value='manual needle';f.search().oninput();await f.flush();assert.match(f.host.textContent,/manual needle/);assert.match(f.host.textContent,/Marco manual/);assert.equal(f.host.textContent.includes('Complete authored group'),false);
});

test('actual grouped display expansion opens an exact preserved handle without substituting preview text',async()=>{
 const f=uiFixture();await f.render();f.reply({kind:'native_case_display_members',displaySignature:'b'.repeat(64),entryId:'group',memberCount:300,members:[member(7)],nextCursor:'more'});await f.button('Complete authored group').onclick();assert.equal(f.calls[1].command,'case_evidence_display_members');assert.match(f.body.textContent,/1–1 de 300/);await f.button('Ocorrência 1 · item 1').onclick();assert.equal(f.opens[0].member.occurrenceId,'occurrence-7');assert.equal(f.opens[0].reference.manifestId,'manifest');f.store.store.revision='2';assert.equal(f.opens[0].options.guard(),false);
});

test('actual full-search response is discarded after a newer query replaces its captured ticket',async()=>{
 const f=uiFixture();let release;f.reply(()=>new Promise(resolve=>{release=resolve;}));const rendering=f.render();for(let i=0;i<12&&!release;i++)await Promise.resolve();assert.ok(release);f.search().value='new query';f.search().oninput();release(page());await rendering;assert.equal(f.host.textContent.includes('Complete authored group'),false);
 f.reply({...page(),matchingDisplayEntryCount:0,entries:[]});await f.flush();assert.match(f.host.textContent,/0–0 de 0 entradas correspondentes/);assert.equal(f.calls.at(-1).args.request.displayQuery,'new query');
});

test('display dates preserve existing finite fractional manual milestones without inventing event timestamps',async()=>{
 const f=fixture(),result=page();result.entries=[{...entry(),entryId:'m:m',type:'manual',memberCount:0,memberToken:null,firstMemberEntryId:null,firstMemberEditKey:null,itemIndex:null,groupIndex:null,manualIndex:0,startMs:1.25,endMs:2.75}];f.reply(result);const found=await f.reader.page(f.capture());assert.equal(found.entries[0].startMs,1.25);assert.equal(found.entries[0].endMs,2.75);
 f.reply({...result,entries:[{...result.entries[0],startMs:Infinity}]});await assert.rejects(f.reader.page(f.capture()),/INVALID/);
});

test('both graphical views receive one complete 300-member group summary without materializing Event rows',async()=>{
 for(const mode of ['horizontal','vertical']){
  const f=uiFixture(),rendered=[];f.context.window.CaseTimeline={render:(host,item,renderMode,callbacks)=>rendered.push({host,item,renderMode,callbacks})};
  f.c.timeline.annotations.push({id:'internal',anchor:'n:container:occurrence-299',text:'Internal retained note'},{id:'off-page',anchor:'n:elsewhere:x',text:'Off page retained note'});
  const before=JSON.stringify(f.c.timeline.annotations);await f.context.window.CaseEvidenceDisplay.renderGraph(f.host,f.c,mode,{menu(){},notify:assert.fail});
  assert.equal(rendered.length,1);assert.equal(rendered[0].renderMode,mode);const graph=rendered[0].callbacks.collected;
  assert.equal(graph.entries.length,1);assert.equal(graph.entries[0].recordCount,300);assert.equal(graph.entries[0].start,1);assert.equal(graph.entries[0].end,300);assert.deepEqual(plain(graph.entries[0].rows),[]);assert.equal(Object.hasOwn(graph.entries[0],'members'),false);
  assert.deepEqual(plain(graph.visibleAnnotations.map(note=>note.id)),['note']);assert.equal(graph.omittedRelationships,2);assert.match(rendered[0].callbacks.pageLabel,/Entradas 1–1 de 1.*2 vínculos sem posição/);assert.equal(graph.targetVisible('n:container:occurrence-299'),false);assert.match(f.host.textContent,/2 vínculos internos ou fora desta página/);assert.equal(JSON.stringify(f.c.timeline.annotations),before);
  assert.equal(f.calls[0].args.request.includeUntimed,false);assert.equal(f.calls[0].args.request.displayQuery,'');
 }
});

test('graphical actions consume all exact member pages, retain display multiplicity and deduplicate mutation handles',async()=>{
 const f=fixture(),ticket=f.capture(),result=await f.reader.page(ticket),api=f.context.window.CaseEvidenceDisplay,graph=api.collectGraph(f.c,result),calls=[];
 f.reply((command,args)=>{assert.equal(command,'case_evidence_display_members');const offset=Number(args.request.cursor||0);calls.push(offset);const end=Math.min(offset+128,300);return{kind:'native_case_display_members',displaySignature:'b'.repeat(64),entryId:'group',memberCount:300,members:Array.from({length:end-offset},(_,n)=>member((offset+n)%299)),nextCursor:end<300?String(end):null};});
 const members=await api.actionMembers(f.reader,ticket,graph.entries[0],f.c);assert.deepEqual(calls,[0,128,256]);assert.equal(members.length,299);assert.equal(graph.entries[0].recordCount,300);assert.equal(members[298].occurrence.member.occurrenceId,'occurrence-298');assert.equal(members.every(value=>value.rows[0].kind==='preserved_member'&&!Object.hasOwn(value.rows[0],'raw')),true);
});

test('graphical actions refuse incomplete or repeated member cursors and stale native scopes',async()=>{
 for(const mode of ['incomplete','cursor','stale']){
  const f=fixture(),ticket=f.capture(),result=await f.reader.page(ticket),api=f.context.window.CaseEvidenceDisplay,graph=api.collectGraph(f.c,result);
  f.reply(()=>{if(mode==='stale')f.invalidate();return{kind:'native_case_display_members',displaySignature:'b'.repeat(64),entryId:'group',memberCount:300,members:[member(1)],nextCursor:mode==='cursor'?'again':null};});
  await assert.rejects(api.actionMembers(f.reader,ticket,graph.entries[0],f.c),/INVALID|CHANGED/);
 }
});

test('native edit lookup preserves legacy color and never writes clipped previews back as authored titles',async()=>{
 const f=fixture(),value=page();f.c.timeline.edits['e:old:1']={color:'#abcdef'};value.entries[0]={...value.entries[0],type:'auto',groupIndex:null,firstMemberEditKey:'e:old:1',title:{text:'Clipped',complete:false}};f.reply(value);const result=await f.reader.page(f.capture()),graph=f.context.window.CaseEvidenceDisplay.collectGraph(f.c,result);assert.equal(graph.entries[0].color,'#abcdef');assert.equal(graph.entries[0].editTitle,'');assert.match(graph.entries[0].title,/prévia/);
 value.entries[0].firstMemberEditKey='missing';f.reply(value);await assert.rejects(f.reader.page(f.capture()),/INVALID/);
});

test('attached note text stays shared and is rendered only on explicit paged disclosure',async()=>{
 const f=uiFixture();f.c.timeline.annotations[0].text='long note '.repeat(1000);await f.render();assert.equal(f.host.textContent.includes('long note'),false);assert.ok(f.button('1 notas'));f.button('1 notas').onclick();assert.match(f.body.textContent,/long note/);assert.match(f.body.textContent,/1–1 de 1/);
});

test('actual Rust serde display and expansion bytes compose with the strict frontend reader',async()=>{
 // Generated by case_evidence_display's 300-member fixture at native checkpoint
 // 3a935a7. This exercises the actual serialized DTO, not a JS-shaped stand-in.
 const wire=JSON.parse(readFileSync(new URL('./fixtures/native-case-display-wire.json',import.meta.url),'utf8')),store=wire.caseView,c=store.cases[0],context=vm.createContext({window:{},structuredClone,TextEncoder});for(const name of ['case-evidence.js','case-evidence-display.js'])vm.runInContext(read(name),context);
 const calls=[];let selected=wire.page,memberIndex=0;const reader=context.window.CaseEvidenceDisplay.create({client:{analysisShape:()=>context.window.CaseEvidence.analysisShape(c)},getStore:()=>store,currentCase:()=>c,invoke:async(command,args)=>{calls.push({command,args:plain(args)});return command==='case_evidence_display_members'?wire.memberPages[memberIndex++]:selected;}});
 const ticket=reader.capture({...wire.request,item:c}),found=await reader.page(ticket);assert.deepEqual(calls[0].args.request,wire.request);assert.equal(found.entries[0].memberCount,300);const graph=context.window.CaseEvidenceDisplay.collectGraph(c,found);assert.equal(graph.entries[0].color,'#123456');const targets=await context.window.CaseEvidenceDisplay.actionMembers(reader,ticket,graph.entries[0],c);assert.equal(targets.length,300);assert.equal(memberIndex,3);assert.deepEqual(calls.slice(1).map(call=>call.args.request.cursor),[null,wire.memberPages[0].nextCursor,wire.memberPages[1].nextCursor]);
 selected=wire.manualPage;const manual=await reader.page(reader.capture({...wire.manualRequest,item:c}));assert.deepEqual(calls.at(-1).args.request,wire.manualRequest);assert.equal(manual.entries.length,1);assert.equal(manual.entries[0].startMs,400.5);assert.equal(manual.entries[0].memberCount,0);assert.equal(reader.notes(reader.capture({...wire.manualRequest,item:c}),manual.entries[0])[0].text,'manual note needle');
});

test('graph note layout stays bounded while all authored notes remain accessible in the paged panel',()=>{
 const f=fixture();f.c.timeline.annotations=Array.from({length:300},(_,index)=>({id:`chain-${index}`,anchor:index?`chain-${index-1}`:'group',text:`Note ${index}`}));const before=JSON.stringify(f.c.timeline.annotations),graph=f.context.window.CaseEvidenceDisplay.collectGraph(f.c,page());assert.equal(graph.visibleAnnotations.length,128);assert.equal(graph.omittedAnnotations,172);assert.equal(graph.omittedRelationships,172);assert.equal(JSON.stringify(f.c.timeline.annotations),before);assert.equal(graph.targetVisible('chain-299'),false);
});

test('malformed authored graph configuration remains preserved with an explicit unavailable state',async()=>{
 const f=uiFixture();f.c.timeline='opaque configuration';await f.context.window.CaseEvidenceDisplay.renderGraph(f.host,f.c,'horizontal',{notify:assert.fail});assert.equal(f.c.timeline,'opaque configuration');assert.match(f.host.textContent,/configuração histórica está indisponível/);assert.equal(f.calls.length,0);
});
