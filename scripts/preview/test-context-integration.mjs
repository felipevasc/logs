import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const browser=await chromium.launch({headless:true,...(!existsSync(chromium.executablePath())?{executablePath:'C:/Users/felip/AppData/Local/ms-playwright/chromium-1217/chrome-win64/chrome.exe'}:{})});
const page=await browser.newPage({viewport:{width:1440,height:1000},reducedMotion:'reduce'});page.setDefaultTimeout(15000);
const errors=[];page.on('pageerror',error=>errors.push(error.message));
const result={};mkdirSync('output/playwright',{recursive:true});
const waitScope=scope=>page.waitForFunction(scope=>WorkspaceContext.scope()===scope&&!WorkspaceContext.changing,scope);
try{
 await page.goto(process.argv[2]||'http://127.0.0.1:4175');await page.waitForFunction(()=>state.loaded&&state.total===6000&&window.WorkspaceContext?.ready&&!WorkspaceContext.changing);
 await page.evaluate(async()=>{
   const data=await api('query_events',{filters:[],offset:0,limit:3,sortColumn:'id',sortDir:'asc'});
   activeCase().items=[{id:'scope-fixture',label:'Seleção relevante',name:'Seleção relevante',artifactId:state.currentArtifact?.id,rows:data.rows}];caseEventsCache.sig=null;window.__contextRows=data.rows;
   state.filters=[];state.quick='';state.sortCol='id';state.sortDir='asc';updateAnalysisBadge();
   await WorkspaceContext.setScope('dataset',{page:'explore',tab:'table',animate:false});await refresh();
 });
 assert.equal(await page.locator('#events-table tbody tr.event-in-case').count(),3,'Selected source records have case membership marks');
 result.analysis={total:await page.evaluate(()=>state.total),included:3,accent:await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--accent').trim())};
 await page.screenshot({path:resolve('output/playwright/contexts-analysis-dark-1440.png')});
 await page.locator('[data-workspace-scope="case"]').click();await waitScope('case');
 await page.getByRole('button',{name:'Explorar',exact:true}).click();await page.waitForFunction(()=>state.total===3&&document.querySelectorAll('#events-table tbody tr').length===3);
 await page.waitForFunction(()=>document.querySelector('#explore-tree').dataset.treeScope==='case'&&Number(document.querySelector('#explore-tree .field-row[data-column="source"] small')?.textContent)===new Set(window.__contextRows.map(row=>row.source)).size);
 assert.equal(await page.locator('#ws-remote').isVisible(),false);
 assert.equal(await page.locator('#ws-open').isVisible(),false);
 const selectedSource=await page.evaluate(()=>window.__contextRows[0].source);
 const expected=await page.evaluate(source=>window.__contextRows.filter(row=>row.source===source).length,selectedSource);
 await page.evaluate(source=>Discovery.applySelection([{column:'source',op:'equals_exact',value:source,value2:null}],'case',true),selectedSource);
 await page.waitForFunction(expected=>state.total===expected,expected);
 await page.waitForFunction(expected=>!state.activeOperation&&state.treeAgg.case?.level?.reduce((sum,row)=>sum+row[1],0)===expected,expected);
 const savedCaseFilters=await page.evaluate(()=>structuredClone(state.filters));
 result.case={total:await page.evaluate(()=>state.total),accent:await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--accent').trim())};
 assert.notEqual(result.analysis.accent,result.case.accent);
 await page.screenshot({path:resolve('output/playwright/contexts-case-dark-1440.png')});
 await page.locator('[data-workspace-scope="dataset"]').click();await waitScope('dataset');
 await page.waitForFunction(()=>state.total===6000&&state.rows.length===100);
 assert.deepEqual(await page.evaluate(()=>state.filters),[],'Case filters do not leak into Analysis');
 assert.equal(await page.locator('#events-table tbody tr.event-in-case').count(),3);
 await page.locator('[data-workspace-scope="case"]').click();await waitScope('case');
 assert.deepEqual(await page.evaluate(()=>state.filters),savedCaseFilters,'Case filters are restored');
 await page.setViewportSize({width:1024,height:768});await page.locator('#btn-theme').click();
 await page.waitForFunction(()=>!state.activeOperation);
 assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Light Case fits minimum width');
 await page.screenshot({path:resolve('output/playwright/contexts-case-light-1024.png')});
 await page.locator('[data-workspace-scope="dataset"]').click();await waitScope('dataset');
 await page.screenshot({path:resolve('output/playwright/contexts-analysis-light-1024.png')});
 assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Light Analysis fits minimum width');
 await page.evaluate(async()=>{activeCase().items=[];caseEventsCache.sig=null;updateAnalysisBadge();await WorkspaceContext.setScope('case',{page:'explore',tab:'table',animate:false});await refresh();});
 assert.equal(await page.evaluate(()=>state.total),0,'Empty Case never reads source records');
 assert.equal(await page.locator('#events-table tbody tr[data-event-id]').count(),0);
 await page.evaluate(async()=>{await WorkspaceContext.setScope('dataset',{page:'explore',animate:false});await refresh();});
 assert.equal(await page.evaluate(()=>state.total),6000);
 await page.evaluate(async()=>{activeCase().items=[{id:'restored-selection',label:'Seleção relevante',rows:window.__contextRows,artifactId:state.currentArtifact?.id}];caseEventsCache.sig=null;await WorkspaceContext.setScope('case',{page:'explore',animate:false});await saveCases();});
 await page.reload();await page.waitForFunction(()=>window.WorkspaceContext?.scope()==='case'&&!WorkspaceContext.changing&&state.loaded);
 assert.deepEqual(await page.evaluate(()=>state.filters),savedCaseFilters,'Case selection survives reload');
 assert.equal(await page.evaluate(()=>state.total),expected);
 await page.locator('[data-workspace-scope="dataset"]').click();await waitScope('dataset');await page.waitForFunction(()=>state.total===6000);
 assert.deepEqual(await page.evaluate(()=>state.filters),[]);
 await page.emulateMedia({reducedMotion:'no-preference'});
 await page.evaluate(()=>{
   window.__contextTransitions=[];
   const start=document.startViewTransition.bind(document);
   document.startViewTransition=(...args)=>{
     const transition=start(...args), check={ready:false,finished:false,error:null};window.__contextTransitions.push(check);
     transition.ready.then(()=>check.ready=true).catch(error=>check.error=error.message);
     transition.finished.then(()=>check.finished=true).catch(error=>check.error=error.message);
     return transition;
   };
 });
 for(const scope of ['case','dataset']) {await page.locator(`[data-workspace-scope="${scope}"]`).click();await waitScope(scope);}
 await page.waitForFunction(()=>window.__contextTransitions.length===2&&window.__contextTransitions.every(item=>item.finished));
 assert.deepEqual(await page.evaluate(()=>window.__contextTransitions),[{ready:true,finished:true,error:null},{ready:true,finished:true,error:null}],'Both slide directions render real view transitions');
 result.slideAnimation=true;
 assert.deepEqual(errors,[]);Object.assign(result,{caseMembership:true,fieldCountsScoped:true,isolatedFilters:true,emptyCase:true,reload:true,lightDark:true,reducedMotion:true,errors});
 writeFileSync('output/playwright/context-integration-validation.json',JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
}catch(error){
 console.error(JSON.stringify(await page.evaluate(()=>({scope:window.WorkspaceContext?.scope(),ready:window.WorkspaceContext?.ready,changing:window.WorkspaceContext?.changing,total:state.total,rowCount:state.rows.length,filters:state.filters,loaded:state.loaded,tab:state.activeDatasetTab,page:state.workspace?.page,caseItems:activeCase()?.items?.length,table:document.querySelector('#events-table')?.textContent?.slice(0,1200)})),null,2));
 await page.screenshot({path:resolve('output/playwright/context-integration-failure.png')});throw error;
}finally{await browser.close();}
