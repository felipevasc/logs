// Native image persistence is stubbed; browser encoding, header guards, paste and editor flows are real.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {existsSync,mkdirSync,writeFileSync} from 'node:fs';
const url=process.argv[2]||'http://127.0.0.1:4189';
const browser=await chromium.launch({...(!existsSync(chromium.executablePath())?{executablePath:`${process.env.LOCALAPPDATA}/ms-playwright/chromium-1217/chrome-win64/chrome.exe`}:{})});
const page=await browser.newPage({viewport:{width:1024,height:900}}),errors=[],result={};page.on('pageerror',error=>errors.push(error.message));mkdirSync('output/playwright',{recursive:true});
try{
  await page.goto(url);await page.waitForFunction(()=>window.CaseContent&&WorkspaceContext?.ready&&!WorkspaceContext.changing&&state.loaded&&document.querySelector('#load-overlay').hidden);
  const image=await page.evaluate(async()=>{
    const canvas=document.createElement('canvas');canvas.width=160;canvas.height=90;const ctx=canvas.getContext('2d');ctx.fillStyle='#7850a5';ctx.fillRect(0,0,160,90);ctx.fillStyle='#fff';ctx.fillRect(15,20,100,10);
    window.imageFixtureData=canvas.toDataURL('image/png');const small=document.createElement('canvas');small.width=48;small.height=27;small.getContext('2d').drawImage(canvas,0,0,48,27);window.thumbFixtureData=small.toDataURL('image/png');
    window.imageReads=[];window.imageWrites=[];const original=api;
    api=async(command,args,options)=>{if(command==='case_image_read'){imageReads.push(args);await new Promise(resolve=>setTimeout(resolve,10));return{dataUrl:args.thumbnail?thumbFixtureData:imageFixtureData};}if(command==='case_image_add'){imageWrites.push(args);return{id:'c'.repeat(64),name:args.name,width:160,height:90,mime:'image/png',bytes:100};}return original(command,args,options);};
    window.imageFixtureTarget={summary:'Original',details:'Antes',attachments:[]};window.contentSaves=0;
    return imageFixtureData.split(',')[1];
  });
  result.header=await page.evaluate(async()=>{
    const original=createImageBitmap;let decodes=0;window.createImageBitmap=async(...args)=>{decodes++;return original(...args);};
    const rejected=[];
    try{
      const png=new Uint8Array(33);png.set([137,80,78,71,13,10,26,10]);const p=new DataView(png.buffer);p.setUint32(8,13);png.set([73,72,68,82],12);p.setUint32(16,8000);p.setUint32(20,8000);
      const jpeg=new Uint8Array([255,216,255,192,0,8,8,31,64,31,64,3,255,217]);
      const webp=new Uint8Array(30);webp.set([...new TextEncoder().encode('RIFF')]);const w=new DataView(webp.buffer);w.setUint32(4,22,true);webp.set(new TextEncoder().encode('WEBPVP8X'),8);w.setUint32(16,10,true);webp[24]=63;webp[25]=31;webp[27]=63;webp[28]=31;
      for(const [type,bytes] of [['image/png',png],['image/jpeg',jpeg],['image/webp',webp]]){try{await CaseContent.importImage(new File([bytes],'Oversized',{type}));rejected.push(false);}catch(error){rejected.push(error.message.includes('16 megapixels'));}}
      try{await CaseContent.importImage(new File(['not an image'],'Bad',{type:'image/png'}));rejected.push(false);}catch(error){rejected.push(error.message.includes('Cabeçalho'));}
    }finally{window.createImageBitmap=original;}
    return{decodes,rejected};
  });
  assert.equal(result.header.decodes,0);assert.deepEqual(result.header.rejected,[true,true,true,true]);
  result.formats=await page.evaluate(async()=>{
    const canvas=document.createElement('canvas');canvas.width=100;canvas.height=60;canvas.getContext('2d').fillRect(0,0,100,60);const formats=[];
    for(const mime of ['image/png','image/jpeg','image/webp']){const blob=await new Promise(resolve=>canvas.toBlob(resolve,mime));formats.push((await CaseContent.importImage(new File([blob],'Test',{type:mime}))).id.length===64);}
    const original=window.createImageBitmap;window.createImageBitmap=undefined;try{const blob=await new Promise(resolve=>canvas.toBlob(resolve,'image/png'));formats.push((await CaseContent.importImage(new File([blob],'Fallback.png',{type:'image/png'}))).id.length===64);}finally{window.createImageBitmap=original;}return formats;
  });assert.deepEqual(result.formats,[true,true,true,true]);
  result.cache=await page.evaluate(async()=>{
    imageReads.length=0;const id='a'.repeat(64);const thumbnails=await Promise.all([CaseContent.imageData(id,{thumbnail:true}),CaseContent.imageData(id,{thumbnail:true})]);const full=await CaseContent.imageData(id);await CaseContent.imageData(id,{thumbnail:true});
    return{reads:imageReads.map(read=>read.thumbnail),same:thumbnails[0]===thumbnails[1],distinct:thumbnails[0]!==full};
  });assert.deepEqual(result.cache,{reads:[true,false],same:true,distinct:true});
  await page.evaluate(()=>{window.editorPromise=CaseContent.edit(imageFixtureTarget,{onSave:async()=>{contentSaves++;return true;}});});
  await page.getByLabel('Resumo',{exact:true}).fill('Rascunho');await page.locator('.case-content-dialog input[type=file]').setInputFiles({name:'Print.png',mimeType:'image/png',buffer:Buffer.from(image,'base64')});
  await page.waitForFunction(()=>document.querySelector('.case-content-dialog .case-image-card img')?.naturalWidth===48);
  await page.getByRole('button',{name:'Cancelar',exact:true}).click();assert.equal(await page.evaluate(()=>editorPromise),false);assert.deepEqual(await page.evaluate(()=>imageFixtureTarget),{summary:'Original',details:'Antes',attachments:[]});
  await page.evaluate(()=>{window.editorPromise=CaseContent.edit(imageFixtureTarget,{onSave:async()=>{contentSaves++;return true;}});});
  await page.getByLabel('Resumo',{exact:true}).fill('Resumo do incidente');await page.getByLabel('Detalhes',{exact:true}).fill('Detalhes com evidências e contexto.');
  await page.evaluate(async()=>{const blob=await(await fetch(imageFixtureData)).blob(),transfer=new DataTransfer();transfer.items.add(new File([blob],'Colado.png',{type:'image/png'}));document.querySelector('textarea[name=details]').dispatchEvent(new ClipboardEvent('paste',{bubbles:true,cancelable:true,clipboardData:transfer}));});
  await page.waitForFunction(()=>document.querySelector('.case-content-dialog .case-image-card img')?.naturalWidth===48);await page.getByLabel('Legenda de Colado.png',{exact:true}).fill('Print do incidente');
  await page.screenshot({path:'output/playwright/case-content-editor-dark-1024.png'});await page.evaluate(()=>document.documentElement.dataset.theme='light');await page.screenshot({path:'output/playwright/case-content-editor-light-1024.png'});
  await page.getByRole('button',{name:'Salvar',exact:true}).click();assert.equal(await page.evaluate(()=>editorPromise),true);
  const saved=await page.evaluate(()=>({target:imageFixtureTarget,saves:contentSaves}));assert.equal(saved.target.summary,'Resumo do incidente');assert.equal(saved.target.details,'Detalhes com evidências e contexto.');assert.equal(saved.target.attachments[0].caption,'Print do incidente');assert.equal(saved.saves,1);
  result.editor={attach:true,paste:true,cancel:true,save:true,thumbnail:true};assert.deepEqual(errors,[]);writeFileSync('output/playwright/case-content-validation.json',JSON.stringify({...result,errors},null,2));console.log(JSON.stringify({...result,errors},null,2));
}finally{await browser.close();}
