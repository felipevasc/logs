/* Compatibility transfer on real Chromium with moveBefore deliberately absent.
   The harness never seeks. Production restores clocks after legacy DOM append;
   new CSSAnimation identities are expected. This is not a WebKit/WebView run. */
import assert from 'node:assert/strict';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
const output=resolve('output/playwright');mkdirSync(output,{recursive:true});
const report={evidence:{component:'Production WaitingVisuals',receipts:'Synthetic fixtures',compatibility:'moveBefore disabled in Chromium; production transfers clocks',nativeEngineVerified:false,installedWebViewVerified:false},scenarios:[],errors:[]};
let browser,context,page,video;
try {
  browser=await launchBrowser();context=await browser.newContext({viewport:{width:1280,height:880},reducedMotion:'no-preference',recordVideo:{dir:resolve(output,'completion-fallback-video'),size:{width:1280,height:880}}});
  page=await context.newPage();video=page.video();page.on('pageerror',error=>report.errors.push(error.message));
  await page.setContent('<html><body><h1>Compatibilidade · continuação do relógio original</h1><main></main><button id="result">Resultado disponível</button></body></html>');
  await page.addStyleTag({content:'body{background:#0e131b;color:#e6eaf2;font:16px system-ui;padding:30px}main{width:330px;padding:22px;border:1px solid #3e5065;border-radius:12px}h1{font-size:20px}#result{position:fixed;right:14px;bottom:14px;width:180px;height:130px}'});
  await page.addStyleTag({content:readFileSync('frontend/waiting-visuals.css','utf8')});
  await page.addScriptTag({content:readFileSync('frontend/waiting-visuals.js','utf8')});
  report.capabilities=await page.evaluate(()=>({nativeAtomicMove:typeof document.body.moveBefore==='function',userAgent:navigator.userAgent}));
  await page.evaluate(()=>{
    Object.defineProperty(document.body,'moveBefore',{value:undefined,configurable:true});
    window.captureTracks=()=>tail.element.querySelector('.wv-art').getAnimations({subtree:true});
    window.describeTracks=tracks=>tracks.map(a=>({name:a.animationName,target:a.effect.target.classList.value,time:a.currentTime,state:a.playState,rate:a.playbackRate,start:a.startTime,transform:getComputedStyle(a.effect.target).transform}));
    window.transfer=()=>{
      const before=captureTracks(),details=describeTracks(before),focus=document.activeElement;
      const adapter=tail.element.dataset.adapter;
      const clockName=adapter==='react'?'wv-episode-boundary':['prepare','resume'].includes(adapter)?`wv-${adapter}-boundary`:tail.element.dataset.pace==='gesture'?'wv-gesture-boundary':'wv-work-boundary';
      const clock=before.find(a=>a.animationName===clockName);
      const duration=clock.effect.getComputedTiming().duration;
      const expectedRemainingMs=clock.animationName==='wv-work-boundary'?duration-clock.currentTime%duration:duration-clock.currentTime;
      window.completionStarted=performance.now();const continued=tail.complete(),after=captureTracks();
      window.tailEvents=[];window.tailRemovedAt=null;
      tail.element.addEventListener('animationiteration',e=>tailEvents.push({type:e.type,name:e.animationName,elapsedTime:e.elapsedTime,trusted:e.isTrusted,afterMs:performance.now()-completionStarted}),true);
      tail.element.addEventListener('animationend',e=>tailEvents.push({type:e.type,name:e.animationName,elapsedTime:e.elapsedTime,trusted:e.isTrusted,afterMs:performance.now()-completionStarted}),true);
      window.removalObserver=new MutationObserver(()=>{if(!tail.element.isConnected&&tailRemovedAt===null)tailRemovedAt=performance.now()-completionStarted;});
      removalObserver.observe(document.body,{childList:true});
      return{continued,transfer:tail.element.dataset.completionTransfer,inert:tail.element.inert,parentIsBody:tail.element.parentElement===document.body,
        sameFocus:focus===document.activeElement,pointerEvents:getComputedStyle(tail.element).pointerEvents,
        sameAnimations:before.length===after.length&&after.every((a,i)=>a===before[i]),before:details,after:describeTracks(after),expectedRemainingMs};
    };
    document.querySelector('#result').onclick=()=>window.clicked=true;
  });
  for(const scenario of ['short-gesture','later-work-loop','coffee-react','bridge-prepare','bridge-resume']) {
    await page.evaluate(scenario=>{
      window.clicked=false;document.querySelector('#result').focus();
      const reaction=scenario==='coffee-react'?'coffee':scenario.startsWith('bridge-')?'wave':null;
      const receipt={operationId:scenario,phaseId:reaction?'command:case_report_render':'engine-index',state:'running',elapsedMs:scenario==='short-gesture'?0:reaction?60000:9000,label:scenario};
      let reactionSeed;
      if(reaction) {
        for(let seed=0;seed<4096;seed++)if(WaitingVisuals.createDirector(receipt.operationId,seed).boundary(WaitingVisuals.derive(receipt))?.episode===reaction){reactionSeed=seed;break;}
        if(reactionSeed===undefined)throw Error(`No seed for ${reaction}`);
      }
      tail=WaitingVisuals.mount(document.querySelector('main'),receipt,{reactionSeed});
      if(scenario==='later-work-loop')tail.update({...receipt,phaseId:'engine-checkpoint-committed'});
      window.stages=[];window.stageObserver=new MutationObserver(()=>stages.push({episode:tail.element.dataset.episode,adapter:tail.element.dataset.adapter,at:performance.now()}));
      stageObserver.observe(tail.element,{attributes:true,attributeFilter:['data-episode','data-adapter']});
    },scenario);
    await page.waitForFunction(()=>tail.element.dataset.motion==='running');
    if(scenario==='later-work-loop')await page.waitForFunction(()=>captureTracks().some(a=>a.animationName==='wv-work-boundary'&&a.currentTime>8000));
    else if(scenario==='coffee-react')await page.waitForFunction(()=>tail.element.dataset.episode==='coffee'&&tail.element.dataset.adapter==='react');
    else if(scenario.startsWith('bridge-'))await page.waitForFunction(adapter=>tail.element.dataset.episode==='wave'&&tail.element.dataset.adapter===adapter,scenario.slice(7));
    await page.waitForTimeout(500);
    const result={scenario,handoff:await page.evaluate(()=>transfer())};report.scenarios.push(result);
    const handoff=result.handoff;
    assert.equal(handoff.continued,true);assert.equal(handoff.transfer,'clock-transfer');assert.equal(handoff.sameAnimations,false,'legacy append necessarily replaces CSSAnimation objects');
    for(const key of ['inert','parentIsBody','sameFocus'])assert.equal(handoff[key],true,key);
    assert.equal(handoff.pointerEvents,'none');assert.equal(handoff.after.length,handoff.before.length);
    for(let i=0;i<handoff.before.length;i++) {
      const a=handoff.before[i],b=handoff.after[i];assert.equal(b.name,a.name);assert.equal(b.target,a.target);assert.equal(b.state,a.state);assert.equal(b.rate,1);
      assert.ok(Math.abs(b.time-a.time)<50,'legacy transfer keeps elapsed time, including completed iterations');
      assert.equal(b.transform,a.transform,'no pose reset is visible before rendering');
    }
    await page.locator('#result').click();assert.equal(await page.evaluate(()=>clicked),true);
    await page.waitForTimeout(350);
    assert.equal(await page.evaluate(()=>tail.element.isConnected),true,'restored prior iterations must not prematurely remove the continuation');
    result.after350ms=await page.evaluate(()=>describeTracks(captureTracks()));
    for(const track of result.after350ms) {
      const before=handoff.after.find(a=>a.name===track.name&&a.target===track.target);assert.ok(before);
      if(before.state==='running')assert.ok(track.time>before.time+250,'active tracks continue after legacy transfer');
      else {
        assert.equal(track.state,before.state);
        assert.ok(Math.abs(track.time-before.time)<1,'parked or finished work keeps its final time and pose');
        assert.equal(track.transform,before.transform);
      }
    }
    if(scenario==='coffee-react')assert.ok(handoff.before.some(a=>a.state==='paused'),'exercise parked work as well as the active reaction');
    await page.screenshot({path:resolve(output,`waiting-completion-fallback-${scenario}.png`),animations:'allow'});
    await page.waitForFunction(()=>!tail.element.isConnected,null,{timeout:45000});
    Object.assign(result,await page.evaluate(()=>{removalObserver.disconnect();stageObserver.disconnect();return{removedAfterMs:tailRemovedAt,events:tailEvents,stages};}));
    const extra=scenario==='coffee-react'?3200:scenario==='bridge-prepare'?3600+3200:0;
    assert.ok(Math.abs(result.removedAfterMs-handoff.expectedRemainingMs-extra)<250,'only the original remainder and required return bridge may run');
    if(scenario==='short-gesture')assert.ok(result.events.some(e=>e.name==='wv-gesture-boundary'&&e.trusted&&e.elapsedTime===2.4));
    if(scenario==='later-work-loop')assert.ok(result.events.some(e=>e.name==='wv-work-boundary'&&e.trusted&&e.elapsedTime===14.4),'the second complete work boundary settles the continuation');
    if(scenario==='coffee-react') {
      assert.ok(result.stages.some(s=>s.adapter==='resume'));assert.ok(result.stages.every(s=>s.episode==='coffee'));
      assert.ok(result.events.some(e=>e.name==='wv-episode-boundary'&&e.trusted&&e.elapsedTime===32));
      assert.ok(result.events.some(e=>e.name==='wv-resume-boundary'&&e.trusted&&e.elapsedTime===3.2));
    }
    if(scenario.startsWith('bridge-')) {
      assert.ok(result.stages.every(s=>s.episode==='wave'));
      assert.ok(result.events.some(e=>e.name==='wv-resume-boundary'&&e.trusted&&e.elapsedTime===3.2));
      if(scenario==='bridge-prepare') {
        assert.ok(result.events.some(e=>e.name==='wv-prepare-boundary'&&e.trusted&&e.elapsedTime===3.2));
        assert.ok(result.events.some(e=>e.name==='wv-episode-boundary'&&e.trusted&&e.elapsedTime===3.6));
      }
    }
  }
  report.cleanup=[];
  for(const mode of ['new-operation','context-change','reduced-motion']) {
    await page.evaluate(()=>{tail=WaitingVisuals.mount(document.querySelector('main'),{operationId:'cleanup',phaseId:'engine-index',state:'running',elapsedMs:0});});
    await page.waitForFunction(()=>tail.element.dataset.motion==='running');await page.waitForTimeout(150);
    assert.equal(await page.evaluate(()=>tail.complete()),true);
    if(mode==='new-operation')await page.evaluate(()=>{window.nextTail=WaitingVisuals.mount(document.querySelector('main'),{operationId:'next',phaseId:'engine-index',state:'running',elapsedMs:0});});
    else if(mode==='context-change')await page.evaluate(()=>document.dispatchEvent(new CustomEvent('workspace-context-change')));
    else {
      // Media changes are delivered by the browser rendering/event-loop step,
      // not by the protocol acknowledgement returned from emulateMedia().
      await page.evaluate(()=>{
        window.reducedCompletionEvent=null;
        window.completionMediaProbe=matchMedia('(prefers-reduced-motion: reduce)');
        if(completionMediaProbe.matches)throw Error('Reduced-motion probe must begin unmatched');
        completionMediaProbe.addEventListener('change',event=>{
          reducedCompletionEvent={trusted:event.isTrusted,matches:event.matches,connectedAtChange:tail.element.isConnected};
        },{once:true});
      });
      await page.emulateMedia({reducedMotion:'reduce'});
      await page.waitForFunction(()=>window.reducedCompletionEvent!==null,null,{timeout:1000});
      report.reducedCompletion=await page.evaluate(()=>reducedCompletionEvent);
      assert.equal(report.reducedCompletion.trusted,true);
      assert.equal(report.reducedCompletion.matches,true);
      // CSSOM dispatches MediaQueryList changes in creation order. Production's
      // earlier listener must already have removed the tail when this probe runs.
      assert.equal(report.reducedCompletion.connectedAtChange,false,'production change listener must synchronously remove completion');
    }
    assert.equal(await page.evaluate(()=>tail.element.isConnected),false,mode);
    if(mode==='new-operation')await page.evaluate(()=>nextTail.destroy());
    report.cleanup.push(mode);
  }
  assert.deepEqual(report.errors,[]);report.ok=true;
} catch(error) {
  report.ok=false;report.error=String(error.stack||error);process.exitCode=1;
  if(page)await page.screenshot({path:resolve(output,'waiting-completion-fallback-failure.png'),animations:'allow'}).catch(()=>{});
} finally {
  await context?.close();
  if(video){report.video='waiting-completion-fallback-real-preview.webm';await video.saveAs(resolve(output,report.video));await video.delete();}
  await browser?.close();writeFileSync(resolve(output,'waiting-completion-fallback-results.json'),JSON.stringify(report,null,2));
}
