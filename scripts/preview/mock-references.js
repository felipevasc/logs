/* UI fixture for immutable JSONL references; native tests own disk semantics. */
window.createMockReferences = ({ contextFor, identity, persist, columnText }) => {
  const stores = new Map(), copy = value => structuredClone(value), encoder = new TextEncoder(); let serial = 0;
  const ownerKey = context => JSON.stringify([context.caseId, context.analysisId]);
  const defaultPath = 'C:\\mock\\services.jsonl';
  const defaultText = ['API','Worker','Auth','Scheduler','Firewall'].map(service => JSON.stringify({service,environment:'prod',team:`Equipe ${service}`})).join('\n')+'\n';
  const fileText = path => Object.hasOwn(window.__mockReferenceFiles || {}, path) ? window.__mockReferenceFiles[path] : path === defaultPath ? defaultText : undefined;
  const digest = async bytes => [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(value=>value.toString(16).padStart(2,'0')).join('');
  function rejectDuplicateColumns(line) {
    let depth=0,expectKey=false;const names=new Set();
    for(let i=0;i<line.length;i++){
      const char=line[i];
      if(char==='"'){
        const start=i;for(i++;i<line.length;i++){if(line[i]==='\\'){i++;continue;}if(line[i]==='"')break;}
        if(depth===1&&expectKey){const name=JSON.parse(line.slice(start,i+1));if(names.has(name))throw Error('Coluna JSON duplicada na referência.');names.add(name);expectKey=false;}
      }else if(char==='{'||char==='['){depth++;if(depth===1)expectKey=true;}
      else if(char==='}'||char===']')depth--;
      else if(char===','&&depth===1)expectKey=true;
    }
  }
  async function inspect(path) {
    const text = fileText(path); if (typeof text !== 'string') throw Error('Não foi possível abrir a referência do preview.');
    const bytes = encoder.encode(text); if (bytes.length > 8*1024*1024) throw Error('Esta etapa aceita referências JSONL de até 8 MiB.');
    const rows = []; let start=0, columns;
    while (start<text.length) {
      const newline=text.indexOf('\n',start),end=newline<0?text.length:newline+1,line=text.slice(start,end);start=end;
      if (encoder.encode(line).length>512*1024) throw Error('Um registro de referência excede 512 KiB.');
      if (rows.length>=100000) throw Error('Esta etapa aceita até 100.000 registros de referência.');
      let row;try{row=JSON.parse(line);}catch{throw Error(`JSON inválido na linha ${rows.length+1}.`);}
      if (!row||typeof row!=='object'||Array.isArray(row)) throw Error('Cada linha deve conter um objeto JSON.');
      rejectDuplicateColumns(line);
      const names=Object.keys(row).sort();if(!names.length||names.length>1024||names.some(name=>!name.trim()||encoder.encode(name).length>4096))throw Error('Colunas inválidas.');
      if(columns&&JSON.stringify(names)!==JSON.stringify(columns))throw Error('Todas as linhas devem ter as mesmas colunas.');
      columns=names;rows.push(row);
    }
    if(!rows.length)throw Error('A referência está vazia.');
    return {inspection:{name:path.split(/[\\/]/).at(-1),format:'jsonl',contentSha256:await digest(bytes),columns,rowCount:rows.length,sourceBytes:bytes.length},rows};
  }
  const encode = values => {
    if(values.some(value=>value===null||value===undefined||typeof value==='object'||typeof value==='number'&&!Number.isFinite(value)))throw Error('A chave deve conter escalares JSON não nulos, na ordem declarada.');
    const key=JSON.stringify(values);if(encoder.encode(key).length>64*1024)throw Error('A chave da referência excede 64 KiB.');return key;
  };
  const snapshot = context => {context.configRevision++;persist(context);return{analysisContext:copy(context)};};
  const stored = (context,id) => stores.get(ownerKey(context))?.get(id);
  function validateLookup(context,lookup) {
    const reference=context.config.references.find(reference=>reference.id===lookup?.referenceId);
    if(lookup?.schemaVersion!==1||!reference||!reference.columns.includes(lookup.valueColumn)||!Array.isArray(lookup.keys)||lookup.keys.length!==reference.keyColumns.length
      ||lookup.keys.some((key,index)=>key.referenceColumn!==reference.keyColumns[index]||typeof key.sourceField!=='string'||!key.sourceField.trim()))throw Error('Mapeamento de referência inválido.');
    return reference;
  }
  const handlers = {
    reference_inspect: async args => {contextFor(args);const result=await inspect(args.path);contextFor(args);return result.inspection;},
    reference_import: async args => {
      const context=contextFor(args),checked=await inspect(args.path);contextFor(args);
      if(JSON.stringify(checked.inspection)!==JSON.stringify(args.inspection))throw Error('A referência mudou depois da inspeção. Inspecione novamente.');
      if(!args.name?.trim()||!args.keyColumns?.length||args.keyColumns.length>16||new Set(args.keyColumns).size!==args.keyColumns.length||args.keyColumns.some(key=>!checked.inspection.columns.includes(key)))throw Error('Escolha colunas de chave distintas presentes na referência.');
      const table=new Map();for(const row of checked.rows){const key=encode(args.keyColumns.map(column=>row[column]));if(table.has(key))throw Error('Chave de referência duplicada; preparação descartada.');table.set(key,copy(row));}
      const reference={schemaVersion:1,id:`preview-reference-${++serial}`,name:args.name.trim(),contentSha256:checked.inspection.contentSha256,format:'jsonl',columns:checked.inspection.columns,keyColumns:[...args.keyColumns],duplicatePolicy:'reject'};
      if(!stores.has(ownerKey(context)))stores.set(ownerKey(context),new Map());
      stores.get(ownerKey(context)).set(reference.id,{descriptor:copy(reference),table,inspection:checked.inspection,available:true});
      context.config.references.push(reference);return{...snapshot(context),reference:copy(reference),prepared:{rowCount:checked.inspection.rowCount,sourceBytes:checked.inspection.sourceBytes}};
    },
    reference_list: args => {
      const context=contextFor(args);return{analysisContext:identity(context),references:context.config.references.map(descriptor=>{
        const value=stored(context,descriptor.id),available=!!value?.available&&value.descriptor.contentSha256===descriptor.contentSha256;
        return{descriptor:copy(descriptor),available,rowCount:available?value.inspection.rowCount:null,sourceBytes:available?value.inspection.sourceBytes:null,reason:available?null:'Conteúdo da referência indisponível para este Caso.'};
      })};
    },
    reference_remove: args => {
      const context=contextFor(args);if(context.config.derivedFields.some(field=>field.lookup?.referenceId===args.referenceId))throw Error('A referência ainda é usada por um campo derivado.');
      context.config.references=context.config.references.filter(reference=>reference.id!==args.referenceId);return snapshot(context);
    },
    reference_save_lookup: args => {
      const context=contextFor(args);validateLookup(context,args.lookup);if(!args.name?.trim())throw Error('Informe o nome do campo.');
      const index=context.config.derivedFields.findIndex(field=>field.name===args.name),prior=context.config.derivedFields[index];
      if(args.lookup.keys.some(key=>key.sourceField===args.name))throw Error('O campo não pode consultar sua própria saída.');
      const field={id:prior?.id||`preview-field-${++serial}`,name:args.name,lookup:copy(args.lookup)};
      if(index<0)context.config.derivedFields.push(field);else context.config.derivedFields[index]=field;return snapshot(context);
    },
  };
  const canonical = new Set(['id','event_ref','timestamp','source','level','code','name','description','message','raw']);
  function sourceValue(event,field) {
    if(!canonical.has(field)&&Object.hasOwn(event.fields||{},field))return event.fields[field];
    if(field==='timestamp'&&event.timestamp==null)return undefined;
    if(canonical.has(field))return columnText(event,field);
    if(field.startsWith('@')&&window.QueryLang)return window.QueryLang.fieldValue(event,window.QueryLang.resolve(field))??undefined;
    return undefined;
  }
  function prepare(context) {
    const bindings=new Map();
    for(const field of context.config.derivedFields.filter(field=>field.lookup)){
      const descriptor=validateLookup(context,field.lookup),value=stored(context,descriptor.id);
      if(!value?.available||value.descriptor.contentSha256!==descriptor.contentSha256)throw Error(`Referência indisponível: ${descriptor.name}.`);
      const table=value.table,keys=copy(field.lookup.keys),column=field.lookup.valueColumn;
      bindings.set(field.name,event=>{const values=keys.map(key=>sourceValue(event,key.sourceField));if(values.some(value=>value===undefined))return{matched:false};const row=table.get(encode(values));return row?{matched:true,value:copy(row[column])}:{matched:false};});
    }
    return bindings;
  }
  return {handlers,prepare,defaultPath,setAvailable:(analysis,id,available)=>{const value=stored(analysis,id);if(value)value.available=available;}};
};
