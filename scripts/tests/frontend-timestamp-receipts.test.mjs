import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const code=source.slice(source.indexOf('async function commitTsConfig('),source.indexOf('async function resetTsConfig('));
function fixture(changeCase=false){
  const state={cases:{active:'case-a'},sourcePublication:{generation:4}},requests=[],invalidated=[];
  const helper={capture:()=>({caseId:state.cases.active,sourceGeneration:state.sourcePublication.generation}),assertOwner:owner=>{
    if(owner.caseId!==state.cases.active||owner.sourceGeneration!==state.sourcePublication.generation)throw Error('ANALYSIS_CONTEXT_CHANGED');
  }};
  state.tsAnalysisOwner=helper.capture();
  const context=vm.createContext({state,window:{AnalysisContexts:helper},invalidateAnalysisComputedData:value=>invalidated.push(value),
    api:async(cmd,args,opts)=>{helper.assertOwner(opts.analysisOwner);requests.push({cmd,args,opts});if(changeCase)state.cases.active='case-b';return{publication:{generation:5+requests.length-1,operationId:'timestamp'}};},
  });vm.runInContext(code,context);return{state,requests,invalidated,context};
}
const f=fixture();await f.context.commitTsConfig(['one.jsonl','two.jsonl'],{format:'unix'});
assert.deepEqual(f.requests.map(request=>request.opts.analysisOwner.sourceGeneration),[4,5]);
assert.equal(f.state.sourcePublication.generation,6);assert.equal(f.state.tsAnalysisOwner.sourceGeneration,6);assert.equal(f.invalidated.length,2);
const moved=fixture(true);await assert.rejects(moved.context.commitTsConfig(['one','two'],null),/ANALYSIS_CONTEXT_CHANGED/);
assert.equal(moved.requests.length,1,'timestamp loop does not continue applying after Case ownership changes');
assert.equal(moved.state.sourcePublication.generation,4,'old action does not replace the new Case publication');
console.log('Timestamp configuration consumes each native source publication before its next file');
