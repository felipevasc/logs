/* Explicit local previews and Case-owned typed-field persistence, for browser CI. */
import assert from 'node:assert/strict';
import {launchBrowser} from './browser.mjs';
import {captureFailure} from './diagnostics.mjs';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const output=resolve('output/playwright');mkdirSync(output,{recursive:true});
const browser=await launchBrowser(),page=await browser.newPage({viewport:{width:1440,height:960},reducedMotion:'reduce'});
await page.addInitScript(()=>{window.__mockRareDerivedFieldsEnabled=true;});
page.setDefaultTimeout(20000);const errors=[],results={};let phase='startup';page.on('pageerror',error=>errors.push(error.message));
const settled=async()=>{
  await page.waitForFunction(()=>Number.isFinite(state.total)&&explorerAnalytics.get(explorerKey())?.status==='done'&&document.querySelector('#events-table').getAttribute('aria-busy')==='false');
  await page.evaluate(()=>settleFilterTabCounts());await page.waitForFunction(()=>Tasks.pending()===0);
};
const addStep=async value=>{await page.locator('#ft-step-choice').selectOption(value);await page.locator('#ft-add-step').click();};
const openHeader=async()=>{await page.locator('#events-table th').filter({hasText:'mock_payload_b64'}).click({button:'right'});await page.getByRole('menuitem',{name:'Transformar campo',exact:true}).click();};
// Run in real CI browser geometry; this covers form reflow rather than native WebView zoom.
const dialogLayout=async()=>{
  const layout=await page.locator('.field-transform-modal').evaluate(modal=>{
    const body=modal.querySelector('.modal-body'),footer=modal.querySelector('.ft-footer'),bounds=modal.getBoundingClientRect();
    body.scrollTop=0;const before=footer.getBoundingClientRect().top;body.scrollTop=body.scrollHeight;
    const after=footer.getBoundingClientRect(),controls=[...footer.querySelectorAll('button')];
    return{width:innerWidth,inViewport:bounds.left>=0&&bounds.right<=innerWidth&&bounds.top>=0&&bounds.bottom<=innerHeight,
      bodyWidth:body.clientWidth,bodyScrollWidth:body.scrollWidth,bodyBottom:body.getBoundingClientRect().bottom,footerTop:after.top,footerShift:after.top-before,
      statusOutsideBody:!body.contains(document.querySelector('#ft-status')),actionsOutsideBody:controls.every(button=>!body.contains(button)),
      controls:controls.map(button=>{const rect=button.getBoundingClientRect();return{id:button.id,height:rect.height,fontSize:parseFloat(getComputedStyle(button).fontSize),reachable:rect.left>=0&&rect.right<=innerWidth&&rect.top>=0&&rect.bottom<=innerHeight&&button.contains(document.elementFromPoint(rect.x+rect.width/2,rect.y+rect.height/2))};})};
  });
  assert.equal(layout.inViewport,true);assert.ok(layout.bodyScrollWidth<=layout.bodyWidth+1,`transform form overflows at ${layout.width}px`);
  assert.ok(layout.bodyBottom<=layout.footerTop+1);assert.equal(layout.footerShift,0);assert.equal(layout.statusOutsideBody,true);assert.equal(layout.actionsOutsideBody,true);
  for(const control of layout.controls){assert.equal(control.reachable,true,`${control.id} remains reachable at ${layout.width}px`);assert.ok(control.height>=30);assert.ok(control.fontSize>=12);}
  await page.locator('#ft-save').focus();await page.keyboard.press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'ft-close');
  await page.keyboard.press('Shift+Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'ft-save');
  return layout;
};
const readablePreview=async()=>{
  const values=await page.locator('#ft-original,#ft-output').evaluateAll(nodes=>{
    const luminance=color=>{
      const [r,g,b]=color.match(/[\d.]+/g).slice(0,3).map(Number).map(value=>{value/=255;return value<=.04045?value/12.92:((value+.055)/1.055)**2.4;});
      return .2126*r+.7152*g+.0722*b;
    };
    return nodes.map(node=>{const style=getComputedStyle(node),a=luminance(style.color),b=luminance(style.backgroundColor);return{id:node.id,contrast:(Math.max(a,b)+.05)/(Math.min(a,b)+.05),selection:style.userSelect,overflow:style.overflowY};});
  });
  for(const value of values){assert.ok(value.contrast>=4.5,`${value.id} text contrast: ${value.contrast}`);assert.equal(value.selection,'text');assert.equal(value.overflow,'auto');}
  return values;
};
try{
  await page.goto(process.argv[2]||'http://127.0.0.1:4174');
  await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&!state.loadOverlay&&document.querySelector('#load-overlay').hidden);
  await page.getByRole('button',{name:'Explorar',exact:true}).click();
  await page.waitForFunction(()=>explorerAnalytics.get(explorerKey())?.status==='done');
  await page.evaluate(()=>{state.visibleCols=[...new Set([...state.visibleCols,'mock_payload_b64'])];saveVisibleCols();renderTable({total:state.total,rows:state.rows},{reuseRows:true});});
  const originalCase=await page.evaluate(()=>activeCase().id);
  results.original=await page.evaluate(()=>state.rows.find(row=>row.fields?.mock_payload_b64)?.fields.mock_payload_b64);
  phase='explicit preview';await openHeader();await page.locator('#ft-name').fill('decoded_payload');
  await addStep('base64_decode');await addStep('parse_json');
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.preview_field_transform||0),0,'editing does not preview on keystrokes');
  await page.locator('#ft-preview').click();await page.waitForFunction(()=>document.querySelector('#ft-result-type').textContent==='Objeto');
  assert.match(await page.locator('#ft-output').textContent(),/preview-user/);assert.match(await page.locator('#ft-output').textContent(),/true/);
  results.darkPreview=await readablePreview();
  await page.screenshot({path:resolve(output,'field-transform-json-1440.png')});
  phase='typed save and field catalog';await page.locator('#ft-save').click();await page.waitForFunction(()=>document.querySelector('#field-transform-modal').hidden);
  await page.waitForFunction(()=>state.columns.includes('decoded_payload.allowed')&&state.rows.some(row=>typeof row.fields?.decoded_payload?.allowed==='boolean'));
  results.typed=await page.evaluate(()=>{const row=state.rows.find(row=>row.fields?.decoded_payload);return{source:row.fields.mock_payload_b64,parent:row.fields.decoded_payload,child:row.fields['decoded_payload.allowed']};});
  assert.equal(results.typed.source,results.original);assert.equal(typeof results.typed.parent.allowed,'boolean');assert.equal(typeof results.typed.child,'boolean');
  await page.locator('#btn-colpicker').click();assert.equal(await page.locator('#col-list').getByText('decoded_payload.allowed',{exact:true}).count(),1);await page.locator('#btn-colpicker').click();
  // Real UI wiring against the preview transport; native transform semantics have separate tests.
  phase='derived child filter and chart';await settled();
  const analysisBefore=await page.evaluate(()=>({owner:AnalysisContexts.capture(),filters:structuredClone(state.filters),quick:state.quick,tab:state.activeDatasetTab,mode:Discovery.mode(),total:state.total,
    sourceRows:state.rows.filter(row=>row.fields?.mock_payload_b64).map(row=>({id:row.id,payload:row.fields.mock_payload_b64}))}));
  assert.equal(analysisBefore.total,6000);assert.deepEqual(analysisBefore.filters,[]);assert.equal(analysisBefore.quick,'');assert.equal(analysisBefore.sourceRows.length,8,'all eight encoded fixture rows must be visible before filtering');
  const expectedIds=analysisBefore.sourceRows.filter(row=>JSON.parse(Buffer.from(row.payload,'base64').toString()).allowed===true).map(row=>row.id).sort((a,b)=>a-b);
  assert.equal(expectedIds.length,4);
  await page.evaluate(()=>{
    window.__fieldAnalysisCalls=[];window.__fieldAnalysisApi=api;
    api=async(command,args,options)=>{
      const observed=['query_page','compute_series'].includes(command)&&args.filters?.some(filter=>filter.column==='decoded_payload.allowed');
      const request=observed?{command,args:structuredClone(args),owner:AnalysisContexts.capture()}:null;
      const response=await window.__fieldAnalysisApi(command,args,options);
      if(request)window.__fieldAnalysisCalls.push({...request,response:structuredClone(response),transportOwner:structuredClone(window.__mockRequests.findLast(call=>call.cmd===command)?.analysisContext)});
      return response;
    };
  });
  await page.locator('#btn-add-filter').click();await page.getByRole('combobox',{name:'Campo do filtro',exact:true}).selectOption('decoded_payload.allowed');
  await page.getByRole('combobox',{name:'Operador do filtro',exact:true}).selectOption('equals_exact');await page.locator('#fp-val').fill('true');await page.locator('#fp-apply').click();
  await page.waitForFunction(()=>state.total===4&&document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&explorerAnalytics.get(explorerKey())?.status==='done');
  const childFilter={column:'decoded_payload.allowed',op:'equals_exact',value:'true',value2:null};
  const filteredChild=await page.evaluate(()=>({filters:state.filters,rows:state.rows.map(row=>({id:row.id,value:row.fields['decoded_payload.allowed']})),request:window.__fieldAnalysisCalls.findLast(call=>call.command==='query_page')}));
  assert.deepEqual(filteredChild.filters,[childFilter]);assert.deepEqual(filteredChild.rows.map(row=>row.id).sort((a,b)=>a-b),expectedIds);
  assert.ok(filteredChild.rows.every(row=>row.value===true));assert.deepEqual(filteredChild.request.args.filters,[childFilter]);
  assert.deepEqual(filteredChild.request.response.rows.map(row=>row.id).sort((a,b)=>a-b),expectedIds);assert.equal(filteredChild.request.response.hasMore,false);
  assert.deepEqual(filteredChild.request.owner,analysisBefore.owner);assert.deepEqual(filteredChild.request.transportOwner,analysisBefore.owner.identity);
  await page.getByRole('button',{name:'Descobrir',exact:true}).click();await page.getByRole('button',{name:'Meus gráficos',exact:true}).click();
  await page.locator('#btn-dash-add').click();await page.locator('#cp-title').fill('Permissões derivadas');await page.locator('#cp-type').selectOption('bar');
  await page.locator('#cp-chart').selectOption('terms');await page.locator('#cp-metric').selectOption('count');await page.locator('#cp-field').selectOption('decoded_payload.allowed');await page.locator('#cp-apply').click();
  const derivedChart=page.locator('#dash-grid .dash-card').filter({has:page.locator('.dash-card-title').filter({hasText:'Permissões derivadas'})});
  await derivedChart.locator('.hbar-row').waitFor();
  const chartRequest=await page.evaluate(()=>window.__fieldAnalysisCalls.findLast(call=>call.command==='compute_series'&&call.args.spec.field==='decoded_payload.allowed'));
  assert.deepEqual(chartRequest.args.filters,[childFilter]);assert.equal(chartRequest.args.spec.chart,'terms');assert.equal(chartRequest.args.spec.metric,'count');
  assert.deepEqual(chartRequest.owner,analysisBefore.owner);assert.deepEqual(chartRequest.transportOwner,analysisBefore.owner.identity);
  assert.deepEqual(chartRequest.response,{kind:'terms',unit:null,x:['true'],x_values:['true'],series:[{name:'decoded_payload.allowed',points:[4]}]});
  assert.deepEqual(await derivedChart.locator('.hbar-row').evaluateAll(rows=>rows.map(row=>({label:row.querySelector('.hbar-label').textContent,value:row.querySelector('.hbar-val').textContent}))),[{label:'true',value:'4'}]);
  results.derivedAnalysis={evidence:'Real browser controls with preview transport; not native-engine verification',owner:analysisBefore.owner.identity,filter:childFilter,eventIds:expectedIds,chart:chartRequest.response};
  await page.screenshot({path:resolve(output,'field-transform-derived-chart-1440.png')});
  await derivedChart.getByTitle('Remover gráfico',{exact:true}).click();await page.locator(`.discovery-mode[data-mode="${analysisBefore.mode}"]`).click();
  await page.locator(`#tabbtn-${analysisBefore.tab}`).click();await page.locator('#btn-clear-filters').click();
  await page.waitForFunction(total=>state.total===total&&document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&explorerAnalytics.get(explorerKey())?.status==='done',analysisBefore.total);
  assert.deepEqual(await page.evaluate(()=>({filters:state.filters,quick:state.quick,tab:state.activeDatasetTab,mode:Discovery.mode(),owner:AnalysisContexts.capture()})),
    {filters:analysisBefore.filters,quick:analysisBefore.quick,tab:analysisBefore.tab,mode:analysisBefore.mode,owner:analysisBefore.owner});
  await settled();
  await page.evaluate(()=>{api=window.__fieldAnalysisApi;delete window.__fieldAnalysisApi;delete window.__fieldAnalysisCalls;});
  phase='JWT warning and retry';await openHeader();await page.locator('#ft-sample').fill('eyJhbGciOiJub25lIn0.eyJzdWIiOiJsb2NhbCJ9.');await addStep('jwt_payload');
  assert.ok(await page.locator('#ft-jwt-warning').isVisible());await page.locator('#ft-preview').click();
  await page.waitForFunction(()=>document.querySelector('#ft-result-type').textContent==='Objeto');assert.match(await page.locator('#ft-jwt-warning').textContent(),/assinatura não foi verificada/);
  await page.locator('#ft-sample').fill('invalid-token');await page.locator('#ft-preview').click();await page.waitForFunction(()=>document.querySelector('#ft-status').classList.contains('ft-error'));
  assert.ok(await page.locator('#field-transform-modal').isVisible());assert.equal(await page.locator('#ft-sample').inputValue(),'invalid-token');
  results.compactDialogs=[];
  for(const width of [1024,640,320]){
    await page.setViewportSize({width,height:640});results.compactDialogs.push(await dialogLayout());
    assert.equal(await page.locator('#ft-sample').inputValue(),'invalid-token');assert.equal(await page.locator('#ft-jwt-warning').isVisible(),true);
    assert.equal(await page.locator('.field-transform-modal .modal-body p').filter({hasText:'Até 8 etapas'}).count(),1);
    await page.screenshot({path:resolve(output,`field-transform-actions-error-dark-${width}.png`)});
  }
  phase='long transform error keyboard scrolling';
  await page.evaluate(()=>{window.__transformLongErrorApi=api;api=async(command,args,options)=>{if(command==='preview_field_transform')return new Promise((resolve,reject)=>{window.__transformLongErrorReject=()=>reject(Error('Falha de prévia recuperável. '.repeat(100)));});return window.__transformLongErrorApi(command,args,options);};});
  await page.locator('#ft-preview').click();await page.waitForFunction(()=>typeof window.__transformLongErrorReject==='function');
  await page.locator('#ft-cancel').focus();await page.evaluate(()=>window.__transformLongErrorReject());
  await page.waitForFunction(()=>document.querySelector('#ft-status').textContent.includes('Falha de prévia recuperável'));
  await page.evaluate(()=>{api=window.__transformLongErrorApi;delete window.__transformLongErrorApi;delete window.__transformLongErrorReject;});
  assert.equal(await page.evaluate(()=>document.activeElement.id),'ft-cancel','incoming error does not steal focus');
  results.longError=await dialogLayout();
  await page.locator('#ft-preview').focus();await page.keyboard.press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'ft-status');
  results.longError.status=await page.locator('#ft-status').evaluate(node=>({scrollable:node.scrollHeight>node.clientHeight,height:node.getBoundingClientRect().height,maxHeight:innerHeight*.2,selection:getComputedStyle(node).userSelect,tabIndex:node.tabIndex,role:node.getAttribute('role'),live:node.getAttribute('aria-live')}));
  assert.equal(results.longError.status.scrollable,true);assert.ok(results.longError.status.height<=results.longError.status.maxHeight+1);
  assert.equal(results.longError.status.selection,'text');assert.equal(results.longError.status.tabIndex,0);assert.equal(results.longError.status.role,'status');assert.equal(results.longError.status.live,'polite');
  await page.keyboard.press('End');await page.waitForFunction(()=>{const node=document.querySelector('#ft-status');return node.scrollTop>0&&node.scrollTop+node.clientHeight>=node.scrollHeight-2;});
  assert.equal(await page.evaluate(()=>document.activeElement.id),'ft-status');await page.keyboard.press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'ft-cancel');
  phase='JWT warning and retry';
  await page.setViewportSize({width:1024,height:768});await page.locator('#btn-theme').evaluate(button=>button.click());
  await page.locator('#ft-sample').fill('eyJhbGciOiJub25lIn0.eyJzdWIiOiJsb2NhbCJ9.');await page.locator('#ft-preview').click();
  await page.waitForFunction(()=>document.querySelector('#ft-result-type').textContent==='Objeto');assert.match(await page.locator('#ft-output').textContent(),/local/);
  results.lightPreview=await readablePreview();
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await page.screenshot({path:resolve(output,'field-transform-jwt-light-1024.png')});
  await page.locator('#ft-cancel').click();assert.equal(await page.evaluate(()=>document.activeElement.id),'btn-colpicker');
  phase='Case ownership';await page.evaluate(()=>newCase('Outro Caso',{keepArtifact:true}));
  await page.waitForFunction(id=>activeCase()?.id!==id&&!WorkspaceContext.changing&&!state.analysisDefinitionsPending,originalCase);
  assert.equal(await page.evaluate(()=>state.derivedFields.some(field=>field.name==='decoded_payload')),false);
  await page.evaluate(id=>WorkspaceContext.changeCase(id),originalCase);
  await page.waitForFunction(()=>state.derivedFields.some(field=>field.name==='decoded_payload'));
  phase='persistent typed discovery';await page.evaluate(()=>saveCases());await page.reload();
  await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&!state.loadOverlay);
  await page.getByRole('button',{name:'Explorar',exact:true}).click();
  await page.waitForFunction(()=>state.columns.includes('decoded_payload.allowed'));
  assert.ok(await page.evaluate(()=>state.derivedFields.some(field=>field.name==='decoded_payload'&&field.steps.join(',')==='base64_decode,parse_json')));
  phase='rare derived child outside sampled fields';await settled();
  const rareField='decoded_payload.rare.flag',rare=await page.evaluate(()=>window.__mockRareDerivedFixture);
  assert.ok(rare?.eventRef);assert.equal(await page.evaluate(field=>state.columns.includes(field),rareField),false,'the rare child is outside the 3000-row field sample');
  await page.evaluate(()=>{
    window.__rareFieldCalls=[];window.__rareFieldApi=api;
    api=async(command,args,options)=>{const response=await window.__rareFieldApi(command,args,options);if(['grouped_timeline','aggregate_events','compute_series','pivot'].includes(command))window.__rareFieldCalls.push({command,args:structuredClone(args),response:structuredClone(response)});return response;};
  });
  const timelineField=page.getByRole('combobox',{name:'Agrupar Timeline por campo',exact:true});
  const beforePath=await page.evaluate(()=>window.__mockCommandCalls.grouped_timeline||0);
  await timelineField.selectOption({label:'Usar caminho exato…'});await page.locator('#name-pop').waitFor({state:'visible'});
  await page.locator('#np-val').fill(rareField);
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.grouped_timeline||0),beforePath,'typing the exact path does not query');
  await page.locator('#np-ok').click();
  await page.waitForFunction(field=>window.__rareFieldCalls.some(call=>call.command==='grouped_timeline'&&call.args.field===field),rareField);
  const rareTimeline=await page.evaluate(field=>window.__rareFieldCalls.findLast(call=>call.command==='grouped_timeline'&&call.args.field===field),rareField);
  assert.equal(rareTimeline.response.total.count,6000);assert.equal(rareTimeline.response.series.find(series=>series.key==='true')?.count,1);
  assert.equal(rareTimeline.response.missing.count,5999);assert.equal(await page.evaluate(field=>state.columns.includes(field),rareField),false,'explicit choices do not masquerade as sampled discovery');
  await page.locator('#btn-add-filter').click();await page.locator('#fp-col').selectOption(rareField);await page.locator('#fp-op').selectOption('equals_exact');
  await page.locator('#fp-val').fill('true');await page.locator('#fp-apply').click();
  await page.waitForFunction(id=>state.total===1&&state.rows.length===1&&state.rows[0].id===id&&document.querySelector('#events-table').getAttribute('aria-busy')==='false',rare.id);
  await page.locator('#events-table tbody tr').first().locator('td[data-column="message"]').click();
  await page.waitForFunction(ref=>state.currentDetailEv?.event_ref===ref,rare.eventRef);
  await page.locator('#pane-overview .detail-tree-row[data-col="decoded_payload.rare.latency"] .detail-tree-name').click({button:'right'});
  assert.equal(await page.evaluate(()=>AnalysisFields.names().includes('decoded_payload.rare.latency')),true,'an actual admitted detail child becomes a contextual choice');
  await page.evaluate(()=>{closeCtxMenu();closeDrawer();});
  await page.getByRole('button',{name:'Resumir',exact:true}).click();await page.getByRole('combobox',{name:'Campo para resumir',exact:true}).selectOption(rareField);
  await page.waitForFunction(()=>document.querySelector('#aw-group-summary').textContent.includes('1 registros')&&document.querySelector('#group-table .aw-group-key')?.textContent==='true');
  await page.getByRole('button',{name:'Descobrir',exact:true}).click();await page.getByRole('button',{name:'Meus gráficos',exact:true}).click();
  await page.locator('#btn-dash-add').click();await page.locator('#cp-title').fill('Filho raro');await page.locator('#cp-type').selectOption('bar');await page.locator('#cp-chart').selectOption('terms');
  await page.locator('#cp-field').selectOption(rareField);await page.locator('#cp-apply').click();
  const rareChart=page.locator('#dash-grid .dash-card').filter({has:page.locator('.dash-card-title').filter({hasText:'Filho raro'})});
  await rareChart.locator('.hbar-row').waitFor();assert.equal(await rareChart.locator('.hbar-label').textContent(),'true');assert.equal(await rareChart.locator('.hbar-val').textContent(),'1');
  await page.getByRole('button',{name:'Cruzar dados',exact:true}).click();
  const dimensions=await page.evaluate(()=>[...activeCube().rows]);for(const _field of dimensions)await page.locator('#cz-rows .cube-chip .x').first().click();
  await page.getByRole('combobox',{name:'Adicionar campo em linhas',exact:true}).selectOption(rareField);
  await page.waitForFunction(field=>activeCube().rows[0]===field&&cubeResultForTable(activeCube())&&document.querySelector('#cube-table .cube-leaf-row .cube-dimension')?.textContent==='true',rareField);
  assert.equal(await page.locator('#cube-table .cube-grand-total .cube-value').textContent(),'1');
  await page.setViewportSize({width:1440,height:960});await page.screenshot({path:resolve(output,'field-transform-rare-child-pivot-1440.png')});
  results.rareChild={field:rareField,event:rare,timeline:{total:rareTimeline.response.total.count,matched:1,missing:rareTimeline.response.missing.count},calls:await page.evaluate(()=>window.__rareFieldCalls.map(call=>({command:call.command,field:call.args.field||call.args.groupColumn||call.args.spec?.field||call.args.spec?.rows?.[0]})))};
  await page.evaluate(()=>newCase('Outro Caso sem filho raro',{keepArtifact:true}));
  await page.waitForFunction(id=>activeCase()?.id!==id&&!WorkspaceContext.changing&&!state.analysisDefinitionsPending,originalCase);
  assert.equal(await page.evaluate(field=>AnalysisFields.names().includes(field)||AnalysisFields.available(field),rareField),false,'a rare choice never leaks into another Case');
  await page.evaluate(id=>WorkspaceContext.changeCase(id),originalCase);await page.waitForFunction(()=>state.derivedFields.some(field=>field.name==='decoded_payload')&&!WorkspaceContext.changing);
  assert.equal(await page.evaluate(()=>state.groupCol),rareField,'the saved group choice retains its exact path on return');
  results.rareChild.restoreLimit='Dataset chart/cube layouts reset on source reload; authored-field editor preservation is verified by focused Node tests.';
  await page.evaluate(()=>{api=window.__rareFieldApi;delete window.__rareFieldApi;});
  assert.deepEqual(errors,[]);results.ok=true;writeFileSync(resolve(output,'field-transform.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results,null,2));
}catch(error){await captureFailure(page,'field-transform',error,{phase,errors,results});throw error;}
finally{await browser.close();}
