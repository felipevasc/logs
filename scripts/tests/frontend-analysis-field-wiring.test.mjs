import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8');
const app=read('app.js'),workbench=read('analysis-workbench.js'),workspace=read('workspace-context.js');
const part=(source,start,end)=>source.slice(source.indexOf(start),source.indexOf(end,source.indexOf(start)));
const plain=value=>JSON.parse(JSON.stringify(value));

function fixture(scope='case'){
  const nodes=new Map(),listeners=new Map(),cases=new Map(),calls={native:[],saves:[],renders:[],focus:[],notices:[],timers:[]};
  let evidence='case-evidence-a';
  const state={analyticsScope:scope,columns:['timestamp','message','payload','decoded'],derivedFields:[{name:'decoded',source:'payload',steps:['parse_json']}],
    analysisDefinitionsPending:false,loaded:true,loadOverlay:false,datasetDashboard:[],groupCol:'message',aggs:[{func:'count',column:'*',alias:'Registros'}],
    rows:[],visibleCols:['timestamp'],treeAgg:{},treeAggSig:{},treeAggError:{},
    owner:{caseId:'case-a',instance:1,identity:{caseId:'case-a',analysisId:'analysis-a',configRevision:2,visibilityRevision:3},sourceGeneration:7,sourceKey:'source-a:7'}};
  function node(tag='div',className='',textContent=''){
    const classes=new Set(),item={tag,className,textContent,children:[],dataset:{},attrs:{},listeners:{},hidden:false,disabled:false,isConnected:true,_value:'',
      classList:{add:name=>classes.add(name),remove:name=>classes.delete(name),contains:name=>classes.has(name),toggle(name,on){if(on??!classes.has(name))classes.add(name);else classes.delete(name);}},
      setAttribute(name,value){this.attrs[name]=String(value);},getAttribute(name){return this.attrs[name];},
      append(...children){for(const child of children){child.parentElement=this;this.children.push(child);}},appendChild(child){this.append(child);return child;},
      prepend(...children){this.children.unshift(...children);},replaceChildren(...children){this.children=[];this.append(...children);},
      addEventListener(type,fn){(this.listeners[type]||=[]).push(fn);},
      dispatchEvent(event){this[`on${event.type}`]?.(event);for(const callback of this.listeners[event.type]||[])callback(event);},
      get options(){return this.children;},get selectedOptions(){return this.children.filter(option=>option.value===this.value);},
      get value(){return this.tag==='select'&&!this.children.some(option=>option.value===this._value)?'':this._value;},set value(value){this._value=String(value);},
      set innerHTML(_html){this.children=[];},querySelector:selector=>$(selector),querySelectorAll:()=>[],
      focus(){calls.focus.push(this);},closest(){return null;},getBoundingClientRect:()=>({left:10,top:10,right:160,bottom:40,width:150,height:30}),
    };return item;
  }
  const $=selector=>{if(!nodes.has(selector))nodes.set(selector,node(['#cp-field','#cp-split','#group-col'].includes(selector)?'select':selector==='#np-val'?'input':'div'));return nodes.get(selector);};
  const document={body:node('body'),querySelector:$,querySelectorAll:()=>[],
    addEventListener(type,fn){const callbacks=listeners.get(type)||[];callbacks.push(fn);listeners.set(type,callbacks);},
    dispatchEvent(event){for(const callback of listeners.get(event.type)||[])callback(event);},
  };
  const currentCase=()=>{if(!cases.has(state.owner.caseId))cases.set(state.owner.caseId,{id:state.owner.caseId,caseDashboard:[],workspace:{}});return cases.get(state.owner.caseId);};
  const capture=()=>structuredClone(state.owner);
  const context=vm.createContext({state,$,el:node,document,TextEncoder,structuredClone,scope,
    window:{AnalysisContexts:{capture,isCurrent:owner=>JSON.stringify(owner)===JSON.stringify(state.owner)}},
    workspaceScope:()=>context.scope,caseSig:()=>evidence,activeCase:currentCase,ensureCase:currentCase,
    colLabel:String,fmtNum:String,trunc:String,positionPop(){},filterFocusTarget:anchor=>anchor,
    toast:(...args)=>calls.notices.push(args),saveCases:()=>calls.saves.push(state.owner.caseId),renderDashboard:area=>calls.renders.push(area),
    api:(...args)=>{calls.native.push(args);throw Error('unexpected native calculation');},
    setTimeout:callback=>{calls.timers.push(callback);return calls.timers.length;},clearTimeout(){},
    scopeProfiles:()=>[],scopeHasEvents:()=>state.loaded,STANDARD:['timestamp','level','source','code'],
    renderChips(){},renderExploreTree(){},updateContextBar(){},restoreVisiblePreferences(){},
    runtime:new Map(),key:()=>`${state.owner.caseId}:${context.scope}`,caseEvents:()=>[{id:0}],caseEventsCache:{summary:{columns:[...state.columns]}},
    explorerAnalytics:new Map(),explorerKey:()=> 'current',treeAggVersion:{dataset:0,case:0},cubeState:{collapsed:new Set(),requestVersion:0},
    AGG_FUNCS:[['count','Contagem'],['sum','Soma'],['count_distinct','Valores únicos']],
  });
  vm.runInContext(part(app,'let namePopCb =','\nlet editingCaseItem ='),context);
  vm.runInContext(read('analysis-fields.js'),context,{filename:'analysis-fields.js'});
  vm.runInContext(part(app,'const DASH_TYPE_LABELS =','function dashboardTypeLabel('),context);
  vm.runInContext(part(app,'function dashboardCharts(','function scopeProfiles('),context);
  vm.runInContext(part(app,'let chartEditing =','// =========================================================================='),context);
  vm.runInContext(part(workbench,'  const measureLabels =','  function filterGroup('),context);
  vm.runInContext(part(app,'function fillColumnControls(','async function openTsModal('),context);
  vm.runInContext(part(workspace,'  const copy =','  function capture()'),context);
  vm.runInContext(part(workspace,'  function apply(snapshot)','  function invalidateAnalysis('),context);
  vm.runInContext(part(app,'  $("#np-ok").onclick =','\n  $("#btn-colpicker").onclick ='),context);
  const selectExact=(select,field)=>{
    const sentinel=select.options.find(option=>option.textContent==='Usar caminho exato…');assert.ok(sentinel);
    select.value=sentinel.value;select.dispatchEvent({type:'change',target:select});
    $('#np-val').value=field;context.commitNamePop();
  };
  const emit=type=>document.dispatchEvent({type});
  return{state,context,$,calls,cases,node,selectExact,emit,evidence:value=>{evidence=value;}};
}

