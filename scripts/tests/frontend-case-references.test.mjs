import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/case-references.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const settle=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const inspection=()=>({name:'hosts.jsonl',format:'jsonl',contentSha256:'a'.repeat(64),columns:['host','env','owner'],rowCount:3,sourceBytes:123});
const descriptor=(id='ref-one')=>({schemaVersion:1,id,name:'Hosts',format:'jsonl',contentSha256:'a'.repeat(64),columns:['host','env','owner'],keyColumns:['host','env'],duplicatePolicy:'reject'});
function fixture(){
  const nodes=new Map(),calls=[],messages=[],order=[],cancelled=[],handlers=new Map(),document={activeElement:null};
  const state={columns:['host.name','environment','message'],derivedFields:[],rows:[],owner:{caseId:'case-a',instance:1,identity:{caseId:'case-a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-seven'}};
  let entries=[{descriptor:descriptor(),available:true,rowCount:3,sourceBytes:123,reason:null}],scope='dataset',reload=true,dialogPath='/fixtures/hosts.jsonl';
  function node(tag='div',className='',textContent=''){
    const classes=new Set(),item={tag,className,textContent,hidden:false,disabled:false,isConnected:true,value:'',children:[],attrs:{},parent:null,
      classList:{toggle(name,on){if(on)classes.add(name);else classes.delete(name);},contains:name=>classes.has(name)},
      get id(){return this.attrs.id;},set id(value){this.attrs.id=value;nodes.set(`#${value}`,this);},
      setAttribute(name,value){this.attrs[name]=String(value);},getAttribute(name){return this.attrs[name];},
      append(...children){for(const child of children){child.parent=this;this.children.push(child);}},replaceChildren(...children){this.children=[];this.append(...children);},
      querySelector:selector=>nodes.get(selector),querySelectorAll(selector){const out=[];const visit=parent=>{for(const child of parent.children){if(selector.split(',').includes(child.tag))out.push(child);visit(child);}};visit(this);return out;},
      addEventListener(){},matches:selector=>selector.split(',').includes(tag),focus(){document.activeElement=this;},
      closest(selector){for(let parent=this;parent;parent=parent.parent)if(selector.includes('[hidden]')&&parent.hidden)return parent;return null;},
      set innerHTML(html){this.children=[];for(const match of html.matchAll(/<(\w+)\b([^>]*\bid="([^"]+)"[^>]*)>/g)){const child=node(match[1]);child.id=match[3];child.hidden=/\bhidden\b/.test(match[2]);this.append(child);}},
    };return item;
  }
  document.body=node('body');const $=selector=>{if(!nodes.has(selector))nodes.set(selector,node('button'));return nodes.get(selector);};
  const capture=()=>structuredClone(state.owner),assertOwner=(owner,{revisions=true}={})=>{
    if(owner.caseId!==state.owner.caseId||owner.instance!==state.owner.instance||owner.identity.analysisId!==state.owner.identity.analysisId||owner.sourceKey!==state.owner.sourceKey||revisions&&JSON.stringify(owner.identity)!==JSON.stringify(state.owner.identity))throw Error('ANALYSIS_CONTEXT_CHANGED');};
  const snapshot=()=>({schemaVersion:1,...structuredClone(state.owner.identity),config:{derivedFields:structuredClone(state.derivedFields),references:entries.map(entry=>structuredClone(entry.descriptor))},migrationDiagnostics:[]});
  const receipt=()=>{state.owner.identity.configRevision++;return{analysisContext:snapshot()};};
  const native=(cmd,args)=>{
    if(cmd==='reference_list')return{analysisContext:structuredClone(state.owner.identity),references:structuredClone(entries)};
    if(cmd==='reference_inspect')return inspection();
    if(cmd==='reference_import'){const reference={...descriptor(`ref-${entries.length+1}`),name:args.name,columns:args.inspection.columns,keyColumns:args.keyColumns};entries.push({descriptor:reference,available:true,rowCount:3,sourceBytes:123,reason:null});return{...receipt(),reference,prepared:{rowCount:3,sourceBytes:123}};}
    if(cmd==='reference_remove'){entries=entries.filter(entry=>entry.descriptor.id!==args.referenceId);return receipt();}
    if(cmd==='reference_save_lookup'){const old=state.derivedFields.find(field=>field.name===args.name);state.derivedFields=state.derivedFields.filter(field=>field.name!==args.name);state.derivedFields.push({id:old?.id||'lookup-id',name:args.name,lookup:structuredClone(args.lookup)});return receipt();}
    if(cmd==='delete_derived_field'){state.derivedFields=state.derivedFields.filter(field=>field.name!==args.name);return receipt();}
    if(cmd==='profile_fields')return ['host.name','environment','message',...state.derivedFields.flatMap(field=>[field.name,`${field.name}.child`])].map(name=>({name}));
    throw Error(`Unexpected command ${cmd}`);
  };
  const context=vm.createContext({state,document,$,el:node,structuredClone,fmtNum:String,trunc:(value,size)=>String(value).slice(0,size),toast:message=>messages.push(message),
    activeCase:()=>({id:state.owner.caseId,name:'Captured Case'}),workspaceScope:()=>scope,caseEvents:()=>state.rows,caseSig:()=>JSON.stringify(state.rows),
    window:{AnalysisContexts:{capture,assertOwner,context:snapshot,signature:value=>JSON.stringify(value),prepare:async owner=>{assertOwner(owner);return owner;},refresh:async()=>({snapshot:snapshot()}),validSnapshot:value=>value?.schemaVersion===1&&!!value.config},Tasks:{cancelLatest:key=>cancelled.push(key)}},
    dialogApi:{open:async options=>{calls.push({cmd:'dialog',args:options});return typeof dialogPath==='function'?dialogPath():dialogPath;}},
    api:async(cmd,args,opts)=>{calls.push({cmd,args,opts});order.push(cmd);return handlers.has(cmd)?handlers.get(cmd)(args,opts):native(cmd,args);},
    loadDerivedFields:async owner=>{assertOwner(owner);order.push('loadDerivedFields');return reload;},
    includeDiscoveredFields:profiles=>{order.push('includeDiscoveredFields');state.columns=[...new Set([...state.columns,...profiles.map(profile=>profile.name)])];},
    fillColumnControls(){},renderExploreTree(){},refresh:async()=>{order.push('refresh');return true;},
  });
  vm.runInContext(source,context);
  const field=name=>nodes.get(`#rf-${name}`),controller=context.window.CaseReferences;
  const addKey=value=>{field('key-choice').value=value;field('add-key').onclick();};
  const map=(index,value)=>{const input=field('mappings').children[index].children.find(child=>child.tag==='select');input.value=value;input.onchange();};
  return{context,controller,state,field,document,$,calls,messages,order,cancelled,native,receipt,capture,addKey,map,
    on:(cmd,handler)=>handlers.set(cmd,handler),setEntries:value=>{entries=value;},setReload:value=>{reload=value;},setScope:value=>{scope=value;},setDialog:value=>{dialogPath=value;}};
}
test('manager uses only the captured Case list and distinguishes unavailable references',async()=>{
  const f=fixture();f.setEntries([{descriptor:descriptor(),available:false,rowCount:null,sourceBytes:null,reason:'Verified content absent'}]);await f.controller.openManager();
  assert.equal(f.calls[0].cmd,'reference_list');assert.equal(f.calls[0].opts.analysisOwner.instance,1);
  const card=f.field('list').children[0];assert.match(card.children[1].textContent,/Indisponível.*Verified content absent/);
  assert.equal(card.children.find(child=>child.className==='rf-tools').children[0].disabled,true);
  assert.equal(f.calls.some(call=>call.cmd==='profile_fields'),false,'opening metadata does not run data queries');
});
test('import sends exact inspection, path and explicitly ordered keys without requests while editing',async()=>{
  const f=fixture();await f.controller.openManager();await f.controller.chooseFile();
  const inspected=f.calls.find(call=>call.cmd==='reference_inspect');assert.equal(inspected.args.path,'/fixtures/hosts.jsonl');
  assert.deepEqual(plain(f.calls.find(call=>call.cmd==='dialog').args.filters),[{name:'Referência JSONL',extensions:['jsonl']}]);
  f.addKey('env');f.addKey('host');f.addKey('env');assert.equal(f.field('keys').children.length,2);
  assert.equal(f.field('keys').children[0].children[1].disabled,true,'the first key cannot move above the list');
  f.field('keys').children[1].children[1].onclick();
  const before=f.calls.length;f.field('import-name').value='Ordered reference';assert.equal(f.calls.length,before);
  await f.controller.importReference();const saved=f.calls.find(call=>call.cmd==='reference_import');
  assert.deepEqual(plain(saved.args.inspection),inspection());assert.deepEqual(plain(saved.args.keyColumns),['host','env']);assert.equal(saved.args.name,'Ordered reference');
  assert.equal(f.field('import-pane').hidden,true);assert.equal(f.field('list').children.length,2);
  assert.ok(f.order.indexOf('loadDerivedFields')<f.order.lastIndexOf('reference_list'));
});
test('stale dialog and inspect replies never import into a replacement Case or source',async()=>{
  const f=fixture(),gate=deferred();await f.controller.openManager();f.setDialog(()=>gate.promise);const choose=f.controller.chooseFile();
  f.state.owner.instance++;gate.resolve('/fixtures/late.jsonl');await choose;assert.equal(f.calls.some(call=>call.cmd==='reference_inspect'),false);
  const moved=fixture(),response=deferred();await moved.controller.openManager();moved.on('reference_inspect',()=>response.promise);const read=moved.controller.inspectFile('/fixtures/a.jsonl');await settle();moved.state.owner.sourceKey='new-source';response.resolve(inspection());await read;
  assert.equal(moved.field('import-pane').hidden,true);assert.match(moved.field('status').textContent,/CONTEXT_CHANGED/);
});
test('unconfirmed imports keep their draft and reconcile a saved descriptor before another import',async()=>{
  const f=fixture();await f.controller.openManager();await f.controller.inspectFile('/fixtures/hosts.jsonl');f.addKey('host');f.field('import-name').value='Exact attempt';
  f.on('reference_import',args=>{f.native('reference_import',args);throw Error('Receipt lost');});await f.controller.importReference();
  assert.equal(f.field('import-name').value,'Exact attempt');assert.equal(f.field('import').disabled,true);assert.match(f.field('status').textContent,/Atualize as referências/);
  await f.controller.importReference();assert.equal(f.calls.filter(call=>call.cmd==='reference_import').length,1);
  await f.controller.refreshList({reconcile:true});assert.match(f.field('status').textContent,/Importação confirmada/);assert.equal(f.field('import-pane').hidden,true);
  assert.equal(f.calls.filter(call=>call.cmd==='reference_import').length,1);
});
test('lookup requires every explicit mapping, uses ordered typed-field names and refreshes discovery after receipt',async()=>{
  const f=fixture();await f.controller.openLookup(null,{referenceId:'ref-one'});f.field('field-name').value='enriched';f.field('value-column').value='owner';
  await f.controller.saveLookup();assert.equal(f.calls.some(call=>call.cmd==='reference_save_lookup'),false);assert.match(f.field('status').textContent,/Mapeie todas/);
  f.map(0,'host.name');f.map(1,'environment');await f.controller.saveLookup();const request=f.calls.find(call=>call.cmd==='reference_save_lookup');
  assert.deepEqual(plain(request.args.lookup),{schemaVersion:1,referenceId:'ref-one',keys:[{referenceColumn:'host',sourceField:'host.name'},{referenceColumn:'env',sourceField:'environment'}],valueColumn:'owner'});
  assert.ok(f.order.indexOf('loadDerivedFields')<f.order.indexOf('profile_fields'));assert.ok(f.state.columns.includes('enriched.child'));
});
test('failed lookup save keeps owner and mapping draft; committed refresh failure never saves twice',async()=>{
  const f=fixture();await f.controller.openLookup(null,{referenceId:'ref-one'});f.field('field-name').value='enriched';f.field('value-column').value='owner';f.map(0,'host.name');f.map(1,'environment');
  f.on('reference_save_lookup',()=>{throw Error('CAS rejected');});await f.controller.saveLookup();assert.equal(f.field('field-name').value,'enriched');assert.match(f.field('status').textContent,/CAS rejected/);
  f.on('reference_save_lookup',args=>f.native('reference_save_lookup',args));f.setReload(false);await f.controller.saveLookup();assert.match(f.field('save-lookup').textContent,/Atualizar campos/);
  f.setReload(true);await f.controller.saveLookup();assert.equal(f.calls.filter(call=>call.cmd==='reference_save_lookup').length,2,'one failed save, one committed save, then refresh only');
});
test('unavailable and dependent references stay distinct from an unmatched lookup',async()=>{
  const f=fixture();f.setEntries([{descriptor:descriptor(),available:false,rowCount:null,sourceBytes:null,reason:'Unavailable'}]);await f.controller.openLookup({id:'stable-id',name:'enriched',lookup:{referenceId:'ref-one',keys:[{referenceColumn:'host',sourceField:'host.name'},{referenceColumn:'env',sourceField:'environment'}],valueColumn:'owner'}});
  assert.equal(f.field('field-name').disabled,true);assert.equal(f.field('save-lookup').disabled,true);assert.match(f.field('availability').textContent,/não será tratado.*sem correspondência/);
  await f.controller.saveLookup();assert.equal(f.calls.some(call=>call.cmd==='reference_save_lookup'),false);
  f.state.derivedFields=[{name:'enriched',lookup:{referenceId:'ref-one'}}];await f.controller.openManager();await f.controller.removeReference('ref-one');
  assert.equal(f.calls.some(call=>call.cmd==='reference_remove'),false);assert.match(f.field('status').textContent,/enriched/);
});
test('literal prototype-shaped key names are mapped safely and source changes reject delayed save actions',async()=>{
  const f=fixture(),reference={...descriptor(),columns:['__proto__','owner'],keyColumns:['__proto__']};f.setEntries([{descriptor:reference,available:true,rowCount:1,sourceBytes:22,reason:null}]);
  await f.controller.openLookup(null,{referenceId:'ref-one'});f.field('field-name').value='safe';f.field('value-column').value='owner';f.map(0,'host.name');await f.controller.saveLookup();
  assert.equal(f.calls.find(call=>call.cmd==='reference_save_lookup').args.lookup.keys[0].referenceColumn,'__proto__');
  const stale=fixture();await stale.controller.openLookup(null,{referenceId:'ref-one'});stale.field('field-name').value='blocked';stale.field('value-column').value='owner';stale.map(0,'host.name');stale.map(1,'environment');stale.state.owner.identity.visibilityRevision++;
  await stale.controller.saveLookup();assert.equal(stale.calls.some(call=>call.cmd==='reference_save_lookup'),false);
});
test('removing a lookup field preserves the reference and refreshes its dependent column catalog',async()=>{
  const f=fixture(),definition={id:'stable-id',name:'enriched',lookup:{schemaVersion:1,referenceId:'ref-one',keys:[{referenceColumn:'host',sourceField:'host.name'},{referenceColumn:'env',sourceField:'environment'}],valueColumn:'owner'}};
  f.state.derivedFields=[definition];f.state.columns.push('enriched','enriched.child');await f.controller.openLookup(definition);await f.controller.deleteLookup();
  assert.equal(f.calls.find(call=>call.cmd==='delete_derived_field').args.name,'enriched');assert.equal(f.calls.some(call=>call.cmd==='reference_remove'),false);assert.equal(f.state.columns.includes('enriched'),false);
  await f.controller.openManager();await f.controller.removeReference('ref-one');assert.ok(f.calls.some(call=>call.cmd==='reference_remove'));
});
test('Cancel restores a usable focus target and context menu ownership is captured before invocation',async()=>{
  const f=fixture(),anchor={isConnected:true,hidden:false,disabled:false,matches:()=>true,closest:()=>null,focus:()=>{f.document.activeElement=anchor;}};
  await f.controller.openManager({anchor});f.controller.close();assert.equal(f.document.activeElement,anchor);
  const item=f.controller.lookupMenuItem('host.name',anchor);f.state.owner.instance++;await item.onClick();assert.equal(f.calls.filter(call=>call.cmd==='reference_list').length,1);
  assert.match(f.messages.at(-1),/Caso mudou/);
});
