/* CI-only natural playback of the production component with synthetic receipts.
   No native-engine/installed-WebView claim. Never seek or change playback rates. */
import assert from 'node:assert/strict';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { launchBrowser } from './browser.mjs';
const output=resolve('output/playwright'); mkdirSync(output,{recursive:true});
const report={ evidence:{component:'Production WaitingVisuals SVG/CSS/controller',receipts:'Synthetic fixtures',timing:'Natural1x; no seek or playback-rate changes',nativeEngineVerified:false,installedWebViewVerified:false},errors:[] };
let browser,context,page,video;
try {
  browser=await launchBrowser(); context=await browser.newContext({viewport:{width:1280,height:880},reducedMotion:'no-preference',recordVideo:{dir:resolve(output,'playback-video'),size:{width:1280,height:880}}});
  page=await context.newPage(); video=page.video(); page.on('pageerror',error=>report.errors.push(error.message));
  await page.setContent('<html><head></head><body><h1>Reprodução contínua · componente real · recibos sintéticos</h1><main></main></body></html>');
  await page.addStyleTag({content:'body{background:#0e131b;color:#e6eaf2;font:16px system-ui;padding:30px}h1{font-size:20px}main{display:grid;grid-template-columns:repeat(3,1fr);gap:24px}section{padding:22px;border:1px solid #3e5065;border-radius:12px;background:#141b25}h2{font-size:14px;font-weight:normal}'});
  await page.addStyleTag({content:readFileSync('frontend/waiting-visuals.css','utf8')});
  await page.addScriptTag({content:readFileSync('frontend/waiting-visuals.js','utf8')});
  report.capabilities=await page.evaluate(()=>({atomicMove:typeof document.body.moveBefore==='function',userAgent:navigator.userAgent}));
  assert.equal(report.capabilities.atomicMove,true,'this Chromium gate must exercise identity-preserving atomic moves; legacy fallback has its own gate');
  await page.evaluate(()=>{
    window.playback={entries:[],events:[],started:performance.now()};
    for(const [name,elapsedMs] of [['quiet',0],['sparse',9000],['churn',9000]]) {
      const host=document.createElement('section'),heading=document.createElement('h2'); heading.textContent=name; host.append(heading); document.querySelector('main').append(host);
      const receipt={operationId:name,phaseId:'engine-index',state:'running',label:name==='quiet'?'Operação silenciosa':name==='sparse'?'Tempo omitido nas atualizações':'Troca rápida de fases',elapsedMs,completed:12,total:99,unit:'registros'};
      const view=WaitingVisuals.mount(host,receipt), entry={name,view,receipt,svg:view.element.querySelector('svg'),replaced:0};
      entry.observer=new MutationObserver(()=>{const svg=view.element.querySelector('svg');if(svg!==entry.svg){entry.svg=svg;entry.replaced++;}});
      entry.observer.observe(view.element,{childList:true,subtree:true});
      for(const type of ['animationend','animationiteration','animationcancel']) view.element.addEventListener(type,event=>{
        if(event.animationName.includes('boundary')) playback.events.push({name,type,animationName:event.animationName,trusted:event.isTrusted,elapsedTime:event.elapsedTime,ms:performance.now()-playback.started});
      },true);
      playback.entries.push(entry);
    }
    playback.sample=()=>playback.entries.map(e=>({name:e.name,family:e.view.element.dataset.family,pace:e.view.element.dataset.pace,motion:e.view.element.dataset.motion,episode:e.view.element.dataset.episode,replaced:e.replaced,
      animations:e.view.element.querySelector('.wv-art').getAnimations({subtree:true}).map(a=>({name:a.animationName,time:a.currentTime,duration:a.effect.getComputedTiming().duration,playState:a.playState,rate:a.playbackRate}))}));
    playback.timer=setInterval(()=>{
      const sparse=playback.entries[1];sparse.view.update({...sparse.receipt,elapsedMs:undefined});
      const churn=playback.entries[2],time=performance.now()-playback.started;
      churn.view.update({...churn.receipt,phaseId:Math.floor(time/300)%2?'engine-checkpoint-publish':'engine-index',completed:Math.floor(time/300)});
    },300);
  });
  await page.waitForTimeout(11000);report.at11s=await page.evaluate(()=>playback.sample());
  await page.screenshot({path:resolve(output,'waiting-playback-continuous.png'),animations:'allow'});
  await page.waitForTimeout(2500);report.at13s=await page.evaluate(()=>playback.sample());
  report.events=await page.evaluate(()=>{clearInterval(playback.timer);return playback.events;});
  for(const sample of report.at13s) {
    assert.equal(sample.pace,'loop');assert.ok(sample.animations.some(a=>a.playState==='running'));assert.ok(sample.animations.every(a=>a.rate===1));
    assert.ok(report.events.some(e=>e.name===sample.name&&e.type==='animationiteration'&&e.trusted),`${sample.name} completed a real cycle`);
  }
  assert.ok(report.at13s[2].replaced<=2,'phase churn is coalesced at complete boundaries');
  await page.evaluate(()=>{
    playback.entries.forEach(e=>{e.observer.disconnect();e.view.destroy();});document.querySelector('main').replaceChildren();
    const host=document.createElement('section');document.querySelector('main').append(host);
    const target=document.createElement('button');target.id='tail-target';target.textContent='Resultado já disponível';
    Object.assign(target.style,{position:'fixed',right:'14px',bottom:'14px',width:'180px',height:'130px'});
    target.onclick=()=>{window.tailClicked=true;};document.body.append(target);target.focus();
    window.tail=WaitingVisuals.mount(host,{operationId:'quick-success',phaseId:'engine-index',state:'running',label:'Tarefa curta',elapsedMs:0});
  });
  await page.waitForFunction(()=>tail.element.dataset.motion==='running');await page.waitForTimeout(500);
  report.tailHandoff=await page.evaluate(()=>{
    const art=tail.element.querySelector('.wv-art'),before=art.getAnimations({subtree:true}),times=before.map(a=>a.currentTime),focus=document.activeElement;
    const clock=before.find(a=>a.animationName==='wv-gesture-boundary'),expectedRemainingMs=2400-clock.currentTime;
    const started=performance.now(),continued=tail.complete(),after=art.getAnimations({subtree:true});window.tailBoundary=null;
    tail.element.addEventListener('animationend',event=>{if(event.animationName==='wv-gesture-boundary')tailBoundary={trusted:event.isTrusted,elapsedTime:event.elapsedTime,afterCompletionMs:performance.now()-started};},true);
    return{continued,inert:tail.element.inert,parentIsBody:tail.element.parentElement===document.body,sameFocus:focus===document.activeElement,
      sameAnimations:before.length===after.length&&after.every((a,i)=>a===before[i]),beforeTimes:times,afterTimes:after.map(a=>a.currentTime),expectedRemainingMs,
      transfer:tail.element.dataset.completionTransfer,pointerEvents:getComputedStyle(tail.element).pointerEvents};
  });
  for(const key of ['continued','inert','parentIsBody','sameFocus','sameAnimations'])assert.equal(report.tailHandoff[key],true,key);
  assert.equal(report.tailHandoff.pointerEvents,'none');
  assert.equal(report.tailHandoff.transfer,'atomic');
  assert.ok(report.tailHandoff.afterTimes.every((t,i)=>t>=report.tailHandoff.beforeTimes[i]),'moving to the completion region never rewinds a track');
  await page.locator('#tail-target').click();assert.equal(await page.evaluate(()=>tailClicked),true,'real result remains clickable through the inert continuation');
  await page.screenshot({path:resolve(output,'waiting-playback-short-completion.png'),animations:'allow'});
  await page.waitForFunction(()=>!tail.element.isConnected);report.tailBoundary=await page.evaluate(()=>tailBoundary);
  assert.equal(report.tailBoundary.trusted,true);assert.equal(report.tailBoundary.elapsedTime,2.4);
  assert.ok(Math.abs(report.tailBoundary.afterCompletionMs-report.tailHandoff.expectedRemainingMs)<250,'only the original remaining gesture completes; a full restart must fail');
  // Longest adapter: park proof, complete coffee, return, recover proof. Natural1x.
  await page.evaluate(()=>{
    const receipt={operationId:'complete-coffee',phaseId:'command:case_report_render',state:'running',elapsedMs:60000,label:'Compondo relatório'};
    const model=WaitingVisuals.derive(receipt);let reactionSeed;
    for(let seed=0;seed<4096;seed++)if(WaitingVisuals.createDirector(receipt.operationId,seed).boundary(model)?.episode==='coffee'){reactionSeed=seed;break;}
    if(reactionSeed===undefined)throw Error('No coffee seed');
    tail=WaitingVisuals.mount(document.querySelector('main'),receipt,{reactionSeed});window.tailStages=[];
    window.tailObserver=new MutationObserver(()=>tailStages.push({episode:tail.element.dataset.episode,adapter:tail.element.dataset.adapter,at:performance.now()}));
    tailObserver.observe(tail.element,{attributes:true,attributeFilter:['data-episode','data-adapter']});
  });
  await page.waitForFunction(()=>tail.element.dataset.episode==='coffee'&&tail.element.dataset.adapter==='prepare');
  report.coffeeCompletedAt=await page.evaluate(()=>{const time=performance.now();if(!tail.complete())throw Error('Continuation did not start');return time;});
  await page.waitForFunction(()=>!tail.element.isConnected,null,{timeout:45000});
  report.coffeeStages=await page.evaluate(()=>{tailObserver.disconnect();return tailStages;});
  assert.ok(report.coffeeStages.some(s=>s.adapter==='react'));assert.ok(report.coffeeStages.some(s=>s.adapter==='resume'));
  assert.ok(report.coffeeStages.every(s=>s.episode==='coffee'),'success never schedules a new work loop or reaction');
  for(const mode of ['new-operation','context-change','reduced-motion']) {
    await page.evaluate(()=>{tail=WaitingVisuals.mount(document.querySelector('main'),{operationId:'cleanup',phaseId:'engine-index',state:'running',elapsedMs:9000});});
    await page.waitForFunction(()=>tail.element.dataset.motion==='running');await page.evaluate(()=>tail.complete());
    if(mode==='new-operation')await page.evaluate(()=>{window.nextTail=WaitingVisuals.mount(document.querySelector('main'),{operationId:'new',phaseId:'engine-index',state:'running',elapsedMs:9000});});
    else if(mode==='context-change')await page.evaluate(()=>document.dispatchEvent(new CustomEvent('workspace-context-change')));
    else await page.emulateMedia({reducedMotion:'reduce'});
    assert.equal(await page.evaluate(()=>tail.element.isConnected),false,mode);
    if(mode==='new-operation')await page.evaluate(()=>nextTail.destroy());
  }
  report.completionCleanup=true;assert.deepEqual(report.errors,[]);report.ok=true;
} catch(error) {
  report.ok=false;report.error=String(error.stack||error);process.exitCode=1;
  if(page)await page.screenshot({path:resolve(output,'waiting-playback-failure.png'),animations:'allow'}).catch(()=>{});
} finally {
  await context?.close();
  if(video){report.video='waiting-playback-real-preview.webm';await video.saveAs(resolve(output,report.video));await video.delete();}
  await browser?.close();writeFileSync(resolve(output,'waiting-playback-results.json'),JSON.stringify(report,null,2));
}