const chartSpec=(field=null,split=null)=>({id:'chart-a',title:'Saved chart',chart:'time',metric:'count',field,split,type:'line',interval_ms:null});

test('installed chart editor selects rare child fields through the exact-path popover and saves them',()=>{
  const f=fixture(),spec=chartSpec(),before=[...f.state.columns];f.context.openChartEditor(spec,f.node('button'),'case');
  f.selectExact(f.$('#cp-field'),'decoded.rare.metric');f.selectExact(f.$('#cp-split'),'decoded.rare.category');
  assert.equal(f.$('#cp-field').value,'decoded.rare.metric');assert.equal(f.$('#cp-split').value,'decoded.rare.category');
  assert.equal(spec.field,null,'editing selectors does not modify the authored chart before Save');assert.equal(f.calls.native.length,0);
  f.context.applyChartEditor();
  assert.equal(spec.field,'decoded.rare.metric');assert.equal(spec.split,'decoded.rare.category');
  assert.equal(f.cases.get('case-a').caseDashboard[0],spec);assert.deepEqual(f.calls.saves,['case-a']);
  assert.deepEqual(f.calls.renders,['case']);assert.deepEqual(f.state.columns,before);assert.equal(f.$('#chart-modal').hidden,true);
});

test('installed chart editor preserves unavailable authored field and split instead of saving null',()=>{
  for(const scope of ['case','dataset']){
    const f=fixture(scope),spec=chartSpec('foreign.saved.metric','foreign.saved.category');
    const charts=f.context.dashboardCharts(scope);charts.push(spec);f.context.openChartEditor(spec,f.node('button'),scope);
    for(const [selector,field] of [['#cp-field',spec.field],['#cp-split',spec.split]]){
      assert.equal(f.$(selector).value,field);assert.equal(f.$(selector).options.find(option=>option.value===field).disabled,true);
    }
    f.$('#cp-title').value='Retitled saved chart';f.context.applyChartEditor();
    assert.equal(spec.field,'foreign.saved.metric');assert.equal(spec.split,'foreign.saved.category');assert.equal(spec.title,'Retitled saved chart');
    assert.equal(charts.length,1);assert.equal(f.calls.native.length,0);
  }
});

