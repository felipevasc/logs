/* Remote browser evidence for the actual CaseReport modal/render pipeline.
   The unchanged renderer is held at its CaseTimeline.rows dependency and the
   preview save boundary. Synthetic evidence/gates do not verify the native engine.
   Robot skits play in natural real time. */
import assert from 'node:assert/strict';
import { mkdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
const output=resolve('output/playwright');mkdirSync(output,{recursive:true});
const browser=await launchBrowser(),context=await browser.newContext({viewport:{width:1440,height:960},reducedMotion:'no-preference',recordVideo:{dir:resolve(output,'video-raw'),size:{width:1440,height:960}}});
const started=Date.now(),page=await context.newPage(),video=page.video(),errors=[];
const results={evidence:{app:'Actual CaseReport open/render/save and WaitingVisuals DOM/CSS',gates:'Test-only waits at CaseTimeline.rows and the preview export_timeline boundary',data:'Small explicit synthetic Case; generated PDF bytes and captured application pixels are real',nativeEngineVerified:false,generatedImageAssets:false},screenshots:[],markers:[]};
const mark=label=>results.markers.push({label,offsetMs:Date.now()-started});
const scene=page.locator('.case-report-dialog .waiting-visual'),dialog=page.locator('.case-report-dialog');
const motion=state=>page.waitForFunction(state=>document.querySelector('.case-report-dialog .waiting-visual')?.dataset.motion===state,state);
let phase='startup',failure=null;
page.setDefaultTimeout(20000);page.on('pageerror',error=>errors.push(error.message));
const snap=async name=>{await page.screenshot({path:resolve(output,name),animations:'allow'});results.screenshots.push({name,viewport:page.viewportSize(),theme:await page.evaluate(()=>document.documentElement.dataset.theme)});};
const begin=async()=>{
  await page.evaluate(()=>{__reportWaitGate.holdRows=true;CaseReport.open();});
  await dialog.locator('[data-generate]').click();
  await page.waitForFunction(()=>__reportWaitGate.pendingRows.length===1);
  await scene.waitFor({state:'visible'});await motion('running');
};
try {
  await page.goto(process.argv[2]||'http://127.0.0.1:4173');
  await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&document.querySelector('#load-overlay').hidden&&window.CaseReport);
  await page.evaluate(async()=>{
    const c=activeCase();c.name='Composição de relatório';c.items=[];c.caseTrails=[];
    // Warm the real local fonts/PDF dependencies before timing the short UI gesture.
    const warm=await CaseReport.render(structuredClone(c));if(warm.blob.size<100)throw Error('Real PDF warmup failed');
    window.__timelineExportMock={files:[],cancel:false,download:false};
    const originalRows=CaseTimeline.rows,originalApi=api;
    const gate=window.__reportWaitGate={holdRows:false,holdSave:false,failRows:false,pendingRows:[],pendingSave:[],saveEntries:[],aborted:0,
      releaseRows(){this.holdRows=false;for(const resume of this.pendingRows.splice(0))resume();},
      releaseSave(){this.holdSave=false;for(const resume of this.pendingSave.splice(0))resume();}};
    CaseTimeline.rows=async(...args)=>{
      if(gate.holdRows)await new Promise((resolve,reject)=>{
        const signal=args[2]?.signal;
        const finish=()=>{signal?.removeEventListener('abort',cancel);gate.pendingRows=gate.pendingRows.filter(value=>value!==finish);resolve();};
        const cancel=()=>{gate.aborted++;signal?.removeEventListener('abort',cancel);gate.pendingRows=gate.pendingRows.filter(value=>value!==finish);reject(new DOMException('Geração cancelada','AbortError'));};
        if(signal?.aborted)return cancel();gate.pendingRows.push(finish);signal?.addEventListener('abort',cancel,{once:true});
      });
      if(gate.failRows)throw Error('Falha de leitura do Caso (fixture de teste).');
      return originalRows(...args);
    };
    api=async(command,args,options)=>{
      if(command==='export_timeline') {
        gate.saveEntries.push({sceneCount:document.querySelectorAll('.case-report-dialog .waiting-visual').length,filename:args.filename});
        if(gate.holdSave)await new Promise(resolve=>gate.pendingSave.push(resolve));
      }
      return originalApi(command,args,options);
    };
  });
  phase='room';await begin();mark('real-render-wait');
  results.room=await scene.evaluate(root=>({family:root.dataset.family,ring:!!root.querySelector('.wv-ring-arc'),buttons:root.querySelectorAll('button').length,
    size:root.querySelector('.wv-art').getBoundingClientRect().width,status:root.querySelector('.wv-status').textContent,metricHidden:root.querySelector('.wv-metric').hidden}));
  assert.equal(results.room.family,'composition');assert.equal(results.room.ring,true);assert.equal(results.room.buttons,0);
  assert.ok(Math.abs(results.room.size-100)<1);assert.equal(results.room.metricHidden,true);
  // A whole skit begins shortly after the loader and runs on its own clock.
  await page.waitForFunction(()=>document.querySelector('.case-report-dialog .waiting-visual')?.dataset.skit,null,{timeout:4000});
  await page.waitForTimeout(1600);
  results.skit=await scene.evaluate(root=>({skit:root.dataset.skit,running:root.querySelector('.wv-actors').getAnimations({subtree:true}).filter(a=>a.playState==='running').length}));
  assert.ok(results.skit.running>5);mark('skit-playing');await snap('case-report-wait-dark.png');
  phase='pause-and-context';await scene.evaluate(root=>{root.style.transform='translateX(200vw)';});await motion('static');
  assert.equal(await scene.locator('.wv-actors').evaluate(node=>node.getAnimations({subtree:true}).filter(animation=>animation.playState==='running').length),0);
  assert.equal(await page.evaluate(()=>__reportWaitGate.pendingRows.length),1,'pausing artwork does not cancel rendering');
  await scene.evaluate(root=>{root.style.transform='';});await motion('running');
  await page.setViewportSize({width:1024,height:800});await page.evaluate(()=>{if(document.documentElement.dataset.theme!=='light')toggleTheme();});
  await page.evaluate(async()=>{const animations=document.getAnimations().filter(a=>a instanceof CSSTransition);await Promise.all(animations.map(a=>a.finished.catch(()=>{})));});
  const bounds=await scene.boundingBox();assert.ok(bounds.x>=0&&bounds.x+bounds.width<=1024&&bounds.y>=0&&bounds.y+bounds.height<=800);await snap('case-report-wait-long-light-1024.png');
  await page.emulateMedia({forcedColors:'active'});await snap('case-report-wait-forced-colors.png');await page.emulateMedia({forcedColors:'none',reducedMotion:'reduce'});await motion('static');
  assert.equal(await scene.locator('button').count(),0);await snap('case-report-wait-reduced-motion.png');
  await page.emulateMedia({reducedMotion:'no-preference'});await motion('running');
  await scene.evaluate(root=>{root.style.transform='translateX(200vw)';});await motion('static');await scene.evaluate(root=>{root.style.transform='';});await motion('running');
  const savesBefore=await page.evaluate(()=>__reportWaitGate.saveEntries.length);
  await dialog.locator('[data-cancel]').click();await dialog.waitFor({state:'detached'});await page.waitForFunction(()=>__reportWaitGate.pendingRows.length===0);
  assert.equal(await scene.count(),0);assert.equal(await page.evaluate(()=>__reportWaitGate.saveEntries.length),savesBefore);
  results.cancel={aborted:await page.evaluate(()=>__reportWaitGate.aborted),saved:false};
  phase='error-retry-save';await begin();await page.evaluate(()=>{__reportWaitGate.failRows=true;__reportWaitGate.releaseRows();});
  await dialog.locator('[data-status].case-report-error').waitFor({state:'visible'});assert.match(await dialog.locator('[data-status]').textContent(),/Falha de leitura do Caso/);assert.equal(await scene.count(),0);
  await snap('case-report-wait-error.png');
  await page.evaluate(()=>{__reportWaitGate.failRows=false;__reportWaitGate.holdRows=true;__reportWaitGate.holdSave=true;__timelineExportMock.cancel=true;});
  await dialog.locator('[data-generate]').click();await scene.waitFor({state:'visible'});await motion('running');
  await page.evaluate(()=>{activeCase().name='Nome alterado após o snapshot';__reportWaitGate.releaseRows();});
  await page.waitForFunction(()=>__reportWaitGate.pendingSave.length===1);assert.equal(await scene.count(),0);assert.equal(await dialog.locator('[data-status]').textContent(),'Salvando PDF…');
  assert.equal(await page.evaluate(()=>__reportWaitGate.saveEntries.at(-1).sceneCount),0);assert.match(await page.evaluate(()=>__reportWaitGate.saveEntries.at(-1).filename),/^relatorio-composicao-de-relatorio-/);
  await dialog.locator('[data-cancel]').click();assert.equal(await dialog.count(),1,'save guard preserves the existing non-cancellable write');
  await page.evaluate(()=>__reportWaitGate.releaseSave());await page.waitForFunction(()=>document.querySelector('.case-report-dialog [data-status]')?.textContent==='Salvamento cancelado.');
  assert.equal(await scene.count(),0);await dialog.locator('[data-cancel]').click();await dialog.waitFor({state:'detached'});
  phase='settlement';await page.evaluate(()=>{activeCase().name='Composição de relatório';CaseReport.open();});await dialog.locator('[data-generate]').click();await dialog.waitFor({state:'detached'});
  const file=await page.evaluate(()=>__timelineExportMock.files.at(-1));const bytes=Buffer.from(file.base64,'base64');assert.equal(bytes.subarray(0,5).toString(),'%PDF-');
  results.output={filename:file.filename,bytes:bytes.length};writeFileSync(resolve(output,'case-report-wait-output.pdf'),bytes);
  assert.equal(await scene.count(),0);assert.deepEqual(errors,[]);results.ok=true;mark('finished');
} catch(error) {
  failure=error;results.ok=false;results.error=String(error.stack||error);await captureFailure(page,'case-report-waiting',error,{phase,errors,results});throw error;
} finally {
  results.phase=phase;results.errors=errors;
  try {await context.close();if(!video)throw Error('Report recording is unavailable');const filename='case-report-waiting-real-preview.webm';await video.saveAs(resolve(output,filename));results.video={filename,bytes:statSync(resolve(output,filename)).size,finalized:true};await video.delete();}
  catch(error){results.videoError=String(error);if(!failure)process.exitCode=1;}
  writeFileSync(resolve(output,'case-report-waiting-results.json'),JSON.stringify(results,null,2));
  await browser.close();
}
