/* Interactive work must not wait for exact analytics or resend a huge case. */
import assert from 'node:assert/strict';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
const browser=await launchBrowser();
const page=await browser.newPage({viewport:{width:1440,height:900},reducedMotion:'reduce'});
const errors=[];page.on('pageerror',error=>errors.push(error.message));
try {
  await page.goto(process.argv[2]||'http://127.0.0.1:4173');
  await page.waitForFunction(()=>window.WorkspaceContext?.ready&&state.loaded&&!state.loadOverlay);
  await page.evaluate(()=>Workspace.showPage('explore'));
  await page.waitForTimeout(1200);
  const first=await page.evaluate(async()=>{
    window.__mockLatency={count_filtered:1500,stats_events:1500};
    state.filters=[{column:'message',op:'not_contains',value:'__responsiveness_unique_missing__'}];state.page=0;
    const began=performance.now();await refresh();
    return {elapsed:performance.now()-began,rows:state.rows.length,total:state.total,next:document.querySelector('#pg-next').disabled,label:document.querySelector('#result-count').textContent};
  });
  assert.ok(first.rows>0);assert.equal(first.total,null);assert.equal(first.next,false);assert.match(first.label,/em cálculo/);
  assert.ok(first.elapsed<1300,'first page arrives before delayed exact count');
  await page.click('#pg-next');
  await page.waitForFunction(()=>state.page===1&&document.querySelector('#events-table').getAttribute('aria-busy')==='false');
  assert.equal(await page.evaluate(()=>window.__mockRequests.filter(r=>r.cmd==='query_page').at(-1).cursor),'100');
  await page.waitForFunction(()=>Number.isFinite(state.total));
  await page.evaluate(()=>{window.__mockLatency={};});
  await page.evaluate(()=>loadExplorerAnalytics(explorerKey(),workspaceScope(),backendFilters()).promise);
  await page.waitForFunction(()=>Tasks.pending()===0,null,{timeout:30000});
  const local=await page.evaluate(async()=>{
    window.__mockLatency={};
    const queryCalls=()=>Object.entries(window.__mockCommandCalls).filter(([k])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(k));
    const before=JSON.stringify(queryCalls()),saves=window.__mockCommandCalls.cases_save||0;
    const row=document.querySelector('#events-table tbody tr'),cell=row.children[0],field=state.columns.find(c=>!state.visibleCols.includes(c));
    if(field) toggleDetailColumn(field);
    window.dispatchEvent(new Event('resize'));await new Promise(r=>setTimeout(r,600));
    const currentRow=document.querySelector('#events-table tbody tr');
    return {sameRow:row===currentRow,sameCell:cell===currentRow.children[0],sameQueries:before===JSON.stringify(queryCalls()),sameSaves:saves===(window.__mockCommandCalls.cases_save||0)};
  });
  assert.deepEqual(local,{sameRow:true,sameCell:true,sameQueries:true,sameSaves:true});
  const cancel=await page.evaluate(async()=>{
    window.__mockLatency={query_page:300};const args={filters:[],sortColumn:'timestamp',sortDir:'desc',offset:0};
    const old=api('query_page',{...args,limit:11},{latest:'test-a'}).then(()=>false,()=>true);
    const other=api('query_page',{...args,limit:12},{latest:'test-b'}).then(()=>true);
    const newest=api('query_page',{...args,limit:13},{latest:'test-a'}).then(()=>true);
    return Promise.all([old,other,newest]);
  });
  assert.deepEqual(cancel,[true,true,true]);
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({first,local,cancel,errors},null,2));
} catch (error) { await captureFailure(page, 'responsiveness', error, { errors }); throw error; }
finally {await browser.close();}
