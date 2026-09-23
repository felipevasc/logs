import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { existsSync,readFileSync,mkdirSync,writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const url=process.argv[2]||'http://127.0.0.1:4173';
const browser=await chromium.launch({headless:true,...(!existsSync(chromium.executablePath())?{executablePath:'C:/Users/felip/AppData/Local/ms-playwright/chromium-1217/chrome-win64/chrome.exe'}:{})});
const page=await browser.newPage({viewport:{width:1280,height:900}});page.setDefaultTimeout(15000);
const output=resolve('output/playwright');mkdirSync(output,{recursive:true});
const errors=[];page.on('pageerror',error=>errors.push(error.message));
const catalog=JSON.parse(readFileSync('frontend/vendor/fa/icon-catalog.json','utf8'));
const css=readFileSync('frontend/vendor/fa/css/all.min.css','utf8');
assert.equal(catalog.icons.filter(icon=>icon.style==='solid').length,1422);assert.equal(catalog.icons.filter(icon=>icon.style==='brands').length,572);
assert(catalog.icons.every(icon=>css.includes(`.fa-${icon.name}`)),'All catalog icons exist in bundled CSS');
const result={url,solid:1422,brands:572};
try{
  await page.goto(url);await page.waitForFunction(()=>state.loaded&&state.total===6000&&window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&document.querySelector('#load-overlay').hidden);
  await page.evaluate(()=>{
    const c=activeCase();c.items=[{id:'note-icons',name:'Icon fixture',kind:'events',rows:[{id:0,event_ref:'icon:0',timestamp:Date.now(),source:'API',level:'Informação',name:'Início',message:'Evento de teste',fields:{}},{id:1,event_ref:'icon:1',timestamp:Date.now()+60000,source:'API',level:'Informação',name:'Fim',message:'Evento de teste',fields:{}}]}];
    c.timeline={groups:[],edits:{},layout:{},compact:false,annotations:[{id:'old-icon',anchor:'e:note-icons:icon:0',text:'Nota anterior',icon:'fa-comment'}]};caseEventsCache.sig=null;
  });
  await page.evaluate(()=>WorkspaceContext.setScope('case',{page:'case-timeline',animate:false}));
  await page.getByRole('button',{name:'Linha do tempo',exact:true}).click();
  assert.equal(await page.locator('.ct-note .fa-comment').count(),1);
  await page.locator('.ct-note').dblclick();
  await page.waitForFunction(()=>document.querySelectorAll('.ct-icon-choice').length===96);
  const checkGridGeometry=async()=>{
    const boxes=await page.locator('.ct-icon-choice').evaluateAll(nodes=>nodes.map(node=>{const rect=node.getBoundingClientRect();return{x:rect.x,y:rect.y,right:rect.right,bottom:rect.bottom,width:rect.width,height:rect.height};}));
    const columns=await page.locator('.ct-icon-grid').evaluate(node=>getComputedStyle(node).gridTemplateColumns.split(/\s+/).length);
    assert.equal(columns,8);
    assert(boxes[0].bottom<=boxes[8].y+0.1,`First and second rows overlap: ${JSON.stringify([boxes[0],boxes[8]])}`);
    for(const [index,box]of boxes.entries()){
      if(index%columns<columns-1&&boxes[index+1])assert(box.right<=boxes[index+1].x+0.1,`Adjacent columns overlap at ${index}`);
      if(boxes[index+columns])assert(box.bottom<=boxes[index+columns].y+0.1,`Adjacent rows overlap at ${index}`);
    }
    return {first:boxes[0],nextRow:boxes[8],columns};
  };
  result.gridGeometry=await checkGridGeometry();
  assert.equal(await page.locator('input[name="icon"]').inputValue(),'fa-comment');
  await page.getByRole('button',{name:'Salvar',exact:true}).click();
  assert.equal(await page.evaluate(()=>activeCase().timeline.annotations[0].icon),'fa-comment','Legacy saved value remains unchanged');
  await page.locator('[data-ct-action="note"]').click();
  await page.waitForFunction(()=>document.querySelectorAll('.ct-icon-choice').length===96);
  assert(await page.locator('.ct-icon-grid').evaluate(node=>node.scrollHeight>node.clientHeight),'Grid scrolls instead of stretching modal');
  assert.equal(await page.locator('.ct-icon-choice[tabindex="0"]').count(),1,'One keyboard entry point');
  const first=await page.locator('.ct-icon-choice').first().getAttribute('data-value');
  await page.locator('.ct-icon-choice').first().focus();await page.keyboard.press('ArrowRight');
  assert.notEqual(await page.evaluate(()=>document.activeElement.dataset.value),first);
  await page.keyboard.press('PageDown');
  await page.waitForFunction(()=>document.querySelector('.ct-icon-pager [role="status"]').textContent.includes('2 /'));
  assert.equal(await page.locator('.ct-icon-choice').count(),96);
  await page.getByLabel('Buscar ícone',{exact:true}).fill('servidor');
  await page.waitForFunction(()=>document.querySelector('.ct-icon-choice[data-value="fa-server"]'));
  const server=page.locator('.ct-icon-choice[data-value="fa-server"]');
  assert.match(await server.getAttribute('title'),/Servidor.*Server/);
  await server.click();assert.equal(await page.locator('input[name="icon"]').inputValue(),'fa-server');
  await page.getByLabel('Buscar ícone',{exact:true}).fill('usuário');
  await page.waitForFunction(()=>!!document.querySelector('.ct-icon-choice[data-value="fa-user"]'));
  for(const term of ['malware','ransomware','antivirus','invasão','ataque','backup','endpoint','api','container','firewall','vpn']){
    await page.getByLabel('Buscar ícone',{exact:true}).fill(term);
    await page.waitForFunction(()=>document.querySelector('.ct-icon-grid')?.getAttribute('aria-busy')==='false');
    assert((await page.locator('.ct-icon-choice').count())>0,`Technical search: ${term}`);
  }
  result.technicalAliases=true;
  await page.getByLabel('Buscar ícone',{exact:true}).fill('semcorrespondencia123');
  await page.locator('.ct-icon-empty').waitFor();assert.equal(await page.locator('input[name="icon"]').inputValue(),'fa-server','Searching never discards selection');
  await page.getByLabel('Buscar ícone',{exact:true}).fill('');
  await page.getByLabel('Categoria de ícones',{exact:true}).selectOption('Segurança');
  await page.waitForFunction(()=>document.querySelector('.ct-icon-choice[data-value="fa-shield-halved"]'));
  assert((await page.locator('.ct-icon-choice').count())<=96);
  await page.screenshot({path:resolve(output,'note-icons-security-1280.png')});
  await page.getByLabel('Coleção de ícones',{exact:true}).selectOption('brands');
  await page.getByLabel('Buscar ícone',{exact:true}).fill('aws');
  await page.waitForFunction(()=>!!document.querySelector('.ct-icon-choice[data-value="fab fa-aws"]'));
  await page.locator('.ct-icon-choice[data-value="fab fa-aws"]').click();
  await page.locator('textarea[name="text"]').fill('Nuvem da aplicação');
  await page.getByRole('button',{name:'Salvar',exact:true}).click();
  await page.waitForFunction(()=>!!document.querySelector('.ct-note .fab.fa-aws'));
  assert.equal(await page.evaluate(()=>activeCase().timeline.annotations.at(-1).icon),'fab fa-aws');
  assert.match(await page.locator('.ct-note .fab.fa-aws').evaluate(node=>getComputedStyle(node).fontFamily),/Brands/);
  await page.locator('.ct-note').filter({hasText:'Nuvem da aplicação'}).dblclick();
  await page.waitForFunction(()=>!!document.querySelector('.ct-icon-choice'));
  assert.equal(await page.getByLabel('Coleção de ícones',{exact:true}).inputValue(),'brands');
  assert.equal(await page.locator('input[name="icon"]').inputValue(),'fab fa-aws');
  await page.getByRole('button',{name:'Sem ícone',exact:true}).click();
  await page.getByRole('button',{name:'Salvar',exact:true}).click();
  assert.equal(await page.evaluate(()=>activeCase().timeline.annotations.at(-1).icon),'');
  assert.equal(await page.locator('.ct-note').filter({hasText:'Nuvem da aplicação'}).locator('i').count(),0);
  await page.locator('[data-ct-action="note"]').click();
  await page.waitForFunction(()=>!!document.querySelector('.ct-icon-choice'));
  await page.setViewportSize({width:1024,height:680});
  result.minimumGridGeometry=await checkGridGeometry();
  const bounds=await page.locator('.ct-editor').boundingBox();assert(bounds.x>=0&&bounds.y>=0&&bounds.x+bounds.width<=1024&&bounds.y+bounds.height<=680,'Editor stays in minimum app viewport');
  const saveBounds=await page.getByRole('button',{name:'Salvar',exact:true}).boundingBox();assert(saveBounds.y+saveBounds.height<bounds.y+bounds.height,'Save action is fully visible without scrolling the modal');
  assert(await page.locator('.ct-icon-grid').evaluate(node=>node.scrollWidth<=node.clientWidth),'Grid has no horizontal overflow');
  await page.screenshot({path:resolve(output,'note-icons-1024.png')});
  await page.keyboard.press('Escape');assert.equal(await page.locator('.ct-editor-backdrop').isVisible(),false);
  assert.equal(await page.evaluate(()=>document.activeElement.dataset.ctAction),'note','Escape restores trigger focus');
  for(const [saved,canonical]of [['fas fa-shield-alt','fa-shield-halved'],['fa-worm','fa-worm']]){
    await page.evaluate(saved=>{activeCase().timeline.annotations[0].icon=saved;renderAnalysis();},saved);
    await page.locator('.ct-note[data-note="old-icon"]').dblclick();
    await page.locator(`.ct-icon-choice[data-value="${canonical}"]`).waitFor();
    assert.equal(await page.locator(`.ct-icon-choice[data-value="${canonical}"]`).getAttribute('aria-pressed'),'true','The saved glyph is selected on reopening, including legacy aliases');
    assert.equal(await page.locator('input[name="icon"]').inputValue(),saved);
    await page.getByRole('button',{name:'Salvar',exact:true}).click();
    assert.equal(await page.evaluate(()=>activeCase().timeline.annotations[0].icon),saved);
  }
  await page.locator('[data-ct-action="note"]').click();await page.waitForFunction(()=>document.querySelectorAll('.ct-icon-choice').length===96);
  for(const width of [1024,1440])for(const theme of ['dark','light']){
    await page.setViewportSize({width,height:900});await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
    const editor=await page.locator('.ct-editor').boundingBox(),save=await page.getByRole('button',{name:'Salvar',exact:true}).boundingBox();
    assert(editor.x>=0&&editor.x+editor.width<=width&&editor.y>=0&&editor.y+editor.height<=900);
    assert(save.y+save.height<=editor.y+editor.height);
    await checkGridGeometry();await page.screenshot({path:resolve(output,`note-icons-audit-${width}-${theme}.png`)});
  }
  await page.keyboard.press('Escape');
  assert.deepEqual(errors,[]);
  Object.assign(result,{legacyPreserved:true,keyboard:true,searchPtEn:true,boundedGrid:true,brandsPersisted:true,minimumViewport:true,pageErrors:errors});
  writeFileSync(resolve(output,'note-icons-validation.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
}finally{await browser.close();}
