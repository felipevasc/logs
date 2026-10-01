import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const context=vm.createContext({window:{},TextEncoder,TextDecoder,atob,btoa,structuredClone});
vm.runInContext(readFileSync(new URL('../preview/mock-field-transforms.js',import.meta.url),'utf8'),context);
const {transform,expand}=context.window.__mockFieldTransforms,plain=value=>JSON.parse(JSON.stringify(value));
assert.equal(transform('Olá 🌎',['base64_encode','base64_decode']).value,'Olá 🌎');
assert.equal(transform('Olá 🌎',['base64_url_encode','base64_url_decode']).value,'Olá 🌎');
assert.equal(transform('a+b%20c',['url_decode']).value,'a+b c');
assert.equal(transform('a+b%20c',['form_decode']).value,'a b c');
assert.equal(transform("a !'()*",['url_encode']).value,'a%20%21%27%28%29%2A');
assert.equal(transform('Olá',['text_to_hex','hex_to_text']).value,'Olá');
assert.deepEqual(plain(transform('eyJvayI6dHJ1ZSwibiI6Mn0=',['base64_decode','parse_json']).value),{ok:true,n:2});
assert.deepEqual(plain(transform('?a=1&a=2&plus=a+b',['parse_query']).value),{a:['1','2'],plus:'a b'});
assert.deepEqual(plain(transform('?',['parse_query']).value),{});
const token='eyJhbGciOiJub25lIn0.eyJzdWIiOiJsb2NhbCJ9.';
assert.deepEqual(plain(transform(token,['jwt_payload'])),{value:{sub:'local'},notices:['jwt_signature_not_verified']});
assert.throws(()=>transform('%%%', ['base64_decode']));assert.throws(()=>transform('a',['hex_to_text']));assert.throws(()=>transform('x',Array(9).fill('url_encode')));
assert.throws(()=>transform('x'.repeat(262145),[]));
const original={child:{enabled:true},'child.enabled':'literal',list:[1,false]};
const expanded=plain(expand('decoded',original));assert.deepEqual(expanded.decoded,original);assert.equal(expanded['decoded.child.enabled'],'literal');assert.equal(expanded['decoded.list.1'],false);
assert.deepEqual(original,{child:{enabled:true},'child.enabled':'literal',list:[1,false]});
console.log('Preview fixture codecs, typed children, JWT notice and bounded failures passed');

// Exercise the real preview transport: scoped fields survive regex-only edits and
// are evaluated as typed descendants without altering the shared raw fixture.
const storage=new Map();context.localStorage={getItem:key=>storage.get(key)||null,setItem:(key,value)=>storage.set(key,value)};
context.setTimeout=setTimeout;context.window.addEventListener=()=>{};
vm.runInContext(readFileSync(new URL('../preview/mock-tauri.js',import.meta.url),'utf8'),context);
const invoke=context.window.__TAURI__.core.invoke;
const store=await invoke('cases_load'),snapshot=store.cases[0].analysisContext;
const identity=value=>Object.fromEntries(['caseId','analysisId','configRevision','visibilityRevision'].map(key=>[key,value[key]]));
const initial=await invoke('query_events',{filters:[],limit:1,sortColumn:'timestamp',sortDir:'desc',analysisContext:identity(snapshot)});
const raw=initial.rows[0].fields.mock_payload_b64;
const saved=await invoke('save_derived_field',{name:'decoded',source:'mock_payload_b64',rules:[],steps:['base64_decode','parse_json'],analysisContext:identity(snapshot)});
const transformed=await invoke('query_events',{filters:[],limit:1,sortColumn:'timestamp',sortDir:'desc',analysisContext:identity(saved.analysisContext)});
assert.equal(transformed.rows[0].fields.mock_payload_b64,raw);assert.equal(typeof transformed.rows[0].fields['decoded.allowed'],'boolean');
const regex=await invoke('save_derived_field',{name:'decoded',source:'mock_payload_b64',rules:[{pattern:'(.*)'}],analysisContext:identity(saved.analysisContext)});
assert.deepEqual(plain((await invoke('list_derived_fields',{analysisContext:identity(regex.analysisContext)}))[0].steps),['base64_decode','parse_json'],'omitted steps preserve the existing pipeline');
const another=await invoke('cases_save',{data:{...store,cases:[...store.cases,{id:'separate',name:'Separate'}]}});
const other=another.analysisContexts.find(value=>value.caseId==='separate');
const clean=await invoke('query_events',{filters:[],limit:1,sortColumn:'timestamp',sortDir:'desc',analysisContext:identity(other)});
assert.equal(clean.rows[0].fields.decoded,undefined,'another Case reads only the raw source');assert.equal(clean.rows[0].fields.mock_payload_b64,raw);
console.log('Preview native transport preserves scoped pipelines, typed descendants and original source values');


// Fixed Event metadata wins over a shadowing custom field. Custom values retain
// their original JSON type, including an explicitly present null.
initial.rows[0].fields.level='shadowed level';
initial.rows[0].fields.custom_null=null;
initial.rows[0].fields.custom_object={enabled:false,attempt:0};
let latest=regex.analysisContext;
for(const [name,source,steps] of [['canonical_level','level',['text_to_hex']],['typed_null','custom_null',['parse_json']],['typed_object','custom_object',['parse_json']]]) {
  latest=(await invoke('save_derived_field',{name,source,rules:[],steps,analysisContext:identity(latest)})).analysisContext;
}
const parity=await invoke('query_events',{filters:[],limit:1,sortColumn:'timestamp',sortDir:'desc',caseEvents:initial.rows,analysisContext:identity(latest)});
assert.equal(parity.rows[0].fields.canonical_level,transform(initial.rows[0].level,['text_to_hex']).value);
assert.equal(parity.rows[0].fields.level,'shadowed level','the original custom shadow remains untouched');
assert.equal(parity.rows[0].fields.typed_null,null);
assert.deepEqual(plain(parity.rows[0].fields.typed_object),{enabled:false,attempt:0});
console.log('Canonical Event sources take precedence while typed custom fields and null survive');
