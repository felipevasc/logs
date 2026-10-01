import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const app=read('app.js');
const section=(start,end)=>app.slice(app.indexOf(start),app.indexOf(end,app.indexOf(start)));
const plain=value=>JSON.parse(JSON.stringify(value));
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const settle=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};

// Install the actual field tree, entrypoint handlers and canonical controller.
// DOM endpoints and native I/O remain controllable so late replies are explicit.
function fixture({column='value',value='display value',canonical='native value',valueType='string',historical=false}={}){
  const nodes=[],selectors=new Map(),calls={native:[],prepares:[],filters:[],drafts:[],copies:[],notices:[],menus:[],focus:[]},hooks={};
  const node=(tag,cls='',text='')=>{
    const item={tag,cls,textContent:text,children:[],dataset:{},attrs:{},isConnected:true,hidden:false,
      classList:{toggle(){}},setAttribute(key,value){this.attrs[key]=value;},getAttribute(key){return this.attrs[key];},
      append(...children){this.children.push(...children);},appendChild(child){this.children.push(child);return child;},
      contains(other){return this===other||this.children.some(child=>child.contains?.(other));},focus(){calls.focus.push(this);},
    };nodes.push(item);return item;
  };
  const $=selector=>{if(!selectors.has(selector))selectors.set(selector,node('div',selector));return selectors.get(selector);};
  const state={columns:[column],visibleCols:[],currentDetailEv:{id:7,event_ref:'source:7',fields:{[column]:value}},
    owner:{caseId:'case-a',instance:1,identity:{caseId:'case-a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-7'}};
  const capture=()=>structuredClone(state.owner),isCurrent=owner=>JSON.stringify(owner)===JSON.stringify(state.owner);
  const reply=args=>({kind:'exact_field',version:1,row:{id:args.id,eventRef:args.eventRef},column:args.column,
    presence:'present',valueType,canonicalText:canonical,
    receipt:{analysisContext:args.analysisContext,sourceGeneration:args.sourceGeneration,caseKey:null,caseContentToken:null,catalogSignature:'a'.repeat(64),catalogEpoch:0}});
  const context=vm.createContext({state,$,el:node,TextEncoder,structuredClone,detailRequest:1,detailValueNode:null,
    window:{AnalysisContexts:{capture,isCurrent,prepare:async owner=>{calls.prepares.push(owner);await hooks.prepare?.();return owner;}},Tasks:{cancelLatest(){}}},
    document:{addEventListener(){}},workspaceScope:()=> 'dataset',caseSig:()=> 'saved-evidence',caseEvents:()=>[],
    caseArgs:async args=>args,api:async(command,args,options)=>{
      assert.equal(command,'analysis_field_text');const call={command,args,options};calls.native.push(call);
      return hooks.native?hooks.native(call):reply(args);
    },
    addFilter:filter=>calls.filters.push(filter),openValueFilter:(...args)=>calls.drafts.push(args),
    toast:(message,level)=>calls.notices.push({message,level}),filterFocusTarget:anchor=>anchor,
    navigator:{clipboard:{writeText:async text=>calls.copies.push(text)}},
    colLabel:String,trunc:String,cellValue:(event,key)=>event.fields[key],
    showCtxMenu(_x,_y,items){context.window.CanonicalFields.cancel();calls.menus.push(items);},
    openDetailValue(value){context.detailValueNode=value;},openDeriveModal(){},showDetailNameMenu(){},saveVisibleCols(){},renderTable(){},
  });
  state.detailAdmission=historical?null:{scope:'dataset',owner:capture(),signature:null};
  vm.runInContext(read('canonical-fields.js'),context);
  vm.runInContext(read('detail-fields.js'),context);context.DetailFields=context.window.DetailFields;
  vm.runInContext(section('function detailAdmissionCurrent(','async function openDetail('),context);
  vm.runInContext(section('function detailFieldColumn(','function toggleDetailColumn('),context);
  vm.runInContext(section('function showDetailValueMenu(','function showDetail('),context);
  const selectionStart=app.indexOf('  $("#detail-value-content").oncontextmenu = (event) => {');
  vm.runInContext(app.slice(selectionStart,app.indexOf('\n  };',selectionStart)+6),context);
  context.renderDetailTree([{key:column,value,filterValue:value}]);
  const inline=nodes.find(item=>item.cls==='kv-filter'),valueButton=nodes.find(item=>item.cls.split(' ').includes('detail-tree-value'));
  const event=target=>({target,preventDefault(){},stopPropagation(){},clientX:12,clientY:34});
  function menu(text,{collapsed=false,inside=true,absent=false}={}){
    valueButton.onclick();const content=$('#detail-value-content'),anchor=node('span'),focus=collapsed?anchor:node('span');
    if(inside)content.append(anchor,focus);
    context.window.getSelection=()=>absent?null:{anchorNode:anchor,focusNode:focus,isCollapsed:collapsed,toString:()=>text};
    content.oncontextmenu(event(content));return calls.menus.at(-1);
  }
  return{state,context,calls,hooks,$,inline,menu,reply,click:()=>inline.onclick(event(inline))};
}

test('installed detail quick filters use native u64, float and structured text exactly',async()=>{
  for(const data of [
    {column:'native_u64',value:18446744073709551615,canonical:'18446744073709551615',valueType:'number'},
    {column:'native_float',value:1,canonical:'1.0',valueType:'number'},
    {column:'native_object',value:{2:1,10:'ten'},canonical:'{"10":"ten","2":1.0}',valueType:'object'},
  ]){
    const f=fixture(data);assert.ok(f.inline);await f.click();
    assert.equal(f.calls.native.length,1);assert.equal(f.calls.native[0].args.id,7);assert.equal(f.calls.native[0].args.eventRef,'source:7');
    assert.deepEqual(plain(f.calls.filters),[{column:data.column,op:'equals_exact',value:data.canonical,value2:null}]);
    assert.equal(f.calls.drafts.length,0);assert.equal(f.calls.notices.filter(notice=>notice.message==='Filtro adicionado.').length,1);
  }
});

test('installed quick-filter actions remain bound to their rendered owner and detail',async()=>{
  for(const stage of ['before click','native reply'])for(const change of ['owner','detail request','detail event','closed drawer']){
    const f=fixture(),gate=deferred();f.hooks.native=()=>gate.promise;
    const replace=()=>{
      if(change==='owner')f.state.owner.identity.visibilityRevision++;
      if(change==='detail request')f.context.detailRequest++;
      if(change==='detail event')f.state.currentDetailEv={...f.state.currentDetailEv,event_ref:'replacement:7'};
      if(change==='closed drawer')f.$('#drawer').hidden=true;
    };
    if(stage==='before click')replace();const click=f.click();await settle();
    if(stage==='native reply'){
      assert.equal(f.calls.native.length,1);replace();gate.resolve(f.reply(f.calls.native[0].args));
    }
    await click;assert.equal(f.calls.filters.length,0,`${change} ${stage}`);
    assert.equal(f.calls.notices.some(notice=>notice.message==='Filtro adicionado.'),false,'failed admission never reports a successful filter');
    if(stage==='before click')assert.equal(f.calls.native.length,0,'a retained obsolete button never resolves the replacement event');
  }
});

test('installed quick filters report native failure without applying decoded fallback values',async()=>{
  const f=fixture({value:1,canonical:'1.0',valueType:'number'});f.hooks.native=()=>{throw Error('native field unavailable');};
  await f.click();assert.equal(f.calls.filters.length,0);assert.match(f.calls.notices.at(-1).message,/native field unavailable/);
  assert.equal(f.calls.notices.some(notice=>notice.message==='Filtro adicionado.'),false);
});

test('installed quick filters preserve historical strings and refuse numeric or structured approximations',async()=>{
  for(const value of ['  saved\n',1,{2:1,10:'ten'}]){
    const f=fixture({value,historical:true});assert.ok(f.inline);await f.click();
    assert.equal(f.calls.native.length,0);assert.equal(f.calls.prepares.length,0);
    if(typeof value==='string')assert.deepEqual(plain(f.calls.filters),[{column:'value',op:'equals_exact',value,value2:null}]);
    else{
      assert.equal(f.calls.filters.length,0);assert.match(f.calls.notices.at(-1).message,/evidência histórica/);
      assert.equal(f.calls.notices.some(notice=>notice.message==='Filtro adicionado.'),false);
    }
  }
});

test('installed modal selection forwards surrounding and whitespace-only text byte-exactly',async()=>{
  for(const selected of ['  literal\n','\n  \t']){
    const f=fixture({value:`prefix${selected}suffix`,canonical:'entire native field'}),menu=f.menu(selected);
    const composer=menu.find(item=>item.label==='Criar filtro: value'),copy=menu.find(item=>item.label==='Copiar seleção');
    assert.ok(composer);assert.ok(copy,'a nonempty whitespace selection remains a selection');
    assert.equal(menu.some(item=>item.label==='Copiar valor'),false);
    await composer.onClick();assert.equal(f.calls.drafts.length,1);assert.equal(f.calls.drafts[0][1],selected);assert.equal(f.calls.drafts[0][3],'contains');
    await copy.onClick();assert.deepEqual(f.calls.copies,[selected]);
    assert.equal(f.calls.native.length,0,'selection does not resolve the entire field');assert.equal(f.calls.prepares.length,0);
  }
});

test('collapsed, empty and outside modal selections still resolve the complete native field',async()=>{
  for(const [selected,options] of [['',{collapsed:true}],['',{}],['ignored',{inside:false}],['',{absent:true}]]){
    const canonical='{"10":"ten","2":1.0}',f=fixture({value:{2:1,10:'ten'},canonical,valueType:'object'}),menu=f.menu(selected,options);
    assert.equal(menu.some(item=>item.label==='Copiar seleção'),false);
    await menu.find(item=>item.label==='Criar filtro: value').onClick();
    assert.equal(f.calls.drafts[0][1],canonical);assert.equal(f.calls.drafts[0][3],'equals_exact');
    await menu.find(item=>item.label==='Copiar valor').onClick();assert.deepEqual(f.calls.copies,[canonical]);assert.equal(f.calls.native.length,2);
  }
});

test('modal literal actions retain their captured detail guard',async()=>{
  const f=fixture(),menu=f.menu('  literal\n');f.context.detailRequest++;
  await menu.find(item=>item.label==='Criar filtro: value').onClick();await menu.find(item=>item.label==='Copiar seleção').onClick();
  assert.equal(f.calls.drafts.length,0);assert.equal(f.calls.copies.length,0);assert.equal(f.calls.native.length,0);
});
