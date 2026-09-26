/* Local, paginated case reports. Raw evidence and image assets remain in the portable investigation. */
window.CaseReport = (() => {
  'use strict';
  const MAX_PAGES = 500, MAX_BYTES = 32 * 1024 * 1024, LEFT = 18, RIGHT = 192, BOTTOM = 277, WIDTH = RIGHT - LEFT;
  let libraries, dialog;
  const check = signal => { if (signal?.aborted) throw new DOMException('Geração cancelada', 'AbortError'); };
  const pause = () => new Promise(resolve => setTimeout(resolve, 0));
  const toDataURL = blob => new Promise((resolve,reject) => { const reader=new FileReader();reader.onload=()=>resolve(reader.result);reader.onerror=()=>reject(Error('Não foi possível ler a imagem.'));reader.readAsDataURL(blob); });
  const base64 = buffer => { const bytes=new Uint8Array(buffer);let text='';for(let i=0;i<bytes.length;i+=16384)text+=String.fromCharCode(...bytes.subarray(i,i+16384));return btoa(text); };
  async function prepare() {
    if (!libraries) libraries=(async()=>{
      if(!window.jspdf)await new Promise((resolve,reject)=>{const script=document.createElement('script');script.src='vendor/timeline-export/jspdf.umd.min.js';script.onload=resolve;script.onerror=()=>{script.remove();reject(Error('Não foi possível carregar o gerador de PDF.'));};document.head.append(script);});
      return Promise.all(['Regular','Bold'].map(async(style)=>{
        const response=await fetch(`vendor/noto-sans/NotoSans-${style}.ttf`);if(!response.ok)throw Error('Não foi possível carregar a fonte do relatório.');
        const buffer=await response.arrayBuffer();const face=new FontFace('Case Report Sans',buffer,{weight:style==='Bold'?'700':'400'});await face.load();document.fonts.add(face);return base64(buffer);
      }));
    })().catch(error=>{libraries=null;throw error;});
    return libraries;
  }
  const clean = value => String(value??'').replace(/\r\n?/g,'\n').replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g,'').replace(/\t/g,'    ');
  const stamp = value => value==null ? 'Sem horário' : new Date(value).toLocaleString('pt-BR',{year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit',fractionalSecondDigits:3});
  function filename(c,date=new Date()) { const slug=String(c?.name||'caso').normalize('NFD').replace(/[\u0300-\u036f]/g,'').toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-+|-+$/g,'').slice(0,100)||'caso';return `relatorio-${slug}-${date.toISOString().slice(0,10)}.pdf`; }
  async function render(c,{signal,progress=()=>{}}={}) {
    if(!c)throw Error('Selecione um Caso.');check(signal);progress('Preparando a timeline…');
    const fonts=await prepare();check(signal);
    const timeline=await CaseTimeline.rows(c,()=>true,{signal,limit:10000,maxChars:8000000,includeUndated:true});
    const items=c.items||[], trails=c.caseTrails||[], byId=new Map(items.map((item,i)=>[item.id,{item,ref:`I${String(i+1).padStart(3,'0')}`} ]));
    if(items.length>10000||trails.length>1000)throw Error('O relatório aceita até 10.000 itens e 1.000 trilhas. Divida o Caso para gerar o PDF.');
    const narrativeChars=[...items,...trails].reduce((total,target)=>{const n=CaseContent.narrative(target);return total+clean(n.summary).length+clean(n.details).length+clean(target.title||target.label).length;},0);
    if(narrativeChars>8000000)throw Error('Os textos do Caso excedem o limite de 8 milhões de caracteres por relatório.');
    const pdf=new window.jspdf.jsPDF({unit:'mm',format:'a4',compress:true,putOnlyUsedFonts:true});
    ['normal','bold'].forEach((style,i)=>{pdf.addFileToVFS(`NotoSans-${style}.ttf`,fonts[i]);pdf.addFont(`NotoSans-${style}.ttf`,'CaseSans',style);});
    pdf.setProperties({title:c.name||'Relatório do Caso',subject:'Timeline, trilhas e itens preservados',creator:'LogInsight',author:'',keywords:'investigação, timeline, caso'});
    let y=23, section='Relatório do Caso', linesSinceYield=0;
    const canvas=document.createElement('canvas'),ctx=canvas.getContext('2d'), pageMap=new Map();
    function font(size=10,bold=false,color=[37,42,53]) {pdf.setFont('CaseSans',bold?'bold':'normal');pdf.setFontSize(size);pdf.setTextColor(...color);}
    const fallback = value => [...value].some(char=>char!==' '&&!pdf.getFont().metadata.characterToGlyph(char.codePointAt(0)));
    function textWidth(value,size,bold){font(size,bold);if(!fallback(value))return pdf.getTextWidth(value);ctx.font=`${bold?700:400} ${size*96/72}px "Case Report Sans", sans-serif`;return ctx.measureText(value).width*25.4/96;}
    function wrap(value,width,size=10,bold=false) {
      const lines=[];for(const paragraph of clean(value).split('\n')){
        if(!paragraph){lines.push('');continue;}let line='';
        for(const token of paragraph.match(/\S+\s*|\s+/gu)||[]){
          if(textWidth(line+token,size,bold)<=width){line+=token;continue;}
          if(line){lines.push(line.trimEnd());line='';}
          if(textWidth(token,size,bold)<=width){line=token;continue;}
          for(const char of token){if(line&&textWidth(line+char,size,bold)>width){lines.push(line);line='';}line+=char;}
        }lines.push(line.trimEnd());
      }return lines;
    }
    function draw(value,x,baseline,size=10,bold=false,color=[37,42,53]) {
      value=clean(value);if(!value)return;font(size,bold,color);
      if(!fallback(value)){pdf.text(value,x,baseline);return;}
      // Preserve characters unavailable in the embedded font using the local system fallback.
      const width=textWidth(value,size,bold),scale=3,px=size*96/72;
      canvas.width=Math.max(1,Math.ceil(width*96/25.4*scale+8));canvas.height=Math.ceil(px*1.8*scale);
      ctx.scale(scale,scale);ctx.font=`${bold?700:400} ${px}px "Case Report Sans", sans-serif`;ctx.fillStyle=`rgb(${color.join(',')})`;ctx.textBaseline='alphabetic';ctx.fillText(value,0,px*1.2);
      pdf.addImage(canvas.toDataURL('image/png'),'PNG',x,baseline-px*1.2*25.4/96,canvas.width/scale*25.4/96,canvas.height/scale*25.4/96,undefined,'FAST');canvas.width=canvas.height=1;
    }
    function newPage() {check(signal);if(pdf.getNumberOfPages()>=MAX_PAGES)throw Error(`O relatório ultrapassa ${MAX_PAGES} páginas. Divida o Caso para exportar.`);pdf.addPage();y=27;draw(wrap(section,WIDTH,8,true)[0],LEFT,16,8,true,[101,82,137]);pdf.setDrawColor(225,226,232);pdf.line(LEFT,20,RIGHT,20);}
    function ensure(height) {if(y+height>BOTTOM)newPage();}
    async function paragraph(value,{size=10,bold=false,color=[57,64,76],indent=0,gap=4}={}) {
      if(!value)return;const lineHeight=size*.35278*1.5;
      for(const line of wrap(value,WIDTH-indent,size,bold)){check(signal);ensure(lineHeight);draw(line,LEFT+indent,y+size*.35278,size,bold,color);y+=lineHeight;if(++linesSinceYield%70===0)await pause();}y+=gap;
    }
    async function heading(value,{level=1,fresh=false}={}) {if(fresh&&y>30)newPage();ensure(level===1?24:20);await paragraph(value,{size:level===1?19:13,bold:true,color:[40,37,52],gap:level===1?6:3});}
    async function image(data,width,height,caption,maxHeight=190) {
      const ratio=Math.min(WIDTH/width,maxHeight/height),w=width*ratio,h=height*ratio;ensure(h+14);const x=LEFT+(WIDTH-w)/2;
      pdf.addImage(data,'PNG',x,y,w,h,undefined,'FAST');y+=h+3;if(caption)await paragraph(caption,{size:8,color:[106,111,124],gap:5});else y+=5;
    }
    async function gallery(target) {
      for(const asset of CaseContent.attachments(target)){
        check(signal);progress(`Incluindo imagem: ${asset.caption||asset.name||'Print'}…`);
        const data=await CaseContent.imageData(asset.id);check(signal);
        const img=new Image();img.src=data;await img.decode();
        // Decode one image at a time. PNG preserves print text; jsPDF deduplicates repeated images.
        const scale=Math.min(1,2000/Math.max(img.naturalWidth,img.naturalHeight));canvas.width=Math.max(1,Math.round(img.naturalWidth*scale));canvas.height=Math.max(1,Math.round(img.naturalHeight*scale));
        ctx.fillStyle='#fff';ctx.fillRect(0,0,canvas.width,canvas.height);ctx.drawImage(img,0,0,canvas.width,canvas.height);
        const encoded=canvas.toDataURL('image/png'),width=canvas.width,height=canvas.height;canvas.width=canvas.height=1;img.src='';
        await image(encoded,width,height,asset.caption||asset.name||'Imagem');await pause();
      }
    }
    progress('Montando a visão do Caso…');
    draw('LOGINSIGHT  /  CASO',LEFT,y,9,true,[113,88,151]);y+=10;
    await heading(c.name||'Relatório do Caso');
    await paragraph(`${items.length} itens  ·  ${trails.length} trilhas  ·  ${timeline.eventCount} registros`,{size:10,color:[93,99,112]});
    await paragraph(`Gerado em ${stamp(Date.now())}  ·  Horários: ${Intl.DateTimeFormat().resolvedOptions().timeZone}`,{size:8,color:[112,117,129],gap:9});
    const overview=await CaseTimeline.image(c,{mode:'horizontal',width:1100,maxHeight:900,passes:()=>true,signal});check(signal);
    if(overview.blob)await image(await toDataURL(overview.blob),overview.width,overview.height,overview.overview?overview.summary:'Visão temporal dos registros preservados no Caso.',125);
    else await paragraph('Os registros deste Caso não têm horário reconhecido. A tabela a seguir inclui as ocorrências sem data.',{size:10});
    if(timeline.undated)await paragraph(`${timeline.undated} registros sem horário estão identificados na tabela.`,{size:8,color:[112,117,129]});
    const facts=window.CaseIntel?.synthesis(c,item=>byId.get(item.id)?.ref);
    if(facts&&(facts.hypotheses.length||facts.techniques.length||facts.indicators.length||facts.custody.length)){
      section='Síntese da investigação';progress('Incluindo hipóteses, indicadores e integridade…');await heading(section,{fresh:true});
      if(facts.hypotheses.length){await heading('Hipóteses',{level:2});for(const h of facts.hypotheses)await paragraph(`${h.status.toUpperCase()} · ${h.text}${h.refs.length?`\nEvidências: ${h.refs.join(', ')}`:''}`,{size:9.5});y+=3;}
      if(facts.techniques.length){await heading('Técnicas observadas (MITRE ATT&CK)',{level:2});for(const t of facts.techniques)await paragraph(`${t.id} ${t.name}${t.tactics.length?` · ${t.tactics.join(', ')}`:''}${t.refs.length?` · ${t.refs.join(', ')}`:''}`,{size:9,gap:2});y+=3;}
      if(facts.indicators.length){await heading('Indicadores',{level:2});for(const i of facts.indicators)await paragraph(`${i.value} · ${i.kind} · ${i.status} · ${CaseIntel.sightingText(i.sightings)}${i.note?`\n${i.note}`:''}`,{size:9,gap:2});y+=3;}
      if(facts.custody.length){await heading('Integridade das fontes (SHA-256)',{level:2});for(const a of facts.custody){await paragraph(a.label,{size:9,bold:true,gap:1});for(const f of a.files)await paragraph(`${f.name}${f.origin==='extraído'?' (extraído do pacote)':''} · ${fmtBytes(f.bytes)}\n${f.sha256}`,{size:8,color:[80,86,98],gap:2});await paragraph(`Calculado em ${stamp(a.at)}`,{size:7.5,color:[120,124,136]});}}
    }
    section='Linha do tempo';await heading(section,{fresh:true});
    await paragraph(`${timeline.rows.length} ocorrências em ordem temporal. Referências I001, I002… identificam os itens detalhados nas próximas seções.`,{size:9,color:[104,110,124]});
    const columns=[LEFT,53,158,RIGHT], tableLine=4.2;
    function tableHead(){ensure(12);pdf.setFillColor(245,243,249);pdf.rect(LEFT,y,WIDTH,8,'F');draw('Momento',LEFT+2,y+5.5,8,true);draw('Ocorrência',55,y+5.5,8,true);draw('Itens / qtd.',160,y+5.5,8,true);y+=10;}
    tableHead();
    for(let i=0;i<timeline.rows.length;i++){
      check(signal);const row=timeline.rows[i];
      const dateText=row.start==null?'Sem horário':stamp(row.start)+(row.end!=null&&row.end!==row.start?`\naté ${stamp(row.end)}`:'');
      const body=[row.title,row.detail&&row.detail!==row.title?row.detail:'',row.source?`Origem: ${row.source}`:'',...(row.notes||[]).map(note=>`Nota: ${note.text}`)].filter(Boolean).join('\n');
      const refs=(row.itemIds||[]).map(id=>byId.get(id)?.ref).filter(Boolean).join(', ');
      const parts=[wrap(dateText,31,7.7),wrap(body,99,8),wrap([refs,row.count?`${row.count} registros`:'Manual'].filter(Boolean).join('\n'),30,7.7)];
      let offset=0,total=Math.max(...parts.map(lines=>lines.length));
      while(offset<total){
        if(BOTTOM-y<16){newPage();tableHead();}
        const available=Math.max(1,Math.floor((BOTTOM-y-4)/tableLine)),take=Math.min(total-offset,available);
        for(let col=0;col<3;col++)for(let n=0;n<take;n++){const text=parts[col][offset+n];if(text)draw(text,columns[col]+2,y+3.3+n*tableLine,col===1?8:7.7,false,col===1?[43,48,60]:[105,111,124]);}
        y+=take*tableLine+4;pdf.setDrawColor(233,234,239);pdf.line(LEFT,y-1,RIGHT,y-1);offset+=take;
        if(offset<total){newPage();tableHead();draw('Continuação da ocorrência',LEFT+2,y+2,7,false,[116,107,134]);y+=6;}
      }
      if(i%25===0){progress(`Organizando timeline: ${i+1} / ${timeline.rows.length}…`);await pause();}
    }
    if(!timeline.rows.length)await paragraph('Nenhuma ocorrência registrada.');
    async function evidence(entry) {
      const {item,ref}=entry;if(pageMap.has(item.id)){await paragraph(`${ref} · ${item.label||'Item do Caso'} — explicação e imagens na página ${pageMap.get(item.id)}.`,{size:9});return;}
      ensure(28);pageMap.set(item.id,pdf.getNumberOfPages());await heading(`${ref} · ${item.label||'Item do Caso'}`,{level:2});
      const rows=item.rows||[], sources=[...new Set(rows.map(row=>row.source).filter(Boolean))];
      await paragraph(`${rows.length} registros preservados${sources.length?` · ${sources.slice(0,6).join(', ')}${sources.length>6?` e mais ${sources.length-6} origens`:''}`:''}`,{size:8,color:[108,114,127]});
      const narrative=CaseContent.narrative(item);await paragraph(narrative.summary,{bold:true});await paragraph(narrative.details);await gallery(item);y+=4;
    }
    const associated=new Set();
    for(let i=0;i<trails.length;i++){
      check(signal);const trail=trails[i];section=`Trilha ${i+1} · ${trail.title||'Sem título'}`;progress(`Organizando trilha ${i+1} / ${trails.length}…`);await heading(section,{fresh:true});
      const narrative=CaseContent.narrative(trail);await paragraph(narrative.summary,{bold:true});await paragraph(narrative.details);await gallery(trail);
      const ids=[...new Set(trail.itemIds||[])];if(!ids.length)await paragraph('Nenhum item associado a esta trilha.',{size:9,color:[110,115,125]});
      for(const id of ids){if(!byId.has(id))continue;associated.add(id);await evidence(byId.get(id));check(signal);}
    }
    const remaining=items.filter(item=>!associated.has(item.id));
    if(remaining.length){section=trails.length?'Itens sem trilha':'Itens do Caso';await heading(section,{fresh:true});for(const item of remaining){await evidence(byId.get(item.id));check(signal);}}
    progress('Finalizando páginas…');
    const pages=pdf.getNumberOfPages();for(let page=1;page<=pages;page++){pdf.setPage(page);pdf.setDrawColor(225,226,232);pdf.line(LEFT,284,RIGHT,284);draw('LogInsight · Relatório do Caso',LEFT,289,7,false,[123,126,138]);const number=`${page} / ${pages}`;draw(number,RIGHT-textWidth(number,7,false),289,7,false,[123,126,138]);}
    check(signal);const blob=pdf.output('blob');if(blob.size>MAX_BYTES)throw Error('O relatório excede 32 MB. Reduza a quantidade de imagens ou divida o Caso.');
    return {blob,pages,filename:filename(c),rows:timeline.rows.length,items:items.length,trails:trails.length,overview:overview.overview};
  }
  function open() {
    if(dialog)return;const c=activeCase();if(!c){toast('Selecione um Caso.','info');return;}
    const overlay=el('div','modal-overlay'),controller=new AbortController(),origin=document.activeElement;dialog=overlay;
    overlay.innerHTML='<section class="modal case-report-dialog" role="dialog" aria-modal="true" aria-labelledby="case-report-title"><div class="modal-head"><h3 id="case-report-title">Relatório do Caso</h3><button class="icon-btn" data-close aria-label="Fechar"><i class="fas fa-xmark"></i></button></div><div class="modal-body"><p data-case></p><p class="muted small">Visão do Caso, timeline em tabela, trilhas e itens com suas explicações e imagens.</p><p class="muted small" data-status role="status"></p><progress hidden></progress><div class="modal-actions"><button class="btn ghost" data-cancel>Cancelar</button><button class="btn primary" data-generate><i class="fas fa-file-pdf"></i> Gerar PDF</button></div></div></section>';
    overlay.querySelector('[data-case]').textContent=`${c.name} · Caso completo · ${c.items?.length||0} itens`;
    let running=false,saving=false;
    const close=()=>{if(saving)return;controller.abort();overlay.remove();dialog=null;origin?.focus();};
    overlay.querySelector('[data-close]').onclick=close;overlay.querySelector('[data-cancel]').onclick=close;overlay.onclick=e=>{if(e.target===overlay)close();};overlay.onkeydown=e=>{if(e.key==='Escape'){e.preventDefault();e.stopPropagation();close();}};
    overlay.querySelector('[data-generate]').onclick=async()=>{
      if(running)return;running=true;const button=overlay.querySelector('[data-generate]'),status=overlay.querySelector('[data-status]'),bar=overlay.querySelector('progress');button.disabled=true;bar.hidden=false;status.classList.remove('case-report-error');
      try {
        // UI state may keep changing during a long export; the report represents this snapshot.
        const snapshot=structuredClone(c);const result=await render(snapshot,{signal:controller.signal,progress:value=>{status.textContent=value;}});check(controller.signal);
        saving=true;status.textContent='Salvando PDF…';const data=await toDataURL(result.blob);const saved=await api('export_timeline',{format:'pdf',filename:result.filename,base64:data.slice(data.indexOf(',')+1)},{silent:true});saving=false;
        if(saved?.saved===false){status.textContent='Salvamento cancelado.';return;}toast(`Relatório salvo · ${result.pages} páginas.`,'ok');close();
      }catch(error){saving=false;if(error.name!=='AbortError'){status.textContent=String(error.message||error);status.classList.add('case-report-error');}}
      finally{running=false;button.disabled=false;bar.hidden=true;}
    };
    document.body.append(overlay);overlay.querySelector('[data-generate]').focus();
  }
  return {open,render,filename,limits:{maxPages:MAX_PAGES,maxBytes:MAX_BYTES}};
})();
