import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/analysis-workbench.js',import.meta.url),'utf8');
const display=value=>value==null?'(vazio)':value==='(vazio)'?'“(vazio)”':String(value);
const context=vm.createContext({displayGroup:display});
vm.runInContext(source.slice(source.indexOf('  let pivotPresentationCache ='),source.indexOf('  renderCubeTable = function')),context);
let paths=[['a'],['a','p2'],['a','p1'],['b'],['b',null],['b','(vazio)']];
const result={cells:paths.map((_,i)=>[[i%3]])};
const view={search:'',sort:'tree',page:0,columnPage:0},collapsed=[];let revision='dataset-a:schema-a:true',depth=2;
const current=()=>context.pivotPresentation(result,paths,depth,view,collapsed,revision);
const reference=()=>{
  const key=p=>JSON.stringify(p),parents=new Set(paths.filter(p=>p.length>1).map(p=>key(p.slice(0,-1))));
  let rows=paths.map((path,ri)=>({path,ri})).filter(({path})=>!depth||!collapsed.some(prefix=>prefix.length<path.length&&prefix.every((v,i)=>path[i]===v))&&(path.length===depth||!parents.has(key(path))||collapsed.some(prefix=>key(prefix)===key(path))));
  if(view.search)rows=rows.filter(row=>row.path.some(value=>display(value).toLocaleLowerCase().includes(view.search.toLocaleLowerCase())));
  if(view.sort!=='tree')rows.sort((a,b)=>((Number(result.cells[a.ri]?.[0]?.[0])||0)-(Number(result.cells[b.ri]?.[0]?.[0])||0))*(view.sort==='desc'?-1:1));
  return rows.map(row=>row.ri);
};
let first=current();assert.deepEqual(Array.from(first.rows,row=>row.ri),reference());
view.page++;view.columnPage++;view.heat=false;assert.equal(current(),first,'row/column paging and visual heat toggle reuse the local order');
for(const change of [()=>collapsed.push(['a']),()=>collapsed[0][0]='b',()=>collapsed.length=0,()=>view.search='vazio',()=>view.search='',()=>view.sort='desc',()=>view.sort='asc',()=>revision='dataset-a:schema-b:true',()=>result.cells[1][0][0]=100,()=>paths[1][1]='edited',()=>paths[2]=[...paths[2]],()=>paths=[...paths],()=>depth=1]){
  const before=current();change();const after=current();assert.notEqual(after,before);assert.deepEqual(Array.from(after.rows,row=>row.ri),reference());assert.equal(current(),after);
}
view.sort='tree';first=current();result.cells[0][0][0]=1000;assert.equal(current(),first,'a metric change does not change tree ordering; visible metric/heat values are still read fresh');
const nested=[[[1,2]]],nestedResult={cells:[[[1]]]};const nestedFirst=context.pivotPresentation(nestedResult,nested,1,view,[],revision);
assert.notEqual(context.pivotPresentation(nestedResult,nested,1,view,[],revision),nestedFirst,'compound path keys are deliberately not cached');
nested[0][0].push(3);assert.notEqual(context.pivotPresentation(nestedResult,nested,1,view,[],revision),nestedFirst);
const wide=Array.from({length:500},()=>Array(501).fill('x')),wideResult={cells:[]};
assert.notEqual(context.pivotPresentation(wideResult,wide,501,view,[],revision),context.pivotPresentation(wideResult,wide,501,view,[],revision),'over-wide snapshots are not retained');
assert.match(source,/groupPresentationCache = null; pivotPresentationCache = null/);
console.log('Pivot paging reuses path/search/order while respecting collapse, schema and in-place path/measure changes');
