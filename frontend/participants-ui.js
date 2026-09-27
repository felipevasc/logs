/* The backend assigns roles. This view never infers an attacker from a random IP. */
window.ParticipantsUI=(()=>{
  const titles={attacker:'Atacante',victim:'Alvo',context:'Outros envolvidos'};
  const kinds={ip:'IP',host:'Host',user:'Usuário',domain:'Domínio',application:'Aplicação',account:'Conta / tenant',resource:'Recurso',asset_type:'Tipo de ativo'};
  const roles={source:'Origem observada',destination:'Destino observado',affected_asset:'Ativo associado à operação',identity_used:'Identidade utilizada',target_account:'Conta alvo',target_application:'Aplicação alvo',logging_application:'Componente que registrou a atividade',target_resource:'Recurso alvo',forwarded_unverified:'Informado por proxy · não validado',observer:'Observador / intermediário',logging_identity:'Identidade que registrou a operação',remote_endpoint:'Destino remoto · possível infraestrutura',asset_type:'Tipo registrado'};
  const escape=value=>esc(String(value??''));
  function asset(facts){
    const types=facts.filter(f=>f.kind==='asset_type').map(f=>f.value.toLowerCase());
    if(types.some(v=>['desktop','workstation','laptop','endpoint'].includes(v)))return ['endpoint','Estação de trabalho'];
    if(facts.some(f=>f.kind==='application'))return ['application','Aplicação / serviço'];
    if(types.some(v=>['server','servidor'].includes(v)))return ['server','Servidor'];
    return ['server','Ativo de TI'];
  }
  function card(profile,side){
    const own=(profile?.facts||[]).filter(f=>f.side===side),[art,type]=side==='attacker'?['attacker','Origem observada']:asset(own);
    const order=side==='attacker'?['ip','user','host','domain','account','application','resource']:['application','host','ip','user','domain','resource','account'];
    // Show one representative per kind; the inspector retains every value and origin.
    const shown=order.map(kind=>own.find(f=>f.kind===kind&&f.certainty!=='unverified')||own.find(f=>f.kind===kind)).filter(Boolean).slice(0,3);
    if(!shown.length)return '';
    const icons={ip:'fa-network-wired',host:'fa-server',user:'fa-user',domain:'fa-globe',account:'fa-building',application:'fa-window-maximize',resource:'fa-cube'};
    const additional=own.filter(f=>f.kind!=='asset_type'&&!shown.includes(f)).length;
    return `<button type="button" class="sec-persona-card persona-${side}" data-act="participants" data-side="${side}" aria-label="Ver informações de ${titles[side]}" title="${escape(type)} · Ver informações e procedência">
      <span class="sec-persona-heading"><img src="assets/personas/${art}.png" alt="" width="48" height="48" loading="lazy" decoding="async"><span class="sec-persona-role">${titles[side]}</span></span>
      <span class="sec-persona-values">${shown.map(f=>`<span class="sec-persona-value" title="${escape(kinds[f.kind])}: ${escape(f.value)} · ${escape(roles[f.role]||f.role)}${f.namespace?' · '+escape(f.namespace):''}"><i class="fas ${icons[f.kind]}" aria-hidden="true"></i><span>${escape(f.value)}</span>${f.certainty==='unverified'?'<small aria-label="IP não validado">?</small>':''}</span>`).join('')}</span>
      <span class="sec-persona-more">${additional?`+${additional} ${additional===1?'dado':'dados'} · `:''}Detalhes <i class="fas fa-angle-right" aria-hidden="true"></i></span>
    </button>`;
  }
  function cards(profile,content=''){
    const context=(profile?.facts||[]).some(f=>f.side==='context');
    const attacker=card(profile,'attacker'),target=card(profile,'victim');
    return `<section class="sec-personas${attacker?' has-attacker':''}${target?' has-target':''}" aria-label="Possível origem e alvo da atividade">${attacker}<div class="sec-attack-content">${content}${context||profile?.limited?`<div class="sec-personas-context">${profile?.limited?'<span>Resumo limitado</span>':''}${context?'<button class="text-button" data-act="participants" data-side="context">Observadores e contexto</button>':''}</div>`:''}</div>${target}</section>`;
  }
  function report(profile){
    if(!profile?.facts?.length)return '';
    return ['Participantes possíveis — identidade real e dano não confirmados.',...profile.facts.map(f=>`${titles[f.side]||f.side} · ${kinds[f.kind]||f.kind}: ${f.value} · ${roles[f.role]||f.role}${f.namespace?' · Escopo: '+f.namespace:''}\n${f.origins.map(o=>`${o.field} · evento ${o.event_ref} · ${o.method}`).join('; ')}${f.origins_limited?' · Outras origens nos registros':''}`),...(profile.limited?['Resumo de participantes limitado; consultar registros completos.']:[])].join('\n');
  }
  function inspect(profile,side,options){
    const focus=document.activeElement,facts=profile?.facts?.filter(f=>f.side===side)||[];
    const dialog=document.createElement('dialog');dialog.className='sec-persona-inspector';
    const art=side==='attacker'?'attacker':asset(facts)[0];
    dialog.innerHTML=`<header><img src="assets/personas/${art}.png" alt="" width="64" height="64"><div><h2>${titles[side]||'Participantes'}</h2><p>Informações observadas e sua procedência</p></div><button class="icon-btn" data-close aria-label="Fechar"><i class="fas fa-xmark" aria-hidden="true"></i></button></header><p class="sec-persona-note">Os valores pertencem aos eventos deste item. IPs, contas e hosts não identificam, por si só, uma pessoa. Identidades utilizadas também podem ter sido comprometidas.</p><div class="sec-persona-facts">${facts.length?facts.map((f,i)=>`<article><div class="sec-persona-fact-title"><span>${kinds[f.kind]||escape(f.kind)}</span><strong>${escape(f.value)}</strong><small>${escape(roles[f.role]||f.role)}${f.certainty==='contextual'?' · papel contextual':''}</small></div>${f.namespace?`<p>Escopo: <code>${escape(f.namespace)}</code></p>`:''}<details><summary>${f.origins.length} ${f.origins.length===1?'origem registrada':'origens registradas'}${f.origins_limited?' (amostra)':''}</summary>${f.origins.map((o,j)=>`<details class="sec-persona-origin" data-fact="${i}" data-origin="${j}"><summary><code>${escape(o.field)}</code><span>Evento ${escape(o.event_id)} · ${escape(o.method)}</span></summary><div class="sec-persona-original"></div></details>`).join('')}${f.origins_limited?'<p>As demais referências permanecem nas ocorrências e eventos do grupo.</p>':''}</details></article>`).join(''):'<p>Nenhum campo ou padrão com contexto suficiente para identificar este papel. Nenhum endereço foi inferido a partir da simples presença de um IP.</p>'}</div>${profile?.limited?'<p class="sec-persona-note">A extração ou o resumo atingiu seu limite. Consulte os eventos completos para as demais informações.</p>':''}`;
    const close=()=>{dialog.close();dialog.remove();if(focus?.isConnected)focus.focus();};
    dialog.querySelector('[data-close]').onclick=close;dialog.oncancel=e=>{e.preventDefault();close();};
    dialog.addEventListener('click',e=>{if(e.target===dialog){const r=dialog.getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)close();}});
    for(const details of dialog.querySelectorAll('.sec-persona-origin')){
      const origin=facts[+details.dataset.fact].origins[+details.dataset.origin];let loading=false,loaded=false;
      details.addEventListener('toggle',async()=>{
        if(!details.open||loading||loaded)return;loading=true;
        const body=details.querySelector('.sec-persona-original');body.textContent='Carregando evento original…';
        try{
          const event=await api('triage_evidence_event',{...options.request,analysisId:options.analysisId,eventId:origin.event_id,eventRef:origin.event_ref},{silent:true});
          if(!dialog.isConnected)return;
          if(!event)throw Error('Evento indisponível');
          const safe=EvidenceUI.redact(event);
          const rows=Object.entries(safe.fields||{}).map(([k,v])=>`<div><dt>${escape(k)}</dt><dd><pre>${escape(typeof v==='string'?v:JSON.stringify(v,null,2))}</pre></dd></div>`).join('');
          body.innerHTML=`<p><code>${escape(origin.event_ref)}</code></p><h4>Evento original</h4><pre>${escape(safe.message||'')}</pre><dl>${rows}</dl><details><summary>Conteúdo bruto</summary><pre>${escape(safe.raw||'(não registrado)')}</pre></details>`;
          loaded=true;
        }catch(error){body.textContent=`Não foi possível consultar o evento: ${error}`;}finally{loading=false;}
      });
    }
    document.body.append(dialog);dialog.showModal();dialog.querySelector('[data-close]').focus();
  }
  return {cards,inspect,report};
})();
