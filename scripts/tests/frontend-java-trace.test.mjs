import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const source=readFileSync(new URL('../../frontend/java-trace.js',import.meta.url),'utf8');
const appSource=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const nativeTraces=new WeakMap();
const nativeTrace=event=>nativeTraces.get(event);
const available=event=>({state:'available',trace:nativeTrace(event),reason:null,row:{id:event.id,eventRef:event.event_ref}});
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const settle=async()=>{for(let i=0;i<20;i++)await Promise.resolve();};

// Keep this fixture local: exercise the installed controller with the same small
// DOM/VM boundary used by the other frontend tests, without a browser or IPC.
function fixture(){
  const created=[],copied=[],menus=[],scalarMenus=[],rawCalls=[],loads=[];
  let current=true;
  function element(tag='div',className='',initialText=''){
    let ownText=String(initialText);
    const node={tag,tagName:tag.toUpperCase(),className,children:[],dataset:{},style:{setProperty(){}},attrs:{},listeners:{},
      parentElement:null,parentNode:null,hidden:false,disabled:false,isConnected:true,open:false,
      get textContent(){return ownText+this.children.map(child=>child.textContent).join('');},
      set textContent(value){ownText=String(value??'');this.children=[];},
      get innerHTML(){return '';},
      set innerHTML(value){assert.equal(value,'','log values must be rendered as text, never HTML');ownText='';this.children=[];},
      append(...items){for(let child of items){if(typeof child==='string')child=element('#text','',child);child.parentNode=this;child.parentElement=this;this.children.push(child);}},
      appendChild(child){this.append(child);return child;},
      replaceChildren(...items){ownText='';this.children=[];this.append(...items);},
      remove(){if(this.parentNode)this.parentNode.children=this.parentNode.children.filter(child=>child!==this);this.isConnected=false;},
      setAttribute(name,value){this.attrs[name]=String(value);if(name==='class')this.className=String(value);},
      getAttribute(name){return this.attrs[name]??null;},
      removeAttribute(name){delete this.attrs[name];},
      addEventListener(name,listener){(this.listeners[name]??=[]).push(listener);},
      querySelectorAll(selector){return descendants(this).filter(child=>child.matches(selector));},
      querySelector(selector){return this.querySelectorAll(selector)[0]??null;},
      matches(selector){return selector.split(',').some(part=>{
        const match=part.trim().match(/^(\w+)?(?:\.([\w-]+))?(?:\[([\w-]+)(?:="([^"]*)")?\])?$/);
        if(!match)return false;
        return (!match[1]||match[1]===this.tag)&&(!match[2]||this.className.split(/\s+/).includes(match[2]))
          &&(!match[3]||(this.getAttribute(match[3])!==null&&(match[4]===undefined||this.getAttribute(match[3])===match[4])));
      });},
      closest(selector){let parent=this;while(parent){if(parent.matches(selector))return parent;parent=parent.parentNode;}return null;},
    };
    node.classList={add(...names){node.className=[...new Set([...node.className.split(/\s+/).filter(Boolean),...names])].join(' ');},
      remove(...names){node.className=node.className.split(/\s+/).filter(name=>!names.includes(name)).join(' ');},
      contains(name){return node.className.split(/\s+/).includes(name);},
      toggle(name,on){const keep=on??!this.contains(name);this[keep?'add':'remove'](name);return keep;}};
    created.push(node);return node;
  }
  const document={createElement:element,createTextNode:text=>element('#text','',text)};
  document.body=element('body');
  const context=vm.createContext({window:{},document,el:element,TextEncoder,TextDecoder,structuredClone,
    navigator:{clipboard:{writeText:async text=>copied.push(text)}},
    api(){assert.fail('Java details must not issue IPC');},invoke(){assert.fail('Java details must not issue IPC');},
  });
  vm.runInContext(source,context,{filename:'java-trace.js'});
  assert.equal(typeof context.window.JavaTrace?.render,'function');
  const options={admission:{scope:'case',request:4},isCurrent:()=>current,
    copy:async text=>copied.push(text),raw:event=>rawCalls.push(event),
    menu:(event,items)=>menus.push({event,items}),scalarMenu:(event,column)=>scalarMenus.push({event,column})};
  return{context,created,copied,menus,scalarMenus,rawCalls,loads,options,
    render:(event,extra={})=>context.window.JavaTrace.render(event,{...options,load:async()=>{loads.push(event);return available(event);},...extra}),
    stale(){current=false;},current(){current=true;}};
}

function descendants(node){return node.children.flatMap(child=>[child,...descendants(child)]);}
function matching(node,selector){return node.querySelectorAll(selector);}
function button(node,label){const found=matching(node,'button').find(item=>label.test(item.textContent));assert.ok(found,`button ${label}`);return found;}
async function dispatch(node,type){
  const event={type,target:node,currentTarget:node,clientX:10,clientY:20,preventDefault(){},stopPropagation(){}};
  if(typeof node[`on${type}`]==='function')await node[`on${type}`](event);
  for(const listener of node.listeners[type]??[])await listener(event);
  return event;
}
async function expand(node){node.open=true;await dispatch(node,'toggle');}
function traceEvent({nodeCount=1,frameCount=0,complete=true}={}){
  const nodes=Array.from({length:nodeCount},(_,index)=>({relation:index===0?'root':index%2?'cause':'suppressed',
    parent:index===0?null:0,depth:index===0?0:1,header:{start:0,end:20},class:`demo.Exception${index}`,message:`message ${index}`,
    frameStart:index===0?0:frameCount,frameCount:index===0?frameCount:0,elidedFrames:index===0?3:0}));
  const frames=Array.from({length:frameCount},(_,index)=>({node:0,span:{start:20+index*40,end:60+index*40},
    parts:{class:`demo.Service${index}`,method:'run',loader:'app',module:'service',moduleVersion:'1.0',
      location:{kind:'file',name:`Service${index}.java`,line:index+1}}}));
  const trace={schemaVersion:1,spanBasis:'utf8_bytes_in_raw_block',complete,root:0,nodes,frames,diagnostics:[],
    fingerprintVersion:1,fingerprint:'java-v1:fixture'};
  const event={id:7,event_ref:'source:7',raw:'complete raw source\n',fields:{'java.trace.complete':complete}};
  nativeTraces.set(event,trace);return event;
}

test('missing eligibility markers and raw text alone never synthesize or load a Java tree',()=>{
  const f=fixture();
  const events=[null,{}, {fields:{}}, {raw:'java.lang.Exception: raw only\n\tat demo.Service.run(Service.java:9)',fields:{}},
    {fields:{'java.exception.class':'legacy.Exception','java.exception.message':'legacy rawless'}},
    {fields:{'java.trace':null}},{fields:{'java.trace':'not structured'}},
    {fields:{'java.trace':nativeTrace(traceEvent())}},
    {fields:{'java.trace.complete':'true'}},{fields:{exception:'',stacktrace:[]}}];
  for(const event of events)assert.equal(f.render(event),null);
  assert.equal(f.rawCalls.length,0);assert.equal(f.menus.length,0);assert.equal(f.scalarMenus.length,0);assert.equal(f.loads.length,0);
});

test('legacy exception or stacktrace hints offer neutral opt-in loading without inventing scalar fields',async()=>{
  for(const legacy of [{exception:'LegacyFailure'},{stacktrace:['original legacy frame']}]){
    const f=fixture(),event=traceEvent();event.fields=legacy;const before=structuredClone(event),root=f.render(event);
    assert.ok(root);assert.equal(root.open,false);assert.match(root.textContent,/Interpretar stack trace/);
    assert.doesNotMatch(root.textContent,/Exceção Java/);assert.equal(f.loads.length,0);assert.equal(matching(root,'.java-trace-node').length,0);
    await expand(root);assert.equal(f.loads.length,1);assert.equal(matching(root,'.java-trace-node').length,1);
    assert.match(root.textContent,/Exceção Java · estrutura observada/);assert.equal(matching(root,'.java-trace-scalar-value').length,0);
    await dispatch(matching(root,'.java-trace-node-title')[0],'contextmenu');assert.equal(f.scalarMenus.length,0);
    assert.deepEqual(event,before);assert.equal(Object.hasOwn(event.fields,'java.trace.complete'),false);assert.equal(Object.hasOwn(event.fields,'java.trace'),false);
  }
});

test('legacy missing-raw and historical views keep the original array accessible without fabricated structure or field menus',async()=>{
  for(const historical of [false,true]){
    const f=fixture(),event=traceEvent(),opened=[];event.raw='';event.fields={exception:'LegacyFailure',stacktrace:['untouched legacy frame']};
    const before=structuredClone(event);let loads=0;
    const root=f.render(event,{admission:historical?null:f.options.admission,
      load:historical?null:async()=>{loads++;return{state:'unavailable',trace:null,reason:'raw_unavailable',row:{id:event.id,eventRef:event.event_ref}};},
      original:(column,anchor)=>opened.push({column,anchor})});
    assert.match(root.textContent,/Interpretar stack trace/);assert.equal(loads,0);await expand(root);
    assert.equal(loads,historical?0:1);assert.equal(matching(root,'.java-trace-node').length,0);assert.equal(matching(root,'.java-trace-scalar-value').length,0);
    assert.match(root.textContent,historical?/evidência preservada/:/Texto decodificado indisponível para estruturar/);
    assert.equal(matching(root,'.java-trace-source')[0].disabled,true);
    const original=button(root,/Ver stacktrace original/);await dispatch(original,'click');
    assert.deepEqual(opened,[{column:'stacktrace',anchor:original}]);assert.equal(f.scalarMenus.length,0);assert.deepEqual(event,before);
    f.stale();await dispatch(original,'click');assert.equal(opened.length,1,'historical source access also expires with its drawer');
  }
});

test('malformed loaded schema, diagnostics and frame locations decline structural rendering safely',async()=>{
  const f=fixture();
  for(const malformed of [trace=>{trace.diagnostics=[null];},trace=>{trace.frames[0].parts.location.line='12';},
    trace=>{trace.schemaVersion=99;},trace=>{trace.spanBasis='utf16_characters';}]){
    const event=traceEvent({frameCount:1});malformed(nativeTrace(event));const root=f.render(event);await expand(root);
    assert.equal(matching(root,'.java-trace-node').length,0);assert.match(root.textContent,/não é compatível/);
    assert.equal(Object.hasOwn(event.fields,'java.trace'),false,'rich response never becomes an Event field');
  }
});

test('a named loader without a module preserves the JVM double slash in displayed and copied frames',async()=>{
  const f=fixture(),event=traceEvent({frameCount:1}),parts=nativeTrace(event).frames[0].parts;
  Object.assign(parts,{loader:'custom.loader',module:null,moduleVersion:null});
  const root=f.render(event);await expand(root);const node=matching(root,'.java-trace-node')[0];await expand(node);
  const frame=matching(node,'.java-trace-frame')[0];assert.match(frame.textContent,/custom\.loader\/\/demo\.Service0\.run/);
  await dispatch(frame,'contextmenu');await f.menus.at(-1).items.find(item=>/copiar/i.test(item.label)).onClick();
  assert.match(f.copied[0],/^custom\.loader\/\/demo\.Service0\.run\(Service0\.java:1\)$/);
});

test('valid absolute byte spans beyond the parser body budget remain renderable without string slicing',async()=>{
  const f=fixture(),event=traceEvent({frameCount:1}),trace=nativeTrace(event);
  trace.nodes[0].header={start:400_000,end:400_040};trace.frames[0].span={start:400_040,end:400_100};
  const before=structuredClone(event),root=f.render(event);assert.ok(root);await expand(root);await expand(matching(root,'.java-trace-node')[0]);
  assert.match(matching(root,'.java-trace-frame')[0].textContent,/demo\.Service0\.run\(Service0\.java:1\)/);
  assert.deepEqual(event,before);
});

test('the compatibility stacktrace opens explicitly and its action expires with detail ownership',async()=>{
  const f=fixture(),event=traceEvent(),opened=[];event.fields.stacktrace=['untouched original frame'];
  const root=f.render(event,{original:(column,anchor)=>opened.push({column,anchor})});await expand(root);
  const original=button(root,/Ver stacktrace original/);assert.equal(opened.length,0);
  await dispatch(original,'click');assert.deepEqual(opened,[{column:'stacktrace',anchor:original}]);
  assert.deepEqual(event.fields.stacktrace,['untouched original frame']);f.stale();await dispatch(original,'click');assert.equal(opened.length,1);
});

test('opening Java details and loading exceptions are lazy and bounded to eight nodes per action',async()=>{
  const f=fixture(),event=traceEvent({nodeCount:19}),before=structuredClone(event),root=f.render(event);
  assert.equal(root.tag,'details');assert.equal(root.open,false);
  assert.equal(f.loads.length,0,'initial render does not request native Java detail');
  assert.equal(matching(root,'.java-trace-node').length,0);assert.equal(matching(root,'.java-trace-frame').length,0);
  await expand(root);
  assert.equal(f.loads.length,1,'first expansion opts into one native response');
  assert.equal(matching(root,'.java-trace-node').length,8);assert.match(root.textContent,/8 de 19 exceções/);
  assert.ok(matching(root,'.java-trace-node').every(node=>!node.open));
  await dispatch(button(root,/Mostrar mais exceções/),'click');
  assert.equal(matching(root,'.java-trace-node').length,16);assert.match(root.textContent,/16 de 19 exceções/);
  await dispatch(button(root,/Mostrar mais exceções/),'click');
  assert.equal(matching(root,'.java-trace-node').length,19);assert.match(root.textContent,/19 de 19 exceções/);
  assert.equal(matching(root,'.java-trace-frame').length,0);
  root.open=false;await dispatch(root,'toggle');await expand(root);
  assert.equal(matching(root,'.java-trace-node').length,19,'reopening does not duplicate or eagerly grow nodes');
  assert.equal(f.loads.length,1,'reopening reuses its owned structure');assert.equal(Object.hasOwn(event.fields,'java.trace'),false);
  assert.deepEqual(event,before,'rendering preserves admitted Event data');
});

test('each exception expands frames lazily in explicit pages of twenty',async()=>{
  const f=fixture(),root=f.render(traceEvent({frameCount:57}));await expand(root);
  const node=matching(root,'.java-trace-node')[0];assert.equal(matching(node,'.java-trace-frame').length,0);
  await expand(node);assert.equal(matching(node,'.java-trace-frame').length,20);assert.match(node.textContent,/20 de 57 frames/);
  await dispatch(button(node,/Mostrar mais frames/),'click');
  assert.equal(matching(node,'.java-trace-frame').length,40);assert.match(node.textContent,/40 de 57 frames/);
  await dispatch(button(node,/Mostrar mais frames/),'click');
  assert.equal(matching(node,'.java-trace-frame').length,57);assert.match(node.textContent,/57 de 57 frames/);
  node.open=false;await dispatch(node,'toggle');await expand(node);
  assert.equal(matching(node,'.java-trace-frame').length,57,'reopening preserves the loaded frame page');
  assert.match(node.textContent,/Service0\.java/);assert.match(node.textContent,/Service56\.java/);
});

test('an in-flight expansion is shared and its successful response remains local to the Java controller',async()=>{
  const f=fixture(),event=traceEvent({frameCount:1}),before=structuredClone(event),gate=deferred();let loads=0;
  const root=f.render(event,{load:()=>{loads++;return gate.promise;}});assert.equal(loads,0);
  const first=expand(root);await settle();assert.equal(loads,1);assert.equal(matching(root,'.java-trace-node').length,0);
  await expand(root);assert.equal(loads,1,'repeated expansion never duplicates the pending request');
  gate.resolve(available(event));await first;assert.equal(matching(root,'.java-trace-node').length,1);
  root.open=false;await dispatch(root,'toggle');await expand(root);assert.equal(loads,1);
  assert.deepEqual(event,before);assert.equal(Object.hasOwn(event.fields,'java.trace'),false);
});

test('cancellation and collapse discard late replies while explicit retry owns the replacement response',async()=>{
  for(const method of ['button','collapse']){
    const f=fixture(),event=traceEvent(),first=deferred(),second=deferred();let loads=0,cancellations=0;
    const root=f.render(event,{load:()=>++loads===1?first.promise:second.promise,cancel:()=>cancellations++});
    const old=expand(root);await settle();
    if(method==='button')await dispatch(matching(root,'.java-trace-cancel')[0],'click');
    else{root.open=false;await dispatch(root,'toggle');}
    assert.equal(cancellations,1);assert.match(root.textContent,/cancelada/);assert.equal(matching(root,'.java-trace-node').length,0);
    root.open=true;const retry=dispatch(matching(root,'.java-trace-retry')[0],'click');await settle();assert.equal(loads,2);
    second.resolve(available(event));await retry;assert.equal(matching(root,'.java-trace-node').length,1);
    const obsolete=available(event);obsolete.trace=structuredClone(obsolete.trace);obsolete.trace.nodes[0].class='ObsoleteException';
    first.resolve(obsolete);await old;assert.doesNotMatch(root.textContent,/ObsoleteException/);
    assert.equal(matching(root,'.java-trace-node').length,1);assert.equal(Object.hasOwn(event.fields,'java.trace'),false);
  }
});

test('collapsing a stale Java view never cancels the new owners shared native task',async()=>{
  const f=fixture(),oldEvent=traceEvent(),newEvent=traceEvent(),oldGate=deferred(),newGate=deferred();let owner=1,cancellations=0;
  nativeTrace(oldEvent).nodes[0].class='OldOwnerException';nativeTrace(newEvent).nodes[0].class='NewOwnerException';
  const oldRoot=f.render(oldEvent,{isCurrent:()=>owner===1,load:()=>oldGate.promise,cancel:()=>cancellations++});
  const oldRead=expand(oldRoot);await settle();owner=2;
  const newRoot=f.render(newEvent,{isCurrent:()=>owner===2,load:()=>newGate.promise,cancel:()=>cancellations++});
  const newRead=expand(newRoot);await settle();oldRoot.open=false;await dispatch(oldRoot,'toggle');
  assert.equal(cancellations,0,'the obsolete toggle cannot cancel java-trace-detail for the replacement drawer');
  oldGate.resolve(available(oldEvent));await oldRead;assert.equal(matching(oldRoot,'.java-trace-node').length,0);
  newGate.resolve(available(newEvent));await newRead;
  assert.match(newRoot.textContent,/NewOwnerException/);assert.doesNotMatch(newRoot.textContent,/OldOwnerException/);
  assert.equal(matching(newRoot,'.java-trace-node').length,1);assert.equal(cancellations,0);
});

test('a detached old cancel button cannot cancel a retried request in the same Java view',async()=>{
  const f=fixture(),event=traceEvent(),first=deferred(),second=deferred();let loads=0,cancellations=0;
  const root=f.render(event,{load:()=>++loads===1?first.promise:second.promise,cancel:()=>cancellations++});
  const oldRead=expand(root);await settle();const oldCancel=matching(root,'.java-trace-cancel')[0];
  await dispatch(oldCancel,'click');assert.equal(cancellations,1);
  const retry=dispatch(matching(root,'.java-trace-retry')[0],'click');await settle();assert.equal(loads,2);
  assert.notEqual(matching(root,'.java-trace-cancel')[0],oldCancel);await dispatch(oldCancel,'click');
  assert.equal(cancellations,1,'the old button retains only its original request serial');
  second.resolve(available(event));await retry;assert.equal(matching(root,'.java-trace-node').length,1);
  first.resolve(available(event));await oldRead;assert.equal(matching(root,'.java-trace-node').length,1);
});

test('stale native responses and failures never install nodes or show obsolete errors',async()=>{
  for(const outcome of ['resolve','reject']){
    const f=fixture(),event=traceEvent(),gate=deferred(),root=f.render(event,{load:()=>gate.promise});
    const reading=expand(root);await settle();f.stale();
    if(outcome==='resolve')gate.resolve(available(event));else gate.reject(Error('obsolete backend failure'));
    await reading;assert.equal(matching(root,'.java-trace-node').length,0);assert.doesNotMatch(root.textContent,/obsolete backend failure/);
    assert.equal(matching(root,'.java-trace-retry').length,0);assert.equal(Object.hasOwn(event.fields,'java.trace'),false);
  }
});

test('failed native detail is retried explicitly and unavailable or preserved evidence never invents structure',async()=>{
  const f=fixture(),event=traceEvent();let calls=0;
  const root=f.render(event,{load:async()=>{if(++calls===1)throw Error('temporary failure');return available(event);}});
  await expand(root);assert.equal(calls,1);assert.match(root.textContent,/temporary failure/);
  await dispatch(matching(root,'.java-trace-retry')[0],'click');assert.equal(calls,2);assert.equal(matching(root,'.java-trace-node').length,1);
  for(const reason of ['record_unavailable','raw_unavailable','not_java']){
    const unavailable=f.render(event,{load:async()=>({state:'unavailable',reason,trace:null,row:reason==='record_unavailable'?null:{id:event.id,eventRef:event.event_ref}})});
    await expand(unavailable);assert.equal(matching(unavailable,'.java-trace-node').length,0);assert.equal(matching(unavailable,'.java-trace-frame').length,0);
    assert.ok(matching(unavailable,'.java-trace-source').length);assert.equal(matching(unavailable,'.java-trace-retry').length,0);
  }
  for(const raw of ['preserved decoded source','']){
    const preserved=traceEvent();preserved.raw=raw;
    const saved=f.render(preserved,{admission:null,load:null});await expand(saved);
    assert.equal(f.loads.length,0);assert.match(saved.textContent,/evidência preservada/);assert.equal(matching(saved,'.java-trace-node').length,0);
    assert.equal(matching(saved,'.java-trace-source')[0].disabled,!raw);
  }
});

test('only present literal scalar fields expose the shared field menu',async()=>{
  const f=fixture(),event=traceEvent({frameCount:1});
  nativeTrace(event).nodes[0].class='OriginalException';
  Object.assign(event.fields,{'java.exception.class':'OverlayException','java.exception.message':'literal message',
    'java.root_cause.class':'literal.Cause','java.trace.fingerprint':'literal-fingerprint','java.trace.complete':false,
    'java.frames.class':'virtual-looking field',java:{exception:{class:'nested imitation'}}});
  const root=f.render(event);await expand(root);await expand(matching(root,'.java-trace-node')[0]);
  const scalars=matching(root,'.java-trace-scalar-value');assert.equal(scalars.length,4);
  assert.equal(scalars.find(item=>item.dataset.column==='java.exception.class').textContent,'OverlayException');
  assert.match(matching(root,'.java-trace-node-title')[0].textContent,/OriginalException/);
  assert.doesNotMatch(matching(root,'.java-trace-node-title')[0].textContent,/OverlayException/);
  for(const scalar of scalars)await dispatch(scalar,'contextmenu');
  assert.deepEqual(f.scalarMenus.map(item=>item.column).sort(),[
    'java.exception.class','java.exception.message','java.root_cause.class','java.trace.fingerprint']);
  assert.ok(f.scalarMenus.every(item=>item.event.type==='contextmenu'));
  for(const item of [...matching(root,'.java-trace-node-title'),...matching(root,'.java-trace-frame')])await dispatch(item,'contextmenu');
  assert.equal(f.scalarMenus.length,4,'virtual node and frame data cannot create field actions');
  assert.equal(f.menus.length,2);
  for(const {items} of f.menus){
    assert.ok(items.some(item=>/copiar/i.test(item.label)));assert.ok(items.some(item=>/texto|fonte|origem/i.test(item.label)));
    assert.ok(items.every(item=>!/filtro|agrupar|coluna|pivô|transformar/i.test(item.label)));
  }
  await f.menus[0].items.find(item=>/copiar/i.test(item.label)).onClick();
  assert.equal(f.copied[0],'OriginalException','virtual native class copies the original while literal scalar actions use the overlay');
  assert.equal(f.scalarMenus.length,4,'copying the native class never becomes a scalar field action');
  const missing=fixture(),missingRoot=missing.render(traceEvent());await expand(missingRoot);
  assert.equal(matching(missingRoot,'.java-trace-scalar').length,0,'trace members do not manufacture literal scalar fields');
  const structured=traceEvent();Object.assign(structured.fields,{'java.exception.class':{nested:'object'},'java.exception.message':['array']});
  const structuredRoot=missing.render(structured);await expand(structuredRoot);
  assert.equal(matching(structuredRoot,'.java-trace-scalar').length,0,'objects and arrays cannot become scalar menu values');
});

test('virtual copy and source actions retain their captured Event and never issue field actions',async()=>{
  const f=fixture(),event=traceEvent({frameCount:1}),root=f.render(event);await expand(root);
  const node=matching(root,'.java-trace-node')[0];await expand(node);
  const frame=matching(node,'.java-trace-frame')[0];await dispatch(frame,'contextmenu');
  const menu=f.menus.at(-1).items;
  await menu.find(item=>/copiar/i.test(item.label)).onClick();
  assert.equal(f.copied.length,1);assert.match(f.copied[0],/demo\.Service0/);assert.match(f.copied[0],/run/);
  await menu.find(item=>/texto|fonte|origem/i.test(item.label)).onClick();
  assert.equal(f.rawCalls.at(-1),event);assert.equal(f.scalarMenus.length,0);
  await dispatch(button(root,/texto|fonte|origem/i),'click');assert.equal(f.rawCalls.at(-1),event);
});

test('stale ownership rejects expansions, page buttons, source, copy and all context menus',async()=>{
  const closed=fixture(),closedRoot=closed.render(traceEvent({nodeCount:19}));closed.stale();await expand(closedRoot);
  assert.equal(matching(closedRoot,'.java-trace-node').length,0);
  const f=fixture(),event=traceEvent({nodeCount:19,frameCount:57});event.fields['java.exception.class']='literal.Root';
  const root=f.render(event);await expand(root);const node=matching(root,'.java-trace-node')[0];await expand(node);
  const frame=matching(node,'.java-trace-frame')[0];await dispatch(frame,'contextmenu');
  const capturedMenu=f.menus.at(-1).items;f.stale();
  await dispatch(button(root,/Mostrar mais exceções/),'click');await dispatch(button(node,/Mostrar mais frames/),'click');
  await expand(matching(root,'.java-trace-node')[1]);
  await dispatch(button(root,/texto|fonte|origem/i),'click');
  await dispatch(matching(root,'.java-trace-scalar-value')[0],'contextmenu');await dispatch(frame,'contextmenu');
  await dispatch(matching(node,'.java-trace-node-title')[0],'contextmenu');
  for(const item of capturedMenu)if(item.onClick)await item.onClick();
  assert.equal(matching(root,'.java-trace-node').length,8);assert.equal(matching(root,'.java-trace-frame').length,20);
  assert.equal(f.rawCalls.length,0);assert.equal(f.copied.length,0);assert.equal(f.scalarMenus.length,0);assert.equal(f.menus.length,1);
});

test('native byte spans are never treated as JavaScript string offsets and uninterpreted frames retain source access',async()=>{
  const f=fixture(),event=traceEvent({frameCount:1,complete:false});
  event.raw='🔥 início não ASCII\r\n\tat démon.Service.run(Serviço.java:12)\r\n';
  const start=Buffer.byteLength(event.raw.split('\r\n')[0]+'\r\n','utf8');
  nativeTrace(event).frames[0]={node:0,span:{start,end:Buffer.byteLength(event.raw,'utf8')-2},parts:null};
  nativeTrace(event).diagnostics=[{code:'unparsed_frame',message:'frame incomplete',span:{start,end:start+4}}];
  const before=structuredClone(event);f.context.forbiddenRaw=event.raw;
  vm.runInContext(`const originalSlice=String.prototype.slice;
    String.prototype.slice=function(...args){if(String(this)===forbiddenRaw)throw Error('native byte offsets cannot slice raw strings');return originalSlice.apply(this,args);};`,f.context);
  const root=f.render(event);await expand(root);const node=matching(root,'.java-trace-node')[0];await expand(node);
  const frame=matching(node,'.java-trace-frame')[0];assert.match(frame.textContent,/Frame não interpretado/);
  await dispatch(frame,'contextmenu');const sourceAction=f.menus.at(-1).items.find(item=>/texto|fonte|origem/i.test(item.label));
  assert.ok(sourceAction);await sourceAction.onClick();assert.equal(f.rawCalls.at(-1),event);
  assert.deepEqual(plain(event),before);assert.equal(f.scalarMenus.length,0);
});

test('an incomplete trace reports its limits and does not present definitive root-cause or fingerprint scalar actions',async()=>{
  const f=fixture(),event=traceEvent({complete:false});
  Object.assign(event.fields,{'java.exception.class':'observed.Exception','java.exception.message':'observed message',
    'java.root_cause.class':'unreliable.LastCause','java.trace.fingerprint':'unreliable-fingerprint'});
  nativeTrace(event).diagnostics=[{code:'node_limit'}, {code:'frame_limit'}];
  const root=f.render(event);await expand(root);assert.match(root.textContent,/incompleta/);
  assert.match(root.textContent,/Limite de exceções/);assert.match(root.textContent,/Limite de frames/);
  assert.doesNotMatch(root.textContent,/unreliable/);
  for(const value of matching(root,'.java-trace-scalar-value'))await dispatch(value,'contextmenu');
  assert.deepEqual(f.scalarMenus.map(item=>item.column).sort(),['java.exception.class','java.exception.message']);
});

test('copy completion never reports success or failure after the captured detail becomes stale',async()=>{
  for(const result of ['resolve','reject']){
    const f=fixture();let release;const completions=[],pending=new Promise((resolve,reject)=>{release=result==='resolve'?resolve:reject;});
    const root=f.render(traceEvent({frameCount:1}),{copy:()=>pending,copied:()=>completions.push('success'),copyFailed:()=>completions.push('failure')});
    await expand(root);const node=matching(root,'.java-trace-node')[0];await expand(node);
    await dispatch(matching(node,'.java-trace-frame')[0],'contextmenu');
    const copy=f.menus.at(-1).items.find(item=>/copiar/i.test(item.label)).onClick();
    f.stale();release(result==='reject'?new Error('clipboard unavailable'):undefined);await copy;
    assert.deepEqual(completions,[],result);
  }
});

test('the installed drawer wiring opts into native detail without storing rich traces in Event',async()=>{
  const start=appSource.indexOf('  const javaRequest = detailRequest;');
  const end=appSource.indexOf('  $("#drawer").hidden = false;',start);
  assert.ok(start>=0&&end>start,'the Java block is installed in the real detail drawer');
  function installed({mode='success',admission={scope:'case',request:4}}={}){
    const event=traceEvent(),original='literal <message> '+ '🔥'.repeat(3000);
    Object.assign(event.fields,{'java.exception.message':original,stacktrace:['legacy frame one','legacy frame two']});
    const before=structuredClone(event);
    const pane=()=>({textContent:'old text',innerHTML:'old html',children:[],appendChild(child){this.children.push(child);}});
    const tree={kind:'generic-tree'},specialized={kind:'java-details'},diagnostics={kind:'diagnostics'},drawer={hidden:false},overview=pane(),json=pane(),raw=pane();
    const nodes={'#drawer':drawer,'#pane-overview':overview,'#pane-json':json,'#pane-raw':raw};
    const captured={options:null,event:null,tree:null,menus:[],tabs:[],admissions:[],contextMenus:[],redactions:[],highlights:[],originals:[],loads:[],cancelled:[]};
    const evidence=[event];
    let admissionCurrent=true;
    const state={currentDetailEv:event};
    const EvidenceUI={redact(value){captured.redactions.push(value);return value;}};
    const FieldTransforms={renderDiagnostics:ev=>{assert.equal(ev,event);return diagnostics;}};
    const window={EvidenceUI,FieldTransforms,Tasks:{cancelLatest:key=>captured.cancelled.push(key)}};
    if(mode!=='unavailable')window.JavaTrace={render(ev,options){captured.event=ev;captured.options=options;return mode==='success'?specialized:null;}};
    const context=vm.createContext({ev:event,admission,state,detailRequest:4,detailDeferredPane:null,window,EvidenceUI,
      $:selector=>{assert.ok(Object.hasOwn(nodes,selector),selector);return nodes[selector];},fmtTsFull:()=> 'formatted timestamp',
      caseEvents:()=>evidence,loadJavaTraceDetail:async(...args)=>{captured.loads.push(args);return available(event);},
      highlightJson:value=>{captured.highlights.push(value);return 'highlighted json';},
      renderDetailTree(received,collapsed){captured.tree={rows:received,collapsed};return tree;},
      detailAdmissionCurrent(received){captured.admissions.push(received);return admissionCurrent;},
      switchDetailTab:tab=>captured.tabs.push(tab),showDetailValueMenu:(domEvent,item)=>captured.menus.push({domEvent,item}),
      openDetailValue:(value,anchor)=>captured.originals.push({value,anchor}),
      showCtxMenu:(x,y,items)=>captured.contextMenus.push({x,y,items}),toast(){},
    });
    vm.runInContext(appSource.slice(start,end),context,{filename:'app.js:Java drawer wiring'});
    return{event,before,evidence,original,tree,specialized,diagnostics,drawer,overview,json,raw,captured,state,context,admission,expireAdmission(){admissionCurrent=false;}};
  }
  for(const mode of ['success','rejected','unavailable']){
    const f=installed({mode}),success=mode==='success';
    assert.equal(f.captured.tree.rows.some(row=>row.key==='java.trace'),false,'Event has no rich Java field in any renderer path');
    for(const key of ['stacktrace']){
      assert.equal(f.captured.tree.rows.some(row=>row.key===key),!success,`${mode}: ${key} generic rendering`);
      assert.equal(f.captured.redactions.some(value=>value&&typeof value==='object'&&Object.keys(value).length===1&&Object.hasOwn(value,key)),!success,`${mode}: ${key} generic redaction`);
    }
    assert.ok(f.captured.tree.rows.some(row=>row.key==='java.exception.message'),'literal scalar fields remain in the normal tree');
    assert.deepEqual([...f.captured.tree.collapsed],success?['java.trace']:[]);
    assert.deepEqual(f.overview.children,success?[f.diagnostics,f.specialized,f.tree]:[f.diagnostics,f.tree]);
    if(mode!=='unavailable'){assert.equal(f.captured.event,f.event);assert.equal(f.captured.options.admission,f.admission);}
    assert.deepEqual(f.event,f.before,'all original fields remain on the admitted Event');
    assert.equal(f.captured.loads.length,0,'constructing the normal drawer never issues native Java detail');
    assert.equal(f.captured.highlights.length,success?0:1);
    assert.equal(f.captured.redactions.includes(f.event),!success);assert.equal(f.captured.redactions.includes(f.event.raw),!success);
    if(success){
      assert.equal(typeof f.context.detailDeferredPane,'function');assert.equal(f.json.textContent,'');assert.equal(f.raw.textContent,'');
      f.context.detailDeferredPane('overview');assert.equal(f.captured.highlights.length,0);
      f.context.detailDeferredPane('json');assert.equal(f.captured.highlights.length,1);assert.equal(f.captured.highlights[0],f.event);
      assert.equal(f.json.innerHTML,'highlighted json');assert.equal(f.captured.redactions.includes(f.event.raw),false);
      f.context.detailDeferredPane('raw');assert.equal(f.raw.textContent,f.event.raw);assert.equal(f.captured.redactions.filter(value=>value===f.event.raw).length,1);
      f.context.detailDeferredPane('json');f.context.detailDeferredPane('raw');assert.equal(f.captured.highlights.length,1);
      assert.equal(f.captured.redactions.filter(value=>value===f.event.raw).length,1,'each deferred pane formats at most once');
    }else assert.equal(f.context.detailDeferredPane,null,'ordinary fallback details retain their existing eager panes');
  }
  const f=installed(),options=f.captured.options;
  assert.equal(options.isCurrent(),true);assert.equal(f.captured.admissions.at(-1),f.admission);
  const response=await options.load();assert.equal(response.state,'available');assert.equal(f.captured.loads.length,1);
  assert.equal(f.captured.loads[0][0],f.event);assert.equal(f.captured.loads[0][1],f.admission);assert.equal(f.captured.loads[0][2],f.evidence);
  assert.equal(f.captured.loads[0][3],options.isCurrent);assert.equal(Object.hasOwn(f.event.fields,'java.trace'),false);
  options.cancel();assert.deepEqual(f.captured.cancelled,['java-trace-detail']);
  options.raw();assert.deepEqual(f.captured.tabs,['raw'],'source action selects the existing raw tab');
  const anchor={getBoundingClientRect:()=>({left:11,bottom:29})};
  let prevented=0,stopped=0;
  options.scalarMenu({currentTarget:anchor,target:{},clientX:0,clientY:0,preventDefault(){prevented++;},stopPropagation(){stopped++;}},'java.exception.message');
  const menu=f.captured.menus[0];
  assert.deepEqual(plain(menu.item),{path:'java.exception.message',original:true,hasValue:true,value:f.original,filterValue:f.original});
  assert.equal(menu.domEvent.target,anchor);assert.equal(menu.domEvent.clientX,11);assert.equal(menu.domEvent.clientY,29);
  menu.domEvent.preventDefault();menu.domEvent.stopPropagation();assert.equal(prevented,1);assert.equal(stopped,1);
  options.original('stacktrace',anchor);const original=f.captured.originals[0];
  assert.equal(original.anchor,anchor);assert.equal(original.value.value,f.event.fields.stacktrace);assert.equal(original.value.filterValue,f.event.fields.stacktrace);
  assert.deepEqual(plain(original.value),{path:'stacktrace',original:true,hasValue:true,value:f.event.fields.stacktrace,filterValue:f.event.fields.stacktrace,mono:true});
  assert.deepEqual(f.event,f.before,'explicit original-stacktrace access also preserves evidence');
  const virtualItems=[{label:'Copiar frame',onClick(){}}];options.menu({clientX:3,clientY:4},virtualItems);
  assert.equal(f.captured.contextMenus[0].items,virtualItems);
  f.context.detailRequest++;assert.equal(options.isCurrent(),false,'a replacement detail request expires the Java view');f.context.detailRequest--;
  f.state.currentDetailEv={...f.event};assert.equal(options.isCurrent(),false,'same ID in another Event object is insufficient');f.state.currentDetailEv=f.event;
  f.drawer.hidden=true;assert.equal(options.isCurrent(),false,'a closed drawer cannot retain active Java actions');f.drawer.hidden=false;
  assert.equal(options.isCurrent(),true);f.expireAdmission();assert.equal(options.isCurrent(),false,'expired Case/source admission expires the Java view');
  for(const changed of ['request','event','hidden','admission']){
    const stale=installed(),deferred=stale.context.detailDeferredPane;
    if(changed==='request')stale.context.detailRequest++;
    if(changed==='event')stale.state.currentDetailEv={...stale.event};
    if(changed==='hidden')stale.drawer.hidden=true;
    if(changed==='admission')stale.expireAdmission();
    deferred('json');deferred('raw');assert.equal(stale.captured.highlights.length,0,changed);
    assert.equal(stale.captured.redactions.includes(stale.event.raw),false,changed);
    assert.equal(stale.json.textContent,'',changed);assert.equal(stale.raw.textContent,'',changed);
  }
  const saved=installed({admission:null});assert.equal(saved.captured.options.isCurrent(),true);
  assert.equal(saved.captured.admissions.length,0,'explicit saved evidence uses its captured request and Event without live admission');
  assert.equal(saved.captured.options.load,null,'historical evidence never looks up a live Event by ID');assert.equal(saved.captured.loads.length,0);
  const dataset=installed({admission:{scope:'dataset'}});await dataset.captured.options.load();assert.equal(dataset.captured.loads[0][2],null);
});

function nativeLoaderFixture(scope='case'){
  const start=appSource.indexOf('async function loadJavaTraceDetail('),end=appSource.indexOf('async function openDetail(',start);
  assert.ok(start>=0&&end>start);
  const event=traceEvent(),evidence=scope==='case'?[event]:null;
  const admission={scope,owner:{identity:{caseId:'case-a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:9}};
  const requests=[],preparations=[],hooks={};let current=true,response=available(event);
  const context=vm.createContext({
    caseArgs:async(args,retry,publication,options)=>{
      preparations.push({args,retry,publication,options});if(hooks.prepare)await hooks.prepare();
      const {caseEvents,...identity}=args;return{...identity,caseKey:'captured-case-key',caseContentToken:'captured-content-token'};
    },
    api:async(command,args,options)=>{requests.push({command,args,options});return hooks.request?hooks.request():response;},
  });
  vm.runInContext(appSource.slice(start,end),context,{filename:'app.js:loadJavaTraceDetail'});
  return{event,evidence,admission,requests,preparations,hooks,expire(){current=false;},respond(value){response=value;},
    read:()=>context.loadJavaTraceDetail(event,admission,evidence,()=>current)};
}

test('the installed native loader uses the captured admission, Case token and exact Event identity',async()=>{
  for(const scope of ['case','dataset']){
    const f=nativeLoaderFixture(scope),before=structuredClone(f.event),result=await f.read();
    assert.equal(result.state,'available');assert.equal(result.trace,nativeTrace(f.event));assert.equal(f.requests.length,1);
    const {command,args,options}=f.requests[0];assert.equal(command,'java_trace_detail');
    assert.equal(args.id,f.event.id);assert.equal(args.eventRef,f.event.event_ref);assert.equal(args.analysisContext,f.admission.owner.identity);
    assert.equal(args.sourceGeneration,9);assert.equal(options.latest,'java-trace-detail');assert.equal(options.silent,true);
    assert.equal(options.analysisOwner,f.admission.owner);assert.equal(options.caseEvents,f.evidence);assert.equal(Object.hasOwn(args,'caseEvents'),false);
    if(scope==='case'){
      assert.equal(f.preparations.length,1);assert.equal(f.preparations[0].args.caseEvents,f.evidence);assert.equal(f.preparations[0].options.canonical,true);
      assert.equal(args.caseKey,'captured-case-key');assert.equal(args.caseContentToken,'captured-content-token');
    }else{assert.equal(f.preparations.length,0);assert.equal(args.caseKey,undefined);}
    assert.deepEqual(f.event,before);assert.equal(Object.hasOwn(f.event.fields,'java.trace'),false);
    f.expire();assert.throws(options.onCasePrepared,/contexto mudou/);
  }
});

test('native Java detail rejects malformed envelopes and replacement rows without storing any response on Event',async()=>{
  const good=available(traceEvent());
  const malformed=[null,{}, {state:'unknown'}, {...good,row:{id:99,eventRef:'source:7'}}, {...good,row:{id:7,eventRef:'replacement:7'}},
    {...good,row:null}, {...good,reason:'not_java'}, {...good,trace:null},
    {state:'unavailable',trace:null,reason:'not_java',row:null},
    {state:'unavailable',trace:{},reason:'record_unavailable',row:null}];
  for(const response of malformed){
    const f=nativeLoaderFixture(),before=structuredClone(f.event);f.respond(response);
    await assert.rejects(f.read(),/estrutura|registro solicitado/);assert.deepEqual(f.event,before);
  }
  for(const reason of ['record_unavailable','raw_unavailable','not_java']){
    const f=nativeLoaderFixture(),response={state:'unavailable',trace:null,reason,row:reason==='record_unavailable'?null:{id:f.event.id,eventRef:f.event.event_ref}};
    f.respond(response);assert.equal(await f.read(),response);assert.equal(Object.hasOwn(f.event.fields,'java.trace'),false);
  }
});

test('expired ownership stops native Java detail before dispatch, after Case preparation and after the reply',async()=>{
  const initial=nativeLoaderFixture();initial.expire();await assert.rejects(initial.read(),/contexto mudou/);
  assert.equal(initial.preparations.length,0);assert.equal(initial.requests.length,0);
  for(const stage of ['preparation','reply']){
    const f=nativeLoaderFixture(),gate=deferred();
    if(stage==='preparation')f.hooks.prepare=()=>gate.promise;else f.hooks.request=()=>gate.promise;
    const read=f.read(),rejected=assert.rejects(read,/contexto mudou/);await settle();f.expire();gate.resolve(available(f.event));await rejected;
    assert.equal(f.requests.length,stage==='preparation'?0:1);assert.equal(Object.hasOwn(f.event.fields,'java.trace'),false);
  }
});
