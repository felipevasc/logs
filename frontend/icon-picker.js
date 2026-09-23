/* Local Font Awesome chooser. Metadata loads only when a note editor opens. */
window.NoteIconPicker=(()=>{
  'use strict';
  const PAGE=96,preferred=['comment','triangle-exclamation','shield-halved','user','users','server','computer','network-wired','wifi','database','cloud','key','lock','bug','terminal','code','file-lines','folder','link','magnifying-glass','clock','calendar','circle-info','flag'];
  let pending;
  const normalize=value=>String(value||'').normalize('NFD').replace(/[\u0300-\u036f]/g,'').toLowerCase();
  const make=(tag,className,text)=>{const node=document.createElement(tag);if(className)node.className=className;if(text!==undefined)node.textContent=text;return node;};
  const classes=value=>{const match=String(value||'').match(/^(?:(fas|fa-solid|fab|fa-brands)\s+)?fa-([a-z0-9-]+)$/);return match?`${['fab','fa-brands'].includes(match[1])?'fab':'fas'} fa-${match[2]}`:'';};
  const valueOf=icon=>`${icon.style==='brands'?'fab ':''}fa-${icon.name}`;
  const glyph=value=>{const icon=make('i',classes(value));icon.setAttribute('aria-hidden','true');return icon;};
  async function load(){
    pending||=fetch('vendor/fa/icon-catalog.json').then(response=>{if(!response.ok)throw Error('Catálogo de ícones indisponível.');return response.json();}).then(data=>{
      const icons=data.icons.map(icon=>({...icon,hay:normalize(`${icon.label} ${icon.search}`)}));
      icons.sort((a,b)=>{const pa=preferred.indexOf(a.name),pb=preferred.indexOf(b.name);return (pa<0?9999:pa)-(pb<0?9999:pb)||a.label.localeCompare(b.label,'pt-BR');});return icons;
    }).catch(error=>{pending=null;throw error;});
    return pending;
  }
  function mount(host,initial='fa-comment'){
    host.classList.add('ct-icon-picker');host.textContent='';
    const input=make('input');input.type='hidden';input.name='icon';input.value=String(initial??'');
    const heading=make('div','ct-icon-heading'),selection=make('span','ct-icon-selection'),none=make('button','btn ghost small','Sem ícone');none.type='button';
    heading.append(selection,none);host.append(input,heading);
    let icons=[],page=0,all=[],active=0,timer;
    const tools=make('div','ct-icon-tools'),search=make('input'),collection=make('select'),category=make('select');
    search.type='search';search.placeholder='Buscar ícone: servidor, usuário, rede…';search.setAttribute('aria-label','Buscar ícone');
    collection.setAttribute('aria-label','Coleção de ícones');category.setAttribute('aria-label','Categoria de ícones');
    const option=(select,value,label)=>{const node=make('option','',label);node.value=value;select.append(node);};
    option(collection,'solid','Ícones');option(collection,'brands','Marcas');collection.value=classes(input.value).startsWith('fab')?'brands':'solid';
    option(category,'','Todas as categorias');tools.append(search,collection,category);host.append(tools);
    const grid=make('div','ct-icon-grid');grid.setAttribute('role','group');grid.setAttribute('aria-label','Matriz de ícones');
    const pager=make('div','ct-icon-pager'),previous=make('button','btn ghost small','Anterior'),next=make('button','btn ghost small','Próxima'),status=make('span');
    previous.type=next.type='button';previous.setAttribute('aria-label','Página anterior de ícones');next.setAttribute('aria-label','Próxima página de ícones');status.setAttribute('role','status');status.setAttribute('aria-live','polite');
    pager.append(previous,status,next);host.append(grid,pager);
    function selectedValue(){
      const match=String(input.value).match(/fa-([a-z0-9-]+)$/),style=classes(input.value).startsWith('fab')?'brands':'solid';
      const icon=icons.find(icon=>icon.style===style&&(icon.name===match?.[1]||icon.aliases.includes(match?.[1])));
      return icon?valueOf(icon):input.value;
    }
    function updateSelection(){
      selection.textContent='';
      const match=String(input.value).match(/fa-([a-z0-9-]+)$/),name=match?.[1],style=classes(input.value).startsWith('fab')?'brands':'solid';
      const icon=icons.find(icon=>icon.style===style&&(icon.name===name||icon.aliases.includes(name)));
      if(classes(input.value))selection.append(glyph(input.value));
      selection.append(make('span','',input.value?icon?.label||name||'Ícone salvo':'Sem ícone'));none.setAttribute('aria-pressed',String(!input.value));
      const selected=selectedValue();
      for(const button of grid.querySelectorAll('button'))button.setAttribute('aria-pressed',String(button.dataset.value===selected));
    }
    function select(value){input.value=value;updateSelection();input.dispatchEvent(new Event('change',{bubbles:true}));}
    none.onclick=()=>select('');updateSelection();grid.textContent='Carregando ícones…';
    function focusAt(index){const buttons=[...grid.querySelectorAll('button')];if(!buttons.length)return;active=Math.max(0,Math.min(buttons.length-1,index));buttons.forEach((button,i)=>button.tabIndex=i===active?0:-1);buttons[active].focus();buttons[active].scrollIntoView({block:'nearest'});}
    function render(focus=false,target=0){
      if(!host.isConnected)return;
      const terms=normalize(search.value).trim().split(/\s+/).filter(Boolean);
      all=icons.filter(icon=>icon.style===collection.value&&(!category.value||icon.groups.includes(category.value))&&terms.every(term=>icon.hay.includes(term)));
      const pages=Math.max(1,Math.ceil(all.length/PAGE));page=Math.min(page,pages-1);grid.textContent='';
      const selected=selectedValue(),slice=all.slice(page*PAGE,(page+1)*PAGE);active=Math.max(0,slice.findIndex(icon=>valueOf(icon)===selected));
      for(const [index,icon]of slice.entries()){
        const value=valueOf(icon),button=make('button','ct-icon-choice');button.type='button';button.dataset.value=value;button.title=`${icon.label} · ${icon.en} (${icon.name})`;button.setAttribute('aria-label',`${icon.label} · ${icon.en}`);button.setAttribute('aria-pressed',String(value===selected));button.tabIndex=index===active?0:-1;button.append(glyph(value));button.onclick=()=>{active=index;select(value);};button.onfocus=()=>{active=index;for(const other of grid.querySelectorAll('button'))other.tabIndex=other===button?0:-1;};grid.append(button);
      }
      if(!slice.length)grid.append(make('p','ct-icon-empty','Nenhum ícone. Tente outro nome ou categoria.'));
      previous.disabled=page===0;next.disabled=page===pages-1;status.textContent=`${all.length.toLocaleString('pt-BR')} ícones · ${page+1} / ${pages}`;grid.scrollTop=0;
      grid.setAttribute('aria-busy','false');
      if(focus)focusAt(target);
    }
    function changePage(delta,focus=false){page+=delta;render(focus,delta>0?0:PAGE-1);}
    previous.onclick=()=>changePage(-1);next.onclick=()=>changePage(1);
    search.oninput=()=>{grid.setAttribute('aria-busy','true');clearTimeout(timer);timer=setTimeout(()=>{page=0;render();},100);};
    collection.onchange=()=>{page=0;category.value='';category.disabled=collection.value==='brands';render();};category.onchange=()=>{page=0;render();};
    grid.onkeydown=event=>{
      const length=grid.querySelectorAll('button').length;if(!length)return;
      const columns=getComputedStyle(grid).gridTemplateColumns.split(/\s+/).length;
      let target=active;
      if(event.key==='ArrowRight')target++;else if(event.key==='ArrowLeft')target--;else if(event.key==='ArrowDown')target+=columns;else if(event.key==='ArrowUp')target-=columns;
      else if(event.key==='Home')target=0;else if(event.key==='End')target=length-1;
      else if(event.key==='PageDown'){event.preventDefault();if(!next.disabled)changePage(1,true);return;}
      else if(event.key==='PageUp'){event.preventDefault();if(!previous.disabled)changePage(-1,true);return;}else return;
      event.preventDefault();event.stopPropagation();focusAt(target);
    };
    load().then(data=>{
      if(!host.isConnected)return;icons=data;
      collection.options[0].textContent=`Ícones (${icons.filter(icon=>icon.style==='solid').length.toLocaleString('pt-BR')})`;
      collection.options[1].textContent=`Marcas (${icons.filter(icon=>icon.style==='brands').length.toLocaleString('pt-BR')})`;
      for(const group of [...new Set(icons.filter(icon=>icon.style==='solid').flatMap(icon=>icon.groups))].sort((a,b)=>a.localeCompare(b,'pt-BR')))option(category,group,group);
      category.disabled=collection.value==='brands';
      // Open the saved glyph's page, including legacy aliases, without rewriting it.
      if(!search.value&&!category.value){const index=icons.filter(icon=>icon.style===collection.value).findIndex(icon=>valueOf(icon)===selectedValue());if(index>=0)page=Math.floor(index/PAGE);}
      updateSelection();render();
    }).catch(error=>{if(host.isConnected){grid.textContent=error.message;status.textContent='Seu ícone salvo foi preservado.';previous.disabled=next.disabled=true;}});
    return input;
  }
  return {mount,classes};
})();
