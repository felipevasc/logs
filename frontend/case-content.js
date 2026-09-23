/* Case narratives and image references. Image bytes are loaded only on demand. */
window.CaseContent = (() => {
  const cache = new Map(), pending = new Map(); let cacheBytes = 0, editor = null;
  const LIMIT = 16 * 1024 * 1024, CACHE_LIMIT = 48 * 1024 * 1024;
  const narrative = target => ({ summary: typeof target?.summary === 'string' ? target.summary : '', details: typeof target?.details === 'string' ? target.details : typeof target?.note === 'string' ? target.note : '' });
  const attachments = target => Array.isArray(target?.attachments) ? target.attachments.filter(image => image && /^[a-f0-9]{64}$/.test(image.id || '')) : [];
  async function imageData(id, { thumbnail = false } = {}) {
    if (!/^[a-f0-9]{64}$/.test(id || '')) throw Error('Referência de imagem inválida.');
    const key = `${id}:${thumbnail ? 'thumb' : 'full'}`;
    if (cache.has(key)) { const value = cache.get(key); cache.delete(key); cache.set(key, value); return value; }
    if (pending.has(key)) return pending.get(key);
    const request = (async () => {
      const result = await api('case_image_read', { id, thumbnail: !!thumbnail }, { silent: true });
      if (!/^data:image\/(png|jpeg|webp);base64,/.test(result.dataUrl || '')) throw Error('Formato de imagem inválido.');
      while (cache.size && cacheBytes + result.dataUrl.length > CACHE_LIMIT) { const oldest = cache.keys().next().value; cacheBytes -= cache.get(oldest).length; cache.delete(oldest); }
      if (result.dataUrl.length <= CACHE_LIMIT) { cache.set(key, result.dataUrl); cacheBytes += result.dataUrl.length; }
      return result.dataUrl;
    })();
    pending.set(key, request);
    try { return await request; } finally { pending.delete(key); }
  }
  const dimensions = (width, height) => {
    if (!(width > 0 && height > 0)) throw Error('Cabeçalho de imagem inválido.');
    if (width > 16000 || height > 16000 || width * height > 16e6) throw Error('A imagem excede 16 megapixels ou 16.000 pixels por lado. Reduza suas dimensões antes de anexar.');
    return { width, height };
  };
  function imageHeader(buffer, mime) {
    // Read dimensions before invoking a decoder. Specifications: W3C PNG IHDR,
    // ITU T.81 frame headers, and Google WebP RIFF VP8/VP8L/VP8X headers.
    const data = new Uint8Array(buffer), view = new DataView(buffer), size = data.length;
    const same = (offset, bytes) => bytes.every((byte, index) => data[offset + index] === byte);
    const fourcc = offset => String.fromCharCode(...data.subarray(offset, offset + 4));
    const invalid = () => { throw Error('Cabeçalho de imagem inválido ou formato diferente do informado.'); };
    if (mime === 'image/png') {
      if (size < 33 || !same(0,[137,80,78,71,13,10,26,10]) || view.getUint32(8) !== 13 || fourcc(12) !== 'IHDR') return invalid();
      return dimensions(view.getUint32(16), view.getUint32(20));
    }
    if (mime === 'image/jpeg') {
      if (size < 4 || !same(0,[255,216])) return invalid();
      let offset = 2;
      while (offset < size) {
        if (data[offset++] !== 255) return invalid();
        while (data[offset] === 255) offset++;
        const marker = data[offset++];
        if (marker === 217 || marker === 218 || marker == null || marker === 0) return invalid();
        if (marker === 1 || marker >= 208 && marker <= 215) continue;
        if (offset + 2 > size) return invalid();
        const length = view.getUint16(offset);
        if (length < 2 || offset + length > size) return invalid();
        if ([192,193,194,195,197,198,199,201,202,203,205,206,207].includes(marker)) {
          if (length < 8) return invalid();
          return dimensions(view.getUint16(offset + 5), view.getUint16(offset + 3));
        }
        offset += length;
      }
      return invalid();
    }
    if (mime === 'image/webp') {
      if (size < 20 || fourcc(0) !== 'RIFF' || fourcc(8) !== 'WEBP') return invalid();
      const end = view.getUint32(4, true) + 8; if (end > size || end < 20) return invalid();
      const uint24 = offset => data[offset] | data[offset + 1] << 8 | data[offset + 2] << 16;
      let result;
      for (let offset = 12; offset + 8 <= end;) {
        const kind = fourcc(offset), length = view.getUint32(offset + 4, true), start = offset + 8;
        if (start + length > end) return invalid();
        let found;
        if (kind === 'VP8X') { if (length < 10) return invalid(); found = dimensions(uint24(start + 4) + 1, uint24(start + 7) + 1); }
        else if (kind === 'VP8L') { if (length < 5 || data[start] !== 47) return invalid(); const bits = view.getUint32(start + 1, true); found = dimensions((bits & 16383) + 1, ((bits >>> 14) & 16383) + 1); }
        else if (kind === 'VP8 ') { if (length < 10 || !same(start + 3,[157,1,42])) return invalid(); found = dimensions(view.getUint16(start + 6,true) & 16383, view.getUint16(start + 8,true) & 16383); }
        else if (kind === 'ANMF') { if (length < 16) return invalid(); dimensions(uint24(start + 6) + 1, uint24(start + 9) + 1); }
        if (found && !result) result = found;
        offset = start + length + (length & 1);
      }
      return result || invalid();
    }
    return invalid();
  }
  async function decodeImage(file) {
    if (typeof createImageBitmap === 'function') return createImageBitmap(file);
    const data = await new Promise((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.onerror = () => reject(Error('Não foi possível ler a imagem.')); reader.readAsDataURL(file); });
    const image = new Image(); image.src = data;
    try { await image.decode(); } catch { image.src = ''; throw Error('Não foi possível decodificar a imagem.'); }
    return { width: image.naturalWidth, height: image.naturalHeight, image, close: () => { image.src = ''; } };
  }
  async function importImage(file) {
    if (!['image/png','image/jpeg','image/webp'].includes(file.type)) throw Error('Escolha uma imagem PNG, JPEG ou WebP.');
    if (file.size > LIMIT) throw Error('A imagem excede 16 MB.');
    imageHeader(await file.arrayBuffer(), file.type);
    const bitmap = await decodeImage(file); let canvas;
    try {
      dimensions(bitmap.width, bitmap.height);
      const scale = Math.min(1, 2560 / Math.max(bitmap.width, bitmap.height), Math.sqrt(8e6 / (bitmap.width * bitmap.height)));
      canvas = document.createElement('canvas'); canvas.width = Math.max(1, Math.round(bitmap.width * scale)); canvas.height = Math.max(1, Math.round(bitmap.height * scale));
      const context = canvas.getContext('2d'); if (!context) throw Error('Não foi possível preparar a imagem.');
      context.drawImage(bitmap.image || bitmap, 0, 0, canvas.width, canvas.height);
      const mime = file.type === 'image/jpeg' ? 'image/jpeg' : 'image/png';
      const data = canvas.toDataURL(mime, .9); canvas.width = canvas.height = 1;
      if (data.length > LIMIT * 4 / 3) throw Error('A imagem processada excede 16 MB.');
      return await api('case_image_add', { base64: data.slice(data.indexOf(',') + 1), name: (file.name || 'Print.png').slice(0,200) }, { silent: true });
    } finally { bitmap.close(); if (canvas) canvas.width = canvas.height = 1; }
  }
  function previewImage(image, origin) {
    const overlay = el('div', 'modal-overlay case-image-preview');
    const panel = el('section','case-image-viewer'); panel.setAttribute('role','dialog'); panel.setAttribute('aria-modal','true'); panel.setAttribute('aria-label',image.caption || image.name || 'Imagem do Caso');
    const close = el('button','icon-btn'); close.innerHTML='<i class="fas fa-xmark"></i>'; close.setAttribute('aria-label','Fechar imagem');
    const img = el('img'); img.alt = image.caption || image.name || 'Imagem do Caso'; const caption=el('p','',image.caption || image.name || '');
    panel.append(close,img,caption);overlay.append(panel);document.body.append(overlay);
    const end=()=>{overlay.remove();origin?.focus();};close.onclick=end;overlay.onclick=e=>{if(e.target===overlay)end();};overlay.onkeydown=e=>{if(e.key==='Escape'){e.preventDefault();e.stopPropagation();end();}if(e.key==='Tab'){e.preventDefault();close.focus();}};
    imageData(image.id).then(data=>{if(overlay.isConnected)img.src=data;}).catch(error=>{caption.textContent=`Não foi possível abrir: ${error.message}`;});close.focus();
  }
  function mountAttachments(container,target,{editable=false,onChange=()=>{}}={}) {
    container.classList.add('case-attachments'); target.attachments = attachments(target);
    let busy=false, generation=0;
    const status=el('span','muted small');status.setAttribute('role','status');
    async function add(files) {
      if(busy)return;busy=true;status.textContent='Adicionando imagem…';
      try {
        for(const file of files){
          if(target.attachments.length>=30)throw Error('Cada item ou trilha aceita até 30 imagens.');
          const image=await importImage(file);
          if(!target.attachments.some(saved=>saved.id===image.id)){target.attachments.push({...image,caption:''});onChange();}
        }
        status.textContent='';draw();
      } catch(error){status.textContent=String(error.message||error);draw();}finally{busy=false;}
    }
    function draw(){
      const request=++generation;container.replaceChildren();
      if(editable){
        const tools=el('div','case-attachment-tools'),addButton=el('button','btn ghost small');addButton.type='button';addButton.innerHTML='<i class="fas fa-image"></i> Adicionar imagem';addButton.title='PNG, JPEG ou WebP. Você também pode colar um print com Ctrl+V.';
        const input=el('input');input.type='file';input.multiple=true;input.accept='image/png,image/jpeg,image/webp';input.hidden=true;
        addButton.onclick=()=>input.click();input.onchange=()=>add([...input.files]);tools.append(addButton,input,status);container.append(tools);
      }
      const grid=el('div','case-image-grid');container.append(grid);
      for(const image of target.attachments){
        const card=el('figure','case-image-card'),view=el('button','case-image-thumb');view.type='button';view.setAttribute('aria-label',`Abrir imagem: ${image.caption||image.name||'Print'}`);
        const img=el('img');img.loading='lazy';img.alt=image.caption||image.name||'Print';view.append(img);view.onclick=()=>previewImage(image,view);card.append(view);
        imageData(image.id,{thumbnail:true}).then(data=>{if(container.isConnected&&request===generation)img.src=data;}).catch(()=>{if(request===generation)view.replaceChildren(el('span','muted small','Imagem indisponível'));});
        if(editable){const caption=el('input');caption.type='text';caption.value=image.caption||'';caption.maxLength=500;caption.placeholder='Legenda opcional';caption.setAttribute('aria-label',`Legenda de ${image.name||'imagem'}`);caption.oninput=()=>{image.caption=caption.value;onChange();};const remove=el('button','icon-btn case-image-remove');remove.type='button';remove.innerHTML='<i class="fas fa-xmark"></i>';remove.setAttribute('aria-label',`Remover imagem: ${image.name||'Print'}`);remove.onclick=()=>{target.attachments=target.attachments.filter(item=>item!==image);onChange();draw();};card.append(caption,remove);}
        else if(image.caption)card.append(el('figcaption','',image.caption));grid.append(card);
      }
    }
    container.addEventListener('paste',event=>{if(!editable)return;const files=[...(event.clipboardData?.items||[])].filter(item=>item.kind==='file'&&item.type.startsWith('image/')).map(item=>item.getAsFile()).filter(Boolean);if(files.length){event.preventDefault();add(files);}});
    draw();return {add, get busy(){return busy;}};
  }
  function edit(target,{title='Editar explicação',nameField,onSave}={}) {
    if(editor)return Promise.resolve(false);
    const caseId=activeCase()?.id, draft={...narrative(target),attachments:structuredClone(attachments(target))};
    const overlay=el('div','modal-overlay case-content-editor');editor=overlay;
    overlay.innerHTML='<section class="modal case-content-dialog" role="dialog" aria-modal="true" aria-labelledby="case-content-title"><div class="modal-head"><h3 id="case-content-title"></h3><button type="button" class="icon-btn" data-close aria-label="Fechar edição"><i class="fas fa-xmark"></i></button></div><form class="modal-body"><div data-name></div><label class="fld">Resumo<textarea name="summary" rows="2" maxlength="4000" placeholder="Explicação principal (opcional)"></textarea></label><label class="fld">Detalhes<textarea name="details" rows="5" maxlength="50000" placeholder="Contexto e observações (opcional)"></textarea></label><div data-images></div><p class="case-content-status" role="status"></p><div class="modal-actions"><button type="button" class="btn ghost" data-cancel>Cancelar</button><button type="submit" class="btn primary">Salvar</button></div></form></section>';
    overlay.querySelector('h3').textContent=title;const form=overlay.querySelector('form');form.elements.summary.value=draft.summary;form.elements.details.value=draft.details;
    let name;if(nameField){const label=el('label','fld','Título');name=el('input');name.type='text';name.required=true;name.maxLength=200;name.value=target[nameField]||'';label.append(name);overlay.querySelector('[data-name]').append(label);}
    const gallery=mountAttachments(overlay.querySelector('[data-images]'),draft,{editable:true});
    const origin=document.activeElement;document.body.append(overlay);(name||form.elements.summary).focus();
    return new Promise(resolve=>{
      let saving=false;
      const close=value=>{if(saving)return;overlay.remove();editor=null;origin?.focus();resolve(value);};
      for(const button of overlay.querySelectorAll('[data-close],[data-cancel]'))button.onclick=()=>close(false);
      overlay.onclick=e=>{if(e.target===overlay)close(false);};
      overlay.onpaste=event=>{const files=[...(event.clipboardData?.items||[])].filter(item=>item.kind==='file'&&item.type.startsWith('image/')).map(item=>item.getAsFile()).filter(Boolean);if(files.length&&!event.defaultPrevented){event.preventDefault();gallery.add(files);}};
      overlay.onkeydown=event=>{if(event.key==='Escape'){event.preventDefault();event.stopPropagation();close(false);}};
      form.onsubmit=async event=>{
        event.preventDefault();if(saving)return;const status=overlay.querySelector('.case-content-status');
        if(gallery.busy){status.textContent='Aguarde o envio da imagem.';return;}
        if(activeCase()?.id!==caseId){status.textContent='O Caso ativo mudou. Feche a edição e abra o item novamente.';return;}
        const previous={summary:target.summary,details:target.details,attachments:target.attachments,...(nameField?{[nameField]:target[nameField]}:{})};
        Object.assign(target,{summary:form.elements.summary.value,details:form.elements.details.value,attachments:draft.attachments,...(nameField?{[nameField]:name.value.trim()}:{})});
        saving=true;form.querySelector('[type="submit"]').disabled=true;
        try {const saved=await(onSave?onSave():saveCases());if(saved===false)throw Error('Não foi possível salvar as alterações.');saving=false;close(true);}
        catch(error){Object.assign(target,previous);status.textContent=String(error.message||error);saving=false;form.querySelector('[type="submit"]').disabled=false;}
      };
    });
  }
  async function editItem(itemId){const item=activeCase()?.items?.find(item=>item.id===itemId);if(!item)return false;const changed=await edit(item,{title:'Explicar item',onSave:saveCases});if(changed){window.CaseTrails?.refresh?.();if(Workspace.page()==='evidence')await Workspace.showPage('evidence');else if(Workspace.page()==='case-timeline')renderAnalysis();}return changed;}
  return {narrative,attachments,imageData,importImage,mountAttachments,edit,editItem};
})();
