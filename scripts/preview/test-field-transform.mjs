/* Explicit local previews and Case-owned typed-field persistence, for browser CI. */
import assert from 'node:assert/strict';
import {launchBrowser} from './browser.mjs';
import {captureFailure} from './diagnostics.mjs';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const output=resolve('output/playwright');mkdirSync(output,{recursive:true});
const browser=await launchBrowser(),page=await browser.newPage({viewport:{width:1440,height:960},reducedMotion:'reduce'});
page.setDefaultTimeout(20000);const errors=[],results={};let phase='startup';page.on('pageerror',error=>errors.push(error.message));
const addStep=async value=>{await page.locator('#ft-step-choice').selectOption(value);await page.locator('#ft-add-step').click();};
const openHeader=async()=>{await page.locator('#events-table th').filter({hasText:'mock_payload_b64'}).click({button:'right'});await page.getByRole('button',{name:'Transformar campo',exact:true}).click();};
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
  phase='JWT warning and retry';await openHeader();await page.locator('#ft-sample').fill('eyJhbGciOiJub25lIn0.eyJzdWIiOiJsb2NhbCJ9.');await addStep('jwt_payload');
  assert.ok(await page.locator('#ft-jwt-warning').isVisible());await page.locator('#ft-preview').click();
  await page.waitForFunction(()=>document.querySelector('#ft-result-type').textContent==='Objeto');assert.match(await page.locator('#ft-jwt-warning').textContent(),/assinatura não foi verificada/);
  await page.locator('#ft-sample').fill('invalid-token');await page.locator('#ft-preview').click();await page.waitForFunction(()=>document.querySelector('#ft-status').classList.contains('ft-error'));
  assert.ok(await page.locator('#field-transform-modal').isVisible());assert.equal(await page.locator('#ft-sample').inputValue(),'invalid-token');
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
  assert.deepEqual(errors,[]);results.ok=true;writeFileSync(resolve(output,'field-transform.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results,null,2));
}catch(error){await captureFailure(page,'field-transform',error,{phase,errors,results});throw error;}
finally{await browser.close();}
