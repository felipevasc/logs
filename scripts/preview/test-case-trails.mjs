/* Authored trails: references, optional narratives, images and non-destructive editing. */
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
const url=process.argv[2]||"http://127.0.0.1:4174",output=resolve("output/playwright");mkdirSync(output,{recursive:true});
const fallback=`${process.env.LOCALAPPDATA}/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe`;
const browser=await chromium.launch({executablePath:existsSync(chromium.executablePath())?undefined:fallback});
const page=await browser.newPage({viewport:{width:1200,height:900},reducedMotion:"reduce"}),errors=[],results={};page.on("pageerror",error=>errors.push(error.message));
try{
  await page.goto(url);await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&!state.loadOverlay);
  // Component fallback permits this test while the global route is being integrated.
  if(!await page.evaluate(()=>!!window.CaseContent)){await page.addStyleTag({path:"frontend/case-content.css"});await page.addScriptTag({path:"frontend/case-content.js"});}
  if(!await page.evaluate(()=>!!window.CaseTrails)){await page.addStyleTag({path:"frontend/case-trails.css"});await page.addScriptTag({path:"frontend/case-trails.js"});}
  const png=await page.evaluate(async()=>{
    const c=activeCase(),base=state.rows[0];c.items=Array.from({length:85},(_,index)=>({id:`trail-item-${index}`,label:`Item ${String(index).padStart(2,"0")}`,summary:index===0?"Autenticação fora do padrão":"",details:index===0?"O conteúdo foi preservado para conferência.":"",rows:[{...base,id:index,event_ref:`trail-event:${index}`}],attachments:[]}));c.caseTrails=[];caseEventsCache.sig=null;
    const canvas=document.createElement("canvas");canvas.width=160;canvas.height=90;const ctx=canvas.getContext("2d");ctx.fillStyle="#7253a7";ctx.fillRect(0,0,160,90);ctx.fillStyle="#fff";ctx.fillRect(18,18,100,8);const data=canvas.toDataURL("image/png");
    const original=api;api=async(command,args,options)=>command==="case_image_add"?{id:"a".repeat(64),name:args.name,mime:"image/png",bytes:100,width:160,height:90}:command==="case_image_read"?{dataUrl:data}:original(command,args,options);
    await WorkspaceContext.setScope("case",{page:"summary",animate:false});return data.split(",")[1];
  });
  async function showTrails(){await page.evaluate(async()=>{await Workspace.showPage("case-trails");if(!document.querySelector(".case-trails-workspace")){document.body.dataset.page="case-trails";document.querySelector("#ws-title").textContent="Trilhas";CaseTrails.render(document.querySelector("#ws-content"),activeCase());}});}
  await showTrails();assert.equal(await page.locator('button[data-page="case-trails"]').isVisible(),true);
  await page.getByRole("button",{name:"Nova trilha",exact:true}).click();await page.getByLabel("Título",{exact:true}).fill("Rascunho cancelado");await page.getByRole("button",{name:"Cancelar",exact:true}).click();assert.equal(await page.evaluate(()=>activeCase().caseTrails.length),0);
  await page.getByRole("button",{name:"Nova trilha",exact:true}).click();await page.getByLabel("Título",{exact:true}).fill("Acesso e movimentação");await page.getByLabel("Resumo",{exact:true}).fill("Sequência preservada para análise.");await page.getByLabel("Detalhes",{exact:true}).fill("Hipótese de teste; confirmar com os registros originais.");
  await page.locator('.case-content-dialog input[type="file"]').setInputFiles({name:"Contexto.png",mimeType:"image/png",buffer:Buffer.from(png,"base64")});await page.waitForFunction(()=>document.querySelectorAll('.case-content-dialog .case-image-card').length===1);
  await page.getByRole("button",{name:"Salvar",exact:true}).click();await page.waitForFunction(()=>activeCase().caseTrails.length===1&&!document.querySelector('.case-content-dialog'));
  await page.getByRole("button",{name:"Associar itens",exact:true}).click();assert.equal(await page.locator('.case-trail-choice').count(),40,"association DOM is paginated");
  await page.getByRole("checkbox",{name:"Item 00",exact:true}).check();await page.getByRole("button",{name:"Cancelar",exact:true}).click();assert.deepEqual(await page.evaluate(()=>activeCase().caseTrails[0].itemIds),[]);
  await page.getByRole("button",{name:"Associar itens",exact:true}).click();await page.getByRole("checkbox",{name:"Item 02",exact:true}).check();await page.getByRole("checkbox",{name:"Item 00",exact:true}).check();await page.getByRole("button",{name:"Próximos itens",exact:true}).click();await page.getByRole("checkbox",{name:"Item 40",exact:true}).check();await page.getByRole("button",{name:"Aplicar",exact:true}).click();
  await page.waitForFunction(()=>document.querySelectorAll('.case-trail-item').length===3);assert.deepEqual(await page.evaluate(()=>activeCase().caseTrails[0].itemIds),["trail-item-2","trail-item-0","trail-item-40"]);
  await page.locator('.case-trail-item').nth(1).getByRole('button',{name:'Mover item acima',exact:true}).click();assert.deepEqual(await page.evaluate(()=>activeCase().caseTrails[0].itemIds),["trail-item-0","trail-item-2","trail-item-40"]);
  await page.locator('.case-trail-item').nth(1).getByRole('button',{name:'Desassociar item',exact:true}).click();assert.equal(await page.evaluate(()=>activeCase().items.length),85);assert.equal(await page.locator('.case-trail-item').count(),2);
  await page.getByRole('button',{name:'Remover trilha',exact:true}).click();assert.equal(await page.evaluate(()=>activeCase().caseTrails.length),0);assert.equal(await page.evaluate(()=>activeCase().items.length),85);await page.getByRole('button',{name:'Desfazer remoção',exact:true}).click();assert.equal(await page.evaluate(()=>activeCase().caseTrails.length),1);
  await page.waitForFunction(()=>document.querySelector('.case-trail-images img')?.complete&&document.querySelector('.case-trail-images img')?.naturalWidth>0);
  results.references={items:85,associated:await page.evaluate(()=>activeCase().caseTrails[0].itemIds),copyRows:await page.evaluate(()=>Object.hasOwn(activeCase().caseTrails[0],"rows"))};assert.equal(results.references.copyRows,false);
  await page.screenshot({path:resolve(output,'case-trails-dark-1200.png')});await page.setViewportSize({width:1024,height:900});await page.evaluate(()=>{document.documentElement.dataset.theme='light';});await page.screenshot({path:resolve(output,'case-trails-light-1024.png')});
  const bounds=await page.locator('.case-trails-detail').boundingBox();assert.ok(bounds.x+bounds.width<=1024&&bounds.y+bounds.height<=880);assert.equal(await page.locator('#workspace-home').evaluate(node=>node.scrollHeight>node.clientHeight),false);
  await page.evaluate(()=>saveCases());const saved=await page.evaluate(()=>JSON.parse(localStorage.getItem('__mockStore')).cases.find(c=>c.id===state.cases.active).caseTrails[0]);assert.equal(saved.title,'Acesso e movimentação');assert.equal(saved.attachments.length,1);assert.equal(saved.summary,'Sequência preservada para análise.');assert.ok(saved.details);
  results.savedNarrativeAndImage=true;results.cancelAndUndo=true;results.noOverflow1024=true;assert.deepEqual(errors,[]);writeFileSync(resolve(output,'case-trails-results.json'),JSON.stringify({...results,errors},null,2));console.log(JSON.stringify({...results,errors},null,2));
}catch(error){await page.screenshot({path:resolve(output,'case-trails-failure.png')});throw error;}finally{await browser.close();}
