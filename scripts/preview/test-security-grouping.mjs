/* Browser contract for grouping; Rust tests exercise the actual full-universe grouping. */
import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
import {chromium} from 'playwright';
const browser=await chromium.launch({channel:process.env.PLAYWRIGHT_CHANNEL||'chrome'});
const page=await browser.newPage({viewport:{width:1440,height:1050},reducedMotion:'reduce'});
const errors=[];page.on('pageerror',e=>errors.push(e.message));
try {
  await page.goto(process.argv[2]||'http://127.0.0.1:4173');
  await page.waitForFunction(()=>WorkspaceContext.ready&&state.loaded&&!state.loadOverlay);
  await page.evaluate(()=>WorkspaceContext.setScope('dataset',{animate:false}));
  await page.evaluate(()=>Workspace.showPage('compromises'));
  await page.waitForSelector('.evidence-control');
  await page.evaluate(()=>{
    const original=api,source=structuredClone(Security.cached());
    const base=source.detections[0],event=structuredClone(state.rows[0]),at=1700000000000;
    const findings=Array.from({length:45},(_,i)=>({...base,id:`pattern-finding-${i}`,name:'Resolução de entidade XML sensível bloqueada',
      severity:'high',evidence_level:4,count:1,claim:'attempt',outcome:'blocked',context_only:false,start:at+i*1000,end:at+i*1000,
      event_ids:[i],event_refs:[`pattern:${i}`],relationships:[],evidence_members:[{event_id:i,event_ref:`pattern:${i}`,fields:['arbitrary.text'],step:0}],
      excerpts:[{event_ref:`pattern:${i}`,field:'arbitrary.text',transformation:'original',before:`worker ${i}: `,matched:'Entity resolution disallowed for file:///etc/passwd',after:'',start:10,end:68}],
      checks:[{status:'passed',label:'Diagnóstico de XML observado',expected:'Resolução de entidade sensível bloqueada',observed:`worker ${i}: Entity resolution disallowed for file:///etc/passwd`,event_refs:[`pattern:${i}`],fields:['arbitrary.text']}]}));
    const episode={...source.episodes[0],id:'pattern-group-fixture',title:findings[0].name,severity:'high',evidence_level:4,start:at,end:at+44000,
      detection_count:45,record_count:45,grouping:{kind:'pattern',occurrence_count:45,version:'pattern-groups-1'},members_complete:false};
    window.__groupCalls=[];
    api=async function(command,args,...rest){
      if(['triage','triage_episode','triage_evidence_event'].includes(command))__groupCalls.push({command,...args});
      if(command==='triage')return {...source,analysis_id:'group-analysis',storage:{kind:'sqlite'},counts_by_level:[0,0,0,45,0],
        visible_detections:args.minimumEvidence<=4?45:0,available_detections:45,minimum_evidence:args.minimumEvidence,
        detections:args.minimumEvidence<=4?findings.slice(0,20):[],episodes:args.minimumEvidence<=4?[{...episode,detections:findings.slice(0,20).map((_,i)=>i)}]:[],
        page:{episode_offset:0,episode_limit:20,returned_episodes:args.minimumEvidence<=4?1:0,total_episodes:args.minimumEvidence<=4?1:0,next_offset:null}};
      if(command==='triage_episode')return {analysis_id:'group-analysis',episode_id:episode.id,total:45,detections:structuredClone(findings.slice(args.offset,args.offset+args.limit)),next_offset:args.offset+args.limit<45?args.offset+args.limit:null};
      if(command==='triage_evidence_event'){
        assertReference(args);
        return {...event,id:args.eventId,event_ref:args.eventRef,raw:`worker ${args.eventId}: Entity resolution disallowed for file:///etc/passwd`,fields:{'arbitrary.text':`worker ${args.eventId}: Entity resolution disallowed for file:///etc/passwd`}};
      }
      return original(command,args,...rest);
    };
    function assertReference(args){if(args.eventRef!==`pattern:${args.eventId}`)throw Error('Wrong exact reference');}
    Security.invalidate();
  });
  await page.evaluate(()=>Workspace.showPage('compromises'));
  await page.waitForSelector('.evidence-control');
  assert.equal(await page.locator('.sec-episode').count(),0,'default level is unchanged');
  await page.locator('[data-evidence-min="4"]').click();
  await page.waitForSelector('.sec-pattern-count');
  assert.equal(await page.locator('.sec-episode').count(),1);
  assert.match(await page.locator('.sec-pattern-count').innerText(),/45 ocorrências semelhantes/);
  assert.match(await page.locator('.sec-heading-count').innerText(),/Cartões 1–1 de 1/);
  assert.match(await page.locator('.evidence-control-heading').innerText(),/45 indícios visíveis/);
  assert.equal(await page.locator('.sec-detection').count(),0);
  await mkdir('output/playwright',{recursive:true});
  await page.screenshot({path:'output/playwright/compromises-grouped.png'});
  await page.locator('.sec-pattern-count').click();
  await page.waitForFunction(()=>document.querySelectorAll('.sec-occurrence').length===20);
  assert.equal(await page.locator('.sec-occurrence .evidence-badge').count(),0,'no repeated classification blocks');
  assert.match(await page.locator('.sec-pattern-count').innerText(),/Ocultar ocorrências/);
  await page.locator('[data-members-offset="20"]').click();
  await page.waitForFunction(()=>document.querySelector('.sec-occurrence strong')?.textContent==='Evento 20');
  await page.locator('[data-members-offset="40"]').click();
  await page.waitForFunction(()=>document.querySelectorAll('.sec-occurrence').length===5);
  await page.locator('.sec-occurrence [data-act="d-records"]').last().click();
  await page.locator('.sec-event>summary').click();
  await page.waitForSelector('.sec-event-field');
  assert.match(await page.locator('.sec-event-content').innerText(),/worker 44/);
  assert.match(await page.locator('.sec-preview-source').innerText(),/Evento 44/);
  assert.equal(await page.locator('.sec-excerpts').count(),1);
  await page.locator('.sec-attack-content>[data-act="explain"]').click();
  await page.waitForSelector('dialog.evidence-inspector[open]');
  assert.match(await page.locator('.evidence-check').innerText(),/worker 44/);
  await page.keyboard.press('Escape');
  await page.locator('.sec-episode').evaluate(n=>n.scrollIntoView({block:'start'}));
  await page.screenshot({path:'output/playwright/compromises-grouped-expanded.png'});
  await page.setViewportSize({width:650,height:900});
  await page.evaluate(()=>document.documentElement.dataset.theme='light');
  await page.screenshot({path:'output/playwright/compromises-grouped-narrow.png'});
  assert.ok(await page.locator('.sec-episode').evaluate(n=>n.scrollWidth<=n.clientWidth+1),'narrow layout fits');
  await page.setViewportSize({width:1440,height:1050});
  await page.locator('.sec-episode [data-act="save"]').click();
  await page.waitForFunction(()=>activeCase().items.some(i=>i.detection?.grouping?.kind==='pattern'));
  const saved=await page.evaluate(()=>{
    const i=activeCase().items.find(i=>i.detection?.grouping?.kind==='pattern');
    return {events:i.rows.length,findings:i.detection.detections.length,grouping:i.detection.grouping,levels:[...new Set(i.detection.detections.map(d=>d.evidence_level))]};
  });
  assert.equal(saved.events,45);assert.equal(saved.findings,45);assert.equal(saved.grouping.occurrence_count,45);assert.deepEqual(saved.levels,[4]);
  await page.locator('.sec-episode .sec-main h3').click({button:'right'});
  await page.locator('.ctx-item',{hasText:'Filtrar eventos no Explorar'}).click();
  await page.waitForFunction(()=>document.body.dataset.page==='explore');
  assert.equal(await page.evaluate(()=>state.filters.find(f=>f.column==='event_ref')?.value.split('\n').length),45);
  const calls=await page.evaluate(()=>__groupCalls);
  assert.ok(calls.every(c=>!c.force));
  assert.ok(calls.some(c=>c.command==='triage_evidence_event'&&c.eventRef==='pattern:44'));
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({oneCardFor45:true,allEventsAccessible:true,compactOccurrences:true,exactCaseAndExplore:true,errors},null,2));
}finally{await browser.close();}
