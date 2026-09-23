/* Purpose-led analysis. Exact charts are loaded on demand; sampled discoveries are labelled. */
(() => {
  'use strict';
  const modes = [
    ['overview', 'fa-border-all', 'Visão geral', 'Um panorama do recorte'],
    ['frequency', 'fa-ranking-star', 'Frequências', 'Onde os registros se concentram'],
    ['time', 'fa-wave-square', 'No tempo', 'Picos, intervalos e mudanças de ritmo'],
    ['patterns', 'fa-fingerprint', 'Padrões', 'Mensagens semelhantes, raridades e desvios'],
    ['behavior', 'fa-wand-magic-sparkles', 'Desvios', 'Combinações que passaram a se comportar de outra forma'],
    ['changes', 'fa-arrow-trend-up', 'Mudanças', 'Resultados que ganharam espaço em um intervalo'],
    ['threats', 'fa-shield-halved', 'Ameaças', 'Indícios locais por regras editáveis'],
    ['custom', 'fa-sliders', 'Meus gráficos', 'Monte e preserve suas próprias visualizações'],
  ];
  const savedMode = localStorage.getItem('analysis.mode');
  let mode = modes.some(m => m[0] === savedMode) ? savedMode : 'overview';
  let generation = 0, revision = 0, profileKey = '', fieldRequest = 0;
  const cache = new Map();
  const legacyRender = renderDashboard;
  const originalApi = api;
  api = async function(command, args, options) {
    const result = await originalApi(command, args, options);
    if (/^(load_file|load_files|load_event_log|clear_events|save_codes|save_derived_field|delete_derived_field|set_ts_config|harvest_codes|cases_load)$/.test(command)) {
      revision++; cache.clear(); profileKey = '';
    }
    return result;
  };
  const scopeKey = scope => JSON.stringify([scope, revision, state.currentArtifact?.id, state.currentArtifact?.loadedAt, scope === 'case' ? caseSig() : '', backendFilters(), state.derivedFields]);
  const percent = n => `${(100 * n).toLocaleString('pt-BR', {maximumFractionDigits: 1})}%`;
  async function cached(command, args, key) {
    const id = `${key}:${command}:${JSON.stringify([args.spec || '',args.filters || [],args.groupColumn,args.aggs])}`;
    if (cache.has(id)) return cache.get(id);
    const pending = api(command, args);
    cache.set(id, pending);
    if (cache.size > 32) cache.delete(cache.keys().next().value);
    try { return await pending; } catch (error) { cache.delete(id); throw error; }
  }
  async function applySelection(filters, scope, openRecords = false) {
    if (window.WorkspaceContext && scope !== workspaceScope()) await window.WorkspaceContext.setScope(scope, { page: 'explore', tab: 'table' });
    closeDrawer();
    for (const filter of filters) {
      if (!state.filters.some(f => f.column === filter.column && f.op === filter.op && f.value === filter.value && (f.value2 ?? null) === (filter.value2 ?? null))) state.filters.push({...filter, value2: filter.value2 ?? null});
    }
    state.page = 0;
    if (openRecords) { switchView('viz'); state.activeDatasetTab = 'table'; switchTab('table'); }
    filtersChanged();
  }
  function valueFilter(field, value) {
    return {column: field, op: value === '(vazio)' || value === '' ? 'empty' : 'equals_exact', value: value === '(vazio)' ? '' : String(value)};
  }
  function button(label, onClick, className = 'btn ghost small') {
    const b = el('button', className, label); b.type = 'button'; b.onclick = onClick; return b;
  }
  function showExplanation(title,text,origin) {
    let modal=$('#analysis-help');
    if(!modal){modal=el('div','modal-overlay');modal.id='analysis-help';modal.innerHTML='<section class="modal compact-modal" role="dialog" aria-modal="true" aria-labelledby="analysis-help-title"><div class="modal-head"><h3 id="analysis-help-title"></h3><button type="button" class="icon-btn" aria-label="Fechar explicação"><i class="fas fa-xmark"></i></button></div><div class="modal-body"></div></section>';document.body.append(modal);}
    const close=()=>{modal.hidden=true;origin?.focus();};
    modal.querySelector('h3').textContent=title;modal.querySelector('.modal-body').textContent=text;modal.hidden=false;
    const exit=modal.querySelector('button');exit.onclick=close;modal.onclick=e=>{if(e.target===modal)close();};modal.onkeydown=e=>{if(e.key==='Escape'){e.stopPropagation();close();}else if(e.key==='Tab'){e.preventDefault();exit.focus();}};exit.focus();
  }
  function infoButton(title,text) {
    const info=button('',()=>showExplanation(title,info.dataset.explanation,info),'icon-btn discovery-info');
    info.innerHTML='<i class="fas fa-circle-info"></i>';info.title='Como interpretar';info.setAttribute('aria-label',`Como interpretar ${title}`);info.dataset.explanation=text;return info;
  }
  function addExplanation(box,text) {
    const info=box.closest('.discovery-card')?.querySelector('.discovery-info') || $('#discovery-method');
    if(info && !info.dataset.explanation.includes(text))info.dataset.explanation+=`\n\n${text}`;
  }
  function card(grid, title, subtitle, wide = false, showMeta = false) {
    const node = el('article', `discovery-card${wide ? ' discovery-wide' : ''}`);
    const head = el('header', 'discovery-card-head');
    head.append(el('h3', '', title), infoButton(title,subtitle));
    if(showMeta) head.append(el('p', 'discovery-card-meta', subtitle));
    const body = el('div', 'discovery-card-body'); node.append(head, body); grid.append(node); return body;
  }
  function empty(box, message) { box.append(el('p', 'discovery-empty', message)); }
  function clearCharts() { for (const id of Object.keys(dashCharts)) { dashCharts[id].destroy(); delete dashCharts[id]; } }
  function mount(scope) {
    const panel = $('#dash-grid').parentElement;
    panel.classList.add('discovery-panel');
    let rail = panel.querySelector('.discovery-rail');
    if (!rail) {
      rail = el('nav', 'discovery-rail'); rail.setAttribute('aria-label', 'Modos de análise');
      panel.append(rail);
      const heading = el('div', 'discovery-heading'); heading.innerHTML = '<div><h2></h2></div><span class="discovery-scope"></span>';
      const info=infoButton('Análise','');info.id='discovery-method';heading.firstChild.append(info);
      $('#dash-grid').before(heading);
    }
    rail.innerHTML = '';
    for (const [id, icon, title] of modes) {
      const b = button('', () => { mode = id; localStorage.setItem('analysis.mode', id); renderDashboard(scope); }, 'discovery-mode');
      b.innerHTML = `<i class="fas ${icon}" aria-hidden="true"></i><span>${title}</span>`;
      b.title = title; b.setAttribute('aria-label', title); b.setAttribute('aria-pressed', String(mode === id)); rail.append(b);
    }
    const current = modes.find(m => m[0] === mode);
    panel.querySelector('.discovery-heading h2').textContent = current[2];
    $('#discovery-method').dataset.explanation=current[3];
    $('#discovery-method').setAttribute('aria-label',`Como interpretar ${current[2]}`);
    panel.querySelector('.discovery-scope').textContent = `${scope === 'case' ? 'Registros salvos no caso' : 'Logs abertos'}${backendFilters().length ? ' · com filtros' : ' · sem filtros'}`;
    $('#btn-dash-add').hidden = mode !== 'custom';
    $('#btn-dash-compact').hidden = ['patterns','behavior','changes','threats'].includes(mode);
  }
  function usefulFields(profiles) {
    return profiles.filter(p => p.cardinality > 1 && !['timestamp','message','description','event_ref','id','raw'].includes(p.name) && p.kind !== 'time' && !/request.?id|trace.?id|correlation.?id/i.test(p.name))
      .sort((a,b) => (['source','level','code'].includes(b.name) - ['source','level','code'].includes(a.name)) || a.cardinality - b.cardinality);
  }
  openDashboard = async function(scope = workspaceScope()) { state.analyticsScope = scope; return renderDashboard(scope); };
  renderDashboard = async function(scope = state.analyticsScope) {
    const version = ++generation, key = scopeKey(scope); state.analyticsScope = scope;
    mount(scope);
    const grid = $('#dash-grid'); grid.classList.toggle('discovery-grid', mode !== 'custom');
    grid.classList.toggle('dash-compact', state.dashboardCompact);
    $('#btn-dash-compact').setAttribute('aria-pressed', String(state.dashboardCompact));
    if (mode === 'custom') {
      if (!dashboardCharts(scope)) setDashboardCharts([], scope);
      await legacyRender(scope);
      if (!(dashboardCharts(scope) || []).length) empty(grid, 'Use + Gráfico para montar uma visualização. As sugestões automáticas continuam nos outros modos.');
      return;
    }
    clearCharts(); grid.innerHTML = '<div class="discovery-loading" role="status"><i class="fas fa-circle-notch spin"></i> Lendo o recorte…</div>';
    $('#dash-info').textContent = 'Análise do recorte atual';
    if (!scopeHasEvents(scope)) { grid.innerHTML = ''; empty(grid, 'Abra logs ou salve registros no caso para começar.'); return; }
    const current = () => version === generation && key === scopeKey(scope) && !$('#view-dashboard').hidden;
    try {
      if(mode==='threats') {
        await window.Threats.render(grid,scope,{card,infoButton,isCurrent:current,load:()=>cached('threat_scan',analyticsRequest(scope),key)});
        return;
      }
      if (['patterns','behavior','changes'].includes(mode)) {
        const data = await cached('discover_patterns', analyticsRequest(scope), key);
        if (!current()) return;
        grid.innerHTML = '';
        $('#dash-info').textContent = `${fmtNum(data.sample_count)} analisados de ${fmtNum(data.total)} registros · ${data.limited ? 'amostra distribuída' : 'seleção completa'}${data.complete ? '' : ' · análise interrompida'}`;
        addExplanation(grid,`${data.limited ? 'Descobertas estimadas na amostra. Eventos raros podem não aparecer. ' : ''}${data.fields_limited ? `Perfil limitado a ${data.fields_considered.length} campos. ` : ''}${data.temporal_limited?'A análise temporal atingiu seu orçamento de campos ou contextos. ':''}Indicações para investigar; não comprovam falha nem causa. Os filtros consultam todos os registros do recorte.`);
        if (mode === 'patterns') renderPatterns(grid, data, scope); else renderBehavior(grid, data, scope, mode === 'changes');
        return;
      }
      let profiles = scopeProfiles(scope);
      if (!profiles || profileKey !== key) { profiles = await cached('profile_fields', analyticsRequest(scope), key); if (!current()) return; setScopeProfiles(profiles, scope); profileKey = key; }
      if (!current()) return;
      grid.innerHTML = '';
      const fields = usefulFields(profiles), numeric = profiles.filter(p => RANGE_KINDS.has(p.kind) && p.numeric_ratio >= .85 && !['id','code'].includes(p.name) && !/(^|[_.])(id|status|code)$/.test(p.name));
      const specs = [];
      const time = (title, split = null, type = 'line', field = null, metric = 'count') => ({title, chart:'time', field, metric, split, type, id: nid(), subtitle: split ? `Até 6 valores de ${colLabel(split)} · demais valores e vazios omitidos` : field ? `${metric === 'avg' ? 'Média' : 'Máximo'} por intervalo · confira a unidade no eixo` : 'Contagem completa por intervalo · arraste para filtrar'});
      const terms = p => ({title:colLabel(p.name), chart:'terms', field:p.name, metric:'count', type:'bar', id:nid(), subtitle:'10 valores mais frequentes · clique para investigar'});
      if (mode === 'overview') { specs.push(time('Volume no tempo')); specs.push(...fields.slice(0,3).map(terms)); }
      if (mode === 'frequency') specs.push(...fields.slice(0,4).map(terms));
      if (mode === 'time') {
        specs.push(time('Volume no tempo'));
        const category = fields.find(p=>p.cardinality<=20);
        if (category) specs.push(time(`${colLabel(category.name)} no tempo`, category.name));
        for (const p of numeric.slice(0,2)) specs.push(time(`${colLabel(p.name)} · média e picos`, null, 'line', p.name, 'avg'));
      }
      $('#dash-info').textContent = `${specs.length} visões · recorte completo`;
      addExplanation(grid,'Contagens completas dos valores exibidos. A escolha dos campos sugeridos usa perfil amostral. Rankings mostram os 10 valores mais frequentes; demais valores podem não aparecer.');
      if (!specs.length) empty(grid, 'Nenhum campo adequado neste recorte. Experimente Padrões ou monte um gráfico com o campo desejado.');
      // Two workers at most; switching modes stops enqueuing work from the old view.
      let cursor = 0;
      const jobs = specs.map(spec => ({spec, body:card(grid, spec.title, spec.subtitle, spec.chart === 'time')}));
      const crossBody = mode === 'frequency' ? card(grid,'Frequências cruzadas','Como a distribuição muda dentro de cada grupo · contagens completas',true) : null;
      if(crossBody) grid.prepend(crossBody.parentElement);
      const heatBody = mode === 'time' ? card(grid,'Mapa de calor','Tempo, categoria, quantidade e duas medidas na mesma visualização. Escolha os campos da cor e da borda.',true) : null;
      if(heatBody){jobs[0]?.body.parentElement.after(heatBody.parentElement);$('#dash-info').textContent=`${specs.length+1} visões · recorte completo`;}
      await Promise.all([0,1].map(async () => {
        while (cursor < jobs.length && current()) {
          const {spec, body} = jobs[cursor++]; body.innerHTML = '<span class="muted small">Calculando…</span>';
          try {
            const res = await cached('compute_series', {...analyticsRequest(scope), spec:{chart:spec.chart, metric:spec.metric, field:spec.field, split:spec.split || null, limit:10, unit:'auto', interval_ms:null}}, key);
            if (!current()) return; body.innerHTML = '';
            if(res.incompatible_units) {empty(body,`${fmtNum(res.incompatible_units)} valores têm unidades incompatíveis. Separe esses registros antes de comparar ${colLabel(spec.field)}.`);continue;}
            if (!res.x?.length) { empty(body, spec.chart === 'time' ? 'Sem horários reconhecidos neste recorte.' : 'Sem valores neste recorte.'); continue; }
            if (spec.chart === 'terms') renderRanking(body, res, spec, scope);
            else {
              if (spec.field) {
                const maxRes = await cached('compute_series', {...analyticsRequest(scope), spec:{chart:'time',metric:'max',field:spec.field,split:null,limit:10,unit:'auto',interval_ms:res.interval_ms || null}},key);
                if (!current()) return;
                // Share a series only when the engine used identical bucket boundaries and units.
                if (JSON.stringify(res.x) === JSON.stringify(maxRes.x) && res.unit === maxRes.unit) {
                  const merged = {...res,series:[{...res.series[0],name:'Média'},{...maxRes.series[0],name:'Máximo'}]}; renderInteractiveLine(body,merged,spec,scope);
                } else renderInteractiveLine(body,res,spec,scope);
              } else renderInteractiveLine(body,res,spec,scope);
            }
          } catch (error) { if (current()) { body.innerHTML=''; empty(body, `Não foi possível calcular: ${String(error)}`); } }
        }
      }));
      if(crossBody && current()) await renderCrossFrequency(crossBody,profiles,scope,key,current);
      if(heatBody && current()) await renderContextHeatmap(heatBody,profiles,scope,key,current);
    } catch (error) { if (current()) { grid.innerHTML=''; empty(grid, `Não foi possível analisar: ${String(error)}`); grid.append(button('Tentar novamente',()=>renderDashboard(scope))); } }
  };
  function renderRanking(box, res, spec, scope) {
    const values = res.series[0]?.points || [], max = Math.max(1,...values);
    res.x.forEach((label,i) => {
      const row = button('', ()=>applySelection([valueFilter(spec.field,label)],scope,true), 'discovery-rank');
      row.title = `${label}: ${fmtNum(values[i])} registros. Clique para filtrar.`;
      row.innerHTML = `<span class="discovery-rank-index">${i+1}</span><span class="discovery-rank-label">${esc(label || '(vazio)')}</span><span class="discovery-rank-track"><span style="width:${Math.max(0,100*values[i]/max)}%"></span></span><strong>${fmtNum(values[i])}</strong>`;
      row.oncontextmenu = e => {
        e.preventDefault();
        const filter=valueFilter(spec.field,label);
        showCtxMenu(e.clientX,e.clientY,[
          {icon:'fa-filter',label:'Incluir este valor',onClick:()=>applySelection([filter],scope,true)},
          {icon:'fa-filter-circle-xmark',label:'Excluir este valor',onClick:()=>applySelection([{...filter,op:filter.op==='empty'?'not_empty':'not_equals_exact'}],scope,true)},
          {icon:'fa-copy',label:'Copiar valor',onClick:()=>navigator.clipboard.writeText(String(label)).catch(()=>toast('Não foi possível copiar.','err'))},
        ]);
      }; box.append(row);
    });
    addExplanation(box,'Contagens por valor; a lista pode omitir valores menos frequentes.');
  }
  function renderInteractiveLine(box,res,spec,scope) {
    if(spec.field)res={...res,series:res.series.map(s=>({...s,points:s.points.map((v,i)=>s.samples?.[i]===0?null:v)}))};
    renderLineChart(box,res,spec.id);
    const plot = dashCharts[spec.id];
    if (window.ResizeObserver) {
      const observer = new ResizeObserver(() => {
        if (!box.isConnected || dashCharts[spec.id] !== plot) { observer.disconnect(); return; }
        const width = Math.max(200, box.clientWidth - 8);
        if (Math.abs(plot.width - width) > 1) plot.setSize({width, height: state.dashboardCompact ? 112 : 170});
      });
      observer.observe(box);
    }
    plot.setCursor({left:-10,top:-10});
    const overlay = plot.over; overlay.style.cursor = 'crosshair';
    let start = null;
    overlay.onpointerdown = e => { if (e.button !== 0) return; start=e.clientX; overlay.setPointerCapture(e.pointerId); };
    overlay.onpointerup = e => {
      if (start == null) return;
      const rect=overlay.getBoundingClientRect(), a=Math.max(0,Math.min(rect.width,start-rect.left)),b=Math.max(0,Math.min(rect.width,e.clientX-rect.left)); start=null;
      if(Math.abs(a-b)<5) {
        if(spec.field) {const timestamp=plot.posToVal(b,'x')*1000;let index=0;while(index<res.x.length-1&&Number(res.x[index+1])<=timestamp)index++;if(res.series.some(s=>s.points[index]!=null))showPeak(spec,res,index,scope);}
        return;
      }
      applySelection([{column:'timestamp',op:'between',value:String(Math.floor(plot.posToVal(Math.min(a,b),'x')*1000)),value2:String(Math.ceil(plot.posToVal(Math.max(a,b),'x')*1000))}],scope);
    };
    overlay.onpointercancel=()=>{start=null;};
    if(spec.field) {
      const peaks=res.series.at(-1).points;let peak=-1;peaks.forEach((v,i)=>{if(v!=null&&(peak<0||Number(v)>Number(peaks[peak])))peak=i;});
      if(peak>=0)box.append(button('O que se destaca no maior pico?',()=>showPeak(spec,res,peak,scope),'text-button'));
      addExplanation(box,'Clique em um ponto para comparar os grupos naquele intervalo. Arraste para filtrar.');
    } else box.append(button('Abrir evolução',()=>{ if(scope==='dataset') Workspace.showPage('timeline'); else { mode='time';renderDashboard(scope); } },'text-button'));
  }
  const heatChoice = {row:'', color:null, border:null, bins:'auto'};
  async function renderContextHeatmap(body, profiles, scope, key, isCurrent) {
    const categories = usefulFields(profiles).filter(p => p.cardinality <= 32);
    if (!categories.length) { empty(body, 'Nenhum campo com categorias repetidas neste recorte.'); return; }
    const numeric = profiles.filter(p => RANGE_KINDS.has(p.kind) && p.numeric_ratio >= .85 && !/(^|[_.])(id|status|code)$/.test(p.name));
    const controls = el('div', 'heat-controls');
    function select(label, options, value) {
      const wrapper = el('label', '', label.replace(' do mapa', '')), input = el('select');
      input.setAttribute('aria-label', label);
      for (const [v,t] of options) { const o=el('option','',t); o.value=v; input.append(o); }
      if ([...input.options].some(o => o.value === value)) input.value=value;
      wrapper.append(input); controls.append(wrapper); return input;
    }
    const row = select('Linhas do mapa', categories.map(p => [p.name,colLabel(p.name)]), heatChoice.row);
    const metrics = [['','Sem medida'], ...numeric.flatMap(p => [['avg','Média'],['max','Máximo']].map(([m,l]) => [JSON.stringify([m,p.name]),`${l} · ${colLabel(p.name)}`]))];
    const first = numeric.find(p => p.kind === 'duration') || numeric[0];
    const second = numeric.find(p => p.kind === 'bytes' && p !== first) || numeric.find(p => p !== first);
    const color = select('Cor do mapa', metrics, heatChoice.color ?? (first ? JSON.stringify(['avg',first.name]) : ''));
    const border = select('Borda do mapa', metrics, heatChoice.border ?? (second ? JSON.stringify(['avg',second.name]) : ''));
    const bins = select('Detalhe do mapa', [['auto','Automático'],['12','12 intervalos'],['24','24 intervalos'],['48','48 intervalos']], heatChoice.bins);
    const output = el('div','heat-output'); output.textContent='Cruzando os valores…'; body.append(controls,output);
    let volume;
    try { volume=await cached('compute_series',{...analyticsRequest(scope),spec:{chart:'time',metric:'count',field:null,split:null,limit:10,unit:'auto',interval_ms:null}},key); }
    catch(error) { output.textContent=String(error); return; }
    if (!isCurrent() || !body.isConnected) return;
    if (!volume.x.length) { output.innerHTML=''; empty(output,'Sem horários reconhecidos neste recorte.'); return; }
    const domain={column:'timestamp',op:'between',value:String(volume.x[0]),value2:String(Number(volume.x.at(-1))+(volume.interval_ms||1)-1)};
    let request=0;
    async function draw() {
      const token=++request, field=row.value, colorSpec=color.value, borderSpec=border.value;
      heatChoice.row=field; heatChoice.color=colorSpec; heatChoice.border=borderSpec; heatChoice.bins=bins.value;
      const buckets=bins.value==='auto' ? Math.max(12,Math.min(36,Math.floor((output.clientWidth-100)/24))) : Number(bins.value);
      const interval=Math.max(1,Math.ceil((Number(domain.value2)-Number(domain.value)+1)/buckets));
      const args={...analyticsRequest(scope),filters:[...backendFilters(),domain]};
      const spec={chart:'time',metric:'count',field:null,split:field,limit:6,unit:'auto',interval_ms:interval};
      const current=()=>isCurrent()&&token===request&&body.isConnected;
      output.textContent='Cruzando os valores…';
      try {
        const counts=await cached('compute_series',{...args,spec},key);
        if (!current()) return;
        const channel=async choice => {
          if (!choice) return null;
          const [metric,metricField]=JSON.parse(choice);
          return cached('compute_series',{...args,spec:{...spec,metric,field:metricField,interval_ms:counts.interval_ms}},key);
        };
        const [colors,borders]=await Promise.all([channel(colorSpec),channel(borderSpec)]);
        if (!current()) return;
        output.innerHTML='';
        for (const [data,label] of [[colors,color.selectedOptions[0].textContent],[borders,border.selectedOptions[0].textContent]]) {
          if (data?.incompatible_units) { empty(output,`${label}: há unidades incompatíveis. Separe os registros ou escolha outro campo.`); return; }
          if (data && JSON.stringify(data.x)!==JSON.stringify(counts.x)) { empty(output,'Os intervalos das medidas não coincidem. Recalcule a análise.'); return; }
        }
        const rows=counts.series.filter(s => s.points.some(n => n>0)).slice(0,6);
        if (!rows.length) { empty(output,'Sem registros nas categorias deste recorte.'); return; }
        const maximum=Math.max(1,...rows.flatMap(s=>s.points));
        function measurement(data) {
          if (!data) return null;
          const byName=new Map(data.series.map(s=>[s.name,s]));
          const values=data.series.flatMap(s=>s.points.filter((v,i)=>s.samples?.[i]!==0&&v!=null&&Number.isFinite(v)));
          const min=Math.min(...values), max=Math.max(...values);
          return {unit:data.unit,min,max,
            value(name,i) { const s=byName.get(name); return !s || s.samples?.[i]===0 || s.points[i]==null ? null : s.points[i]; },
            scale(v) { return v==null ? null : max===min ? .6 : Math.max(0,Math.min(1,(v-min)/(max-min))); }
          };
        }
        const fill=measurement(colors), ring=measurement(borders);
        const scroll=el('div','heat-scroll'), plot=el('div','dot-matrix');
        plot.style.setProperty('--dot-bins',counts.x.length); plot.style.minWidth=`${100+counts.x.length*24}px`;
        for (const series of rows) {
          const label=el('span','dot-row-label',series.name); label.title=series.name; plot.append(label);
          counts.x.forEach((timestamp,i) => {
            const count=series.points[i];
            if (!count) { plot.append(el('span','dot-empty')); return; }
            const valueA=fill?.value(series.name,i)??null, valueB=ring?.value(series.name,i)??null;
            const scaleA=fill?.scale(valueA), scaleB=ring?.scale(valueB);
            const start=Number(timestamp), end=Number(counts.x[i+1]??(start+counts.interval_ms))-1;
            const range={column:'timestamp',op:'between',value:String(start),value2:String(end)};
            const cell=button('',()=>applySelection([valueFilter(field,series.name),range],scope,true),'discovery-heat-cell dot-cell');
            const radius=2+8*Math.sqrt(count/maximum), stroke=ring ? (scaleB==null ? .8 : .8+3.2*scaleB) : .7;
            cell.innerHTML=`<svg viewBox="0 0 30 30" aria-hidden="true"><circle cx="15" cy="15" r="${radius}" stroke-width="${stroke}"${ring&&valueB==null?' stroke-dasharray="2 2"':''}/></svg>`;
            cell.classList.toggle('dot-no-fill',!!fill&&valueA==null);
            cell.style.setProperty('--dot-fill',String(fill ? .18+.72*(scaleA??0) : .6));
            cell.dataset.count=String(count); cell.dataset.color=valueA==null?'':String(valueA); cell.dataset.border=valueB==null?'':String(valueB);
            const details=[`${colLabel(field)}: ${series.name}`,`${dashTimeLabel(start)} — ${dashTimeLabel(end)}`,`Quantidade: ${fmtNum(count)}`];
            if(fill)details.push(`${color.selectedOptions[0].textContent}: ${valueA==null?'sem valor':fmtVal(valueA,fill.unit)}`);
            if(ring)details.push(`${border.selectedOptions[0].textContent}: ${valueB==null?'sem valor':fmtVal(valueB,ring.unit)}`);
            cell.title=details.join('\n'); cell.setAttribute('aria-label',cell.title); plot.append(cell);
          });
        }
        plot.append(el('span','dot-axis-label','Data/hora'));
        const tickStride=Math.max(1,Math.ceil(counts.x.length/5));
        for(let i=0;i<counts.x.length;i++) { const tick=el('span','dot-tick',i%tickStride===0?dashTimeLabel(counts.x[i]):''); plot.append(tick); }
        scroll.append(plot); output.append(scroll);
        const legend=el('div','dot-legend');
        const quantity=el('span','dot-size-legend');quantity.innerHTML='<i></i><i></i><i></i>';quantity.append(document.createTextNode(`Qtd. até ${fmtNum(maximum)}`));legend.append(quantity);
        for(const [data,label,type] of [[fill,'Cor','fill'],[ring,'Borda','ring']]) {
          if(!data||!Number.isFinite(data.min))continue;
          const item=el('span',`dot-${type}-legend`);item.append(el('i'),document.createTextNode(`${label}: ${fmtVal(data.min,data.unit)} — ${fmtVal(data.max,data.unit)}`));legend.append(item);
        }
        output.append(legend);
        addExplanation(body,'Posição horizontal = intervalo de tempo; linhas = até 6 categorias mais frequentes. Tamanho do círculo = quantidade de registros. A intensidade do preenchimento e a espessura da borda representam as medidas escolhidas, cada uma com sua própria escala. Círculo sem preenchimento ou borda pontilhada indica medida ausente, nunca zero. As medidas usam valores válidos dentro da categoria e do intervalo. Clique para filtrar categoria e período. As escalas se ajustam ao recorte; compare também os valores no hover.');
      } catch(error) { if(current()){output.innerHTML='';empty(output,String(error));} }
    }
    row.onchange=draw; color.onchange=draw; border.onchange=draw; bins.onchange=draw; await draw();
  }

  function renderPatterns(grid,data,scope) {
    const templates=card(grid,'Famílias de mensagens','Parâmetros variáveis são normalizados; os registros originais são preservados.',true);
    if(!data.templates.length) empty(templates,'Sem mensagens suficientes para identificar famílias.');
    for(const t of data.templates.slice(0,12)) {
      const row=button('',()=>applySelection([{column:'message',op:'pattern',value:t.pattern}],scope,true),'discovery-pattern');
      row.innerHTML=`<code>${esc(t.pattern || '(mensagem vazia)')}</code><span class="discovery-pattern-count">${fmtNum(t.count)}<small>${percent(t.share)} ${data.limited ? 'da amostra' : 'do recorte'}</small></span>`;templates.append(row);
    }
    for(const o of data.outliers.slice(0,4)) {
      const body=card(grid,`${colLabel(o.field)} · valores distantes`,`${fmtNum(o.outlier_count)} de ${fmtNum(o.count)} valores analisados · ${o.unit || 'número'}`,false,true);
      const unit = ({ms:'duration','bits/s':'bits','número':'number'})[o.unit] || o.unit;
      body.innerHTML=`<div class="discovery-stat"><span>Mediana<strong>${esc(fmtVal(o.median,unit))}</strong></span><span>Maior valor<strong>${esc(fmtVal(o.max,unit))}</strong></span></div>`;
      addExplanation(body,'Distância da mediana por desvio absoluto (MAD). É um sinal estatístico, não um limite de erro.');
      const actions=el('div','discovery-actions');
      if(o.max>o.upper) actions.append(button(`Acima de ${fmtVal(o.upper,unit)}`,()=>applySelection([{column:o.field,op:'gt',value:String(o.upper)}],scope,true)));
      if(o.min<o.lower) actions.append(button(`Abaixo de ${fmtVal(o.lower,unit)}`,()=>applySelection([{column:o.field,op:'lt',value:String(o.lower)}],scope,true)));
      body.append(actions);
    }
    const rare=data.categories.filter(c=>c.rare?.length).slice(0,4);
    for(const c of rare) {
      const body=card(grid,`${colLabel(c.field)} · pouco frequentes`,'Raridade no recorte analisado; não implica problema.');
      for(const r of c.rare.slice(0,5)) body.append(button(`${r.value} · ${fmtNum(r.count)} (${percent(r.share)})`,()=>applySelection([{column:c.field,op:'equals',value:r.value}],scope,true),'discovery-rare'));
    }
    if(!data.outliers.length) { const body=card(grid,'Desvios numéricos','Estatística robusta, sem configuração');empty(body,'Nenhum desvio destacado nos campos numéricos compatíveis.'); }
  }
  function renderBehavior(grid,data,scope,global) {
    const shifts=(global?data.changes:data.behavior_shifts)||[];
    addExplanation(grid,`${global?'Mudanças na distribuição dos resultados':'Resultados condicionados a combinações de até 3 campos'}. Cada janela é comparada aos demais intervalos do recorte, incluindo períodos anteriores e posteriores. ${fmtNum(data.timed_sample_count || 0)} registros datados, em até ${data.time_bins || 12} janelas. Modelos locais de frequência; nenhum dado é enviado para IA externa.`);
    if(!shifts.length) {const body=card(grid,global?'Nenhuma mudança consistente neste recorte':'Nenhum desvio contextual consistente','Os sinais precisam de histórico, contraste e registros suficientes.',true);empty(body,'Experimente ampliar o período ou remover um filtro. Variações pequenas, campos constantes e combinações raras demais não viram alertas.');return;}
    for(const s of shifts.slice(0,8)) {
      const body=card(grid,`${colLabel(s.outcome_field)} mudou neste intervalo`,`${fmtTsFull(s.start)} — ${fmtTsFull(s.end)}`,false,true);
      body.parentElement.classList.add('behavior-card');
      const context=el('div','behavior-context');
      for(const item of s.context) {const chip=el('span','',`${colLabel(item.field)}: ${item.value}`);chip.title=chip.textContent;context.append(chip);}
      if(!s.context.length)context.append(el('span','','Todo o recorte'));body.append(context);
      const observation=el('p','behavior-observation');observation.innerHTML=`<strong>${esc(s.observed || '(vazio)')}</strong> passou de <b>${percent(s.baseline_observed_share)}</b> para <b class="behavior-rise">${percent(s.observed_share)}</b>`;body.append(observation);
      const contrast=el('div','behavior-contrast');
      contrast.innerHTML=`<div><span>Demais intervalos</span><i><b style="width:${Math.max(0,Math.min(100,s.baseline_observed_share*100))}%"></b></i><small>${fmtNum(s.baseline_observed)} / ${fmtNum(s.baseline_count)}</small></div><div><span>Neste intervalo</span><i><b style="width:${Math.max(0,Math.min(100,s.observed_share*100))}%"></b></i><small>${fmtNum(s.window_observed)} / ${fmtNum(s.window_count)}</small></div>`;body.append(contrast);
      body.append(el('p','behavior-expected',`Esperado: ${s.expected} · ${percent(s.expected_share)}`));
      addExplanation(body,`O resultado predominante fora desta janela era “${s.expected}”, neste mesmo contexto. ${data.limited?'Contagens da amostra; a investigação consulta todo o recorte.':'Contagens dos registros analisados.'}`);
      const filters=s.context.map(c=>({column:c.field,op:'equals',value:c.value}));filters.push({column:'timestamp',op:'between',value:String(s.start),value2:String(s.end)});
      const actions=el('div','discovery-actions');actions.append(button('Ver desvio',()=>applySelection([...filters,{column:s.outcome_field,op:s.outcome_op,value:s.observed}],scope,true),'btn primary small'),button('Ver contexto',()=>applySelection(filters,scope,true)));body.append(actions);
    }
  }

  const crossChoice={row:'',column:''};
  async function renderCrossFrequency(body,profiles,scope,key,isCurrent) {
    const candidates=usefulFields(profiles).filter(p=>p.cardinality<=32);
    if(candidates.length<2){empty(body,'São necessários dois campos com valores repetidos para comparar distribuições.');return;}
    if(!candidates.some(p=>p.name===crossChoice.row))crossChoice.row=candidates[0].name;
    if(!candidates.some(p=>p.name===crossChoice.column)||crossChoice.column===crossChoice.row)crossChoice.column=candidates.find(p=>p.name!==crossChoice.row).name;
    const controls=el('div','cross-controls'),rows=el('select'),cols=el('select');rows.setAttribute('aria-label','Grupos das frequências cruzadas');cols.setAttribute('aria-label','Distribuição das frequências cruzadas');
    for(const p of candidates){for(const select of [rows,cols]){const o=el('option','',colLabel(p.name));o.value=p.name;select.append(o);}}
    rows.value=crossChoice.row;cols.value=crossChoice.column;
    controls.append(el('span','','Dentro de'),rows,el('span','','comparar'),cols);const output=el('div','cross-output');body.append(controls,output);
    let request=0;
    async function draw(){
      const token=++request;crossChoice.row=rows.value;crossChoice.column=cols.value;output.innerHTML='';
      if(rows.value===cols.value){empty(output,'Escolha dois campos diferentes.');return;}
      output.textContent='Cruzando os valores…';const rowField=rows.value,colField=cols.value;
      try{
        const data=await cached('pivot',{...analyticsRequest(scope),spec:{rows:[rowField],cols:[colField],values:[{func:'count',column:'*',alias:'Registros'}],limit_rows:100}},key);
        if(!isCurrent()||token!==request||!body.isConnected)return;output.innerHTML='';
        const groups=data.row_paths.map((path,i)=>({value:path[0],cells:data.cells[i].map(v=>Number(v[0])||0)})).filter(g=>g.value!==undefined).map(g=>({...g,total:g.cells.reduce((a,b)=>a+b,0)})).sort((a,b)=>b.total-a.total);
        const columnTotals=data.col_keys.map((_,i)=>groups.reduce((sum,g)=>sum+g.cells[i],0));
        const columns=data.col_keys.map((value,i)=>({value,i,count:columnTotals[i]})).sort((a,b)=>b.count-a.count).slice(0,6);
        if(!groups.length){empty(output,'Sem combinações neste recorte.');return;}
        const legend=el('div','cross-legend');columns.forEach((c,i)=>{const item=el('span','',c.value);item.style.setProperty('--color',CHART_COLORS[i%CHART_COLORS.length]);legend.append(item);});output.append(legend);
        for(const g of groups.slice(0,8)){
          const row=el('div','cross-row');const name=el('span','cross-name',g.value);name.title=g.value;
          const bar=el('div','cross-bar');let shown=0;
          columns.forEach((c,i)=>{const count=g.cells[c.i],share=count/Math.max(1,g.total);shown+=count;if(!count)return;const segment=button(share>=.11?percent(share):'',()=>applySelection([valueFilter(rowField,g.value),valueFilter(colField,c.value)],scope,true),'cross-segment');segment.style.width=`${share*100}%`;segment.style.background=CHART_COLORS[i%CHART_COLORS.length];segment.title=`${g.value} · ${c.value}: ${fmtNum(count)} registros (${percent(share)} deste grupo)`;segment.setAttribute('aria-label',segment.title);bar.append(segment);});
          if(shown<g.total){const rest=el('span','cross-other');rest.style.width=`${100*(g.total-shown)/g.total}%`;rest.title=`Outros: ${fmtNum(g.total-shown)}`;bar.append(rest);}
          row.append(name,bar,el('strong','',fmtNum(g.total)));output.append(row);
        }
        const fullTotal=groups.reduce((a,g)=>a+g.total,0), contrasts=[];
        for(const g of groups) for(const c of columns){const n=g.cells[c.i],restCount=columnTotals[c.i]-n,restTotal=fullTotal-g.total;if(g.total>=10&&restTotal>=10&&n>=3){const share=n/g.total,baseline=restCount/restTotal;contrasts.push({g,c,share,baseline,delta:share-baseline});}}
        contrasts.sort((a,b)=>b.delta-a.delta);
        const best=contrasts[0];if(best?.delta>=.1 && data.complete!==false && !data.truncated)output.append(el('p','cross-finding',`${best.g.value}: ${best.c.value} representa ${percent(best.share)} dos registros, contra ${percent(best.baseline)} nos demais grupos.`));
        addExplanation(body,`Percentuais dentro de cada grupo. Mostrando ${Math.min(8,groups.length)} grupos e até 6 categorias; cinza = demais categorias. Clique em um segmento para investigar.`);
        if(data.complete===false||data.truncated)output.append(el('small','discovery-foot','Resultado limitado · refine os campos ou o período'));
      }catch(error){if(isCurrent()&&token===request){output.innerHTML='';empty(output,String(error));}}
    }
    rows.onchange=draw;cols.onchange=draw;await draw();
  }

  async function showPeak(spec,res,index,scope) {
    const key=scopeKey(scope),start=Number(res.x[index]),end=Number(res.x[index+1] ?? (start+(res.interval_ms||1)))-1;
    const body=el('div','peak-context');openContextInspector(`Pico · ${colLabel(spec.field)}`,`${fmtTsFull(start)} — ${fmtTsFull(end)}`,body);$('.detail-quick-actions').hidden=true;
    const token=++fieldRequest, profiles=scopeProfiles(scope)||[],fields=usefulFields(profiles).filter(p=>p.name!==spec.field&&p.cardinality<=32).slice(0,3);
    body.append(infoButton('Comparação do pico','A referência é a média do grupo no período datado do gráfico, incluindo o pico. Valores ausentes não entram na média. As diferenças observadas não provam causa.'));
    const range={column:'timestamp',op:'between',value:String(start),value2:String(Math.max(start,end))};
    body.append(button('Abrir registros do intervalo',()=>applySelection([range],scope,true)));
    if(!fields.length){empty(body,'Nenhum campo categórico adequado para separar este pico. Abra os registros do intervalo para conferir os valores.');return;}
    const current=()=>token===fieldRequest&&key===scopeKey(scope)&&body.isConnected&&!$('#drawer').hidden;
    for(const field of fields){
      const section=el('section','peak-section');section.append(el('h3','',`Por ${colLabel(field.name)}`));const result=el('div');result.textContent='Comparando…';section.append(result);body.append(section);
      const pivotSpec={rows:[field.name],cols:[],values:[{func:'count',column:'*',alias:'Registros'},{func:'avg',column:spec.field,alias:'Média'},{func:'max',column:spec.field,alias:'Máximo'}],limit_rows:100};
      try{
        const domain={column:'timestamp',op:'between',value:String(res.x[0]),value2:String(Number(res.x.at(-1))+(res.interval_ms||1)-1)};
        const baseline=await cached('pivot',{...analyticsRequest(scope),filters:[...backendFilters(),domain],spec:pivotSpec},key);if(!current())return;
        const windowData=await cached('pivot',{...analyticsRequest(scope),filters:[...backendFilters(),range],spec:pivotSpec},key);if(!current())return;result.innerHTML='';
        const values=new Map(baseline.row_paths.map((p,i)=>[p[0],baseline.cells[i]?.[0]]));
        const groups=windowData.row_paths.map((p,i)=>({value:p[0],count:Number(windowData.cells[i]?.[0]?.[0])||0,mean:windowData.cells[i]?.[0]?.[1],max:windowData.cells[i]?.[0]?.[2],baseline:values.get(p[0])?.[1]})).filter(g=>g.value!==undefined&&g.mean!=null).sort((a,b)=>(b.mean-(b.baseline??b.mean))-(a.mean-(a.baseline??a.mean))||b.max-a.max).slice(0,5);
        if(!groups.length){empty(result,'Sem valores numéricos neste intervalo.');continue;}
        const table=el('table','peak-table');table.innerHTML='<thead><tr><th>Grupo</th><th>Média geral</th><th>Neste intervalo</th><th>Máximo</th></tr></thead>';const tbody=el('tbody');
        for(const g of groups){const tr=el('tr'),name=el('td'),open=button(g.value,()=>applySelection([range,valueFilter(field.name,g.value)],scope,true),'text-button');name.append(open,el('small','',`${fmtNum(g.count)} registros`));tr.append(name);for(const n of [g.baseline,g.mean,g.max])tr.append(el('td','',n==null?'—':fmtVal(n,res.unit)));tbody.append(tr);}table.append(tbody);result.append(table);
        if(baseline.complete===false||windowData.complete===false||baseline.truncated||windowData.truncated)result.append(el('p','discovery-explain','Comparação limitada pelo motor. Refine o campo ou o período.'));
      }catch(error){if(current()){result.innerHTML='';empty(result,String(error));}}
    }
  }
  // Preserve customized charts and their editor; refresh invalidates only analysis caches.
  $('#btn-dash-refresh').onclick=()=>{ cache.clear(); profileKey=''; renderDashboard(state.analyticsScope); };
  const compactClick=$('#btn-dash-compact').onclick;
  $('#btn-dash-compact').onclick=e=>{ if(compactClick) compactClick(e); else {state.dashboardCompact=!state.dashboardCompact;renderDashboard();} };
  window.Discovery = {applySelection, valueFilter, showExplanation, clearCache:()=>{revision++;cache.clear();}, capture:()=>({mode,showVolume}), restore:saved=>{generation++;fieldRequest++;mode=modes.some(item=>item[0]===saved?.mode)?saved.mode:'overview';showVolume=saved?.showVolume!==false;cache.clear();profileKey='';}};

  async function fieldTop(column,scope=workspaceScope()) {
    const token=++fieldRequest,key=scopeKey(scope),body=el('div','field-profile');
    body.innerHTML='<p class="muted" role="status">Contando valores no recorte completo…</p>';
    openContextInspector(`Top 10 · ${colLabel(column)}`,'Valores mais frequentes no recorte atual',body);
    $('.detail-quick-actions').hidden=true;
    try {
      const spec={chart:'terms',metric:'count',field:column,limit:10,unit:'auto',split:null,interval_ms:null};
      const [result,total]=await Promise.all([cached('compute_series',{...analyticsRequest(scope),spec},key),cached('count_filtered',analyticsRequest(scope),key)]);
      if(token!==fieldRequest || key!==scopeKey(scope) || !body.isConnected) return;
      body.innerHTML=''; body.append(el('p','discovery-explain',`${fmtNum(total)} registros`));
      if(!result.x.length) empty(body,'Nenhum valor encontrado.'); else renderRanking(body,result,{field:column,chart:'terms'},scope);
      const actions=el('div','discovery-actions');actions.append(button('Somente vazios',()=>applySelection([{column,op:'empty',value:''}],scope,true)),button('Somente preenchidos',()=>applySelection([{column,op:'not_empty',value:''}],scope,true)));body.append(actions);
    } catch(error) { if(body.isConnected) {body.innerHTML='';empty(body,String(error));} }
  }
  function fieldMenu(event,column,scope,anchor) {
    event.preventDefault();event.stopPropagation();
    const items=[
      {icon:'fa-ranking-star',label:`Top 10 de ${colLabel(column)}`,onClick:()=>fieldTop(column,scope)},
      {icon:'fa-filter',label:'Filtro avançado',onClick:()=>{openFilterPop(anchor);const select=$('#fp-col');if(![...select.options].some(o=>o.value===column)){const option=el('option','',colLabel(column));option.value=column;select.append(option);}select.value=column;}},
      {icon:'fa-filter-circle-xmark',label:'Somente vazios',onClick:()=>applySelection([{column,op:'empty',value:''}],scope)},
      {icon:'fa-check',label:'Somente preenchidos',onClick:()=>applySelection([{column,op:'not_empty',value:''}],scope)},
    ];
    items.splice(1,0,{icon:'fa-layer-group',label:`Resumir por ${colLabel(column)}`,onClick:()=>{state.groupCol=column;$('#group-col').value=column;switchTab('group');}});
    const rect=anchor.getBoundingClientRect();
    showCtxMenu(event.clientX || rect.right,event.clientY || rect.bottom,items);
  }
  const oldTree=renderExploreTreeInto;
  renderExploreTreeInto=function(box,scope) {
    oldTree(box,scope);
    box.querySelectorAll('.field-row[data-column]').forEach(row=>{
      const column=row.dataset.column,main=row.querySelector('.field-item'),menu=row.querySelector('.field-adv-btn');
      row.oncontextmenu=e=>fieldMenu(e,column,scope,main);
      main.title+= ' · botão direito para valores e ações';
      menu.innerHTML='<i class="fas fa-ellipsis"></i>';menu.title=`Ações do campo ${colLabel(column)}`;menu.setAttribute('aria-label',menu.title);
      menu.onclick=e=>fieldMenu(e,column,scope,menu);
    });
  };
  const oldTable=renderTable;
  renderTable=function(qr) {
    oldTable(qr);
    document.querySelectorAll('#events-table th').forEach((th,i)=>{
      const column=state.visibleCols[i];
      th.title='Clique para ordenar · arraste para mover · botão direito para analisar o campo';
      th.oncontextmenu=e=>{e.preventDefault();showCtxMenu(e.clientX,e.clientY,[
        {icon:'fa-ranking-star',label:`Top 10 de ${colLabel(column)}`,onClick:()=>fieldTop(column)},
        {icon:'fa-layer-group',label:'Resumir valores',onClick:()=>{state.groupCol=column;$('#group-col').value=column;switchTab('group');}},
        {icon:'fa-filter',label:'Somente preenchidos',onClick:()=>addFilter({column,op:'not_empty',value:''})},
        {icon:'fa-filter-circle-xmark',label:'Somente vazios',onClick:()=>addFilter({column,op:'empty',value:''})},
        {sep:true},{icon:'fa-table-cells',label:'Cruzar nas linhas',onClick:()=>{state.analyticsScope=workspaceScope();cubeAdd('rows',column);switchTab('cube');}},
        {icon:'fa-table-columns',label:'Cruzar nas colunas',onClick:()=>{state.analyticsScope=workspaceScope();cubeAdd('cols',column);switchTab('cube');}}
      ]);};
    });
  };
  const oldDetail=showDetail;
  showDetail=function(...args){fieldRequest++;$('.detail-quick-actions').hidden=false;return oldDetail(...args);};
  const oldClose=closeDrawer;closeDrawer=function(){fieldRequest++;return oldClose();};
  // Names describe the outcome; tooltips preserve the familiar technical terms.
  for(const [id,icon,label,title] of [['table','fa-table-list','Registros','Ler e filtrar registros individuais'],['group','fa-layer-group','Resumir','Agrupar valores e calcular medidas'],['dashboard','fa-chart-line','Descobrir','Explorar gráficos, padrões e relações'],['cube','fa-table-cells','Cruzar dados','Tabela dinâmica: cruzar campos e medidas']]) {
    const tab=$(`#tabbtn-${id}`);tab.innerHTML=`<i class="fas ${icon}"></i> ${label}`;tab.title=title;
  }
  $('#btn-back-drive').innerHTML='<i class="fas fa-folder-open"></i> Arquivos';
  $('#quick-search').placeholder='Buscar nos registros…';
  // Keep the query next to the records; the field catalogue occupies the right edge.
  const shell=$('.shell'),side=$('#workspace-side'),content=shell.querySelector('.content');
  shell.append(side);
  const query=side.querySelector('.filters-block');query.classList.add('explore-query');content.prepend(query);
  side.setAttribute('aria-label','Campos e valores');
  side.querySelector('.explore-block > .side-title').hidden=true;
  const sideToggle=$('#btn-side-toggle');
  const syncSide=()=>{const collapsed=shell.classList.contains('side-collapsed');sideToggle.innerHTML=`<i class="fas fa-chevron-${collapsed?'left':'right'}"></i>`;sideToggle.title=collapsed?'Mostrar campos':'Recolher campos';sideToggle.setAttribute('aria-label',sideToggle.title);sideToggle.setAttribute('aria-expanded',String(!collapsed));};
  sideToggle.onclick=()=>{shell.classList.toggle('side-collapsed');syncSide();};syncSide();
  let showVolume=localStorage.getItem('explore.volume')!=='hidden';
  const histogram=button('',()=>{const panel=$('.hist-panel');showVolume=!showVolume;localStorage.setItem('explore.volume',showVolume?'visible':'hidden');panel.hidden=!showVolume;histogram.setAttribute('aria-pressed',String(showVolume));if(!panel.hidden && chart) chart.setSize({width:Math.max(100,$('#chart').clientWidth-4),height:96});},'icon-btn');
  histogram.id='explore-volume';histogram.title='Mostrar ou recolher volume no tempo';histogram.setAttribute('aria-label',histogram.title);histogram.setAttribute('aria-pressed',String(!$('.hist-panel').hidden));histogram.innerHTML='<i class="fas fa-chart-column"></i>';$('.explore-tools').prepend(histogram);
  const oldRenderChart=renderChart;
  renderChart=function(...args){oldRenderChart(...args);$('.hist-panel').hidden=!showVolume;histogram.setAttribute('aria-pressed',String(showVolume));};
  document.addEventListener('keydown',e=>{if(e.key==='Escape') fieldRequest++;});
})();
