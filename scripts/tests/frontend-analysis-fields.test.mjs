import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const source=readFileSync(new URL('../../frontend/analysis-fields.js',import.meta.url),'utf8');
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');

function fixture(scope='dataset'){
  const nodes=new Map(),listeners=new Map(),calls={native:[],popovers:[],chosen:[],notices:[]};
  let liveScope=scope,evidence='evidence-a';
  const state={columns:['timestamp','message','payload','decoded'],derivedFields:[{name:'decoded',source:'payload',steps:['parse_json']}],
    analysisDefinitionsPending:false,loaded:true,loadOverlay:false,
    owner:{caseId:'case-a',instance:1,identity:{caseId:'case-a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-a:7'}};
  const capture=()=>structuredClone(state.owner),isCurrent=owner=>JSON.stringify(owner)===JSON.stringify(state.owner);
  function node(tag='div',className='',textContent=''){
    const item={tag,className,textContent,children:[],dataset:{},attrs:{},listeners:{},hidden:false,disabled:false,isConnected:true,value:'',
      setAttribute(name,value){this.attrs[name]=String(value);},getAttribute(name){return this.attrs[name];},
      append(...children){for(const child of children){child.parentElement=this;this.children.push(child);}},
      appendChild(child){this.append(child);return child;},replaceChildren(...children){this.children=[];this.append(...children);},
      addEventListener(type,fn){(this.listeners[type]||=[]).push(fn);},
      dispatchEvent(event){this[`on${event.type}`]?.(event);for(const listener of this.listeners[event.type]||[])listener(event);},
      get options(){return this.children;},get selectedOptions(){return this.children.filter(child=>child.value===this.value);},
      get selectedIndex(){return this.children.findIndex(child=>child.value===this.value);},
      set innerHTML(_value){this.children=[];},focus(){},closest(){return null;},
    };return item;
  }
  const $=selector=>{if(!nodes.has(selector))nodes.set(selector,node(selector==='#np-val'?'input':'div'));return nodes.get(selector);};
  const document={addEventListener(type,fn){const callbacks=listeners.get(type)||[];callbacks.push(fn);listeners.set(type,callbacks);},
    createElement:node,querySelector:$,querySelectorAll:()=>[]};
  const context=vm.createContext({state,$,el:node,document,TextEncoder,structuredClone,detailRequest:1,
    window:{AnalysisContexts:{capture,isCurrent}},workspaceScope:()=>liveScope,caseSig:()=>evidence,activeCase:()=>({id:state.owner.caseId}),
    colLabel:String,trunc:(value,size=32)=>String(value).slice(0,size),toast:(...args)=>calls.notices.push(args),
    openNamePop:(anchor,cb,options)=>calls.popovers.push({anchor,cb,options}),
    api:(...args)=>{calls.native.push(args);throw Error('field choice must not perform a native discovery scan');},
  });
  vm.runInContext(app.slice(app.indexOf('function detailAdmissionCurrent('),app.indexOf('async function openDetail(')),context);
  vm.runInContext(source,context,{filename:'analysis-fields.js'});
  function admit(fields={'decoded.rare':42}){
    const event={id:7,event_ref:'source:7',fields};state.currentDetailEv=event;
    state.detailAdmission={scope:liveScope,owner:capture(),signature:liveScope==='case'?evidence:null};
    $('#drawer').hidden=false;return event;
  }
  const fields=context.window.AnalysisFields;
  const choose=field=>calls.chosen.push(field);
  const control=(options={})=>{const select=node('select');fields.control(select,{scope:liveScope,choose,...options});return select;};
  const change=(select,value)=>{select.value=value;select.dispatchEvent({type:'change',target:select});};
  const prompt=select=>{const option=select.options.find(option=>option.textContent.includes('Usar caminho exato'));assert.ok(option,'exact path entry is reachable from the existing selector');change(select,option.value);return calls.popovers.at(-1);};
  const emit=type=>{for(const listener of listeners.get(type)||[])listener({type});};
  admit();return{context,state,fields,calls,$,node,control,change,prompt,emit,admit,capture,
    scope:value=>{liveScope=value;},evidence:value=>{evidence=value;}};
}

test('rare active-derived child paths are selectable without expanding sampled columns or scanning data',()=>{
  const f=fixture(),before=structuredClone(f.state.columns),field='decoded.rare.branch.17.value',captured=f.fields.capture();
  assert.equal(f.fields.available(field),true,'an active derived parent admits an exact child target outside the sample');
  f.fields.remember(field,captured);assert.ok(f.fields.names().includes(field));assert.deepEqual(f.state.columns,before);
  assert.equal(f.fields.available('foreign.rare'),false);assert.throws(()=>f.fields.remember('foreign.rare',captured));
  assert.equal(f.fields.names().includes('foreign.rare'),false,'unknown names are not silently treated as registered fields');
  assert.equal(f.fields.available('decodedOther.child'),false,'parent prefix must end at a field boundary');
  assert.equal(f.calls.native.length,0);
});

test('field-path validation is byte-bounded and preserves Unicode, punctuation and intentional spaces',()=>{
  const f=fixture();
  for(const field of ['decoded.état 🍃 ','  sampled field  ','decoded.e\u0301','decoded.é','x'.repeat(4096),'é'.repeat(2048)])assert.equal(f.fields.valid(field),true,field.slice(0,30));
  for(const field of [null,undefined,0,[],{},'', ' \t\n ', '\u00a0', 'x'.repeat(4097),'é'.repeat(2049),'decoded.\u0000key','decoded.\nkey','decoded.\tkey','decoded.\u007fkey'])assert.equal(f.fields.valid(field),false);
  f.state.columns.push('  sampled field  ');
  for(const field of ['decoded.état 🍃 ','  sampled field  ','decoded.e\u0301','decoded.é'])f.fields.remember(field,f.fields.capture());
  const names=Array.from(f.fields.names());for(const field of ['decoded.état 🍃 ','  sampled field  ','decoded.e\u0301','decoded.é'])assert.ok(names.includes(field));
  assert.equal(names.includes('decoded.état 🍃'),false,'exact field names are not trimmed');
  assert.equal(f.calls.native.length,0);
});

test('field choices and captured callbacks are isolated by Case, configuration, source, scope and evidence',()=>{
  const changes=[
    ['Case instance',f=>{f.state.owner.instance++;}],
    ['Case identity',f=>{f.state.owner.caseId='case-b';f.state.owner.identity.caseId='case-b';}],
    ['analysis',f=>{f.state.owner.identity.analysisId='analysis-b';}],
    ['configuration',f=>{f.state.owner.identity.configRevision++;}],
    ['visibility',f=>{f.state.owner.identity.visibilityRevision++;}],
    ['source generation',f=>{f.state.owner.sourceGeneration++;f.state.owner.sourceKey='source-a:8';}],
    ['source identity',f=>{f.state.owner.sourceKey='source-b:7';}],
    ['scope',f=>{f.scope('dataset');}],
    ['evidence',f=>{f.evidence('evidence-b');}],
  ];
  for(const [name,change] of changes){
    const f=fixture('case'),captured=f.fields.capture(),select=f.control(),prompt=f.prompt(select);
    f.fields.remember('decoded.remembered',captured);assert.equal(f.fields.isCurrent(captured),true);
    change(f);assert.equal(f.fields.isCurrent(captured),false,name);
    assert.throws(()=>f.fields.remember('decoded.stale',captured),/contexto mudou/);assert.equal(prompt.cb('decoded.from-old-prompt'),false);f.change(select,'message');
    assert.equal(f.fields.names().includes('decoded.remembered'),false,`${name}: previous choices do not leak`);
    assert.equal(f.fields.names().includes('decoded.stale'),false,name);assert.equal(f.fields.names().includes('decoded.from-old-prompt'),false,name);
    assert.equal(f.calls.chosen.length,0,name);assert.equal(f.calls.native.length,0,name);
  }
});

test('remembered choices remain globally bounded across historical owners',()=>{
  const f=fixture(),owners=[];
  for(let owner=0;owner<8;owner++){
    f.state.owner={...f.state.owner,caseId:`case-${owner}`,instance:owner+1,identity:{...f.state.owner.identity,caseId:`case-${owner}`,analysisId:`analysis-${owner}`}};
    owners.push(structuredClone(f.state.owner));
    for(let path=0;path<20;path++)f.fields.remember(`decoded.owner${owner}.field${path}`,f.fields.capture());
    assert.ok(f.fields.names().filter(name=>name.startsWith('decoded.owner')).length<=64);
  }
  const retained=new Set();
  for(const owner of owners){f.state.owner=owner;for(const name of f.fields.names())if(name.startsWith('decoded.owner'))retained.add(name);}
  assert.ok(retained.size<=64,`all contexts retain ${retained.size} paths; the bound is global`);
  assert.equal(f.calls.native.length,0);
});

test('authored unavailable paths survive selector restoration without becoming sampled or available',()=>{
  const f=fixture(),authored=['foreign.saved.child','decoded.rare.child'],before=structuredClone(authored);
  const names=Array.from(f.fields.names('dataset',authored));assert.ok(names.includes('foreign.saved.child'));assert.ok(names.includes('decoded.rare.child'));
  const select=f.control({value:'foreign.saved.child'}),option=select.options.find(option=>option.value==='foreign.saved.child');
  assert.ok(option);assert.equal(option.disabled,true);assert.equal(select.value,'foreign.saved.child');
  assert.equal(f.fields.available('foreign.saved.child'),false);assert.equal(f.state.columns.includes('foreign.saved.child'),false);
  f.change(select,'foreign.saved.child');assert.equal(f.calls.chosen.length,0);
  assert.deepEqual(authored,before);assert.equal(f.calls.native.length,0);
});

test('only current admitted original Event field keys can be observed as additional choices',()=>{
  const f=fixture(),event=f.admit({'actual.unprofiled':42,payload:'{"virtual":42}'}),admission=f.state.detailAdmission;
  const actual={path:'actual.unprofiled',original:true},virtual={path:'payload.virtual',original:false};
  assert.equal(f.fields.available(actual.path),false);assert.equal(f.fields.admitted(actual),true);
  f.fields.observe(actual);assert.ok(f.fields.names().includes(actual.path));assert.equal(f.fields.available(actual.path),true);
  for(const node of [virtual,{...virtual,original:true},{path:'absent.child',original:true},{path:'actual.unprofiled',original:false}]){
    assert.equal(f.fields.admitted(node,admission,event),false);f.fields.observe(node,admission,event);
  }
  assert.equal(f.fields.names().includes(virtual.path),false);assert.equal(f.fields.names().includes('absent.child'),false);
  const historical=fixture();historical.state.detailAdmission=null;
  assert.equal(historical.fields.admitted({path:'decoded.rare',original:true}),false);
  historical.fields.observe({path:'decoded.rare',original:true});assert.equal(historical.fields.names().includes('decoded.rare'),false);
  const inherited=fixture();inherited.admit(Object.create({'inherited.key':42}));
  assert.equal(inherited.fields.admitted({path:'inherited.key',original:true}),false,'an inherited property is not a returned Event field');
  f.state.owner.identity.configRevision++;
  assert.equal(f.fields.admitted(actual,admission,event),false);f.fields.observe(actual,admission,event);
  assert.equal(f.fields.names().includes(actual.path),false,'an obsolete admitted detail cannot register into a new configuration');
  assert.equal(f.calls.native.length,0);
});

test('a replaced detail or changed Case evidence cannot prove a field in the current context',()=>{
  for(const change of ['detail','evidence','scope','closed']){
    const f=fixture('case'),event=f.admit({'actual.unprofiled':42}),admission=f.state.detailAdmission,node={path:'actual.unprofiled',original:true};
    if(change==='detail')f.admit({other:42});
    if(change==='evidence')f.evidence('replacement-evidence');
    if(change==='scope')f.scope('dataset');
    if(change==='closed')f.$('#drawer').hidden=true;
    assert.equal(f.fields.admitted(node,admission,event),false,change);
    assert.equal(f.fields.observe(node,admission,event),false,change);assert.equal(f.fields.names().includes(node.path),false,change);
    assert.equal(f.calls.native.length,0);
  }
});

test('pending analysis definitions disable choice and observation without erasing authored paths',()=>{
  const f=fixture();f.state.analysisDefinitionsPending=true;
  assert.equal(f.fields.available('decoded.rare'),false);assert.throws(()=>f.fields.remember('decoded.rare',f.fields.capture()));
  assert.equal(f.fields.observe({path:'decoded.rare',original:true}),false);
  const select=f.control({value:'decoded.rare'});assert.equal(select.disabled,true);assert.equal(select.value,'decoded.rare');
  assert.ok(select.options.find(option=>option.value==='decoded.rare').disabled);
  const sentinel=select.options.find(option=>option.textContent.includes('Usar caminho exato'));f.change(select,sentinel.value);
  assert.equal(f.calls.popovers.length,0);assert.equal(f.calls.chosen.length,0);assert.equal(f.calls.native.length,0);
});

test('exact-path selection uses the existing popover and commits only a current valid target',()=>{
  const f=fixture(),select=f.control({value:'message',fixed:[['','Sem agrupamento'],['*','Todos os registros']]}),before=structuredClone(f.state.columns);
  assert.equal(select.value,'message');assert.ok(select.options.some(option=>option.value===''));assert.ok(select.options.some(option=>option.value==='*'));
  const prompt=f.prompt(select);assert.ok(prompt);assert.equal(prompt.anchor,select);assert.equal(f.calls.chosen.length,0);
  assert.equal(prompt.options.exact,true,'field paths opt into byte-preserving popover entry');assert.equal(prompt.options.confirmLabel,'Usar');
  assert.equal(f.calls.native.length,0,'opening or editing a field entry never scans the dataset');
  prompt.cb('decoded.état 🍃 ');assert.deepEqual(f.calls.chosen,['decoded.état 🍃 ']);
  assert.equal(select.value,'decoded.état 🍃 ');assert.ok(f.fields.names().includes('decoded.état 🍃 '));assert.deepEqual(f.state.columns,before);
  f.change(select,'');f.change(select,'*');assert.deepEqual(f.calls.chosen,['decoded.état 🍃 ','','*']);
  const invalid=f.prompt(select);invalid.cb('foreign.unknown');assert.equal(f.fields.names().includes('foreign.unknown'),false);
  assert.equal(f.calls.chosen.length,3);assert.equal(f.calls.native.length,0);
});