test('installed chart Save cannot mutate its original chart or another Case after ownership changes',()=>{
  for(const change of ['Case','configuration','source','scope','evidence']){
    const f=fixture(),spec=chartSpec('decoded.rare','message'),before=structuredClone(spec);
    f.context.dashboardCharts('case').push(spec);f.context.openChartEditor(spec,f.node('button'),'case');f.$('#cp-title').value='Wrong context edit';
    if(change==='Case'){f.state.owner.caseId='case-b';f.state.owner.identity.caseId='case-b';f.state.owner.instance++;}
    if(change==='configuration')f.state.owner.identity.configRevision++;
    if(change==='source'){f.state.owner.sourceGeneration++;f.state.owner.sourceKey='source-b:8';}
    if(change==='scope')f.context.scope='dataset';if(change==='evidence')f.evidence('case-evidence-b');
    f.context.applyChartEditor();assert.deepEqual(spec,before,change);assert.equal(f.calls.saves.length,0,change);assert.equal(f.calls.renders.length,0,change);
    assert.match(f.calls.notices.at(-1)[0],/contexto mudou/);assert.equal(f.calls.native.length,0);
    if(change==='Case')assert.equal(f.context.dashboardCharts('case').length,0,'the replacement Case receives no old chart');
  }
});

test('installed workspace restore and group controls retain an unavailable authored dimension and measure',async()=>{
  for(const scope of ['case','dataset']){
    const f=fixture(scope),saved={values:{groupCol:'foreign.saved.dimension',aggs:[{func:'sum',column:'foreign.saved.measure',alias:'Saved sum'}]}};
    f.context.apply(f.context.sanitize(saved));
    assert.equal(f.state.groupCol,'foreign.saved.dimension');assert.equal(f.$('#group-col').value,'foreign.saved.dimension');
    assert.ok(f.$('#group-col').options.find(option=>option.value==='foreign.saved.dimension').disabled);
    const measure=f.$('#agg-list').children[0].children[1];assert.equal(measure.value,'foreign.saved.measure');
    assert.ok(measure.options.find(option=>option.value==='foreign.saved.measure').disabled);
    await f.context.runGroup();assert.equal(f.state.groupCol,'foreign.saved.dimension');assert.equal(f.calls.native.length,0);
    assert.match(f.$('#aw-group-summary').textContent,/indisponível/);
    assert.deepEqual(plain(f.state.aggs),saved.values.aggs,'restoration and unavailable calculation preserve authored measures');
  }
});

test('installed workspace restore offers rare active-derived dimensions without substituting sampled fields',()=>{
  const f=fixture();f.context.apply(f.context.sanitize({values:{groupCol:'decoded.rare.dimension'}}));
  assert.equal(f.state.groupCol,'decoded.rare.dimension');assert.equal(f.$('#group-col').value,'decoded.rare.dimension');
  assert.equal(f.$('#group-col').options.find(option=>option.value==='decoded.rare.dimension').disabled,false);
  assert.equal(f.calls.native.length,0);
});

test('installed measure entry commits before the real shared field-change listener rebuilds its controls',()=>{
  const f=fixture(),field='decoded.rare.value',notifications=[];
  f.context.CustomEvent=class{constructor(type){this.type=type;}};
  f.$('#view-cube').hidden=true;f.$('#chart-modal').hidden=true;
  f.context.document.addEventListener('analysis-fields-change',()=>notifications.push(f.state.aggs[0].column));
  vm.runInContext(part(workbench,'  document.addEventListener?.("analysis-fields-change"','  groupShell(); renderAggs(); pivotShell();'),f.context);
  f.state.aggs=[{func:'sum',column:'payload',alias:'Measure'}];f.context.fillColumnControls();
  const box=f.$('#agg-list'),original=box.children[0].children[1],replace=box.replaceChildren;let rebuilds=0;
  box.replaceChildren=function(...children){rebuilds++;return replace.apply(this,children);};
  f.selectExact(original,field);
  const visible=box.children[0].children[1];
  assert.notEqual(visible,original,'the installed shared listener replaces the measure control');
  assert.equal(f.state.aggs[0].column,field);assert.equal(visible.value,field,'the replacement control reflects the committed authored field');
  assert.equal(visible.options.find(option=>option.value===field).disabled,false);
  assert.deepEqual(notifications,[field],'shared listeners see the authored selection after commit');
  assert.equal(rebuilds,1,'registration triggers only one measure-control rebuild');
  assert.equal(f.context.window.AnalysisFields.names().includes(field),true);assert.equal(f.state.columns.includes(field),false);
  assert.equal(f.$('#name-pop').hidden,true);assert.equal(f.calls.notices.length,0);assert.equal(f.calls.native.length,0);
});

