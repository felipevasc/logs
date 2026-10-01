import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const bar=readFileSync(new URL('../../frontend/query-bar.js',import.meta.url),'utf8');
const language=readFileSync(new URL('../../frontend/query-lang.js',import.meta.url),'utf8');
const state={rows:[],treeAggSig:{},datasetRevision:1,derivedFields:[]};
const context=vm.createContext({window:{Workspace:{sourceKey:()=> 'fixture'}},state,workspaceScope:()=> 'dataset',backendFilters:()=>[],valueCache:new Map(),fold:text=>text.toLowerCase(),cellValue:(event,column)=>event.fields[column],fmtNum:String});
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
console.log('Autocomplete quoted values round-trip through the actual search parser with backslashes and quotes intact');
