import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
const url=process.argv[2]||'http://127.0.0.1:4174';mkdirSync('output/playwright',{recursive:true});
const browser=await chromium.launch({executablePath:existsSync(chromium.executablePath())?undefined:existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined}),page=await browser.newPage({viewport:{width:1280,height:900}}),errors=[];
page.on('pageerror',error=>errors.push(error.message));
try{
 await page.addInitScript(()=>{
  // Select only mock command transport. Production scripts activate controllers.
  window.__mockNativeCaseBootstrapEnabled=true;
  document.addEventListener('DOMContentLoaded',()=>{
   window.__nativeStartupOriginals={factory:nativeEvidenceServices,client:CaseEvidence.create,session:CaseEvidenceSession.create,workspace:WorkspaceContext.replaceCases,adopt:AnalysisContexts.adopt};
   window.__nativeStartupEvents=[];
   document.addEventListener('analysis-context-change',event=>window.__nativeStartupEvents.push({kind:'context',revision:event.detail.snapshot.configRevision}));
   document.addEventListener('case-evidence-state',event=>window.__nativeStartupEvents.push({kind:event.detail.kind,revision:event.detail.store?.store?.revision}));
  },{once:true});
 });
 await page.goto(url);
 await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&window.CaseEvidence?.active===true&&nativeEvidenceServices().session.isClean());
 const initial=await page.evaluate(async()=>{
  await window.workspaceBootstrap;
  const services=nativeEvidenceServices();window.__nativeStartupServices=services;
  return{active:CaseEvidence.active,view:state.cases.evidenceViewVersion,id:activeCase().id,name:activeCase().name,scope:WorkspaceContext.scope(),ready:WorkspaceContext.ready,
   context:AnalysisContexts.identity(activeCase().id),reference:activeCase().items[0].rows.reference,status:services.session.status(),clean:services.session.isClean(),calls:__mockNativeCaseBootstrap.calls.map(call=>({command:call.command,nativeActive:call.nativeActive})),
   sameControllers:__nativeStartupOriginals.factory===nativeEvidenceServices&&__nativeStartupOriginals.client===CaseEvidence.create&&__nativeStartupOriginals.session===CaseEvidenceSession.create&&__nativeStartupOriginals.workspace===WorkspaceContext.replaceCases&&__nativeStartupOriginals.adopt===AnalysisContexts.adopt};
 });
 assert.equal(initial.active,true);assert.equal(initial.view,1);assert.equal(initial.id,'native-startup-case');assert.equal(initial.name,'Native startup after adoption');assert.equal(initial.scope,'dataset');assert.equal(initial.status,'ready');assert.equal(initial.clean,true);assert.equal(initial.sameControllers,true);assert.equal(initial.reference.kind,'native_evidence');assert.equal(initial.reference.memberCount,2);assert.equal(initial.context.analysisId,initial.reference.owner.analysisId);assert.equal(initial.context.configRevision,0);assert.equal(initial.calls[0].command,'cases_load_view');assert.equal(initial.calls.every(call=>call.nativeActive),true);
 await page.evaluate(()=>{
  window.__nativeStartupBefore=state.cases;window.__nativeStartupQueue=[];
  window.__nativeStartupSource=WorkspaceContext.sourceChanged(async()=>{__nativeStartupQueue.push('source-enter');await new Promise(resolve=>{window.__nativeStartupRelease=resolve;});__nativeStartupQueue.push('source-exit');});
 });
 await page.waitForFunction(()=>!!window.__nativeStartupRelease&&WorkspaceContext.sourceBusy);
 const loadsBefore=await page.evaluate(()=>__mockNativeCaseBootstrap.calls.filter(call=>call.command==='cases_load_view').length);
 await page.evaluate(()=>{__mockNativeCaseBootstrap.publishName('Native reload waited for source queue');window.__nativeStartupReload=mcpReloadCases().then(()=>{__nativeStartupQueue.push('reload-done');});});
 await page.waitForFunction(count=>__mockNativeCaseBootstrap.calls.filter(call=>call.command==='cases_load_view').length>count,loadsBefore);
 assert.equal(await page.evaluate(()=>state.cases===__nativeStartupBefore&&activeCase().name==='Native startup after adoption'&&WorkspaceContext.sourceBusy),true);
 await page.evaluate(async()=>{__nativeStartupRelease();await __nativeStartupSource;await __nativeStartupReload;});
 await page.waitForFunction(()=>!WorkspaceContext.sourceBusy&&!WorkspaceContext.changing&&nativeEvidenceServices().session.isClean());
 assert.deepEqual(await page.evaluate(()=>__nativeStartupQueue),['source-enter','source-exit','reload-done']);
 assert.equal(await page.evaluate(()=>state.cases!==__nativeStartupBefore&&activeCase().name==='Native reload waited for source queue'&&nativeEvidenceServices()===__nativeStartupServices),true);
 const saved=await page.evaluate(async()=>{
  const previousRevision=state.cases.store.revision,previousContext=AnalysisContexts.identity().configRevision;
  __mockNativeCaseBootstrap.advanceContextOnNextSave();activeCase().name='Metadata saved by production native session';const confirmed=await saveCases();await nativeEvidenceServices().session.pending();
  return{confirmed,previousRevision,previousContext,revision:state.cases.store.revision,context:AnalysisContexts.identity(),clean:nativeEvidenceServices().session.isClean(),
   client:nativeEvidenceServices().client.document(),native:__mockNativeCaseBootstrap.document(),calls:__mockNativeCaseBootstrap.calls,events:__nativeStartupEvents,
   controllersUnchanged:nativeEvidenceServices()===__nativeStartupServices&&__nativeStartupOriginals.factory===nativeEvidenceServices&&__nativeStartupOriginals.adopt===AnalysisContexts.adopt};
 });
 assert.equal(saved.confirmed,true);assert.equal(saved.clean,true);assert.equal(saved.controllersUnchanged,true);assert.ok(BigInt(saved.revision)>BigInt(saved.previousRevision));assert.equal(saved.context.configRevision,saved.previousContext+1);assert.equal(saved.client.store.revision,saved.revision);assert.equal(saved.native.cases[0].name,'Metadata saved by production native session');assert.deepEqual(saved.native.cases[0].items[0].rows.reference,initial.reference);assert.ok(saved.events.some(event=>event.kind==='context'&&event.revision===saved.context.configRevision));
 assert.equal(saved.calls.some(call=>['cases_load','cases_save','case_sync'].includes(call.command)),false);
 for(const call of saved.calls.filter(call=>call.command==='cases_save_view')){assert.equal(typeof call.args.request.documentJson,'string');const document=JSON.parse(call.args.request.documentJson);assert.equal(document.evidenceViewVersion,1);assert.equal(document.cases[0].items[0].rows.kind,'native_evidence_container');assert.equal(Array.isArray(document.cases[0].items[0].rows),false);}
 assert.deepEqual(errors,[]);await page.screenshot({path:'output/playwright/native-case-startup.png'});writeFileSync('output/playwright/native-case-startup-results.json',JSON.stringify({productionActiveBootstrap:true,adoptionLoadContract:true,sourceQueueInstallation:true,metadataSave:true,configurationReceiptAdopted:true,controllersUnchanged:true,errors},null,2));
 console.log('Native Case startup: production controllers, native load, source-queue installation and metadata receipt adoption passed');
}catch(error){
 let stateSnapshot=null;try{stateSnapshot=await page.evaluate(()=>({active:window.CaseEvidence?.active,view:typeof state==='undefined'?null:state.cases?.evidenceViewVersion,workspaceReady:window.WorkspaceContext?.ready,commands:window.__mockNativeCaseBootstrap?.calls.map(call=>call.command),notices:[...document.querySelectorAll('#toast-area>*')].map(node=>node.textContent)}));}catch{}
 writeFileSync('output/playwright/native-case-startup-results.json',JSON.stringify({passed:false,error:String(error),errors,stateSnapshot},null,2));try{await page.screenshot({path:'output/playwright/native-case-startup-failed.png'});}catch{}throw error;
}finally{await browser.close();}
