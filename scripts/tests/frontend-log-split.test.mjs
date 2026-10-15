import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';

const read=path=>readFileSync(new URL(`../../${path}`,import.meta.url),'utf8');

function load(){
  const context={window:{},document:{addEventListener(){},activeElement:null},esc:value=>String(value),fmtNum:String,fmtTsFull:String,toast(){},api:async()=>null,confirm:()=>true,Date,Number,Math,JSON,console};
  context.window.window=context.window;
  vm.createContext(context);
  vm.runInContext(read('frontend/log-split.js'),context);
  return context.window.LogSplit;
}

test('the request carries items, criteria, combination and mode', ()=>{
  const split=load(), ui=split.state;
  ui.framing={kind:'startRegex',pattern:'^\\d{4}-',ignoreCase:true,text:''};
  ui.conditions=[
    {id:1,kind:'text',negate:false,op:'startsWith',values:'ERROR\r\n\nWARN',ignoreCase:false,scope:'firstLine'},
    {id:2,kind:'date',negate:true,from:'2026-10-12T10:00',to:'2026-10-12T11:00:30',zone:'utc',locate:'regex',pattern:'ts=(\\S+)',format:'dd/MM/yyyy',order:'dayFirst',missing:'noMatch'},
    {id:3,kind:'ordinal',negate:false,unit:'line',start:'10',end:''},
  ];
  ui.combine='any'; ui.sorted=true; ui.mode='move';
  const {spec,error}=split.buildSpec();
  assert.equal(error,undefined);
  assert.deepEqual(JSON.parse(JSON.stringify(spec)),{
    framing:{kind:'startRegex',pattern:'^\\d{4}-',ignoreCase:true},
    mode:'move',
    rule:{combine:'any',sorted:true,conditions:[
      {kind:'text',negate:false,op:'startsWith',values:['ERROR','WARN'],ignoreCase:false,scope:'firstLine'},
      {kind:'date',negate:true,from:Date.UTC(2026,9,12,10,0,0),to:Date.UTC(2026,9,12,11,0,30),locate:'regex',pattern:'ts=(\\S+)',format:'dd/MM/yyyy',order:'dayFirst',zone:'utc',missing:'noMatch'},
      {kind:'ordinal',negate:false,unit:'line',from:10,to:null},
    ]},
  });
  // Local wall-clock dates use this computer's zone; sorted needs a date criterion.
  ui.conditions=[{id:4,kind:'date',negate:false,from:'2026-10-12T10:00',to:'',zone:'local',locate:'auto',pattern:'',format:'',order:'auto',missing:'previous'}];
  assert.equal(split.buildSpec().spec.rule.conditions[0].from,new Date(2026,9,12,10,0,0).getTime());
  ui.conditions=[{id:5,kind:'text',negate:false,op:'contains',values:'x',ignoreCase:true,scope:'item'}];
  assert.equal(split.buildSpec().spec.rule.sorted,false);
});

test('incomplete criteria are explained before anything runs', ()=>{
  const split=load(), ui=split.state;
  const error=conditions=>{ui.conditions=conditions;return split.buildSpec().error;};
  ui.framing={kind:'auto',pattern:'',ignoreCase:false,text:''};
  assert.match(error([{id:1,kind:'text',values:'\n',op:'contains'}]),/Critério 1: informe o texto/);
  assert.match(error([{id:1,kind:'regex',pattern:''}]),/expressão regular/);
  assert.match(error([{id:1,kind:'date',from:'',to:'',zone:'local',locate:'auto'}]),/a partir de/);
  assert.match(error([{id:1,kind:'date',from:'2026-10-12T10:00',to:'',zone:'local',locate:'field',pattern:''}]),/campo JSON/);
  assert.match(error([{id:1,kind:'ordinal',unit:'item',start:'0',end:''}]),/inteiros a partir de 1/);
  assert.match(error([]),/ao menos um critério/);
  ui.framing={kind:'delimiter',text:''};
  assert.match(error([{id:1,kind:'text',values:'x',op:'contains'}]),/padrão que delimita/);
});

test('the page is reachable from Estrutura and bundled', ()=>{
  const html=read('frontend/index.html'), workspace=read('frontend/workspace.js'), tasks=read('frontend/tasks.js'), bundle=read('scripts/prepare-frontend.mjs');
  assert.match(html,/data-page="log-split" data-zones="structure"/);
  assert.match(html,/<script src="log-split\.js"><\/script>/);
  assert.match(html,/href="log-split\.css"/);
  assert.match(workspace,/const STRUCTURE = new Set\(\[[^\]]*"log-split"/);
  assert.match(workspace,/next === "log-split"[^\n]*LogSplit\?\.mount/);
  assert.match(tasks,/\["split_run", "split_preview", "split_recover"\]\) named\.add/);
  assert.match(tasks,/committedWork\.has\(entry\.cmd\)/);
  assert.match(bundle,/"log-split\.js", "log-split\.css"/);
  // Paths never travel from the page to the native command: only tokens and ids.
  const page=read('frontend/log-split.js');
  for (const call of page.matchAll(/api\("(split_\w+)", (\{[^}]*\})/g)) assert.doesNotMatch(call[2],/path/i,call[1]);
});
