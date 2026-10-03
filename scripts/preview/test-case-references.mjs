/* Case-owned JSONL import and exact lookup fields, using the preview transport. */
import assert from 'node:assert/strict';
import {launchBrowser} from './browser.mjs';
import {captureFailure} from './diagnostics.mjs';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const output=resolve('output/playwright');mkdirSync(output,{recursive:true});
const browser=await launchBrowser(),page=await browser.newPage({viewport:{width:1440,height:960},reducedMotion:'reduce'});
page.setDefaultTimeout(20000);let phase='startup';const errors=[],results={};page.on('pageerror',error=>errors.push(error.message));
const settled=async()=>{await page.waitForFunction(()=>explorerAnalytics.get(explorerKey())?.status==='done');await page.evaluate(()=>settleFilterTabCounts());await page.waitForFunction(()=>Tasks.pending()===0);};
const manager=async()=>{await page.locator('#btn-case-menu').click();await page.getByRole('menuitem',{name:'Referências deste Caso',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('#rf-reload').disabled&&document.querySelector('#rf-status').textContent==='');};
// Real browser geometry, run by CI. These checks cover the dialog, not full-app 320px reflow or native zoom.
const dialogLayout=async()=>{
  const layout=await page.locator('.case-references-modal').evaluate(modal=>{
    const body=modal.querySelector('.modal-body'),footer=modal.querySelector('.rf-footer'),bounds=modal.getBoundingClientRect();
    body.scrollTop=0;const before=footer.getBoundingClientRect().top;body.scrollTop=body.scrollHeight;
    const after=footer.getBoundingClientRect(),controls=[...footer.querySelectorAll('button')].filter(button=>!button.hidden);
    return{width:innerWidth,inViewport:bounds.left>=0&&bounds.right<=innerWidth&&bounds.top>=0&&bounds.bottom<=innerHeight,
      bodyWidth:body.clientWidth,bodyScrollWidth:body.scrollWidth,bodyBottom:body.getBoundingClientRect().bottom,footerTop:after.top,footerShift:after.top-before,
      statusOutsideBody:!body.contains(document.querySelector('#rf-status')),actionsOutsideBody:controls.every(button=>!body.contains(button)),
      controls:controls.map(button=>{const rect=button.getBoundingClientRect();return{id:button.id,height:rect.height,fontSize:parseFloat(getComputedStyle(button).fontSize),reachable:rect.left>=0&&rect.right<=innerWidth&&rect.top>=0&&rect.bottom<=innerHeight&&button.contains(document.elementFromPoint(rect.x+rect.width/2,rect.y+rect.height/2))};}),
      lastEnabled:controls.filter(button=>!button.disabled).at(-1)?.id};
  });
  assert.equal(layout.inViewport,true);assert.ok(layout.bodyScrollWidth<=layout.bodyWidth+1,`reference form overflows at ${layout.width}px`);
  assert.ok(layout.bodyBottom<=layout.footerTop+1);assert.equal(layout.footerShift,0);assert.equal(layout.statusOutsideBody,true);assert.equal(layout.actionsOutsideBody,true);
  for(const control of layout.controls){assert.equal(control.reachable,true,`${control.id} remains reachable at ${layout.width}px`);assert.ok(control.height>=30);assert.ok(control.fontSize>=12);}
  await page.locator(`#${layout.lastEnabled}`).focus();await page.keyboard.press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'rf-close');
  await page.keyboard.press('Shift+Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),layout.lastEnabled);
  return layout;
};
try{
  await page.goto(process.argv[2]||'http://127.0.0.1:4174');
  await page.waitForFunction(()=>window.WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&!state.loadOverlay&&document.querySelector('#load-overlay').hidden);
  await page.getByRole('button',{name:'Explorar',exact:true}).click();await settled();
  const caseId=await page.evaluate(()=>activeCase().id),baseline=await page.evaluate(()=>state.total);assert.equal(baseline,6000);
  phase='inspect and ordered import';await manager();assert.match(await page.locator('#rf-list').textContent(),/Nenhuma referência/);
  await page.locator('#rf-choose').click();await page.waitForFunction(()=>!document.querySelector('#rf-import-pane').hidden&&!document.querySelector('#rf-key-choice').disabled);
  assert.match(await page.locator('#rf-inspection').textContent(),/5 registros/);
  await page.locator('#rf-import-name').fill('Equipes por serviço');
  for(const key of ['service','environment']){await page.locator('#rf-key-choice').selectOption(key);await page.locator('#rf-add-key').click();}
  await page.setViewportSize({width:320,height:640});results.importCompact=await dialogLayout();
  assert.equal(await page.locator('#rf-save-lookup').isVisible(),false);assert.equal(await page.locator('#rf-delete-lookup').isVisible(),false);
  assert.match(await page.locator('#rf-import-pane').textContent(),/Chaves repetidas são rejeitadas; nenhum registro é escolhido arbitrariamente/);
  await page.setViewportSize({width:1440,height:960});
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.reference_inspect),1);
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.reference_import||0),0,'form edits never import automatically');
  await page.locator('#rf-import').click();await page.waitForFunction(()=>document.querySelector('#rf-status').textContent.includes('Referência importada')&&!document.querySelector('#rf-reload').disabled);
  results.reference=await page.evaluate(()=>structuredClone(AnalysisContexts.context().config.references[0]));
  assert.deepEqual(results.reference.keyColumns,['service','environment']);assert.match(results.reference.contentSha256,/^[a-f0-9]{64}$/);
  phase='explicit lookup mapping';await page.getByRole('button',{name:'Criar campo por referência',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('#rf-field-name').disabled&&document.querySelectorAll('#rf-mappings select').length===2);
  await page.locator('#rf-field-name').fill('reference_team');
  await page.getByLabel('Campo do log para service',{exact:true}).selectOption('source');
  await page.getByLabel('Campo do log para environment',{exact:true}).selectOption('ambiente');
  await page.locator('#rf-value-column').selectOption('team');
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.reference_save_lookup||0),0,'mapping edits never run or save a lookup');
  await page.screenshot({path:resolve(output,'case-reference-lookup-dark-1440.png')});
  await page.locator('#rf-value-column').selectOption('');
  await page.locator('#rf-save-lookup').click();assert.match(await page.locator('#rf-status').textContent(),/Mapeie todas as chaves/);
  results.compactDialogs=[];
  for(const width of [1024,640,320]){
    await page.setViewportSize({width,height:640});results.compactDialogs.push(await dialogLayout());
    assert.equal(await page.locator('#rf-field-name').inputValue(),'reference_team');
    assert.equal(await page.getByLabel('Campo do log para service',{exact:true}).inputValue(),'source');
    assert.equal(await page.getByLabel('Campo do log para environment',{exact:true}).inputValue(),'ambiente');
    await page.screenshot({path:resolve(output,`case-reference-actions-error-dark-${width}.png`)});
  }
  assert.equal(await page.locator('#rf-lookup-pane p').filter({hasText:'Os tipos são preservados'}).count(),1);
  assert.equal(await page.locator('#rf-lookup-pane p').filter({hasText:'diferente de uma chave sem correspondência'}).count(),1);
  await page.locator('#rf-value-column').selectOption('team');
  phase='long lookup error keyboard scrolling';
  await page.evaluate(()=>{window.__referenceLongErrorApi=api;api=async(command,args,options)=>{if(command==='reference_save_lookup')return new Promise((resolve,reject)=>{window.__referenceLongErrorReject=()=>reject(Error('Falha de gravação recuperável. '.repeat(100)));});return window.__referenceLongErrorApi(command,args,options);};});
  await page.locator('#rf-save-lookup').click();await page.waitForFunction(()=>typeof window.__referenceLongErrorReject==='function');
  await page.locator('#rf-cancel').focus();await page.evaluate(()=>window.__referenceLongErrorReject());
  await page.waitForFunction(()=>document.querySelector('#rf-status').textContent.includes('Falha de gravação recuperável'));
  await page.evaluate(()=>{api=window.__referenceLongErrorApi;delete window.__referenceLongErrorApi;delete window.__referenceLongErrorReject;});
  assert.equal(await page.evaluate(()=>document.activeElement.id),'rf-cancel','incoming error does not steal focus');
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.reference_save_lookup||0),0,'the recoverable failure has not saved the draft');
  results.longError=await dialogLayout();
  await page.locator('#rf-value-column').focus();await page.keyboard.press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'rf-status');
  results.longError.status=await page.locator('#rf-status').evaluate(node=>({scrollable:node.scrollHeight>node.clientHeight,height:node.getBoundingClientRect().height,maxHeight:innerHeight*.2,selection:getComputedStyle(node).userSelect,tabIndex:node.tabIndex,role:node.getAttribute('role'),live:node.getAttribute('aria-live')}));
  assert.equal(results.longError.status.scrollable,true);assert.ok(results.longError.status.height<=results.longError.status.maxHeight+1);
  assert.equal(results.longError.status.selection,'text');assert.equal(results.longError.status.tabIndex,0);assert.equal(results.longError.status.role,'status');assert.equal(results.longError.status.live,'polite');
  await page.keyboard.press('End');await page.waitForFunction(()=>{const node=document.querySelector('#rf-status');return node.scrollTop>0&&node.scrollTop+node.clientHeight>=node.scrollHeight-2;});
  assert.equal(await page.evaluate(()=>document.activeElement.id),'rf-status');await page.keyboard.press('Tab');assert.equal(await page.evaluate(()=>document.activeElement.id),'rf-cancel');
  await page.setViewportSize({width:1440,height:960});phase='save lookup after recoverable error';
  await page.locator('#rf-save-lookup').click();await page.waitForFunction(()=>document.querySelector('#case-references-modal').hidden);await settled();
  assert.equal(await page.evaluate(()=>state.total),baseline,'lookup does not multiply rows');
  results.lookup=await page.evaluate(()=>{const row=state.rows.find(row=>Object.hasOwn(row.fields||{},'reference_team'));return{source:row?.source,value:row?.fields.reference_team,definition:state.derivedFields.find(field=>field.name==='reference_team')};});
  assert.equal(results.lookup.value,`Equipe ${results.lookup.source}`);assert.equal(results.lookup.definition.lookup.referenceId,results.reference.id);
  await page.locator('#btn-colpicker').click();assert.equal(await page.locator('#col-list').getByText('reference_team',{exact:true}).count(),1);await page.locator('#btn-colpicker').click();
  // Exercise normal controls; these assertions prove preview UI integration, not native lookup semantics.
  phase='reference field filter and pivot';
  const analysisBefore=await page.evaluate(()=>({owner:AnalysisContexts.capture(),filters:structuredClone(state.filters),quick:state.quick,tab:state.activeDatasetTab,total:state.total}));
  assert.deepEqual(analysisBefore.filters,[]);assert.equal(analysisBefore.quick,'');
  const applyFieldFilter=async(column,op,value='')=>{
    await page.locator('#btn-add-filter').click();await page.getByRole('combobox',{name:'Campo do filtro',exact:true}).selectOption(column);
    await page.getByRole('combobox',{name:'Operador do filtro',exact:true}).selectOption(op);await page.locator('#fp-val').fill(value);await page.locator('#fp-apply').click();
  };
  // The eight encoded source records provide an independent, bounded set of raw IDs.
  await applyFieldFilter('mock_payload_b64','not_empty');
  await page.waitForFunction(()=>state.total===8&&document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&explorerAnalytics.get(explorerKey())?.status==='done');
  const sourceRows=await page.evaluate(()=>state.rows.map(row=>({id:row.id,source:row.source,environment:row.fields.ambiente})));
  assert.equal(sourceRows.length,8);assert.ok(sourceRows.every(row=>row.environment==='prod'));
  const sourceCounts=new Map();for(const row of sourceRows)sourceCounts.set(row.source,(sourceCounts.get(row.source)||0)+1);
  assert.ok(sourceCounts.size>1,'reference filter must select a proper subset of the source records');
  const selectedSource=[...sourceCounts].sort((a,b)=>b[1]-a[1]||a[0].localeCompare(b[0]))[0][0],referenceValue=`Equipe ${selectedSource}`;
  const expectedIds=sourceRows.filter(row=>row.source===selectedSource).map(row=>row.id).sort((a,b)=>a-b);
  const referenceFilters=[{column:'mock_payload_b64',op:'not_empty',value:'',value2:null},{column:'reference_team',op:'equals_exact',value:referenceValue,value2:null}];
  await page.evaluate(()=>{
    window.__referenceAnalysisCalls=[];window.__referenceAnalysisApi=api;
    api=async(command,args,options)=>{
      const observed=['query_page','pivot'].includes(command)&&args.filters?.some(filter=>filter.column==='reference_team');
      const request=observed?{command,args:structuredClone(args),owner:AnalysisContexts.capture()}:null;
      const response=await window.__referenceAnalysisApi(command,args,options);
      if(request)window.__referenceAnalysisCalls.push({...request,response:structuredClone(response),transportOwner:structuredClone(window.__mockRequests.findLast(call=>call.cmd===command)?.analysisContext)});
      return response;
    };
  });
  await applyFieldFilter('reference_team','equals_exact',referenceValue);
  await page.waitForFunction(total=>state.total===total&&document.querySelector('#events-table').getAttribute('aria-busy')==='false'&&explorerAnalytics.get(explorerKey())?.status==='done',expectedIds.length);
  const filteredReference=await page.evaluate(()=>({filters:state.filters,rows:state.rows.map(row=>({id:row.id,value:row.fields.reference_team})),request:window.__referenceAnalysisCalls.findLast(call=>call.command==='query_page')}));
  assert.deepEqual(filteredReference.filters,referenceFilters);assert.deepEqual(filteredReference.rows.map(row=>row.id).sort((a,b)=>a-b),expectedIds);
  assert.ok(filteredReference.rows.every(row=>row.value===referenceValue));assert.deepEqual(filteredReference.request.args.filters,referenceFilters);
  assert.deepEqual(filteredReference.request.response.rows.map(row=>row.id).sort((a,b)=>a-b),expectedIds);assert.equal(filteredReference.request.response.hasMore,false);
  assert.deepEqual(filteredReference.request.owner,analysisBefore.owner);assert.deepEqual(filteredReference.request.transportOwner,analysisBefore.owner.identity);
  await page.getByRole('button',{name:'Cruzar dados',exact:true}).click();
  await page.waitForFunction(()=>cubeResultForTable(activeCube())&&document.querySelector('#cube-table tbody .cube-leaf-row'));
  const cubeBefore=await page.evaluate(()=>({rows:[...activeCube().rows],cols:[...activeCube().cols],values:structuredClone(activeCube().values)}));
  assert.deepEqual(cubeBefore.cols,[]);assert.equal(cubeBefore.values.length,1);assert.equal(cubeBefore.values[0].func,'count');assert.equal(cubeBefore.values[0].column,'*');
  for(const field of cubeBefore.rows)await page.locator('#cz-rows .cube-chip .x').first().click();
  await page.getByRole('combobox',{name:'Adicionar campo em linhas',exact:true}).selectOption('reference_team');
  await page.waitForFunction(()=>activeCube().rows.length===1&&activeCube().rows[0]==='reference_team'&&cubeResultForTable(activeCube())&&document.querySelector('#cube-table .cube-dimension-head')?.textContent==='reference_team'&&Tasks.pending()===0);
  const pivotRequest=await page.evaluate(()=>window.__referenceAnalysisCalls.findLast(call=>call.command==='pivot'&&call.args.spec.rows.length===1&&call.args.spec.rows[0]==='reference_team'));
  assert.deepEqual(pivotRequest.args.filters,referenceFilters);assert.deepEqual(pivotRequest.args.spec,{rows:['reference_team'],cols:[],values:cubeBefore.values,limit_rows:2000});
  assert.deepEqual(pivotRequest.owner,analysisBefore.owner);assert.deepEqual(pivotRequest.transportOwner,analysisBefore.owner.identity);
  assert.deepEqual(pivotRequest.response,{value_names:[cubeBefore.values[0].alias||'count(*)'],col_keys:['(total)'],col_values:[[]],row_paths:[[referenceValue]],row_values:[[referenceValue]],cells:[[[expectedIds.length]]],totals:[[expectedIds.length]],truncated:false,complete:true,processed_events:expectedIds.length});
  assert.equal(await page.locator('#cube-table tbody .cube-leaf-row').count(),1);assert.equal(await page.locator('#cube-table .cube-leaf-row .cube-dimension').textContent(),referenceValue);
  assert.equal(await page.locator('#cube-table .cube-leaf-row .cube-value').textContent(),String(expectedIds.length));assert.equal(await page.locator('#cube-table .cube-grand-total .cube-value').textContent(),String(expectedIds.length));
  results.referenceAnalysis={evidence:'Real browser controls with preview transport; not native-engine verification',owner:analysisBefore.owner.identity,filters:referenceFilters,eventIds:expectedIds,pivot:pivotRequest.response};
  await page.screenshot({path:resolve(output,'case-reference-derived-pivot-1440.png')});
  await page.locator('#cz-rows .cube-chip .x').click();
  for(const field of cubeBefore.rows)await page.getByRole('combobox',{name:'Adicionar campo em linhas',exact:true}).selectOption(field);
  await page.locator(`#tabbtn-${analysisBefore.tab}`).click();await page.locator('#btn-clear-filters').click();
  await page.waitForFunction(total=>state.total===total&&document.querySelector('#events-table').getAttribute('aria-busy')==='false',analysisBefore.total);await settled();
  assert.deepEqual(await page.evaluate(()=>({filters:state.filters,quick:state.quick,tab:state.activeDatasetTab,owner:AnalysisContexts.capture(),rows:activeCube().rows})),
    {filters:analysisBefore.filters,quick:analysisBefore.quick,tab:analysisBefore.tab,owner:analysisBefore.owner,rows:cubeBefore.rows});
  await page.evaluate(()=>{api=window.__referenceAnalysisApi;delete window.__referenceAnalysisApi;delete window.__referenceAnalysisCalls;});
  phase='Case isolation';await page.evaluate(()=>newCase('Caso sem referências'));
  await page.waitForFunction(id=>activeCase()?.id!==id&&!WorkspaceContext.changing&&!state.analysisDefinitionsPending,caseId);await page.waitForFunction(()=>Tasks.pending()===0);assert.equal(await page.evaluate(()=>state.loaded),false);
  assert.equal(await page.evaluate(()=>state.derivedFields.some(field=>field.name==='reference_team')),false);
  await manager();assert.match(await page.locator('#rf-list').textContent(),/Nenhuma referência/);await page.locator('#rf-close').click();
  await page.evaluate(id=>WorkspaceContext.changeCase(id),caseId);await settled();assert.ok(await page.evaluate(()=>state.derivedFields.some(field=>field.name==='reference_team')));
  phase='dependency and unavailable content';await manager();
  assert.equal(await page.getByRole('button',{name:'Remover referência do Caso',exact:true}).isDisabled(),true);assert.match(await page.locator('#rf-list').textContent(),/Campos dependentes: reference_team/);
  await page.evaluate(id=>window.__mockReferences.setAvailable(AnalysisContexts.identity(),id,false),results.reference.id);await page.locator('#rf-reload').click();
  await page.waitForFunction(()=>document.querySelector('#rf-list').textContent.includes('Indisponível')&&!document.querySelector('#rf-reload').disabled);
  await page.setViewportSize({width:1024,height:768});await page.locator('#btn-theme').evaluate(button=>button.click());await page.getByText('Detalhes da referência',{exact:true}).click();
  const readable=await page.locator('.rf-details').evaluate(node=>{const s=getComputedStyle(node),lum=c=>{const v=c.match(/[\d.]+/g).slice(0,3).map(Number).map(n=>{n/=255;return n<=.04045?n/12.92:((n+.055)/1.055)**2.4;});return .2126*v[0]+.7152*v[1]+.0722*v[2];},a=lum(s.color),b=lum(s.backgroundColor);return{contrast:(Math.max(a,b)+.05)/(Math.min(a,b)+.05),selection:s.userSelect,overflow:s.overflowY};});
  assert.ok(readable.contrast>=4.5);assert.equal(readable.selection,'text');assert.equal(readable.overflow,'auto');results.readability=readable;
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await page.screenshot({path:resolve(output,'case-reference-unavailable-light-1024.png')});
  await page.locator('#rf-close').click();await page.locator('#explore-tree').getByTitle('Editar campo reference_team',{exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('#rf-availability').textContent.includes('indisponível')&&!document.querySelector('#rf-delete-lookup').disabled);
  assert.equal(await page.locator('#rf-field-name').inputValue(),'reference_team');assert.equal(await page.locator('#rf-save-lookup').isDisabled(),true);
  await page.setViewportSize({width:320,height:640});results.unavailableEditCompact=await dialogLayout();
  phase='remove dependent field then descriptor';await page.locator('#rf-delete-lookup').click();await page.waitForFunction(()=>document.querySelector('#case-references-modal').hidden);await settled();
  await page.setViewportSize({width:1024,height:768});
  assert.equal(await page.evaluate(()=>state.derivedFields.some(field=>field.name==='reference_team')),false);assert.equal(await page.evaluate(()=>AnalysisContexts.context().config.references.length),1);
  await manager();assert.equal(await page.getByRole('button',{name:'Remover referência do Caso',exact:true}).isDisabled(),false);
  await page.getByRole('button',{name:'Remover referência do Caso',exact:true}).click();await page.waitForFunction(()=>document.querySelector('#rf-status').textContent.includes('Referência removida')&&!document.querySelector('#rf-reload').disabled);
  assert.equal(await page.evaluate(()=>AnalysisContexts.context().config.references.length),0);await page.locator('#rf-close').click();await settled();assert.equal(await page.evaluate(()=>state.total),baseline);
  assert.deepEqual(errors,[]);results.errors=errors;results.ok=true;writeFileSync(resolve(output,'case-references.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results,null,2));
}catch(error){await captureFailure(page,'case-references',error,{phase,errors,results});throw error;}
finally{await browser.close();}
