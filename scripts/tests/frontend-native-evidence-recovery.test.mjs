import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=()=>readFileSync(new URL('../../frontend/case-evidence-recovery.js',import.meta.url),'utf8'),plain=value=>JSON.parse(JSON.stringify(value));
const uuid=n=>`00000000-0000-4000-8000-${String(n).padStart(12,'0')}`;
const status=()=>({currentProfile:{id:null,path:'/profiles/original'},restartOnly:true,startupWarning:null,pending:null,recoveries:[{recoveryId:uuid(1),createdAtMs:1000,state:'requires_verification'}],scanLimited:false,returnOriginalPending:false});
const prepared=()=>({profileId:uuid(2),destination:'/profiles/fresh',restartRequired:true,originalPreserved:true,warning:null});
function fixture(){const calls=[];let reply=status(),failure=null;const context=vm.createContext({window:{},structuredClone,crypto:{randomUUID:()=>uuid(3)}});vm.runInContext(read(),context);const controller=context.window.CaseEvidenceRecovery.create({invoke:async(command,args)=>{calls.push({command,args:plain(args)});if(failure){const error=failure;failure=null;throw Error(error);}return typeof reply==='function'?reply(command):structuredClone(reply);},requestId:()=>uuid(3)});return{controller,calls,context,reply:value=>{reply=value;},fail:value=>{failure=value;}};}

test('recovery status lists candidates without treating them as verified or changing the profile',async()=>{
 const f=fixture();assert.deepEqual(plain(await f.controller.status()),status());assert.deepEqual(f.calls,[{command:'case_recovery_status',args:{}}]);assert.equal(f.controller.snapshot().recoveries[0].state,'requires_verification');
});

test('verified recovery prepares only a fresh destination for restart and retains current profile',async()=>{
 const f=fixture();await f.controller.status();f.reply(prepared());await f.controller.prepare(uuid(1));assert.deepEqual(f.calls[1],{command:'case_recovery_prepare_restart',args:{request:{recoveryId:uuid(1),requestId:uuid(3)}}});assert.equal(f.controller.snapshot().currentProfile.path,'/profiles/original');assert.equal(f.controller.snapshot().pending.destination,'/profiles/fresh');assert.equal(f.controller.snapshot().pending.restartRequired,true);
});

test('a failed preparation preserves previous destination and retries the exact request',async()=>{
 const f=fixture(),prior=status();prior.pending={profileId:uuid(4),recoveryId:uuid(1),requestId:uuid(5),destination:'/profiles/previous-choice',restartRequired:true};f.reply(prior);await f.controller.status();f.fail('verification failed');await assert.rejects(f.controller.prepare(uuid(1)),/verification/);assert.deepEqual(plain(f.controller.snapshot()),prior);assert.deepEqual(plain(f.controller.pendingRequest()),{recoveryId:uuid(1),requestId:uuid(3)});f.reply(prepared());await f.controller.prepare(uuid(1));assert.deepEqual(f.calls[1],f.calls[2]);
});

test('late marker-sync warnings are displayed as acknowledged preparation without claiming a profile switch',async()=>{
 const f=fixture();await f.controller.status();f.reply({...prepared(),warning:'The selection marker needs confirmation on restart'});const result=await f.controller.prepare(uuid(1));assert.match(result.warning,/marker/);assert.equal(f.controller.snapshot().currentProfile.id,null);assert.equal(f.controller.snapshot().pending.profileId,uuid(2));
});

test('recovery rejects a live in-place destination and malformed or falsely verified candidate inventories',async()=>{
 const f=fixture();await f.controller.status();f.reply({...prepared(),destination:'/profiles/original'});await assert.rejects(f.controller.prepare(uuid(1)),/INVALID/);assert.equal(f.controller.snapshot().pending,null);
 for(const mutate of [value=>{value.restartOnly=false;},value=>{value.recoveries[0].state='verified';},value=>{value.recoveries.push(value.recoveries[0]);},value=>{value.currentProfile.id='untrusted';}]){const value=status();mutate(value);assert.throws(()=>f.context.window.CaseEvidenceRecovery.validateStatus(value),/INVALID/);}
});

test('concurrent repeated preparation shares one native verification without duplicate destination creation',async()=>{
 const f=fixture();await f.controller.status();let resolve;f.reply(()=>new Promise(yes=>resolve=yes));const first=f.controller.prepare(uuid(1)),second=f.controller.prepare(uuid(1));resolve(prepared());await Promise.all([first,second]);assert.equal(f.calls.filter(call=>call.command==='case_recovery_prepare_restart').length,1);
});

