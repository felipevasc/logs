/* Read-only, local indicator exploration. Matching stays in the native engine. */
(() => {
  'use strict';
  const severity = {high:'Alta',medium:'Média',low:'Baixa'};
  const kinds = {attempt:'Tentativa',response:'Resposta',indicator:'Indicador'};
  let inspectorRequest=0;
  const emptyView=()=>({category:'',severity:'',kind:'',search:'',page:0});
  const view=emptyView(),views=new Map();
  let viewKey='';
  const contextKey=scope=>JSON.stringify([scope,activeCase()?.id||'',scope==='dataset'?state.currentArtifact?.id||'':null]);
  function useContext(scope){
    const key=contextKey(scope);
    if(key===viewKey)return;
    if(viewKey)views.set(viewKey,{...view});
    if(views.size>12)views.delete(views.keys().next().value);
    Object.assign(view,emptyView(),views.get(key)||{});viewKey=key;inspectorRequest++;
  }
  document.addEventListener('workspace-context-change',event=>useContext(event.detail.scope));
  const button=(text,action,cls='btn ghost small')=>{const b=el('button',cls,text);b.type='button';b.onclick=action;return b;};
  function inspector(title,subtitle,body) {
    inspectorRequest++;openContextInspector(title,subtitle,body);$('.detail-quick-actions').hidden=true;
    return inspectorRequest;
  }
  function catalogDetails(catalog) {
    const box=el('div','threat-catalog');let token;
    box.append(el('h3','',catalog.name||'Catálogo de ameaças'),el('p','',`${fmtNum(catalog.enabled||0)} regras ativas · ${(catalog.categories||[]).length} categorias`));
    const path=el('code','threat-path',catalog.path||'');box.append(path);
    box.append(button('Copiar caminho',()=>navigator.clipboard.writeText(catalog.path||'').then(()=>toast('Caminho copiado.')).catch(()=>toast('Não foi possível copiar.','err'))));
    if(catalog.preview)box.append(el('p','discovery-explain','Prévia: este é o arquivo base do projeto. Atualizações de teste ficam apenas nesta sessão do navegador.'));
    box.append(el('p','discovery-explain','Edite o JSON e use Recalcular. Mantenha IDs estáveis; enabled: false desativa uma regra. Regex inválida interrompe a análise e informa a regra.'));
    if(catalog.error)box.append(el('p','threat-error',catalog.error));
    if(!catalog.error&&catalog.updates_available>0){
      box.append(el('p','discovery-explain','A atualização adiciona somente IDs novos. Suas regras, alterações e regras desativadas são preservadas; o aplicativo salva uma cópia de segurança.'));
      const update=button(`Adicionar ${fmtNum(catalog.updates_available)} regras novas`,async()=>{
        update.disabled=true;
        try{
          const result=await api('threat_catalog_update',{});
          window.Discovery?.clearCache();
          if(token!==inspectorRequest||!box.isConnected||$('#drawer').hidden){toast(`${fmtNum(result.added)} regras adicionadas.`);return;}
          catalogDetails(result.catalog);
          if(result.backup_path)document.querySelector('.threat-catalog')?.append(el('p','discovery-explain',`Backup: ${result.backup_path}`));
          toast(`${fmtNum(result.added)} regras adicionadas.`);
          await renderDashboard(state.analyticsScope);
        }catch(error){box.append(el('p','threat-error',String(error)));update.disabled=false;}
      },'btn primary small');box.append(update);
    }
    const details=el('details');details.append(el('summary','','Categorias'));
    for(const category of catalog.categories||[])details.append(el('p','',category));box.append(details);
    token=inspector('Regras','Arquivo local editável',box);
  }
  function evidence(event,scope) {
    if(scope==='dataset'){openDetail(event.id);return;}
    const original=caseEvents().find(e=>event.event_ref&&e.event_ref===event.event_ref)||caseEvents().find(e=>e.id===event.id)||event;
    showDetail(original);$('#dr-prev').hidden=true;$('#dr-next').hidden=true;$('.detail-quick-actions').hidden=true;
  }
  async function records(scope,ruleId='*',extra=[],name='Indícios') {
    const body=el('div','threat-records'),token=inspector(name,scope==='case'?'Registros salvos no caso':'Logs abertos',body);
    const base=analyticsRequest(scope),filters=[...base.filters,{column:'_all',op:'threat_rule',value:ruleId,value2:null},...extra];
    const key=contextKey(scope);
    let page=0,version=0;
    const current=()=>token===inspectorRequest&&key===contextKey(scope)&&body.isConnected&&!$('#drawer').hidden&&(!window.WorkspaceContext||window.WorkspaceContext.scope()===scope);
    async function load(){
      const request=++version;body.textContent='Buscando registros…';
      try{
        const data=await api('threat_events',{...base,filters,offset:page*50,limit:50});
        if(!current()||request!==version)return;body.innerHTML='';
        const head=el('div','threat-record-head');head.append(el('strong','',`${fmtNum(data.total)} registros`));body.append(head);
        if(data.complete===false)body.append(el('p','threat-partial','Varredura parcial · alguns registros não foram examinados integralmente.'));
        for(const event of data.rows||[]){
          const row=button('',()=>evidence(event,scope),'threat-record');
          row.append(el('small','',`${fmtTsFull(event.timestamp)} · ${event.source||'Sem origem'}`),el('span','',String(event.message||event.name||'(sem mensagem)').slice(0,260)));body.append(row);
        }
        if(!data.rows?.length)body.append(el('p','discovery-empty','Nenhum registro nesta seleção.'));
        if(data.rows_clipped)body.append(el('small','discovery-foot','Prévia reduzida. Abra o registro para consultar o conteúdo completo.'));
        const pages=Math.max(1,Math.ceil(data.total/50)),pager=el('div','threat-pager');
        const previous=button('Anterior',()=>{page--;load();}),next=button('Próxima',()=>{page++;load();});previous.disabled=page===0;next.disabled=page>=pages-1;
        pager.append(previous,el('span','',`${page+1} / ${pages}`),next);body.append(pager);
      }catch(error){if(current()&&request===version){body.innerHTML='';body.append(el('p','threat-error',String(error)),button('Tentar novamente',load));}}
    }
    await load();
  }
  function ruleDetails(rule,scope) {
    const body=el('div','threat-rule-detail');
    body.append(el('h3','',rule.name),el('p','discovery-explain',`${rule.category} · ${severity[rule.severity]||rule.severity} · ${kinds[rule.kind]||rule.kind}`));
    body.append(button(`Ver ${fmtNum(rule.count)} registros`,()=>records(scope,rule.id,[],rule.name),'btn primary small'));
    body.append(el('p','',rule.description));
    const pattern=el('details');pattern.append(el('summary','','Expressão da regra'),el('pre','threat-pattern',rule.pattern));body.append(pattern);
    for(const example of rule.examples||[]){const section=el('section','threat-example');section.append(el('small','',`${fmtTsFull(example.timestamp)}${example.normalized?' · texto normalizado':''}`),el('pre','',example.snippet||''));body.append(section);}
    for(const reference of rule.references||[]){try{const url=new URL(reference);if(!['http:','https:'].includes(url.protocol))continue;const link=el('a','threat-reference',url.hostname+url.pathname);link.href=url.href;link.target='_blank';link.rel='noopener noreferrer';body.append(link);}catch{}}
    inspector('Regra',rule.id,body);
  }
  async function render(grid,scope,{card,infoButton,isCurrent,load}) {
    useContext(scope);
    const key=viewKey,current=()=>isCurrent()&&key===viewKey&&key===contextKey(scope)&&(!window.WorkspaceContext||window.WorkspaceContext.scope()===scope);
    let catalog;
    try{catalog=await api('threat_catalog',{}, {silent:true});}catch(error){catalog={error:String(error),rules:[]};}
    if(!current())return;
    const tools=el('div','threat-tools');tools.append(button('Regras',()=>catalogDetails(catalog)));
    const help=infoButton('Ameaças','Correspondências com regras locais são indícios, não confirmação de invasão. Conteúdo bloqueado, testes autorizados e texto citado também podem corresponder. Alta, média e baixa expressam a prioridade da regra; não a probabilidade de comprometimento. O catálogo é editável e não cobre todas as técnicas. Um registro pode corresponder a várias regras.');tools.append(help);
    if(catalog.error){grid.innerHTML='';grid.append(tools,el('p','threat-error',catalog.error));return;}
    let data;
    try{data=await load();}catch(error){if(current()){grid.innerHTML='';grid.append(tools,el('p','threat-error',String(error)));}return;}
    if(!current())return;grid.innerHTML='';grid.append(tools);
    $('#dash-info').textContent=`${fmtNum(data.matched)} registros com indícios · ${fmtNum(data.total)} examinados · ${fmtNum(data.enabled_rules)} regras`;
    if(data.complete===false){const warning=el('div','threat-partial',`${data.clipped_records?`${fmtNum(data.clipped_records)} registros excedem o limite de leitura. `:''}Análise parcial.`);grid.append(warning);}
    const summary=el('div','threat-summary');
    for(const [value,label] of [[fmtNum(data.matched),'Registros com indícios'],[`${(100*data.matched/Math.max(1,data.total)).toLocaleString('pt-BR',{maximumFractionDigits:1})}%`,'Do recorte'],[fmtNum(data.rules.length),'Regras encontradas']]){const item=el('div');item.append(el('strong','',value),el('span','',label));summary.append(item);}
    summary.append(button('Ver registros',()=>records(scope),'btn primary small'));grid.append(summary);
    if(!data.matched){const body=card(grid,'Nenhum indício pelas regras atuais','Ausência de correspondência não demonstra ausência de ameaça.',true);body.append(el('p','discovery-empty','Nenhuma regra correspondeu aos registros examinados.'));return;}
    const time=card(grid,'Indícios no tempo','Registros únicos por intervalo. Clique para consultar as correspondências naquele período.',true);
    if(data.time?.length){const bars=el('div','threat-bars'),max=Math.max(1,...data.time.map(b=>b.count));
      for(const bucket of data.time){const end=bucket.timestamp+(data.time_bucket_ms||1)-1;const b=button('',()=>records(scope,'*',[{column:'timestamp',op:'between',value:String(bucket.timestamp),value2:String(end)}],'Indícios no intervalo'),'threat-time-bin');b.style.setProperty('--height',String(100*bucket.count/max));b.disabled=bucket.count<=0;b.title=`${fmtTsFull(bucket.timestamp)} — ${fmtTsFull(end)} · ${fmtNum(bucket.count)} registros`;b.setAttribute('aria-label',b.title);bars.append(b);}time.append(bars,el('div','threat-time-labels',`${dashTimeLabel(data.time[0].timestamp)} — ${dashTimeLabel(data.time.at(-1).timestamp+(data.time_bucket_ms||1)-1)}`));
    }else time.append(el('p','discovery-empty','Os registros encontrados não têm horário reconhecido.'));
    if(data.undated_matches>0)time.append(el('p','discovery-foot threat-undated',`${fmtNum(data.undated_matches)} ${data.undated_matches===1?'registro sem horário':'registros sem horário'} · fora do gráfico.`));
    const category=card(grid,'Categorias','Contagens únicas dentro de cada categoria. Um registro pode aparecer em mais de uma categoria. Clique para focar a lista de regras.');
    const origins=card(grid,'Origens','Origens dos registros com indícios; clique para consultar as correspondências.');
    const rank=(box,items,action)=>{const max=Math.max(1,...items.map(i=>i.count));for(const item of items.slice(0,10)){const row=button('',()=>action(item.name),'threat-rank');row.append(el('span','',item.name));const track=el('i');track.style.setProperty('--width',`${100*item.count/max}%`);row.append(track,el('strong','',fmtNum(item.count)));box.append(row);}};
    let drawRules;
    rank(category,data.categories||[],name=>{view.category=view.category===name?'':name;view.page=0;drawRules?.();ruleBody.parentElement.scrollIntoView({block:'nearest',behavior:'smooth'});});
    rank(origins,data.sources||[],name=>records(scope,'*',[{column:'source',op:'equals_exact',value:name,value2:null}],`Indícios · ${name}`));
    const ruleBody=card(grid,'Regras encontradas','As contagens indicam registros por regra, não ataques confirmados. A lista pode ser filtrada sem repetir a varredura.',true);
    const ruleTools=el('div','threat-rule-tools'),search=el('input'),categorySelect=el('select'),severitySelect=el('select'),kindSelect=el('select');
    search.type='search';search.placeholder='Buscar regra…';search.setAttribute('aria-label','Buscar regra encontrada');search.value=view.search;
    function option(select,value,label){const o=el('option','',label);o.value=value;select.append(o);}
    categorySelect.setAttribute('aria-label','Categoria de ameaça');option(categorySelect,'','Todas as categorias');for(const name of [...new Set(data.rules.map(r=>r.category))].sort())option(categorySelect,name,name);
    if(![...categorySelect.options].some(o=>o.value===view.category))view.category='';
    severitySelect.setAttribute('aria-label','Prioridade da regra');option(severitySelect,'','Todas as prioridades');for(const [value,label]of Object.entries(severity))option(severitySelect,value,label);severitySelect.value=view.severity;
    kindSelect.setAttribute('aria-label','Tipo de sinal');for(const [value,label]of [['','Todos os tipos'],['attempt','Tentativas'],['response','Respostas'],['indicator','Indicadores']])option(kindSelect,value,label);kindSelect.value=view.kind;
    ruleTools.append(search,kindSelect,categorySelect,severitySelect);const list=el('div','threat-rule-list');ruleBody.append(ruleTools,list);
    drawRules=()=>{categorySelect.value=view.category;list.innerHTML='';const filtered=data.rules.filter(r=>(!view.category||r.category===view.category)&&(!view.severity||r.severity===view.severity)&&(!view.kind||r.kind===view.kind)&&`${r.name} ${r.category} ${r.id}`.toLocaleLowerCase().includes(view.search.toLocaleLowerCase()));
      const pages=Math.max(1,Math.ceil(filtered.length/30));view.page=Math.min(view.page,pages-1);
      for(const rule of filtered.slice(view.page*30,(view.page+1)*30)){const row=button('',()=>ruleDetails(rule,scope),'threat-rule');const label=el('span');label.append(el('strong','',rule.name),el('small','',`${rule.category} · ${kinds[rule.kind]||rule.kind}`));row.append(el('span',`threat-priority threat-${rule.severity}`,severity[rule.severity]||rule.severity),label,el('strong','',fmtNum(rule.count)));list.append(row);}
      if(!filtered.length)list.append(el('p','discovery-empty','Nenhuma regra com essa seleção.'));
      const pager=el('div','threat-pager'),previous=button('Anterior',()=>{view.page--;drawRules();}),next=button('Próxima',()=>{view.page++;drawRules();});previous.disabled=view.page===0;next.disabled=view.page===pages-1;pager.append(previous,el('span','',`${view.page+1} / ${pages} · ${filtered.length} regras`),next);list.append(pager);
    };
    search.oninput=()=>{view.search=search.value;view.page=0;drawRules();};categorySelect.onchange=()=>{view.category=categorySelect.value;view.page=0;drawRules();};severitySelect.onchange=()=>{view.severity=severitySelect.value;view.page=0;drawRules();};kindSelect.onchange=()=>{view.kind=kindSelect.value;view.page=0;drawRules();};drawRules();
  }
  window.Threats={render,records};
})();
