/* UI contract for disk-backed pages; native Rust tests validate real storage. */
import assert from 'node:assert/strict';
import { launchBrowser } from "./browser.mjs";
const browser=await launchBrowser();
const page=await browser.newPage({viewport:{width:1400,height:1000},reducedMotion:'reduce'});
const errors=[];page.on('pageerror',e=>errors.push(e.message));
try {
  await page.goto(process.argv[2]||'http://127.0.0.1:4173');
  await page.waitForFunction(()=>WorkspaceContext.ready&&state.loaded&&!state.loadOverlay);
  await page.evaluate(()=>WorkspaceContext.setScope('dataset',{animate:false}));
  await page.evaluate(()=>Workspace.showPage('compromises'));
  await page.evaluate(() => Security.get());
  await page.waitForSelector('.evidence-control');
  await page.evaluate(()=>{
    const source=structuredClone(Security.cached()),original=api;
    const template=source.detections[0],baseEpisode=source.episodes[0];
    const findings=Array.from({length:205},(_,i)=>({...template,id:`disk-finding-${i}`,name:`Tentativa comprovada ${i}`,evidence_level:5,claim:'attempt',outcome:'blocked',context_only:false}));
    window.__diskCalls=[];
    api=async function(command,args,...rest){
      if(command==='triage_timeline'){
        __diskCalls.push({command,...args});return {analysis_id:'disk-analysis',count:206,complete:true,bins:[{slot:100,evidence_level:5,count:206,start:args.start,end:args.end}]};
      }
      if(command==='triage_episode'){
        __diskCalls.push({command,...args});const offset=args.offset||0,limit=args.limit||100;
        return {analysis_id:'disk-analysis',episode_id:'disk-episode',total:205,detections:findings.slice(offset,offset+limit),next_offset:offset+limit<205?offset+limit:null};
      }
      if(command==='triage'){
        __diskCalls.push({command,...args});const offset=args.episodeOffset||0;
        const preview=offset===0?findings.slice(0,100):[{...template,id:'second-page',evidence_level:5,context_only:false}];
        return {...source,analysis_id:'disk-analysis',storage:{kind:'sqlite'},minimum_evidence:args.minimumEvidence||5,
          counts_by_level:[1,1,1,1,206],visible_detections:206,available_detections:210,detections:preview,
          episodes:[{...baseEpisode,id:offset===0?'disk-episode':'another-episode',title:'Tentativa de shell reverso bloqueada',evidence_level:5,detections:preview.map((_,i)=>i),detection_count:offset===0?205:1,members_complete:offset!==0}],
          page:{episode_offset:offset,episode_limit:1,returned_episodes:1,total_episodes:2,next_offset:offset===0?1:null}};
      }
      return original(command,args,...rest);
    };
    Security.cached().storage={kind:'sqlite'};Security.setMinimum(5);
  });
  await page.waitForSelector('.sec-pagination');
  await page.evaluate(()=>{const host=document.createElement('div');host.id='disk-timeline-test';document.body.append(host);Security.markers(host,1,100,()=>{});});
  await page.waitForSelector('#disk-timeline-test .sec-marker');
  assert.match(await page.locator('#disk-timeline-test .sec-markers').getAttribute('aria-label'),/206 indícios/);
  assert.match(await page.locator('.evidence-control-heading').innerText(),/206 indícios visíveis/);
  await page.locator('.sec-episode .sec-main h3').first().click();
  await page.waitForFunction(()=>document.querySelectorAll('.sec-detection').length===100);
  assert.equal(await page.locator('.sec-detection').count(),100);
  await page.locator('[data-members-offset="100"]').click();
  await page.waitForFunction(()=>document.querySelector('.sec-detection strong')?.textContent==='Tentativa comprovada 100');
  await page.locator('[data-members-offset="200"]').click();
  await page.waitForFunction(()=>document.querySelectorAll('.sec-detection').length===5);
  assert.match(await page.locator('.sec-detections').innerText(),/205 de 205/);
  await page.locator('.sec-pagination [data-page-offset="1"]').click();
  await page.waitForFunction(()=>Security.cached().page.episode_offset===1);
  await page.locator('.evidence-segments [data-evidence-min="3"]').click();
  await page.waitForFunction(()=>Security.cached().minimum_evidence===3&&Security.cached().page.episode_offset===0);
  const calls=await page.evaluate(()=>__diskCalls);
  assert.ok(calls.some(c=>c.command==='triage_episode'&&c.offset===200));
  assert.ok(calls.every(c=>!c.force),'paging and rigidity never force a rescan');
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({diskPages:true,all205MembersAccessible:true,rigidityUsesCache:true,errors},null,2));
}finally{await browser.close();}