test('reopening chart editors and refilling group controls preserve rare and unavailable authored options',()=>{
  for(const scope of ['case','dataset'])for(const prefix of ['decoded.rare','foreign.saved']){
    const f=fixture(scope),field=`${prefix}.metric`,split=`${prefix}.category`,spec=chartSpec(field,split),unavailable=prefix==='foreign.saved';
    f.context.dashboardCharts(scope).push(spec);f.state.groupCol=split;f.state.aggs=[{func:'sum',column:field,alias:'Saved measure'}];
    for(let repeat=0;repeat<3;repeat++){
      f.context.openChartEditor(spec,f.node('button'),scope);f.context.fillColumnControls();
      const controls=[[f.$('#cp-field'),field],[f.$('#cp-split'),split],[f.$('#group-col'),split],[f.$('#agg-list').children[0].children[1],field]];
      for(const [select,value] of controls){
        assert.equal(select.value,value,`${scope} ${value} survives refresh ${repeat}`);
        assert.equal(select.options.filter(option=>option.value===value).length,1);
        assert.equal(select.options.find(option=>option.value===value).disabled,unavailable);
      }
    }
    f.context.applyChartEditor();assert.equal(spec.field,field);assert.equal(spec.split,split);
    assert.equal(f.state.groupCol,split);assert.equal(f.state.aggs[0].column,field);assert.equal(f.calls.native.length,0);
  }
});

test('installed name popover preserves exact field bytes, keeps invalid drafts and still trims normal names',()=>{
  const f=fixture(),anchor=f.node('button'),values=[];let accept=false;
  f.context.openNamePop(anchor,value=>{values.push(value);return accept;},{exact:true,confirmLabel:'Usar',initialValue:'  exact path  '});
  assert.equal(f.$('#np-ok').textContent,'Usar');f.context.commitNamePop();
  assert.deepEqual(values,['  exact path  ']);assert.equal(f.$('#name-pop').hidden,false);assert.equal(f.$('#np-val').value,'  exact path  ');
  f.$('#np-val').value='   ';f.context.commitNamePop();assert.equal(values.length,1,'wholly blank exact draft remains editable');
  f.$('#np-val').value='  exact path  ';accept=true;f.context.commitNamePop();assert.equal(f.$('#name-pop').hidden,true);assert.equal(f.calls.focus.at(-1),anchor);
  f.context.openNamePop(anchor,value=>values.push(value));f.$('#np-val').value='  renamed group  ';f.context.commitNamePop();
  assert.equal(values.at(-1),'renamed group');assert.equal(f.$('#name-pop').hidden,true);assert.equal(f.calls.native.length,0);
});

test('installed field-entry keyboard respects IME and retries an invalid exact draft without losing spaces',()=>{
  const f=fixture(),spec=chartSpec();f.context.openChartEditor(spec,f.node('button'),'case');
  const field=f.$('#cp-field'),sentinel=field.options.find(option=>option.textContent==='Usar caminho exato…');field.value=sentinel.value;field.dispatchEvent({type:'change'});
  f.$('#np-val').value='foreign.unknown';f.$('#np-ok').onclick();assert.equal(f.$('#name-pop').hidden,false);assert.equal(f.$('#np-val').value,'foreign.unknown');
  f.$('#np-val').value='decoded.rare with spaces  ';
  const enter=extra=>f.$('#np-val').dispatchEvent({type:'keydown',key:'Enter',preventDefault(){},...extra});
  enter({isComposing:true});enter({keyCode:229});enter({repeat:true});assert.equal(field.value,'');assert.equal(f.$('#name-pop').hidden,false);
  enter({});assert.equal(field.value,'decoded.rare with spaces  ');assert.equal(f.$('#name-pop').hidden,true);assert.equal(f.calls.native.length,0);
});

