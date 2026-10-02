import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
let scope='dataset',instance=1,analysisId='a',config=1,visibility=1,generation=7,evidence='rows-1';
const state={cases:{active:'case'},filters:[],datasetRevision:1,currentArtifact:{id:'source',loadedAt:1},selectedEventRows:new Map(),lastSelectedRowId:null};
const context=vm.createContext({state,window:{AnalysisContexts:{capture:()=>({caseId:state.cases.active,instance,identity:{analysisId,configRevision:config,visibilityRevision:visibility},sourceGeneration:generation})}},workspaceScope:()=>scope,caseSig:()=>evidence});
vm.runInContext(app.slice(app.indexOf('function ensureSelectionOwner('),app.indexOf('function toggleRowSelect(')),context);
for(const change of [()=>generation++,()=>instance++,()=>analysisId='new-analysis',()=>config++,()=>visibility++,()=>scope='case',()=>evidence='rows-2']){
  context.ensureSelectionOwner();const event={id:3,message:'captured'};state.selectedEventRows.set(3,event);state.lastSelectedRowId=3;
  change();context.ensureSelectionOwner();assert.equal(state.selectedEventRows.size,0,'replacement Case/source/analysis/scope invalidates numeric row IDs');assert.equal(state.lastSelectedRowId,null);
}
context.ensureSelectionOwner();const event={id:3,message:'same source'};state.selectedEventRows.set(3,event);state.filters=[{column:'level',op:'equals',value:'Erro'}];
context.ensureSelectionOwner();assert.equal(state.selectedEventRows.get(3),event,'filters/paging can retain an explicitly selected member of the same admitted source');
console.log('Selected row IDs stay bound to Case instance, analysis revisions, source generation and area');
