import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const code=app.slice(app.indexOf('const caseTransport ='),app.indexOf('// ------------------------------------------------------------------ helpers de espera'));
for(const command of ['grouped_timeline','exclusion_preview','exclusion_archive_page']){
  const capturedRows=[{id:1,event_ref:'original:1'}],replacementRows=[{id:999,event_ref:'replacement:999'}];
  const analysisContext={caseId:'original-case',analysisId:'original-analysis',configRevision:2,visibilityRevision:3};
  const state={cases:{active:'original-case'},rows:capturedRows},calls=[];let reads=0;
  const context=vm.createContext({state,window:{},explorerAnalytics:new Map(),toast(){},
    invoke:async(cmd,args)=>{
      calls.push({cmd,args});
      if(cmd==='case_sync')return null;
      assert.equal(cmd,command);
      if(++reads===1){state.cases.active='replacement-case';state.rows=replacementRows;throw Error('CASE_CACHE_MISS');}
      return{ok:true};
    },
  });vm.runInContext(code,context);
  const prepared=await context.caseArgs({caseEvents:capturedRows,analysisContext,sourceGeneration:7});
  assert.equal((await context.api(command,prepared,{silent:true,caseEvents:capturedRows})).ok,true);
  const syncs=calls.filter(call=>call.cmd==='case_sync'),requests=calls.filter(call=>call.cmd===command);
  assert.equal(syncs.length,2);assert.equal(requests.length,2);
  for(const sync of syncs){assert.equal(sync.args.events,capturedRows);assert.equal(sync.args.analysisContext,analysisContext);assert.equal(sync.args.sourceGeneration,7);}
  for(const request of requests){assert.equal(request.args.caseKey,prepared.caseKey);assert.equal(request.args.caseEvents,undefined);assert.equal(request.args.analysisContext,analysisContext);assert.equal(request.args.sourceGeneration,7);}
}
console.log('Prepared Case requests resync only their captured evidence, owner and source generation');
