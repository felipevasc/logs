import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const code=readFileSync(new URL('../../frontend/security.js',import.meta.url),'utf8'),start=code.indexOf('  async function saveToCase('),end=code.indexOf('  const updateCountsSafe',start),plain=value=>JSON.parse(JSON.stringify(value));
function fixture(){const c={id:'case',items:[]},calls=[],messages=[];let key='original',change=false;const context=vm.createContext({window:{CaseEvidence:{active:true},AnalysisContexts:{capture:()=>({identity:'owner'}),prepare:async value=>value,isCurrent:()=>true},WorkspaceContext:{refreshMembership(){}}},activeCase:()=>c,workspaceScope:()=> 'dataset',universeKey:()=>key,ensureCase:()=>c,minimumEvidence:5,SEVERITY:{high:['Alta',3]},structuredClone,evidence:()=>({exportMetadata:()=>({analysis_id:'triage'})}),state:{currentOrigin:'source',currentArtifact:{id:'artifact',source:{path:'source.log'}}},registerCurrentArtifact:()=>({id:'artifact'}),queueCustody:()=>calls.push(['custody']),updateCountsSafe(){},toast:message=>messages.push(message),api:async()=>assert.fail('native capture must not request triage Event hydration'),nativeEvidenceServices:()=>({actions:{selection:async(rows,options)=>{calls.push(['selection',plain(rows)]);if(change)key='changed';return{rows,options};},add:async(selected,metadata)=>{calls.push(['add',plain(metadata)]);assert.equal(Object.hasOwn(metadata,'rows'),false);c.items.push({metadata,rows:{kind:'native_evidence_container'}});}}})});vm.runInContext(code.slice(start,end),context);return{c,calls,messages,save:(detections)=>context.saveToCase({analysis_id:'triage'},detections,'Finding','Summary',null),change:()=>{change=true;}};}
const detection=()=>({event_ids:[7,9],event_refs:['source-seven','source-nine'],evidence_members:[{event_id:7,event_ref:'source-seven'},{event_id:9,event_ref:'source-nine'}],severity:'high',start:1,end:2});

test('native security capture preserves exact supporting handles without hydrating archived Events into JavaScript',async()=>{
 const f=fixture();await f.save([detection()]);assert.deepEqual(f.calls[0],['selection',[{id:7,eventRef:'source-seven'},{id:9,eventRef:'source-nine'}]]);assert.equal(f.calls[1][0],'add');assert.equal(f.calls[1][1].detection.analysis_id,'triage');assert.equal(f.calls[1][1].foundCount,2);assert.equal(f.calls[2][0],'custody');assert.equal(f.c.items.length,1);
});

test('native security capture rejects a numeric ID with conflicting exact references',async()=>{
 const f=fixture(),first=detection(),second={...detection(),event_ids:[7],evidence_members:[{event_id:7,event_ref:'different-source'}]};await f.save([first,second]);assert.equal(f.calls.length,0);assert.equal(f.c.items.length,0);assert.match(f.messages[0],/ambígua/);
});

test('native security capture discards a stale source result before adding evidence',async()=>{
 const f=fixture();f.change();await f.save([detection()]);assert.equal(f.calls.some(call=>call[0]==='add'),false);assert.equal(f.c.items.length,0);assert.match(f.messages[0],/conjunto mudou/);
});
