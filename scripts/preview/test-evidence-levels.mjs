/* UI contract fixtures only: evidence strength is supplied by the backend. */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { launchBrowser } from "./browser.mjs";
const browser=await launchBrowser();
const page=await browser.newPage({viewport:{width:1440,height:1050},reducedMotion:"reduce"});
const errors=[]; page.on("pageerror",error=>errors.push(error.message));
try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4173");
  await page.waitForFunction(()=>WorkspaceContext.ready && state.loaded && !state.loadOverlay);
  const redaction=await page.evaluate(()=>{
    const originalCols=state.visibleCols, event={...state.rows[0],message:"password=fixture-secret-value",fields:{password:"fixture-secret-value"}};
    state.visibleCols=["message","fields.password"];
    try {
      const row=buildEventRow(event), chip=EntityMenu.chip({column:"@cmdline",value:event.message});
      const timeline=CaseTimeline.rows({items:[{id:"redaction",rows:[event]}]});
      return {html:row.outerHTML,chip:chip.outerHTML,timeline:JSON.stringify(timeline),original:event.message,
        field:EvidenceUI.redact({"@password":"fixture-secret-value"})["@password"]};
    } finally { state.visibleCols=originalCols; }
  });
  for(const surface of ["html","chip","timeline"]) assert.ok(!redaction[surface].includes("fixture-secret-value"),`${surface} must mask secrets`);
  assert.equal(redaction.original,"password=fixture-secret-value","masking must preserve original evidence");
  assert.equal(redaction.field,"[oculto]");
  await page.evaluate(()=>WorkspaceContext.setScope("dataset",{animate:false}));
  await page.evaluate(()=>Workspace.showPage("compromises"));
  await page.evaluate(() => Security.get());
  await page.waitForSelector(".evidence-control");
  assert.equal(await page.evaluate(()=>Security.minimum()),5);
  assert.equal(await page.locator(".sec-episode").count(),0);
  await mkdir("output/playwright",{recursive:true});
  await page.screenshot({path:"output/playwright/evidence-default-e5.png",fullPage:false});
  // Use five explicit backend-shaped fixtures to verify cumulative projection.
  await page.evaluate(()=>{
    const data=Security.cached(), template=structuredClone(data.detections[0]), episode=structuredClone(data.episodes[0]);
    data.detections=[5,4,3,2,1].map(n=>({...template,id:`fixture-e${n}`,name:`Cenário de interface E${n}`,evidence_level:n,context_only:false}));
    data.episodes=data.detections.map((d,i)=>({...episode,id:`fixture-episode-${i}`,title:d.name,evidence_level:d.evidence_level,detections:[i]}));
    data.counts_by_level=[1,1,1,1,1];
    window.__evidenceFixture=JSON.stringify(data.detections.map(({id,evidence_level,event_refs,relationships})=>({id,evidence_level,event_refs,relationships})));
    window.__triageCalls=0; const original=api;
    api=async function(cmd,...args) { if(cmd==="triage") window.__triageCalls++; return original(cmd,...args); };
    Security.setMinimum(5);
  });
  for (const minimum of [5,4,3,2,1]) {
    await page.locator(`.evidence-segments [data-evidence-min='${minimum}']`).click();
    assert.equal(await page.evaluate(()=>Security.minimum()),minimum);
    const levels=await page.locator(".sec-episode > .sec-main .evidence-badge").allTextContents();
    assert.equal(levels.length, Math.min(3,6-minimum));
    assert.ok(levels.every(text=>["Inconclusivo","Suspeita","Indício","Forte indício","Quase confirmado"].slice(minimum-1).some(label=>text.includes(label))));
    assert.match(await page.locator(".evidence-control-heading").innerText(),new RegExp(`${6-minimum} indícios visíveis`));
    assert.equal(await page.evaluate(()=>window.__triageCalls),0,"rigidity must not request a new analysis");
    assert.equal(await page.evaluate(()=>JSON.stringify(Security.cached().detections.map(({id,evidence_level,event_refs,relationships})=>({id,evidence_level,event_refs,relationships}))) ),await page.evaluate(()=>window.__evidenceFixture));
  }
  await page.evaluate(()=>Workspace.showPage("explore"));
  await page.evaluate(()=>Workspace.showPage("compromises"));
  await page.evaluate(() => Security.get());
  await page.waitForSelector(".evidence-control");
  assert.equal(await page.evaluate(()=>Security.minimum()),1,"navigation within a universe preserves selection");
  await page.locator(".evidence-counts [data-evidence-min='5']").click();
  assert.equal(await page.evaluate(()=>Security.minimum()),5);
  await page.evaluate(()=>{Security.setMinimum(3);document.documentElement.dataset.theme="dark";});
  await page.screenshot({path:"output/playwright/evidence-levels-dark.png",fullPage:false});
  await page.setViewportSize({width:620,height:1000});
  await page.screenshot({path:"output/playwright/evidence-levels-narrow.png",fullPage:false});
  assert.equal(await page.locator(".evidence-segments button").count(),5);
  const report=await page.evaluate(()=>EvidenceUI.report({detection:{...EvidenceUI.exportMetadata(Security.cached(),3,"dataset"),detections:Security.cached().detections,analyst_state:"unreviewed"}}));
  for(const name of ["Inconclusivo","Suspeita","Indício","Forte indício","Quase confirmado"]) assert.ok(report.includes(name));
  await page.evaluate(async()=>{Security.setMinimum(1);await WorkspaceContext.setScope("case",{animate:false});});
  assert.equal(await page.evaluate(()=>Security.minimum()),5,"new universe resets rigidity");
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({levels:5,cumulative:true,stableIds:true,noRescan:true,defaultE5:true,contextReset:true,reportParity:true,redaction:true,errors},null,2));
} finally { await browser.close(); }
