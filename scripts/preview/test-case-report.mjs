import {chromium} from 'playwright';
import {existsSync,mkdirSync,writeFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const browser=await chromium.launch({headless:true,...(!existsSync(chromium.executablePath())?{executablePath:'C:/Users/felip/AppData/Local/ms-playwright/chromium-1217/chrome-win64/chrome.exe'}:{})});
const page=await browser.newPage({viewport:{width:1440,height:900}}),errors=[];page.on('pageerror',error=>errors.push(error.message));
mkdirSync('output/pdf',{recursive:true});
try{
 await page.goto(process.argv[2]||'http://127.0.0.1:4190');await page.waitForFunction(()=>WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&document.querySelector('#load-overlay').hidden);
 const result=await page.evaluate(async()=>{
  const canvas=document.createElement('canvas');canvas.width=1100;canvas.height=500;const ctx=canvas.getContext('2d');ctx.fillStyle='#f5f3fa';ctx.fillRect(0,0,1100,500);ctx.fillStyle='#6c4d9e';ctx.font='32px sans-serif';ctx.fillText('Evidência visual · revisão do acesso',40,62);ctx.font='21px monospace';ctx.fillStyle='#3d4556';['09:10:00   autenticação recusada','09:10:12   nova tentativa','09:10:14   sessão estabelecida'].forEach((line,i)=>ctx.fillText(line,40,140+i*60));
  const image=canvas.toDataURL('image/png'),original=api;api=(command,args,options)=>command==='case_image_read'?Promise.resolve({dataUrl:image}):original(command,args,options);
  const attachment={id:'a'.repeat(64),name:'registro-de-acesso.png',caption:'Comparação dos horários e da origem da sessão.',mime:'image/png',width:1100,height:500,bytes:4000};
  const fixture={id:'report-test',name:'Revisão de acesso · ambiente de demonstração',createdAt:Date.now(),items:Array.from({length:4},(_,i)=>({id:`item-${i}`,label:['Tentativas de acesso','Sessão estabelecida','Atividade posterior','Registro sem horário'][i],summary:`Observação ${i+1}: análise dos registros preservados.`,details:i===0?'Foi observado um conjunto de tentativas. A sequência deve ser lida em conjunto com a sessão subsequente.\n\nNão há conclusão automática de causa. '.repeat(16):'Contexto adicional com acentuação: ação, usuário, sessão, evidência.',attachments:i===0?[attachment]:[],rows:Array.from({length:i===3?1:7},(_,j)=>({id:j,event_ref:`origin:${i}:${j}`,timestamp:i===3?null:1700000000123+i*60000+j*1000,name:['Autenticação recusada','Sessão aceita','Consulta aos registros','Evento sem data'][i],message:`Registro ${j+1} · origem reconhecida · ação documentada.`,source:'Servidor de aplicação'}))})),caseTrails:[{id:'trail-a',title:'Do acesso à sessão',summary:'Sequência relevante para o Caso.',details:'Registros associados após revisão manual.',itemIds:['item-0','item-1'],attachments:[attachment]},{id:'trail-b',title:'Atividade posterior',summary:'Verificação complementar.',details:'O item compartilhado é referenciado pela página para evitar repetição de imagens.',itemIds:['item-1','item-2'],attachments:[]}]};
  const result=await CaseReport.render(fixture);const bytes=new Uint8Array(await result.blob.arrayBuffer());let binary='';for(let i=0;i<bytes.length;i+=16384)binary+=String.fromCharCode(...bytes.subarray(i,i+16384));
  const controller=new AbortController();controller.abort();let aborted=false;try{await CaseReport.render(fixture,{signal:controller.signal});}catch(error){aborted=error.name==='AbortError';}
  return {base64:btoa(binary),pages:result.pages,rows:result.rows,items:result.items,trails:result.trails,filename:result.filename,aborted};
 });
 assert.ok(result.pages>=5);assert.equal(result.items,4);assert.equal(result.trails,2);assert.ok(result.aborted);assert.match(result.filename,/^relatorio-.+\.pdf$/);assert.deepEqual(errors,[]);
 writeFileSync('output/pdf/caso-demonstracao.pdf',Buffer.from(result.base64,'base64'));delete result.base64;writeFileSync('output/pdf/report-results.json',JSON.stringify(result,null,2));console.log(JSON.stringify(result));
}finally{await browser.close();}
