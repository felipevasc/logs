import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/analysis-workbench.js',import.meta.url),'utf8');
const context=vm.createContext({});
vm.runInContext(source.slice(source.indexOf('  let groupPresentationCache ='),source.indexOf('  function renderGroups()')),context);
const result={rows:Array.from({length:500},(_,i)=>({path:`/route/${500-i}`,n:i%19,sum:i*2})),omitted_records:7};
const view={field:'path',sort:'n',direction:-1,search:'',computedKey:'dataset-a-v1',page:0};
const reference=(r,v)=>r.rows.filter(row=>String(row[v.field]??'').toLocaleLowerCase().includes(v.search.toLocaleLowerCase())).slice().sort((a,b)=>{const aa=a[v.sort],bb=b[v.sort];return v.direction*(typeof aa==='number'&&typeof bb==='number'?aa-bb:String(aa??'').localeCompare(String(bb??''),'pt-BR',{numeric:true}));});
const current=()=>context.groupPresentation(result,view,'n');
let first=current();assert.deepEqual(Array.from(first.rows),reference(result,view));
view.page=1;assert.equal(current(),first,'next-page navigation reuses the ordered local view');view.page=2;assert.equal(current(),first);
assert.equal(result.rows[0].path,'/route/500','source result order remains untouched');
assert.equal(first.total,result.rows.reduce((sum,row)=>sum+row.n,7));assert.equal(first.max,18);
for(const change of [()=>view.search='/route/2',()=>view.sort='path',()=>view.direction=1,()=>view.computedKey='dataset-a-v2',()=>result.rows[0].path='/route/2-new',()=>result.rows[0].n=999,()=>result.rows[1]={...result.rows[1]},()=>result.rows.push({path:'/route/2000',n:4}),()=>result.omitted_records=40]){
  const prior=current();change();const next=current();assert.notEqual(next,prior,'mutable inputs invalidate the presentation');assert.deepEqual(Array.from(next.rows),reference(result,view));assert.equal(current(),next);
}
view.search='';view.sort='sum';const old=current();result.rows[2].sum=-123;assert.notEqual(current(),old,'changing a sort-only measure invalidates order');
view.sort='n';result.rows[1].n=NaN;first=current();assert.equal(current(),first,'Object.is keeps stable NaN inputs cacheable');
const other={rows:[{path:'elsewhere',n:2}]};context.groupPresentation(other,view,'n');assert.notEqual(current(),first,'only the most recent result is retained');
const large={rows:Array.from({length:50001},()=>({path:'same',n:1}))};
assert.notEqual(context.groupPresentation(large,view,'n'),context.groupPresentation(large,view,'n'),'oversized external results do not enter the retained cache');
const compound={rows:[{path:['a'],n:1}]};
const compoundFirst=context.groupPresentation(compound,view,'n');compound.rows[0].path.push('b');
assert.notEqual(context.groupPresentation(compound,view,'n'),compoundFirst,'compound mutable keys are deliberately not cached');
assert.match(source,/restore: saved => \{\s*groupPresentationCache = null/);
console.log('Grouped pagination reuses sorting while detecting source, search, order and in-place row changes');
