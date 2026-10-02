/* Remote browser evidence for the actual CaseReport modal/render pipeline.
   The unchanged renderer is held at its CaseTimeline.rows dependency and the
   preview save boundary. Synthetic evidence/gates do not verify the native engine.
   Contact sampling seeks only CSS timelines; the recorded full loop is real time. */
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
  phase='short-alignment';await begin();mark('short-real-render-wait');
  results.short=await scene.evaluate(async root=>{
    const art=root.querySelector('.wv-art'),animations=art.getAnimations({subtree:true});
    const result={family:root.dataset.family,pace:root.dataset.pace,durations:[...new Set(animations.map(a=>a.effect.getComputedTiming().duration))],iterations:[...new Set(animations.map(a=>String(a.effect.getTiming().iterations)))],status:root.querySelector('.wv-status').textContent,metricHidden:root.querySelector('.wv-metric').hidden};
    for(const animation of animations){animation.pause();animation.currentTime=2800*.42;}
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    result.pressTransform=getComputedStyle(root.querySelector('.wv-compose-press')).transform;
    result.heldOpacity=Number(getComputedStyle(root.querySelector('.wv-compose-held')).opacity);
    return result;
  });
  assert.equal(results.short.family,'composition');assert.equal(results.short.pace,'gesture');assert.deepEqual(results.short.durations,[2800]);assert.deepEqual(results.short.iterations,['1']);
  assert.equal(results.short.metricHidden,true);assert.equal(results.short.pressTransform,'none');assert.equal(results.short.heldOpacity,1);
  await snap('case-report-wait-short-dark.png');
  await scene.evaluate(root=>{for(const animation of root.querySelector('.wv-art').getAnimations({subtree:true})){animation.currentTime=0;animation.play();}});
  phase='long-contact';await page.waitForFunction(()=>document.querySelector('.case-report-dialog .waiting-visual')?.dataset.pace==='loop');
  results.rig=await scene.evaluate(async root=>{
    const art=root.querySelector('.wv-art'),hand=root.querySelector('.wv-task-hand'),held=root.querySelector('.wv-compose-held'),loaded=root.querySelector('.wv-compose-loaded'),press=root.querySelector('.wv-compose-press'),bed=root.querySelector('.wv-compose-bed');
    const animations=art.getAnimations({subtree:true});
    const at=async fraction=>{for(const animation of animations){animation.pause();animation.currentTime=7600*fraction;}await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));};
    const point=(node,x,y)=>{const p=new DOMPoint(x,y).matrixTransform(node.getScreenCTM());return {x:p.x,y:p.y};};
    const distance=(a,b)=>Math.hypot(a.x-b.x,a.y-b.y);
    const rig={width:art.getBoundingClientRect().width,height:art.getBoundingClientRect().height,actorCount:root.querySelectorAll('.wv-task').length,carriedByHand:held.parentElement===hand,gripBelongsToPress:root.querySelector('.wv-compose-grip').parentElement===press,durations:[...new Set(animations.map(a=>a.effect.getComputedTiming().duration))],contacts:[],grip:[],reaction:[]};
    for(const fraction of [.28,.86]){await at(fraction);rig.contacts.push({fraction,distances:[[93,52],[109,63]].map(([x,y])=>distance(point(held,x,y),point(loaded,x,y))),heldOpacity:Number(getComputedStyle(held).opacity),loadedOpacity:Number(getComputedStyle(loaded).opacity)});}
    for(let percent=38;percent<=76;percent++){await at(percent/100);rig.grip.push({percent,distance:distance(point(hand,91,55),point(press,114,46))});}
    for(const fraction of [.38,.54,.56,.62,.64,.84]){await at(fraction);rig.reaction.push({fraction,pressY:new DOMMatrix(getComputedStyle(press).transform).m42,bedY:new DOMMatrix(getComputedStyle(bed).transform).m42});}
    for(const animation of animations){animation.currentTime=0;animation.play();}return rig;
  });
  assert.equal(results.rig.actorCount,1);assert.equal(results.rig.carriedByHand,true);assert.equal(results.rig.gripBelongsToPress,true);
  assert.ok(Math.abs(results.rig.width-240)<.5&&Math.abs(results.rig.height-120)<.5);assert.deepEqual(results.rig.durations,[7600]);
  assert.ok(results.rig.contacts.every(contact=>contact.distances.every(value=>value<.15)),'the two corners of the proof meet at both handovers');
  assert.ok(results.rig.grip.every(sample=>sample.distance<.15),'press grip remains connected throughout both strokes');
  assert.equal(results.rig.contacts[0].heldOpacity,0);assert.equal(results.rig.contacts[1].loadedOpacity,0);
  const reaction=fraction=>results.rig.reaction.find(sample=>sample.fraction===fraction);
  assert.ok(Math.abs(reaction(.54).bedY)<.02);assert.ok(Math.abs(reaction(.56).bedY-.7)<.02);assert.ok(Math.abs(reaction(.84).bedY)<.02);
  phase='long-real-time-cycle';mark('long-cycle-start');
  results.cycle=await scene.evaluate(async root=>{
    const animations=root.querySelector('.wv-art').getAnimations({subtree:true});for(const animation of animations){animation.currentTime=0;animation.play();}await Promise.all(animations.map(animation=>animation.ready));
    const started=performance.now();return new Promise((resolve,reject)=>{const frame=()=>{
      const times=animations.map(animation=>animation.currentTime);
      if(!root.isConnected||root.dataset.motion!=='running')return reject(Error('Report scene stopped during real-time capture'));
      if(times.every(value=>typeof value==='number'&&value>=7600))return resolve({elapsedMs:performance.now()-started,minimumTimelineMs:Math.min(...times)});
      if(performance.now()-started>17600)return reject(Error('Report scene did not finish its real-time capture cycle'));
      requestAnimationFrame(frame);
    };requestAnimationFrame(frame);});
  });
  assert.ok(results.cycle.minimumTimelineMs>=7600);mark('long-cycle-end');await snap('case-report-wait-long-dark.png');
  phase='pause-and-context';await scene.locator('.wv-motion-toggle').focus();await page.keyboard.press('Enter');await motion('static');
  assert.equal(await scene.locator('.wv-art').evaluate(node=>node.getAnimations({subtree:true}).filter(animation=>animation.playState==='running').length),0);
  assert.equal(await page.evaluate(()=>__reportWaitGate.pendingRows.length),1,'pausing artwork does not cancel rendering');
  await scene.locator('.wv-motion-toggle').focus();await page.keyboard.press('Enter');await motion('running');
  await page.setViewportSize({width:1024,height:800});await page.evaluate(()=>{if(document.documentElement.dataset.theme!=='light')toggleTheme();});
  await page.evaluate(async()=>{const animations=document.getAnimations().filter(a=>a instanceof CSSTransition);await Promise.all(animations.map(a=>a.finished.catch(()=>{})));});
  const bounds=await scene.boundingBox();assert.ok(bounds.x>=0&&bounds.x+bounds.width<=1024&&bounds.y>=0&&bounds.y+bounds.height<=800);await snap('case-report-wait-long-light-1024.png');
  await page.emulateMedia({forcedColors:'active'});await snap('case-report-wait-forced-colors.png');await page.emulateMedia({forcedColors:'none',reducedMotion:'reduce'});await motion('static');
  assert.equal(await scene.locator('.wv-motion-toggle').isVisible(),false);await snap('case-report-wait-reduced-motion.png');
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
