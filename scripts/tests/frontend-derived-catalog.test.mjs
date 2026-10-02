import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const code=source.slice(source.indexOf('function includeDiscoveredFields('),source.indexOf('function renderExploreTreeInto('));
const state={columns:['timestamp','payload'],visibleCols:['timestamp'],rows:[{id:1}],total:1,analysisDefinitionsPending:false};
let controls=0,rendered=0;
const context=vm.createContext({state,workspaceScope:()=> 'dataset',fillColumnControls:()=>controls++,
  restoreVisiblePreferences:()=>{state.visibleCols=['timestamp','decoded.enabled'].filter(field=>state.columns.includes(field));},
  renderTable:(page,options)=>{assert.equal(page.rows,state.rows);assert.equal(options.reuseRows,true);rendered++;},
});vm.runInContext(code,context);
const profiles=[{name:'decoded'},{name:'decoded.enabled'}];
context.includeDiscoveredFields(profiles,'case');assert.equal(controls,0,'inactive area profiles cannot change current columns');
state.analysisDefinitionsPending=true;context.includeDiscoveredFields(profiles,'dataset');assert.equal(controls,0,'field catalog waits for matching definitions');
state.analysisDefinitionsPending=false;context.includeDiscoveredFields(profiles,'dataset');
assert.deepEqual(Array.from(state.columns),['timestamp','payload','decoded','decoded.enabled']);assert.deepEqual(state.visibleCols,['timestamp','decoded.enabled']);assert.equal(rendered,1);
context.includeDiscoveredFields(profiles,'dataset');assert.equal(controls,1,'unchanged field schema requires no extra rendering');
console.log('Typed field discovery restores saved column choices locally after matching definitions');
