import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const read=name=>readFileSync(new URL(`../../frontend/${name}`,import.meta.url),'utf8'),plain=value=>JSON.parse(JSON.stringify(value));
const stamp=revision=>({storeId:'s',epoch:'epoch',revision}),owner=id=>({storeId:'s',caseId:id,analysisId:`analysis-${id}`});
const summary=id=>({state:'ready',owner:owner(id),evidenceSignature:`sig-${id}`,preservedCount:0});
const doc=()=>({evidenceViewVersion:1,store:stamp('1'),active:'old',cases:[{id:'old',name:'Existing',items:[]}],caseEvidence:[summary('old')],diagnostics:[]});
const imported=request=>({importedCaseId:'new',importedCaseIds:['new'],caseViews:[{id:'new',name:'Imported',items:[]}],receipt:{requestId:request.requestId,committedStore:stamp('2'),currentStore:stamp('2'),evidence:[],analysisContexts:[],caseEvidence:[summary('old'),summary('new')],replayed:false,reconcileRequired:false}});
function fixture(){let store=doc(),result=null,mode='ok',loads=0,replacements=0,clean=true;const calls=[];const context=vm.createContext({window:{},structuredClone,TextEncoder,crypto:{randomUUID:()=> 'request'}});for(const file of ['case-evidence.js','case-evidence-transfer.js'])vm.runInContext(read(file),context);
 const session={pending:async()=>{},status:()=> 'ready',isClean:()=>clean,load:async()=>{loads++;return{...store,store:stamp('2'),active:'new',cases:[...store.cases,{id:'new',name:'Authoritative reload',items:[]}],caseEvidence:[summary('old'),summary('new')]}},assertLoadCurrent:()=>{}};
 const transfer=context.window.CaseEvidenceTransfer.create({client:{status:()=> 'ready'},session,getStore:()=>store,requestId:()=> 'request',invoke:async(command,{request})=>{calls.push([command,plain(request)]);if(mode==='fail'){mode='ok';throw Error('lost acknowledgement');}if(command==='case_export_native')return{kind:'native_case_export',path:request.path,format:request.path.toLowerCase().endsWith('.licase')?'licase':'json',cases:1,records:JSON.parse(request.documentJson).cases[0].kind==='preserved_case_unavailable'?null:0,masked:request.mask};return result?result(request):imported(request);},replace:async(value,before)=>{before();store=value;replacements++;}});
 return{transfer,context,calls,get store(){return store;},mode:value=>{mode=value;},result:value=>{result=value;},clean:value=>{clean=value;},counts:()=>({loads,replacements})};}

test('native export serializes only the selected management document and uses no legacy export/save',async()=>{
 const f=fixture();f.store.cases.push({id:'other',items:[]});f.store.caseEvidence.push(summary('other'));await f.transfer.exportFile(f.store.cases[0],'/tmp/case.licase',{mask:true});assert.equal(f.calls[0][0],'case_export_native');const payload=JSON.parse(f.calls[0][1].documentJson);assert.deepEqual(payload.cases,[{id:'old',name:'Existing',items:[]}]);assert.equal(payload.evidenceViewVersion,1);assert.equal(f.calls[0][1].mask,true);assert.equal(f.calls.length,1);
});

test('issued unavailable stubs are passed verbatim to native export and unknown counts remain null',async()=>{
 const f=fixture(),stub={id:'old',kind:'preserved_case_unavailable',code:'CASE_NATIVE_UNAVAILABLE'};f.store.cases=[stub];f.store.caseEvidence=[{state:'unavailable',caseId:'old',owner:owner('old'),code:'CASE_NATIVE_UNAVAILABLE',message:'preserved',preservedCount:null,readiness:'preserved_only'}];f.store.diagnostics=[{caseId:'old',owner:owner('old'),code:'CASE_NATIVE_UNAVAILABLE',message:'preserved',preservedCount:null,readiness:'preserved_only'}];
 const value=await f.transfer.exportFile(stub,'/tmp/case.json');assert.equal(value.records,null);assert.deepEqual(JSON.parse(f.calls[0][1].documentJson).cases[0],stub);assert.equal(Object.keys(stub).length,3);
});

