import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const bar=readFileSync(new URL('../../frontend/query-bar.js',import.meta.url),'utf8');
const language=readFileSync(new URL('../../frontend/query-lang.js',import.meta.url),'utf8');
const state={rows:[],treeAggSig:{},datasetRevision:1,derivedFields:[]};
const identities=new WeakMap();let identity=0;const sourceIdentity=value=>{if(value==null||typeof value!=='object')return null;if(!identities.has(value))identities.set(value,++identity);return identities.get(value);};
const context=vm.createContext({window:{Workspace:{sourceKey:()=> 'fixture'}},state,workspaceScope:()=> 'dataset',backendFilters:()=>[],valueCache:new Map(),sourceIdentity,fold:text=>text.toLowerCase(),cellValue:(event,column)=>event.fields[column],fmtNum:String});
vm.runInContext(language,context);
vm.runInContext(bar.slice(bar.indexOf('  async function valueOptions('),bar.indexOf('  function suggestionLabel(')),context);
for(const literal of [String.raw`C:\logs\nginx access.log`,String.raw`C:\nginx\access.log`,String.raw`\\server\nginx logs`,String.raw`a"b\c`,String.raw`a\"b(c):`,"quoted trailing\\",'prévia 😀 "literal"']){
  const event={fields:{path:literal}};state.rows=[event];
  const suggestions=await context.valueOptions('path','');assert.equal(suggestions.length,1);
  const query=`path:${suggestions[0].text}`;
  assert.equal(context.window.QueryLang.validate(query),null,query);
  assert.equal(context.window.QueryLang.matches(event,query),true,query);
  assert.equal(context.window.QueryLang.compile(query).m.value,literal.toLowerCase(),'selected full literal survives quote/backslash decoding');
  if(literal.includes('\\'))assert.equal(context.window.QueryLang.matches({fields:{path:literal.replaceAll('\\','')}},query),false,'backslashes are semantically preserved');
}
// Colon equality intentionally trims event values; preserve that existing
// language rule, while proving the quoted literal also survives exact '=' use.
const spaced=String.raw` quoted trailing\ `;state.rows=[{fields:{path:spaced}}];
const [suggestion]=await context.valueOptions('path','');
assert.equal(context.window.QueryLang.compile(`path:${suggestion.text}`).m.value,spaced.toLowerCase());
assert.equal(context.window.QueryLang.matches(state.rows[0],`path=${suggestion.text}`),true);
// Values chosen from observations are literals; only intentionally typed syntax is smart.
for(const literal of ['/api?x=1','*','1..4','/foo/','a,b','[raw]','10.0.0.0/8']){
  const event={fields:{path:literal}};state.rows=[event];
  const [option]=await context.valueOptions('path','');const query=`path:${option.text}`;
  assert.equal(context.window.QueryLang.validate(query),null);
  assert.equal(context.window.QueryLang.compile(query).m.kind,'equals');
  assert.equal(context.window.QueryLang.matches(event,query),true);
  assert.equal(context.window.QueryLang.matches({fields:{path:'unrelated'}},query),false);
}
assert.equal(context.window.QueryLang.matches({fields:{path:'/apiAx=1'}},'path:/api?x=1'),true,'manually typed wildcard syntax is unchanged');
assert.equal(context.window.QueryLang.matches({fields:{path:'present'}},'path:*'),true,'manually typed existence syntax is unchanged');
state.rows=[];state.treeAgg={dataset:{path:[[null,5],['',4],['null',3]]}};state.treeAggSig={dataset:'dataset|1|[]|[]|path|fixture'};
const nullable=await context.valueOptions('path','');assert.equal(nullable.length,2,'missing/null observations are not suggested as a literal');
const empty=nullable.find(option=>option.label==='');assert.ok(empty);
assert.equal(context.window.QueryLang.matches({fields:{path:''}},`path:${empty.text}`),true);
assert.equal(context.window.QueryLang.matches({fields:{}},`path:${empty.text}`),false,'empty value remains distinct from a missing field');
assert.equal(context.window.QueryLang.matches({fields:{path:null}},`path:${empty.text}`),false);
// Ordinary JSON null follows col_ref text; role fallback retains its own boundary.
assert.equal(context.window.QueryLang.fieldValue({fields:{path:''}},context.window.QueryLang.resolve('path')),'');
assert.equal(context.window.QueryLang.fieldValue({fields:{path:null}},context.window.QueryLang.resolve('path')),'null');
assert.equal(context.window.QueryLang.fieldValue({fields:{}},context.window.QueryLang.resolve('path')),null);
assert.equal(context.window.QueryLang.fieldValue({fields:{'@src_ip':''},source:'10.2.3.4'},context.window.QueryLang.resolve('@src_ip')),'10.2.3.4','explicit role fallback behavior remains unchanged in this narrow parity fix');
assert.equal(context.window.QueryLang.fieldValue({fields:{path:0}},context.window.QueryLang.resolve('path')),'0');
assert.equal(context.window.QueryLang.fieldValue({fields:{path:false}},context.window.QueryLang.resolve('path')),'false');
console.log('Autocomplete quoted values round-trip through the actual search parser with backslashes and quotes intact');
