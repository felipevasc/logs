/* Preview fixtures for exact identifier grouping, never a causal model. */
(() => {
  const norm = field => field.toLowerCase().replace(/[^a-z0-9]/g, '');
  const kind = field => {
    const f = norm(field);
    if (f.endsWith('traceid')) return 'trace';
    if (f.endsWith('requestid') || f === 'reqid') return 'request';
    if (['correlationid','transactionid','activityid','operationid'].some(s => f.endsWith(s))) return 'correlation';
    if (f.endsWith('sessionid') || ['idsessao','sessaoid'].includes(f)) return 'session';
    if (['username','userid','userprincipalname','accountname','principalname'].some(s => f.endsWith(s)) || ['user','usuario','nomeusuario','idusuario'].includes(f)) return 'user';
    if (['sourceip','destinationip','clientip','remoteip','srcip','dstip','remoteaddr','sourceaddress','destinationaddress'].some(s => f.endsWith(s)) || ['ip','ipcliente','iporigem','ipdestino','enderecoip'].includes(f)) return 'ip';
    return 'custom';
  };
  const validate = field => { const f=norm(field);if(!field || field.length>256 || /[\x00-\x1f]/.test(field) || ['id','eventref','timestamp','raw','message','description'].includes(f) || f.endsWith('eventid') || f.endsWith('recordid')) throw Error('Escolha um campo de correlação; o identificador interno do registro e campos de texto/data não definem uma jornada.'); };
  const key = (event,field) => { const value=Object.hasOwn(event,field)?event[field]:event.fields?.[field];return value===null || value===undefined || typeof value==='object' || !String(value).trim()?null:String(value); };
  const bytes = value => new TextEncoder().encode(value).length;
  window.__mockJourneySeed = events => {
    for (const row of events) if(row.id<120) row.fields.correlation_id=`flow-${String(Math.floor(row.id/4)).padStart(3,'0')}`;
  };
  window.__mockJourneys = (command,args={},events=[],filterRows=(_,rows)=>rows) => {
    let rows=filterRows(args.filters||[],Array.isArray(args.caseEvents)?args.caseEvents:events);
    if(command==='journey_fields') {
      const fields=new Map();let totalBytes=0,limited=false;
      for(const event of rows) for(const [field,raw] of Object.entries(event.fields||{})) {
        try{validate(field);}catch{continue;}
        if(!['string','number'].includes(typeof raw))continue;
        const value=key(event,field);if(value===null)continue;
        if(!fields.has(field)&&fields.size>=128){limited=true;continue;}
        if(!fields.has(field))fields.set(field,{present:0,values:new Set(),limited:false});
        const acc=fields.get(field);acc.present++;
        if(!acc.values.has(value)){const size=bytes(value);if(acc.values.size>=512 || size>256 || totalBytes+size+64>8*1024*1024)acc.limited=true;else{acc.values.add(value);totalBytes+=size+64;}}
      }
      return [...fields].map(([field,acc])=>({field,label:field,kind:kind(field),suggested:['trace','request','correlation','session'].includes(kind(field)),coverage:acc.present/Math.max(1,rows.length),distinct:acc.limited?null:acc.values.size,distinct_limited:acc.limited,sampled:false,fields_limited:limited})).sort((a,b)=>Number(b.suggested)-Number(a.suggested)||b.coverage-a.coverage||a.field.localeCompare(b.field));
    }
    const field=args.field||'';validate(field);
    if(command==='journey_index') {
      if(['user','ip'].includes(kind(field))&&(args.from==null||args.to==null))throw Error('Investigações por usuário ou IP precisam de início e fim explícitos; use uma janela próxima ao evento.');
      if(args.from!=null&&args.to!=null&&args.from>args.to)throw Error('O início deve ser anterior ao fim.');
      if(args.from!=null||args.to!=null)rows=rows.filter(row=>row.timestamp!=null&&(args.from==null||row.timestamp>=args.from)&&(args.to==null||row.timestamp<=args.to));
      const groups=new Map();let missing_key=0,missing_time=0,skipped_keys=0;
      for(const row of rows){const value=key(row,field);if(value===null){missing_key++;continue;}if(bytes(value)>4096){skipped_keys++;continue;}
        if(!groups.has(value))groups.set(value,{value,count:0,start:null,end:null,errors:0,warnings:0,missing_time:0,sourceCounts:new Map()});
        const g=groups.get(value);g.count++;g.errors+=['Erro','Crítico'].includes(row.level);g.warnings+=row.level==='Aviso';g.sourceCounts.set(row.source,(g.sourceCounts.get(row.source)||0)+1);
        if(row.timestamp===null||row.timestamp===undefined){g.missing_time++;missing_time++;}else{g.start=g.start===null?row.timestamp:Math.min(g.start,row.timestamp);g.end=g.end===null?row.timestamp:Math.max(g.end,row.timestamp);}
      }
      const order=args.sort||'recent';if(!['recent','duration','count','errors'].includes(order))throw Error('Ordenação de jornadas inválida.');
      const out=[...groups.values()].filter(g=>args.includeSingles||g.count>=2).map(g=>{const sources=[...g.sourceCounts].sort((a,b)=>b[1]-a[1]||a[0].localeCompare(b[0])).map(x=>x[0]);delete g.sourceCounts;return {...g,duration_ms:g.start===null?null:g.end-g.start,sources:sources.slice(0,12),sources_limited:sources.length>12};});
      const number=(g,k)=>g[k]===null?-Infinity:g[k];out.sort((a,b)=>{const k=order==='recent'?'end':order==='duration'?'duration_ms':order;return number(b,k)-number(a,k)||(order==='duration'||order==='errors'?b.count-a.count:0)||(order==='count'?number(b,'end')-number(a,'end'):0)||a.value.localeCompare(b.value);});
      return {total:out.length,groups:out.slice(args.offset||0,(args.offset||0)+Math.max(1,Math.min(args.limit||50,200))),complete:true,field,missing_key,missing_time,skipped_keys};
    }
    if(command==='journey_events') {
      const value=args.value;if(typeof value!=='string'||!value.trim()||bytes(value)>4096)throw Error('Informe um identificador não vazio de até 4096 bytes.');
      if(['user','ip'].includes(kind(field))&&(args.from==null||args.to==null))throw Error('Investigações por usuário ou IP precisam de início e fim explícitos; use uma janela próxima ao evento.');
      if(args.from!=null&&args.to!=null&&args.from>args.to)throw Error('O início deve ser anterior ao fim.');
      const matched=rows.map((row,index)=>({row,index})).filter(({row})=>key(row,field)===value && ((args.from==null&&args.to==null)||(row.timestamp!=null&&(args.from==null||row.timestamp>=args.from)&&(args.to==null||row.timestamp<=args.to)))).sort((a,b)=>(a.row.timestamp??Infinity)-(b.row.timestamp??Infinity)||a.index-b.index).map(x=>x.row);
      return {total:matched.length,rows:matched.slice(args.offset||0,(args.offset||0)+Math.max(1,Math.min(args.limit||100,500))),complete:true,field,value,missing_time:matched.filter(e=>e.timestamp==null).length};
    }
    throw Error(`Comando de jornada desconhecido: ${command}`);
  };
})();