test('native import sends only native store/path/request identity and installs a fresh management read',async()=>{
 const f=fixture(),result=await f.transfer.importFile('/tmp/import.licase');assert.deepEqual(Object.keys(f.calls[0][1]).sort(),['path','requestId','store']);assert.equal(f.calls[0][0],'case_import_native');assert.equal(f.store.cases[1].name,'Authoritative reload');assert.equal(f.store.active,'new');assert.equal(result.reconciled,true);assert.deepEqual(f.counts(),{loads:1,replacements:1});
});

test('lost import acknowledgement retries the original immutable request without adding a second import',async()=>{
 const f=fixture();f.mode('fail');await assert.rejects(f.transfer.importFile('/tmp/import.licase'),/lost/);assert.equal(f.transfer.status(),'retry_required');await assert.rejects(f.transfer.importFile('/tmp/other.licase'),/pendente/);await f.transfer.retryImport();assert.deepEqual(f.calls[0],f.calls[1]);assert.equal(f.transfer.status(),'ready');assert.deepEqual(f.counts(),{loads:1,replacements:1});
});

test('a replay with later revision installs the authoritative reread instead of receipt caseViews',async()=>{
 const f=fixture();f.result(request=>{const result=imported(request);result.receipt.currentStore=stamp('7');result.receipt.replayed=true;result.receipt.reconcileRequired=true;return result;});await f.transfer.importFile('/tmp/import.licase');assert.equal(f.store.cases[1].name,'Authoritative reload');assert.equal(f.transfer.status(),'ready');
});

test('native import refuses a dirty local draft before touching the file',async()=>{
 const f=fixture();f.clean(false);await assert.rejects(f.transfer.importFile('/tmp/import.licase'),/rascunho/);assert.equal(f.calls.length,0);assert.equal(f.store.active,'old');
});

test('draft changes during native import preserve selection and local text for reconciliation',async()=>{
 const f=fixture();f.result(request=>{f.store.cases[0].name='Typed while importing';return imported(request);});await assert.rejects(f.transfer.importFile('/tmp/import.licase'),/RECONCILE/);assert.equal(f.store.active,'old');assert.equal(f.store.cases[0].name,'Typed while importing');assert.equal(f.transfer.status(),'reconcile_required');assert.deepEqual(f.counts(),{loads:0,replacements:0});
});

test('a malformed native import receipt cannot install inline rows or mismatched request authority',async()=>{
 for(const mutate of [result=>{result.receipt.requestId='other';},result=>{result.caseViews[0].rows=[{id:1,raw:'forbidden'}];}]){const f=fixture();f.result(request=>{const result=imported(request);mutate(result);return result;});await assert.rejects(f.transfer.importFile('/tmp/import.licase'));assert.deepEqual(f.counts(),{loads:0,replacements:0});assert.equal(f.store.active,'old');}
});

test('an imported unavailable Case keeps its exact stub and authoritative diagnostic through receipt validation',async()=>{
 const f=fixture();f.result(request=>{const result=imported(request);result.caseViews=[{id:'new',kind:'preserved_case_unavailable',code:'PRESERVED'}];result.receipt.caseEvidence=[{state:'unavailable',caseId:'new',owner:owner('new'),code:'PRESERVED',message:'Preserved only',preservedCount:null,readiness:'preserved_only'}];return result;});await f.transfer.importFile('/tmp/import.licase');assert.equal(f.counts().replacements,1);
});

test('native export follows backend JSON fallback for an extensionless or other-suffix destination',async()=>{
 for(const path of ['/tmp/investigation','/tmp/investigation.txt','/tmp/investigation.LICASE']){const f=fixture();const result=await f.transfer.exportFile(f.store.cases[0],path);assert.equal(result.format,path.toLowerCase().endsWith('.licase')?'licase':'json');}
});
