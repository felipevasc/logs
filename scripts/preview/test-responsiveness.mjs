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
    return {elapsed:performance.now()-began,rows:state.rows.length,total:state.total,next:document.querySelector('#pg-next').disabled,label:document.querySelector('#result-count').textContent,summaryStatus:explorerAnalytics.get(explorerKey())?.status};
  });
  assert.ok(first.rows>0);assert.equal(first.total,null);assert.equal(first.next,false);
  assert.ok(['queued','count'].includes(first.summaryStatus),'unknown total is either queued or actively counting');
  assert.match(first.label,first.summaryStatus==='queued'?/total aguardando cálculo$/:/total em cálculo$/,'visible total label matches its actual summary phase');
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
    const currentChart=chart;
    window.dispatchEvent(new Event('resize'));toggleTheme();toggleTheme();await new Promise(r=>setTimeout(r,600));
    if(currentChart&&chart!==currentChart)throw Error('Theme repaint replaced the histogram');
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
  await page.evaluate(async()=>{
    window.__mockLatency={count_filtered:1200,stats_events:1200};
    state.filters=[{column:'message',op:'not_contains',value:'__pause_resume_summary_unique__'}];state.page=0;
    await refresh();
  });
  await page.locator('#query-analytics-status button').filter({hasText:'Pausar resumo'}).click();
  await page.waitForFunction(()=>document.querySelector('#result-count').textContent.includes('total pausado'));
  assert.ok(await page.locator('#events-table tbody tr').count()>0,'paused summary keeps visible rows');
  assert.equal(await page.locator('#pg-next').isDisabled(),false,'paused summary keeps paging available');
  assert.match(await page.locator('#context-summary').innerText(),/pausado/);
  assert.doesNotMatch(await page.locator('#workbar-label').innerText(),/atualizado/i);
  assert.match(await page.locator('#chart').innerText(),/Timeline pausada/);
  assert.equal(await page.getByRole('combobox',{name:'Agrupar Timeline por campo',exact:true}).isVisible(),true,'paused summary keeps the explicit field choice available');
  assert.equal(await page.locator('.explorer-timeline-legend').isVisible(),false,'an empty legend reserves no paused placeholder space');
  assert.equal(await page.locator('.explorer-timeline-note').isVisible(),false,'an empty note reserves no paused placeholder space');
  assert.ok((await page.locator('.hist-panel').boundingBox()).height<80,'paused empty histogram uses a compact honest placeholder');
  await page.screenshot({path:'output/playwright/summary-paused.png',fullPage:true});
  await page.evaluate(()=>{window.__mockLatency={count_filtered:50,stats_events:50};});
  await page.locator('#query-analytics-status button').filter({hasText:'Retomar resumo'}).click();
  await page.waitForFunction(()=>Number.isFinite(state.total)&&document.querySelector('#query-analytics-status').hidden);
  await page.waitForFunction(()=>Tasks.pending()===0,null,{timeout:30000});
  const beforeDraft=await page.evaluate(()=>JSON.stringify(Object.entries(window.__mockCommandCalls).filter(([key])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(key))));
  const beforeFilters=await page.evaluate(()=>state.filters.length);
  await page.locator('#quick-search').fill('status>=500');
  await page.waitForTimeout(500);
  assert.equal(await page.evaluate(()=>JSON.stringify(Object.entries(window.__mockCommandCalls).filter(([key])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(key)))),beforeDraft,'typing a search draft starts no native data work');
  await page.locator('#quick-search').press('Enter');
  await page.waitForFunction(n=>state.filters.length===n+1&&document.querySelector('#events-table').getAttribute('aria-busy')==='false',beforeFilters);
  assert.equal(await page.locator('#quick-search').inputValue(),'');
  assert.equal(await page.evaluate(()=>state.filters.at(-1).op),'query');
  const filterTimestamp=await page.evaluate(()=>state.rows[0].timestamp);
  assert.match(await page.evaluate(()=>state.rows[0].event_ref),/^preview:\d+$/,'ordinary fixture rows carry the stable native handle required by exact field actions');
  await page.locator('#events-table tbody tr td[data-column="timestamp"]').first().click({button:'right'});
  await page.getByText('Criar filtro: Data/hora',{exact:true}).click();
  await page.locator('#filter-pop').waitFor({state:'visible'});
  assert.equal(await page.locator('#fp-col').inputValue(),'timestamp');
  assert.equal(await page.locator('#fp-op').inputValue(),'between');
  assert.equal(Date.parse(await page.locator('#fp-val').inputValue()),filterTimestamp,'native canonical ISO text represents the exact selected timestamp');
  assert.equal(await page.locator('#fp-val2').inputValue(),await page.locator('#fp-val').inputValue());
  for(const operator of ['contains','equals','gt','gte','lt','lte','between','regex','cidr']) assert.ok(await page.locator(`#fp-op option[value="${operator}"]`).count(),operator);
  await page.locator('#fp-val2').press('Escape');
  assert.equal(await page.locator('#filter-pop').isVisible(),false);
  assert.equal(await page.evaluate(()=>document.activeElement.id),'btn-add-filter');
  assert.equal(await page.evaluate(()=>state.filters.length),beforeFilters+1,'cancelled composer never applies its draft');
  // Direct chip editing reuses the same composer without submitting a query.
  // CI55 caught the saved-view Dataset/Case count pair still behind its 300 ms
  // debounce: Tasks.pending() alone cannot include work not yet admitted.
  await page.evaluate(()=>settleFilterTabCounts());
  await page.waitForFunction(()=>Tasks.pending()===0,null,{timeout:30000});
  const chipEditor=page.locator('#chips .chip-edit').first();
  const chipState=await page.evaluate(()=>({filters:JSON.stringify(state.filters),value:state.filters[0].value,
    calls:JSON.stringify(Object.entries(window.__mockCommandCalls).filter(([key])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(key)))}));
  for(const activation of ['click','Enter','Space']){
    if(activation==='click')await chipEditor.click();else await chipEditor.press(activation);
    await page.locator('#filter-pop').waitFor({state:'visible'});
    assert.equal(await page.locator('#fp-val').inputValue(),chipState.value);
    await page.locator('#fp-val').fill(chipState.value+' cancelled draft');
    if(activation==='Enter')await page.locator('#fp-cancel').click();else await page.locator('#fp-val').press('Escape');
    assert.equal(await chipEditor.evaluate(node=>node===document.activeElement),true,activation+' cancellation returns to its chip');
    assert.equal(await page.evaluate(()=>JSON.stringify(state.filters)),chipState.filters);
  }
  assert.equal(await page.evaluate(()=>JSON.stringify(Object.entries(window.__mockCommandCalls).filter(([key])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(key)))),chipState.calls,'opening and cancelling from click/Enter/Space starts no native data work');
  await chipEditor.press('Enter');await page.locator('#fp-val').fill(chipState.value+' edited');await page.locator('#fp-apply').click();
  await page.waitForFunction(()=>document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,null,{timeout:30000});
  assert.equal(await page.evaluate(()=>state.filters[0].value),chipState.value+' edited');
  assert.equal(await chipEditor.evaluate(node=>node===document.activeElement),true,'saving and query completion return to the replacement chip');
  const previousLatency=await page.evaluate(()=>{const value=window.__mockLatency;window.__mockLatency={...value,query_page:600};return value;});
  await chipEditor.press('Space');await page.locator('#fp-val').fill(chipState.value+' saved');await page.locator('#fp-apply').click();
  await page.locator('#quick-search').focus();
  await page.waitForFunction(()=>document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,null,{timeout:30000});
  assert.equal(await page.locator('#quick-search').evaluate(node=>node===document.activeElement),true,'late query completion cannot steal focus moved by the user');
  await page.evaluate(value=>{window.__mockLatency=value;},previousLatency);
  await page.evaluate(()=>addFilter({column:'message',op:'not_contains',value:'__chip_remove_regression__',value2:null}));
  await page.waitForFunction(()=>document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,null,{timeout:30000});
  await page.locator('#chips .filter-chip').filter({hasText:'__chip_remove_regression__'}).locator('.x').click();
  assert.equal(await page.locator('#filter-pop').isVisible(),false,'the separate remove action cannot bubble into editing');
  await page.waitForFunction(()=>document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,null,{timeout:30000});
  assert.equal(await page.evaluate(()=>state.filters.length),beforeFilters+1);
  // Existing exact-list filters must survive the real DOM select unchanged.
  const exactBefore=await page.evaluate(()=>({filters:JSON.stringify(state.filters),total:state.total,
    value:[...new Set(state.rows.map(row=>row.event_ref))].slice(0,2).join('\n')}));
  assert.equal(exactBefore.value.split('\n').length,2,'use two existing stable event references');
  await page.evaluate(value=>addFilter({column:'event_ref',op:'in_exact',value,value2:null}),exactBefore.value);
  await page.waitForFunction(()=>document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,null,{timeout:30000});
  await page.evaluate(()=>settleFilterTabCounts());await page.waitForFunction(()=>Tasks.pending()===0);
  const exactChip=page.locator('#chips .chip-edit').last();
  const exactOpen=await page.evaluate(()=>({filters:JSON.stringify(state.filters),
    calls:JSON.stringify(Object.entries(window.__mockCommandCalls).filter(([key])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(key)))}));
  await exactChip.click();await page.locator('#filter-pop').waitFor({state:'visible'});
  assert.equal(await page.locator('#fp-op').inputValue(),'in_exact','native select retains the temporary exact-list option');
  assert.match(await page.locator('#fp-op option:checked').textContent(),/lista exata.*por linha/);
  assert.equal(await page.locator('#fp-val').inputValue(),exactBefore.value,'multiline list text is literal');
  await page.locator('#fp-val').fill(exactBefore.value+'\ncancelled draft');await page.locator('#fp-cancel').click();
  assert.equal(await page.evaluate(()=>JSON.stringify(state.filters)),exactOpen.filters);
  assert.equal(await exactChip.evaluate(node=>node===document.activeElement),true);
  assert.equal(await page.evaluate(()=>JSON.stringify(Object.entries(window.__mockCommandCalls).filter(([key])=>['query_page','count_filtered','stats_events','tree_aggs'].includes(key)))),exactOpen.calls,'opening/cancelling in_exact starts no data query');
  await exactChip.press('Enter');assert.equal(await page.locator('#fp-op').inputValue(),'in_exact');
  await page.locator('#fp-apply').click();
  await page.waitForFunction(()=>document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,null,{timeout:30000});
  assert.deepEqual(await page.evaluate(()=>state.filters.at(-1)),{column:'event_ref',op:'in_exact',value:exactBefore.value,value2:null});
  assert.equal(await exactChip.evaluate(node=>node===document.activeElement),true);
  await page.locator('#chips .filter-chip').last().locator('.x').click();
  await page.waitForFunction(total=>state.total===total&&document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&Tasks.pending()===0,exactBefore.total,{timeout:30000});
  assert.equal(await page.evaluate(()=>JSON.stringify(state.filters)),exactBefore.filters,'removing the temporary list restores every prior filter');
  assert.equal(await page.evaluate(()=>state.filters.length),beforeFilters+1);
  for(const [width,height] of [[1440,900],[1024,768]]){
    await page.setViewportSize({width,height});await chipEditor.focus();
    assert.equal(await chipEditor.evaluate(node=>getComputedStyle(node).fontSize===getComputedStyle(node.parentElement).fontSize),true,'edit label keeps chip typography');
    assert.ok(await chipEditor.evaluate(node=>node.getBoundingClientRect().height>=node.parentElement.getBoundingClientRect().height-2),'edit target fills existing chip height');
    assert.equal(await chipEditor.locator('button').count(),0,'no nested buttons');
    await page.screenshot({path:`output/playwright/filter-chip-edit-${width}.png`,fullPage:true});
  }
  await page.setViewportSize({width:1440,height:900});
  await chipEditor.click();await page.locator('#filter-pop').waitFor({state:'visible'});
  await page.screenshot({path:'output/playwright/filter-chip-editor-1440.png',fullPage:true});
  await page.locator('#fp-cancel').click();
  const failedImport=await page.evaluate(async()=>{
    window.__mockLatency={};
    const before={id:state.currentArtifact.id,source:JSON.stringify(state.currentArtifact.source),rows:state.rows.length};
    window.__mockFailures={load_file:'Simulated unreadable new source'};
    const result=await loadData({kind:'file',path:'C:\\mock\\missing.log',paths:['C:\\mock\\missing.log'],format:'auto'});
    window.__mockFailures={};
    return {result,before,after:{id:state.currentArtifact.id,source:JSON.stringify(state.currentArtifact.source),rows:state.rows.length},loaded:state.loaded,unconfirmed:state.sourceIdentityUnconfirmed};
  });
  assert.equal(failedImport.result,false);assert.equal(failedImport.loaded,true);assert.equal(failedImport.unconfirmed,false);
  assert.equal(failedImport.before.id,failedImport.after.id);assert.ok(failedImport.after.rows>0,'failed import preserves queryable old source');
  const preview = await page.evaluate(() => {
    const original = state.rows[0], value = '<long-request>'.repeat(80000);
    const event = { ...original, message: value };
    const cell = buildEventRow(event, ['message']).children[0];
    return { length: cell.textContent.length, titleLength: cell.title.length, truncated: cell.dataset.previewTruncated, fullValue: event.message === value, originalUnchanged: state.rows[0] === original };
  });
  assert.ok(preview.length <= 4096); assert.ok(preview.titleLength < 500);
  assert.equal(preview.truncated, 'true'); assert.equal(preview.fullValue, true); assert.equal(preview.originalUnchanged, true);
  const chartDraft = await page.evaluate(() => {
    const value = state.rows[0].source;
    showChartValueActions({ preventDefault() {}, clientX: 300, clientY: 200, target: document.querySelector('#events-table') }, { chart: 'terms', field: 'source' }, value, workspaceScope());
    return { value, filters: JSON.stringify(state.filters) };
  });
  await page.getByText('Criar filtro: Origem', { exact: true }).click();
  assert.equal(await page.locator('#fp-val').inputValue(), chartDraft.value);
  assert.equal(await page.locator('#fp-op').inputValue(), 'equals_exact');
  await page.locator('#fp-op').selectOption('not_contains');
  await page.locator('#fp-cancel').click();
  assert.equal(await page.evaluate(() => JSON.stringify(state.filters)), chartDraft.filters, 'chart composer cancellation preserves active filters');
  // A draft belongs to its workspace and must never become an applied filter.
  await page.evaluate(async () => { await Workspace.showPage('explore'); await loadExplorerAnalytics(explorerKey(), workspaceScope(), backendFilters()).promise; await settleFilterTabCounts(); });
  await page.waitForFunction(() => Tasks.pending() === 0);
  const before = await page.evaluate(() => ({ calls: structuredClone(window.__mockCommandCalls), quick: state.quick, filters: JSON.stringify(state.filters), caseId: activeCase().id }));
  const draft = 'message:"pending nginx';
  await page.locator('#quick-search').fill(draft);
  await page.locator('#quick-search').evaluate(input => input.setSelectionRange(3, 8, 'backward'));
  await page.waitForTimeout(250);
  const typed = await page.evaluate(() => ({ calls: window.__mockCommandCalls, quick: state.quick, filters: JSON.stringify(state.filters) }));
  assert.deepEqual(typed.calls, before.calls, 'typing creates no queries or saves');
  assert.equal(typed.quick, before.quick); assert.equal(typed.filters, before.filters);
  await page.evaluate(() => WorkspaceContext.setScope('case', { page: 'explore', animate: false }));
  assert.equal(await page.locator('#quick-search').inputValue(), '');
  await page.locator('#quick-search').fill('case-only draft');
  await page.evaluate(() => WorkspaceContext.setScope('dataset', { page: 'explore', animate: false }));
  assert.equal(await page.locator('#quick-search').inputValue(), draft);
  assert.deepEqual(await page.locator('#quick-search').evaluate(input => [input.selectionStart, input.selectionEnd, input.selectionDirection]), [3, 8, 'backward']);
  assert.equal(await page.locator('#btn-add-search').isDisabled(), false);
  const draftCase = await page.evaluate(() => newCase('Draft isolation fixture').id);
  await page.waitForFunction(() => !WorkspaceContext.changing && !state.analysisDefinitionsPending);
  assert.notEqual(draftCase, before.caseId);
  assert.deepEqual(await page.evaluate(() => ({ loaded: state.loaded, total: state.total, artifact: state.currentArtifact,
    artifacts: activeCase().artifacts, columns: state.columns, rows: state.rows, filters: state.filters, quick: state.quick })),
    { loaded: false, total: 0, artifact: null, artifacts: [], columns: [], rows: [], filters: [], quick: '' },
    'a new Case inherits neither the old source nor its applied selection');
  assert.equal(await page.locator('#quick-search').inputValue(), '');
  assert.equal(await page.locator('#quick-search').isVisible(), false, 'empty dataset stays on Summary until a source is opened');
  // Dataset Explore intentionally redirects an empty Case to Summary. Open a
  // distinct source explicitly before testing that Case's independent draft.
  const draftSource = 'C:\\mock\\draft-isolation.jsonl';
  assert.equal(await page.evaluate(path => loadData({ kind: 'file', path, paths: [path], format: 'auto' }), draftSource), true);
  await page.evaluate(() => Workspace.showPage('explore'));
  await page.locator('#quick-search').waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(() => state.sourcePublication.analysisContext.caseId), draftCase);
  assert.equal(await page.evaluate(() => state.currentArtifact.source.path), draftSource);
  assert.equal(await page.locator('#quick-search').inputValue(), '');
  await page.locator('#quick-search').fill('other case draft');
  await page.evaluate(id => WorkspaceContext.changeCase(id), before.caseId);
  assert.equal(await page.locator('#quick-search').inputValue(), draft);
  await page.evaluate(() => WorkspaceContext.setScope('case', { page: 'explore', animate: false }));
  assert.equal(await page.locator('#quick-search').inputValue(), 'case-only draft');
  await page.evaluate(() => saveCases());
  await page.reload();
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && !state.loadOverlay);
  assert.equal(await page.locator('#quick-search').inputValue(), 'case-only draft');
  assert.equal(await page.locator('#btn-add-search').isDisabled(), false, 'restored startup draft enables Add without typing');
  assert.deepEqual(errors,[]);
  console.log(JSON.stringify({first,local,cancel,errors},null,2));
} catch (error) { await captureFailure(page, 'responsiveness', error, { errors }); throw error; }
finally {await browser.close();}
