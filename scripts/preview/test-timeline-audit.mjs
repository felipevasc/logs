import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,mkdirSync,existsSync} from 'node:fs';
import {chromium} from 'playwright';
const browser=await chromium.launch({...(!existsSync(chromium.executablePath())?{executablePath:`${process.env.LOCALAPPDATA}/ms-playwright/chromium-1217/chrome-win64/chrome.exe`}:{})});
const page=await browser.newPage({viewport:{width:1440,height:900}}),errors=[],results={};
page.on('pageerror',error=>errors.push(error.message));mkdirSync('output/playwright',{recursive:true});
try{
  await page.setContent('<html data-theme="dark"><body><main id="fixture" style="width:calc(100% - 40px);margin:20px"></main></body></html>');
  for(const file of ['styles.css','workspace.css','timeline-refinements.css','timeline-export.css'])await page.addStyleTag({content:readFileSync(`frontend/${file}`,'utf8')});
  for(const file of ['icon-picker.js','timeline-export.js','case-timeline.js'])await page.addScriptTag({content:readFileSync(`frontend/${file}`,'utf8')});
  await page.evaluate(()=>{
    window.caseFixture={id:'audit',items:[],timeline:{groups:[],annotations:[],edits:{},layout:{},compact:false}};
    window.callbacks={passes:()=>true,detail:event=>window.opened=event,bucket:(x,y,rows)=>window.openedRows=rows,menu:(x,y,items)=>window.menu=items,notify:message=>{throw Error(message);},save:()=>CaseTimeline.render(document.querySelector('#fixture'),window.caseFixture,window.mode,window.callbacks)};
    window.drawAudit=mode=>{window.mode=mode;CaseTimeline.render(document.querySelector('#fixture'),window.caseFixture,mode,window.callbacks);};
    window.caseFixture.items=[{id:'events',label:'Fonte',rows:[{id:2,timestamp:0,name:'Primeiro'},{id:0,timestamp:1000,name:'Zero'},{id:1,timestamp:2000,name:'Um'}]}];window.drawAudit('horizontal');
  });
  assert.deepEqual(await page.locator('.ct-entry').evaluateAll(nodes=>nodes.map(node=>node.dataset.id)),['e:events:2','e:events:0','e:events:1']);
  await page.locator('[data-id="e:events:0"]').focus();await page.keyboard.press('Enter');
  assert.equal(await page.evaluate(()=>window.opened.id),0,'zero ID opens the correct original event');
  await page.evaluate(()=>{const message='L'.repeat(128*1024);window.caseFixture.items[0].rows=[{id:0,timestamp:0,message}];window.drawAudit('horizontal');});
  assert.ok((await page.locator('.ct-entry strong').textContent()).length<=181);
  assert.ok((await page.locator('.ct-entry').getAttribute('title')).length<1700);
  assert.ok((await page.locator('.ct-matrix-dot').getAttribute('title')).length<1500);
  await page.locator('.ct-entry').focus();await page.keyboard.press('Enter');
  assert.equal(await page.evaluate(()=>window.opened.message.length),128*1024,'details retain the full message');
  for(const mode of ['horizontal','vertical']){
    results[mode]=await page.evaluate(async mode=>{
      const rows=Array.from({length:5000},(_,id)=>({id,timestamp:id*1000,name:mode==='horizontal'?'Solicitação':`Registro ${id}`,message:`Registro ${id}`}));
      window.caseFixture.items[0].rows=rows;window.caseFixture.timeline.compact=mode==='horizontal';
      const start=performance.now();window.drawAudit(mode);await new Promise(requestAnimationFrame);
      const renderMs=performance.now()-start,info=TimelineExport.measure({source:document.querySelector('.ct-shell'),type:mode==='horizontal'?'matrix':'vertical'}),planStart=performance.now(),pages=TimelineExport.planPages(info);
      return{renderMs,planMs:performance.now()-planStart,pages:pages.length,entries:document.querySelectorAll('.ct-entry').length,markers:document.querySelectorAll('.ct-matrix-dot').length,elementCount:document.querySelectorAll('#fixture *').length};
    },mode);
    assert.equal(mode==='horizontal'?results[mode].markers:results[mode].entries,5000);
    assert.ok(results[mode].pages<=501,'page planning is bounded before PDF allocation');
  }
  for(const width of [1024,1440])for(const theme of ['dark','light']){
    await page.setViewportSize({width,height:900});
    await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;window.caseFixture.items[0].rows=Array.from({length:12},(_,id)=>({id,timestamp:id*1000,name:`Passo ${id}`,message:'Registro de teste'}));window.caseFixture.timeline.compact=false;window.drawAudit('horizontal');},theme);
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    await page.screenshot({path:`output/playwright/timeline-audit-${width}-${theme}.png`});
  }
  results.table=await page.evaluate(()=>{
    const start=Date.UTC(2026,8,23,12,0,0,123);
    window.caseFixture.items=[{id:'events',label:'API',rows:Array.from({length:210},(_,id)=>({id,timestamp:start+id*1000,name:`Passo ${id}`,message:`Detalhe ${id}`}))},{id:'other',label:'Worker',rows:[{id:0,timestamp:start+123,message:'Outra origem'},{id:1,timestamp:null,message:'Sem horário'}]}];
    window.caseFixture.manual=[{id:'manual',name:'Marco manual',start:start+500,end:start+550,description:'Descrição do marco'}];
    window.caseFixture.timeline={compact:false,groups:[{id:'group',name:'Grupo entre origens',ids:['e:events:0','e:other:0']}],annotations:[{id:'note',anchor:'group',text:'Nota exclusiva completa',icon:'fa-comment'}],edits:{},layout:{}};
    const before=JSON.stringify(window.caseFixture), result=CaseTimeline.rows(window.caseFixture), withUndated=CaseTimeline.rows(window.caseFixture,()=>true,{includeUndated:true});
    const group=result.rows.find(row=>row.id==='group');
    const limited=(()=>{try{CaseTimeline.rows(window.caseFixture,()=>true,{limit:2});return false;}catch(error){return error.message.includes('refine')||error.message.includes('Refine');}})();
    const textLimited=(()=>{try{CaseTimeline.rows(window.caseFixture,()=>true,{maxChars:1});return false;}catch(error){return error.message.includes('nenhum texto');}})();
    const controller=new AbortController();controller.abort();const aborted=(()=>{try{CaseTimeline.rows(window.caseFixture,()=>true,{signal:controller.signal});return false;}catch(error){return error.name==='AbortError';}})();
    const filtered=CaseTimeline.rows(window.caseFixture,event=>event.id===0);const filteredGroup=filtered.rows.find(row=>row.id==='group');
    const single=CaseTimeline.rows(window.caseFixture,event=>event.name==='Passo 0');
    window.CaseReport={open:()=>window.reportOpened=true};window.drawAudit('table');
    const undatedLimit=(()=>{try{CaseTimeline.rows(window.caseFixture,()=>true,{includeUndated:true,limit:211});return false;}catch{return true;}})();
    return {total:result.rows.length,eventCount:result.eventCount,undated:result.undated,group,same:JSON.stringify(window.caseFixture)===before,limited,textLimited,aborted,filteredGroup,singleNotes:single.rows.find(row=>row.type==='event').notes.length,withUndated:{total:withUndated.rows.length,eventCount:withUndated.eventCount,last:withUndated.rows.at(-1),start:withUndated.start,end:withUndated.end,limited:undatedLimit}};
  });
  assert.equal(results.table.total,211);assert.equal(results.table.eventCount,211);assert.equal(results.table.undated,1);assert.deepEqual(results.table.group.itemIds,['events','other']);
  assert.equal(results.table.group.start,Date.UTC(2026,8,23,12,0,0,123));assert.equal(results.table.group.end,Date.UTC(2026,8,23,12,0,0,246));assert.equal(results.table.group.notes[0].text,'Nota exclusiva completa');
  for(const key of ['same','limited','textLimited','aborted'])assert.equal(results.table[key],true,key);assert.equal(results.table.singleNotes,1);
  assert.equal(results.table.withUndated.total,212);assert.equal(results.table.withUndated.eventCount,212);assert.equal(results.table.withUndated.last.id,'e:other:1');assert.equal(results.table.withUndated.last.start,null);assert.equal(results.table.withUndated.last.end,null);assert.deepEqual(results.table.withUndated.last.itemIds,['other']);assert.equal(results.table.withUndated.last.detail,'Sem horário');assert.equal(results.table.withUndated.last.count,1);assert.equal(results.table.withUndated.start,Date.UTC(2026,8,23,12,0,0,123));assert.equal(results.table.withUndated.end,Date.UTC(2026,8,23,12,0,0,123)+209000);assert.equal(results.table.withUndated.limited,true);
  assert.equal(await page.locator('.ct-data-table tbody tr').count(),100);await page.locator('[data-ct-page="next"]').click();assert.equal(await page.locator('.ct-data-table tbody tr').count(),100);
  await page.locator('[data-ct-page="next"]').click();assert.equal(await page.locator('.ct-data-table tbody tr').count(),11);
  await page.getByRole('searchbox',{name:'Buscar na tabela da timeline'}).fill('Nota exclusiva');await page.waitForFunction(()=>document.querySelector('.ct-table-pager [role="status"]').textContent==='1–1 de 1 ocorrências');
  await page.locator('.ct-table-notes summary').click();assert.equal(await page.locator('.ct-table-notes p').textContent(),'Nota exclusiva completa');
  await page.locator('[data-ct-report]').click();assert.equal(await page.evaluate(()=>window.reportOpened),true);
  await page.getByRole('searchbox',{name:'Buscar na tabela da timeline'}).fill('');await page.waitForFunction(()=>document.querySelectorAll('.ct-data-table tbody tr').length===100);
  for(const width of [1024,1440])for(const theme of ['dark','light']){await page.setViewportSize({width,height:900});await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);await page.screenshot({path:`output/playwright/timeline-table-${width}-${theme}.png`});}
  // Connected but hidden export sources must not produce a blank image.
  assert.equal(await page.evaluate(()=>{window.drawAudit('horizontal');const shell=document.querySelector('.ct-shell');shell.style.display='none';try{TimelineExport.measure({source:shell,type:'matrix'});return false;}catch{return true;}finally{shell.style.display='';}}),true);
  await page.goto(process.argv[2]||'http://127.0.0.1:4189');await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&document.querySelector('#load-overlay').hidden);
  results.image=await page.evaluate(async()=>{
    const host=document.createElement('div');host.id='image-fixture';host.style.width='1000px';document.body.append(host);
    const c={id:'image-fixture',name:'Caso gráfico',items:[{id:'item',label:'API',rows:Array.from({length:6},(_,id)=>({id,timestamp:Date.UTC(2026,8,23,12)+id*1000,name:`Passo ${id}`,message:'Descrição'}))}],timeline:{compact:false,matrixZoom:4,groups:[],annotations:[{id:'n',anchor:'e:item:0',text:'Nota do relatório',icon:'fa-comment'}],edits:{},layout:{}}};
    const callbacks={passes:()=>true,detail(){},bucket(){},menu(){},notify(){},save(){}};
    CaseTimeline.render(host,c,'horizontal',callbacks);host.querySelector('.ct-entry').click();const selected=host.querySelector('.ct-entry.selected').dataset.id,before=JSON.stringify(c),theme=document.documentElement.dataset.theme;
    const capture=await CaseTimeline.image(c,{width:1000});const image=await createImageBitmap(capture.blob),canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);const pixel=[...ctx.getImageData(0,0,1,1).data];image.close();
    const stable=before===JSON.stringify(c)&&selected===host.querySelector('.ct-entry.selected')?.dataset.id&&document.documentElement.dataset.theme===theme;
    const large={...c,items:[{id:'item',label:'API',rows:Array.from({length:2500},(_,id)=>({id,timestamp:id,name:'Evento'}))}]};const overview=await CaseTimeline.image(large,{width:1000});
    const tall={...c,items:[{id:'item',label:'API',rows:Array.from({length:35},(_,id)=>({id,timestamp:id,name:`Linha ${id}`}))}]};const fitted=await CaseTimeline.image(tall,{width:1100,maxHeight:900});
    const empty=await CaseTimeline.image({id:'empty',items:[]});host.remove();
    return {stable,bytes:capture.blob.size,width:capture.width,height:capture.height,complete:capture.complete,pixel,overview:{eventCount:overview.eventCount,complete:overview.complete,overview:overview.overview,summary:overview.summary,bytes:overview.blob.size},fit:{overview:fitted.overview,height:fitted.height,width:fitted.width},empty:empty.blob===null,leftovers:document.querySelectorAll('.ct-report-capture').length};
  });
  assert.equal(results.image.stable,true);assert.equal(results.image.complete,true);assert.deepEqual(results.image.pixel,[255,255,255,255]);assert.ok(results.image.bytes>1000);assert.equal(results.image.overview.eventCount,2500);assert.equal(results.image.overview.complete,false);assert.equal(results.image.overview.overview,true);assert.match(results.image.overview.summary,/2\.500 registros/);assert.equal(results.image.fit.overview,true);assert.ok(results.image.fit.height/results.image.fit.width<=900/1100);assert.equal(results.image.empty,true);assert.equal(results.image.leftovers,0);
  assert.deepEqual(errors,[]);writeFileSync('output/playwright/timeline-audit-validation.json',JSON.stringify({results,errors},null,2));console.log(JSON.stringify({results,errors},null,2));
}finally{await browser.close();}