function uiFixture(){const f=fixture(),all=node=>node.children.flatMap(child=>[child,...all(child)]);function el(tag='div',className='',text=''){let own=String(text);return{tag,className,children:[],attrs:{},isConnected:true,hidden:false,disabled:false,value:'',get textContent(){return own+this.children.map(child=>child.textContent).join('');},set textContent(value){own=String(value);this.children=[];},get firstChild(){return this.children[0];},append(...children){this.children.push(...children);},replaceChildren(...children){own='';this.children=children;},setAttribute(name,value){this.attrs[name]=value;}};}
 let next=status();const calls=[];Object.assign(f.context,{el,api:async(command,args)=>{calls.push({command,args});if(next instanceof Error)throw next;return structuredClone(next);},Date});const pane=el('section');return{...f,pane,calls,reply:value=>{next=value;},buttons:()=>all(pane).filter(node=>node.tag==='button'),select:()=>all(pane).find(node=>node.tag==='select'),draw:()=>f.context.window.CaseEvidenceRecovery.renderPane(pane)};}

test('settings recovery UI explains Case-only scope, destination and restart, preserving chosen recovery on failure',async()=>{
 const f=uiFixture();await f.draw();assert.match(f.pane.textContent,/base dos Casos, registros originais, imagens, referências, exclusões/);assert.match(f.pane.textContent,/Preferências globais.*não são copiados/);assert.match(f.pane.textContent,/integridade completa será verificada/);
 const select=f.select();select.value=uuid(1);select.onchange();f.reply(Error('verification failed'));await f.buttons().find(button=>button.textContent==='Verificar e preparar novo destino').onclick();assert.equal(f.select().value,uuid(1));assert.match(f.pane.textContent,/perfil atual continua em uso/);
 f.reply(prepared());await f.buttons().find(button=>button.textContent==='Repetir preparação').onclick();assert.match(f.pane.textContent,/\/profiles\/fresh/);assert.match(f.pane.textContent,/Feche e abra o LogInsight novamente/);assert.equal(f.select().disabled,true);assert.equal(f.calls.filter(call=>/restore_current|relaunch_elevated|update_install/.test(call.command)).length,0);
});

test('return to original verifies and schedules only the next startup while preserving both copies',async()=>{
 const f=fixture(),active=status();active.currentProfile={id:uuid(2),path:'/profiles/restored'};f.reply(active);await f.controller.status();f.reply({destination:'/profiles/original',restartRequired:true,originalPreserved:true,warning:null});await f.controller.returnOriginal();assert.deepEqual(f.calls[1],{command:'case_recovery_return_original',args:{request:{requestId:uuid(3)}}});assert.equal(f.controller.snapshot().currentProfile.path,'/profiles/restored');assert.equal(f.controller.snapshot().returnOriginalPending,true);
});

test('failed return-to-original leaves current and previous pending choice intact and retries the exact request',async()=>{
 const f=fixture(),active=status();active.currentProfile={id:uuid(2),path:'/profiles/restored'};active.pending={profileId:uuid(4),recoveryId:uuid(1),requestId:uuid(5),destination:'/profiles/another',restartRequired:true};f.reply(active);await f.controller.status();f.fail('original verification failed');await assert.rejects(f.controller.returnOriginal(),/verification/);assert.deepEqual(plain(f.controller.snapshot()),active);f.reply({destination:'/profiles/original',restartRequired:true,originalPreserved:true,warning:'Verify again at restart'});await f.controller.returnOriginal();assert.deepEqual(f.calls[1],f.calls[2]);
});

test('return action appears only on a restored profile and shows destination plus restart after confirmation',async()=>{
 const f=uiFixture();await f.draw();assert.equal(f.buttons().some(button=>button.textContent==='Voltar ao perfil original'),false);const active=status();active.currentProfile={id:uuid(2),path:'/profiles/restored'};f.reply(active);await f.draw();f.reply({destination:'/profiles/original',restartRequired:true,originalPreserved:true,warning:null});await f.buttons().find(button=>button.textContent==='Voltar ao perfil original').onclick();assert.match(f.pane.textContent,/Retorno ao perfil original preparado/);assert.match(f.pane.textContent,/cópias original e recuperada permanecem disponíveis/);assert.match(f.pane.textContent,/\/profiles\/original/);
});

for(const operation of ['prepare','return'])test(`late pre-publication status cannot erase confirmed ${operation} selection`,async()=>{
 const f=fixture(),prior=status();if(operation==='return')prior.currentProfile={id:uuid(2),path:'/profiles/restored'};f.reply(prior);await f.controller.status();let resolvePrepare,resolveStatus;
 f.reply(command=>new Promise(resolve=>{if(command==='case_recovery_status')resolveStatus=resolve;else resolvePrepare=resolve;}));
 const mutation=operation==='prepare'?f.controller.prepare(uuid(1)):f.controller.returnOriginal(),reading=f.controller.status();
 resolvePrepare(operation==='prepare'?prepared():{destination:'/profiles/original',restartRequired:true,originalPreserved:true,warning:null});await mutation;resolveStatus(prior);const result=await reading;
 if(operation==='prepare'){assert.equal(result.pending.destination,'/profiles/fresh');assert.equal(f.controller.snapshot().pending.destination,'/profiles/fresh');}else{assert.equal(result.returnOriginalPending,true);assert.equal(f.controller.snapshot().returnOriginalPending,true);}
});
