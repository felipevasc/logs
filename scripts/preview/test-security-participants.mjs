import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
import { launchBrowser } from "./browser.mjs";
const browser=await launchBrowser();
const page=await browser.newPage({viewport:{width:1440,height:1150},reducedMotion:'reduce'});
const errors=[];page.on('pageerror',e=>errors.push(e.message));
try {
  await page.goto(process.argv[2]||'http://127.0.0.1:4173');
  await page.waitForFunction(()=>WorkspaceContext.ready&&state.loaded&&!state.loadOverlay);
  await page.evaluate(()=>WorkspaceContext.setScope('dataset',{animate:false}));
  await page.evaluate(()=>Workspace.showPage('compromises'));
  await page.evaluate(() => Security.get());
  await page.waitForSelector('.evidence-control');
  await page.evaluate(()=>{
    const original=api,data=structuredClone(Security.cached()),base=data.detections[0],e=structuredClone(state.rows[0]);
    e.event_ref='personas:9';e.id=9;e.message='Entity resolution disallowed for file:///etc/passwd';
    e.fields={'source.ip':'198.51.100.24','source.user.name':'tenant-a\\audit-user','destination.ip':'10.20.0.15','server_name':'geo.internal','service.name':'GeoServer','password':'fixture-secret','untrusted':'<script>window.personaInjected=true</script>'};e.raw=JSON.stringify(e.fields);
    const fact=(side,kind,value,field,role='source',certainty='observed')=>({side,kind,value,role,certainty,namespace:'tenant-a',origins:[{event_ref:e.event_ref,event_id:e.id,field,method:'alias'}],origins_limited:false});
    const participants={version:'participants-1',limited:false,facts:[
      fact('attacker','ip','198.51.100.24','source.ip'),fact('attacker','user','tenant-a\\audit-user','source.user.name','identity_used'),
      fact('attacker','ip','203.0.113.7','x-forwarded-for','forwarded_unverified','unverified'),
      fact('victim','ip','10.20.0.15','destination.ip','destination'),fact('victim','host','geo.internal','server_name','destination'),
      fact('victim','application','GeoServer','service.name','target_application'),fact('context','ip','10.20.0.1','observer.ip','observer')]};
    const d={...base,id:'persona-finding',evidence_level:4,name:'Resolução de entidade XML sensível bloqueada',severity:'high',claim:'attempt',outcome:'blocked',participants,event_ids:[9],event_refs:[e.event_ref],evidence_members:[{event_id:9,event_ref:e.event_ref,fields:['message']}],excerpts:[{field:'message',event_ref:e.event_ref,transformation:'original',matched:e.message,before:'',after:'',start:0,end:e.message.length}],context_only:false};
    data.detections=[d];data.episodes=[{...data.episodes[0],title:d.name,severity:'high',evidence_level:4,detections:[0],participants,event_refs:[e.event_ref],record_count:1}];data.counts_by_level=[0,0,0,1,0];data.visible_detections=1;data.available_detections=1;
    window.__personaData=data;window.__personaCalls=[];
    api=async function(command,args,...rest){
      if(command==='triage')return structuredClone(data);
      if(command==='triage_evidence_event'){__personaCalls.push(args);if(args.eventRef!==e.event_ref||args.eventId!==9)throw Error('Wrong original');return structuredClone(e);}
      return original(command,args,...rest);
    };Security.invalidate();
  });
  await page.evaluate(()=>Workspace.showPage('compromises'));
  await page.evaluate(() => Security.get());
  await page.waitForSelector('.evidence-control');
  await page.locator('[data-evidence-min="4"]').click();
  await page.waitForSelector('.sec-personas');
  assert.equal(await page.locator('.sec-persona-card').count(),2);
  assert.match(await page.locator('.persona-attacker').innerText(),/198\.51\.100\.24/);
  assert.match(await page.locator('.persona-attacker').innerText(),/\+1 dado/);
  assert.match(await page.locator('.persona-victim').innerText(),/GeoServer/);
  assert.match(await page.locator('.persona-victim').innerText(),/Alvo/);
  const left=await page.locator('.persona-attacker').boundingBox(),center=await page.locator('.sec-attack-content').boundingBox(),right=await page.locator('.persona-victim').boundingBox();
  assert.ok(left.x+left.width<center.x&&center.x+center.width<right.x,'Atacante → evidências → alvo');
  assert.ok(left.height<170&&right.height<170,'Miniature cards keep the evidence prominent');
  assert.ok((await page.locator('.persona-victim img').getAttribute('src')).endsWith('/application.png'));
  await page.locator('.sec-personas img').evaluateAll(async imgs=>Promise.all(imgs.map(i=>i.decode())));
  assert.ok(await page.locator('.sec-personas img').evaluateAll(imgs=>imgs.every(i=>i.naturalWidth>0)));
  await page.locator('.sec-episode').evaluate(e=>e.scrollIntoView({block:'start'}));
  await mkdir('output/playwright',{recursive:true});
  await page.screenshot({path:'output/playwright/participants-dark.png'});
  await page.evaluate(()=>document.documentElement.dataset.theme='light');
  await page.screenshot({path:'output/playwright/participants-light.png'});
  await page.locator('.persona-attacker').click();
  await page.waitForSelector('.sec-persona-inspector[open]');
  assert.equal(await page.locator('.sec-episode').getAttribute('aria-expanded'),'false');
  assert.match(await page.locator('.sec-persona-facts').innerText(),/Identidade utilizada/);
  assert.match(await page.locator('.sec-persona-facts').innerText(),/não validado/);
  await page.locator('.sec-persona-facts>article').first().locator('details>summary').first().click();
  await page.locator('.sec-persona-origin>summary').first().click();
  await page.waitForSelector('.sec-persona-original dl');
  assert.match(await page.locator('.sec-persona-original').first().innerText(),/source.ip/);
  assert.ok(!(await page.locator('.sec-persona-original').first().innerText()).includes('fixture-secret'));
  assert.equal(await page.evaluate(()=>!!window.personaInjected),false);
  assert.equal(await page.locator('.sec-persona-original script').count(),0);
  await page.screenshot({path:'output/playwright/participants-origin.png'});
  await page.keyboard.press('Escape');
  await page.locator('.sec-episode [data-act="save"]').click();
  await page.waitForFunction(()=>activeCase().items.some(i=>i.detection?.detections?.some(d=>d.id==='persona-finding')));
  assert.ok(await page.evaluate(()=>{
    const item=activeCase().items.find(i=>i.detection?.detections?.some(d=>d.id==='persona-finding'));
    return item.detection.detections[0].participants.facts.length===7&&EvidenceUI.report(item).includes('Identidade utilizada');
  }));
  await page.setViewportSize({width:650,height:1000});
  await page.locator('.sec-personas').evaluate(e=>e.scrollIntoView({block:'start'}));
  assert.ok(await page.locator('.sec-episode').evaluate(e=>e.scrollWidth<=e.clientWidth+1));
  await page.screenshot({path:'output/playwright/participants-narrow.png'});
  await page.evaluate(()=>{
    const host=document.createElement('div');host.id='persona-empty';host.innerHTML=ParticipantsUI.cards({facts:[]});document.body.append(host);
    const preview=ParticipantsUI.cards({facts:[{side:'victim',kind:'asset_type',value:'workstation',role:'asset_type',origins:[]},{side:'victim',kind:'host',value:'desktop-1',role:'affected_asset',origins:[]}]});
    window.__endpointVariant=preview.includes('endpoint.png');
    for(const [id,facts] of [['type-only',[{side:'victim',kind:'asset_type',value:'workstation'}]],['target-only',[{side:'victim',kind:'ip',value:'10.0.0.1'}]],['attacker-only',[{side:'attacker',kind:'ip',value:'192.0.2.1'}]]]){
      const container=document.createElement('div');container.id=id;container.innerHTML=ParticipantsUI.cards({facts},'<p>Informações do ataque</p>');document.body.append(container);
    }
  });
  assert.equal(await page.locator('#persona-empty .sec-persona-card, #type-only .sec-persona-card').count(),0);assert.equal(await page.evaluate(()=>__endpointVariant),true);
  assert.equal(await page.locator('#target-only .sec-persona-card').count(),1);assert.equal(await page.locator('#attacker-only .sec-persona-card').count(),1);
  assert.equal(await page.locator('#target-only .persona-attacker, #attacker-only .persona-victim').count(),0);
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({compactLayout:true,missingSidesHidden:true,transparentAssetsLoaded:true,exactOrigin:true,redaction:true,caseAndReport:true,narrow:true,errors},null,2));
}finally{await browser.close();}