test('installed exact-path popover closes on ownership change without choosing into a replacement chart',()=>{
  const f=fixture(),spec=chartSpec();f.context.openChartEditor(spec,f.node('button'),'case');
  const field=f.$('#cp-field'),sentinel=field.options.find(option=>option.textContent==='Usar caminho exato…');field.value=sentinel.value;field.dispatchEvent({type:'change'});
  f.$('#np-val').value='decoded.pending';f.state.owner.identity.configRevision++;f.emit('analysis-context-change');
  assert.equal(f.$('#name-pop').hidden,true);f.context.commitNamePop();assert.equal(field.value,'');assert.equal(spec.field,null);
  assert.equal(f.context.window.AnalysisFields.names().includes('decoded.pending'),false);assert.equal(f.calls.native.length,0);
});

test('active visibility receipts rebind live group controls locally while preserving an open chart draft owner',()=>{
  for(const scope of ['case','dataset']){
    const f=fixture(scope),spec=chartSpec('decoded.saved.metric');
    f.context.CustomEvent=class{constructor(type){this.type=type;}};
    f.$('#view-cube').hidden=true;
    vm.runInContext(part(workbench,'  document.addEventListener?.("analysis-fields-change"','  groupShell(); renderAggs(); pivotShell();'),f.context);
    f.context.openChartEditor(spec,f.node('button'),scope);f.context.fillColumnControls();
    f.$('#cp-title').value='Unsaved title';const select=f.$('#group-col');
    f.state.owner.identity.visibilityRevision++;
    f.context.document.dispatchEvent({type:'analysis-context-change',detail:{active:true,configChanged:false,visibilityChanged:true}});
    assert.equal(f.calls.native.length,0);assert.equal(f.calls.timers.length,0,'receipt rebinding schedules no calculation');
    select.value='payload';select.dispatchEvent({type:'change',target:select});
    assert.equal(f.state.groupCol,'payload','the existing control now accepts the current receipt owner');
    assert.equal(f.calls.native.length,0);
    assert.equal(f.$('#cp-title').value,'Unsaved title');assert.equal(f.$('#cp-field').value,'decoded.saved.metric');
    f.context.applyChartEditor();assert.equal(spec.title,'Saved chart','a receipt never rebinds an open chart draft');
    assert.equal(f.calls.saves.length,0);assert.equal(f.calls.renders.length,0);
  }
});

test('actual definitionsLoaded rebinds controls only after matching definitions are ready',()=>{
  const f=fixture(),contexts=read('analysis-context.js');
  f.context.CustomEvent=class{constructor(type){this.type=type;}};f.$('#view-cube').hidden=true;f.$('#chart-modal').hidden=true;
  vm.runInContext(part(workbench,'  document.addEventListener?.("analysis-fields-change"','  groupShell(); renderAggs(); pivotShell();'),f.context);
  f.context.fillColumnControls();const select=f.$('#group-col');
  f.state.owner.identity.configRevision++;f.state.analysisDefinitionsPending=true;f.state.derivedFields=[];
  f.context.document.dispatchEvent({type:'analysis-context-change',detail:{active:true,configChanged:true}});
  assert.equal(select.options.some(option=>option.value==='new_parent'),false);assert.equal(f.calls.native.length,0);
  f.context.assertOwner=owner=>assert.deepEqual(plain(owner),f.state.owner);
  f.context.definitionOwnerKey=owner=>JSON.stringify(owner);f.context.definitionKey=null;f.context.activeDefinitionOwner=null;
  vm.runInContext(part(contexts,'  function definitionsLoaded(','  async function receipt('),f.context);
  f.context.definitionsLoaded(structuredClone(f.state.owner),[{name:'new_parent',source:'payload',steps:['parse_json']}]);
  assert.equal(f.state.analysisDefinitionsPending,false);assert.ok(select.options.some(option=>option.value==='new_parent'));
  assert.equal(f.calls.native.length,0);assert.equal(f.calls.timers.length,0,'definitions readiness only rebuilds local choices');
  f.selectExact(select,'new_parent.rare');assert.equal(f.state.groupCol,'new_parent.rare');assert.equal(f.calls.native.length,0);
});
