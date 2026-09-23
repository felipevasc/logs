/* Browser preview: local image assets and portable investigations, stored in IndexedDB. */
const MAX_IMAGE=16*1024*1024, MAX_JSON=64*1024*1024;
let database;
const db=()=>database ||= new Promise((resolve,reject)=>{const open=indexedDB.open('loginsight-preview-assets',1);open.onupgradeneeded=()=>{open.result.createObjectStore('assets');open.result.createObjectStore('files');};open.onsuccess=()=>resolve(open.result);open.onerror=()=>reject(open.error);});
async function store(table,key,value){const database=await db();return new Promise((resolve,reject)=>{const transaction=database.transaction(table,value===undefined?'readonly':'readwrite'),bucket=transaction.objectStore(table),request=value===undefined?bucket.get(key):bucket.put(value,key);transaction.oncomplete=()=>resolve(request.result);transaction.onerror=()=>reject(transaction.error);});}
const encode=bytes=>{let raw='';for(let offset=0;offset<bytes.length;offset+=16384)raw+=String.fromCharCode(...bytes.subarray(offset,offset+16384));return btoa(raw);};
async function validate({base64,name}){
  if(typeof base64!=='string'||base64.length>Math.ceil(MAX_IMAGE*4/3)+4||!/^[A-Za-z0-9+/]+={0,2}$/.test(base64))throw Error('Imagem inválida ou maior que 16 MB.');
  const bytes=Uint8Array.from(atob(base64),character=>character.charCodeAt(0));
  const mime=bytes[0]===137&&bytes[1]===80?'image/png':bytes[0]===255&&bytes[1]===216?'image/jpeg':new TextDecoder().decode(bytes.subarray(8,12))==='WEBP'?'image/webp':null;
  if(!mime||bytes.length>MAX_IMAGE)throw Error('Escolha uma imagem PNG, JPEG ou WebP.');
  const bitmap=await createImageBitmap(new Blob([bytes],{type:mime}));const width=bitmap.width,height=bitmap.height;bitmap.close();
  if(!width||!height||width*height>16e6)throw Error('A imagem excede 16 milhões de pixels.');
  const digest=await crypto.subtle.digest('SHA-256',bytes),id=[...new Uint8Array(digest)].map(byte=>byte.toString(16).padStart(2,'0')).join('');
  return {metadata:{id,name:String(name||'Imagem').slice(0,200),mime,bytes:bytes.length,width,height},base64};
}
export async function addImage(args){const image=await validate(args);await store('assets',image.metadata.id,image);return image.metadata;}
export async function readImage({id,thumbnail=false}){
  const asset=await store('assets',id);if(!asset)throw Error('Imagem não encontrada nesta prévia.');
  let dataUrl=`data:${asset.metadata.mime};base64,${asset.base64}`;
  if(thumbnail&&Math.max(asset.metadata.width,asset.metadata.height)>480){const image=new Image();image.src=dataUrl;await image.decode();const scale=480/Math.max(image.width,image.height),canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(image.width*scale));canvas.height=Math.max(1,Math.round(image.height*scale));canvas.getContext('2d').drawImage(image,0,0,canvas.width,canvas.height);dataUrl=canvas.toDataURL('image/png');canvas.width=canvas.height=1;}
  return {dataUrl};
}
function references(data){const list=[];for(const c of data.cases||[])for(const target of [...(c.items||[]),...(c.caseTrails||[])])for(const image of target.attachments||[])list.push(image);if(list.length>1000)throw Error('Limite de 1.000 referências de imagens excedido.');return list;}
export async function exportInvestigation({path,data,mask=false}){
  if(mask)throw Error('A máscara de dados é validada no aplicativo instalado; desative-a para exportar nesta prévia. As imagens permanecem sem máscara.');
  const copy=structuredClone(data),assets=[];for(const id of new Set(references(copy).map(image=>image.id))){const asset=await store('assets',id);if(!asset)throw Error('Imagem não encontrada: '+id);assets.push({id,name:asset.metadata.name,base64:asset.base64});}
  copy.imageAssets=assets;const bytes=new TextEncoder().encode(JSON.stringify(copy));if(bytes.length>MAX_JSON)throw Error('O arquivo excede 64 MB.');await store('files',path,copy);window.__mockInvestigationExport={path,data:copy};
  if(window.__mockInvestigationDownload!==false){const filename=String(path||'caso.json').split(/[\\/]/).at(-1).replace(/\.[^.]*$/,'')+'.json';const response=await fetch('/__timeline-downloads__/',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({format:'json',filename,base64:encode(bytes)})});const result=await response.json();if(!response.ok)throw Error(result.error);const link=document.createElement('a');link.href=result.url;link.download=result.filename;document.body.append(link);link.click();link.remove();}
  return null;
}
export async function importInvestigation({path}){
  const data=structuredClone(window.__mockInvestigationImport||await store('files',path));if(!data)throw Error('Nenhum Caso exportado neste caminho na prévia.');
  if(new TextEncoder().encode(JSON.stringify(data)).length>MAX_JSON)throw Error('O arquivo excede 64 MB.');
  const referenced=references(data);for(const asset of data.imageAssets||[]){const image=await validate(asset);if(image.metadata.id!==asset.id)throw Error('A identidade da imagem não corresponde aos bytes.');await store('assets',asset.id,image);}
  for(const attachment of referenced){const asset=await store('assets',attachment.id);if(!asset)throw Error('Imagem referenciada ausente.');Object.assign(attachment,{...asset.metadata,name:attachment.name||asset.metadata.name});}
  delete data.imageAssets;return data;
}
