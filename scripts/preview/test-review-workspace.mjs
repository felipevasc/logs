import { chromium } from 'playwright';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const browser=await chromium.launch({headless:true,...(!existsSync(chromium.executablePath())?{executablePath:'C:/Users/felip/AppData/Local/ms-playwright/chromium-1217/chrome-win64/chrome.exe'}:{})});
const page=await browser.newPage({viewport:{width:1024,height:768},reducedMotion:'reduce'}), errors=[];
page.on('pageerror',e=>errors.push(e.message));
mkdirSync('output/playwright/review',{recursive:true});
try {
 await page.goto(process.argv[2]||'http://127.0.0.1:4190');await page.waitForFunction(()=>WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&document.querySelector('#load-overlay').hidden);
 const result={};
 for(const theme of ['dark','light']){
   await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;},theme);
   for(const view of ['summary','compare','sources','explore']){
     await page.evaluate(view=>Workspace.showPage(view),view);await page.waitForFunction(()=>!state.activeOperation);
     await page.screenshot({path:resolve(`output/playwright/review/${theme}-${view}-1024.png`)});
     result[`${theme}-${view}`]=await page.evaluate(()=>({width:innerWidth,overflow:document.documentElement.scrollWidth>innerWidth,heading:document.querySelector('#ws-title').textContent}));
   }
 }
 await page.evaluate(async()=>{state.filters=[{column:'source',op:'equals_exact',value:'no-such-source'}];await refresh();});
 result.emptyResult=await page.evaluate(()=>({total:state.total,header:document.querySelector('#context-summary').textContent}));
 assert.equal(result.emptyResult.total,0);assert.match(result.emptyResult.header,/^0 eventos/);
 await page.evaluate(async()=>{state.filters=[];await refresh();showDetail(state.rows[0]);window.__releaseDetail=null;const original=api;window.__originalApi=api;api=(cmd,args,opts)=>cmd==='event_detail'?new Promise(resolve=>window.__releaseDetail=resolve):original(cmd,args,opts);window.__pendingDetail=openDetail(state.rows[1].id);});
 result.loadingDetail=await page.evaluate(()=>({currentId:state.currentDetailEv?.id,saveVisible:!document.querySelector('#ws-detail-save').hidden,actionsVisible:!document.querySelector('.detail-quick-actions').hidden}));
 assert.equal(result.loadingDetail.currentId,undefined);assert.equal(result.loadingDetail.actionsVisible,false);
 await page.evaluate(async()=>{window.__releaseDetail(state.rows[1]);await window.__pendingDetail;api=window.__originalApi;closeDrawer();const rows=[...state.rows.slice(0,2)].map((row,i)=>({...row,timestamp:1700000000123+i*2}));activeCase().items=[{id:'comparison-fixture',rows}];caseEventsCache.sig=null;await WorkspaceContext.setScope('case',{page:'compare',animate:false});});
 result.shortCompare=await page.evaluate(()=>({beforeStart:document.querySelector('#ws-before-start')?.value,beforeEnd:document.querySelector('#ws-before-end')?.value,afterStart:document.querySelector('#ws-after-start')?.value,afterEnd:document.querySelector('#ws-after-end')?.value,text:document.querySelector('#ws-comparison')?.textContent}));
 assert.equal(+new Date(result.shortCompare.beforeStart),1700000000123);assert.equal(+new Date(result.shortCompare.beforeEnd),1700000000124);
 assert.equal(+new Date(result.shortCompare.afterStart),1700000000125);assert.equal(+new Date(result.shortCompare.afterEnd),1700000000125);assert.match(result.shortCompare.text,/O que mudou/);
 assert.equal(await page.evaluate(()=>state.currentDetailEv),null,'Switching context discards stale detail actions');
 result.caseCache=await page.evaluate(()=>{
   const c=activeCase(), saved=c.items, first=caseEvents()[0].message;
   c.items=[{id:'replacement-fixture',rows:[...saved[0].rows].map(row=>({...row,message:'replacement'}))}];
   const replacement=caseEvents()[0].message;
   const start=performance.now();c.items=[{id:'large-fixture',rows:Array.from({length:200000},(_,id)=>({id,event_ref:`review:${id}`,timestamp:id,source:'test',message:'log',fields:{}}))}];
   updateContextBar();const large={count:caseEvents().length,header:document.querySelector('#context-summary').textContent,elapsedMs:Math.round(performance.now()-start)};
   c.items=saved;caseEvents();updateContextBar();return {first,replacement,large};
 });
 assert.equal(result.caseCache.replacement,'replacement');assert.equal(result.caseCache.large.count,200000);
 await page.evaluate(()=>WorkspaceContext.setScope('dataset',{page:'explore',tab:'table',animate:false}));
 result.originDedup=await page.evaluate(()=>{
   const c=activeCase(), items=c.items, rows=state.rows, row=rows[0];
   c.items=[{id:'source-a',artifactId:'a',rows:[{...row,event_ref:'source-a:1'}]}];state.rows=[{...row,event_ref:'source-b:1'}];sendVisibleToCase();
   const included=pendingCaseAdd?.rows?.length;document.querySelector('#case-add-cancel').click();c.items=items;state.rows=rows;return included;
 });assert.equal(result.originDedup,1,'Identical records from distinct sources remain distinct');
 await page.evaluate(async()=>{state.page=10000;await refresh();});
 assert.equal(await page.evaluate(()=>state.page),59,'Pagination recovers when a saved page no longer exists');
 await page.evaluate(async()=>{const original=api;api=async(cmd,args,opts)=>['query_events','explore_snapshot'].includes(cmd)?Promise.reject(new Error('fixture query failure')):original(cmd,args,opts);try{await refresh();}finally{api=original;}});
 assert.equal(await page.locator('#events-table tbody tr[data-event-id]').count(),0);assert.match(await page.locator('#context-summary').textContent(),/Consulta não concluída/);
 await page.locator('#empty-state [data-retry]').click();await page.waitForFunction(()=>state.rows.length>0&&!state.queryError);
 result.queryFailureRecovery=true;
 assert.deepEqual(errors,[]);result.errors=errors;writeFileSync('output/playwright/review/workspace-results.json',JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
}finally{await browser.close();}
