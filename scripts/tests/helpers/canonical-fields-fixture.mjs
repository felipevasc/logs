import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source=readFileSync(new URL('../../../frontend/canonical-fields.js',import.meta.url),'utf8');
// Small-field transport for existing UI wiring fixtures. The dedicated async
// helper and preview transport tests supply adversarial native canonical text.
export function installCanonicalFields(context){
  context.TextEncoder ||= TextEncoder;
  context.document ||= {};
  context.document.addEventListener ||= ()=>{};
  context.filterFocusTarget ||= ()=>null;
  context.caseEvents ||= ()=>context.state.rows||[];
  context.caseSig ||= ()=> 'fixture-evidence';
  context.caseArgs ||= async args=>{if(!args.caseEvents)return args;const{caseEvents,...rest}=args;return{...rest,caseKey:'fixture-case'};};
  const identity=()=>({caseId:context.activeCase?.()?.id||'fixture-case',analysisId:'fixture-analysis',configRevision:1,visibilityRevision:1});
  const capture=()=>({identity:identity(),instance:1,sourceGeneration:1,sourceKey:'fixture-source'});
  context.window.AnalysisContexts={capture,identity,prepare:async owner=>owner,isCurrent:owner=>JSON.stringify(owner)===JSON.stringify(capture())};
  context.api=async(command,args)=>{
    if(command!=='analysis_field_text')throw Error(`Unexpected fixture command ${command}`);
    const event=(context.state.rows||[]).find(row=>row.id===args.id)||context.state.currentDetailEv;
    if(!event||event.event_ref!==args.eventRef)throw Error('Fixture record mismatch');
    const canonical=['id','event_ref','timestamp','source','level','code','name','description','message','raw'].includes(args.column);
    const value=canonical?event[args.column]:event.fields?.[args.column];
    const nullTimestamp=args.column==='timestamp'&&value==null;
    const presence=value===undefined&&!nullTimestamp?'missing':value===null||nullTimestamp?'null':'present';
    let canonicalText=value===undefined||nullTimestamp?null:value===null?'null':typeof value==='object'?JSON.stringify(value):String(value);
    if(args.column==='timestamp'&&value!=null)canonicalText=new Date(value).toISOString().replace(/\.000Z$/,'+00:00').replace(/Z$/,'+00:00');
    return{kind:'exact_field',version:1,row:{id:args.id,eventRef:args.eventRef},column:args.column,presence,
      valueType:presence==='missing'?null:presence==='null'?'null':Array.isArray(value)?'array':typeof value,
      canonicalText,receipt:{analysisContext:args.analysisContext,sourceGeneration:args.caseKey?null:args.sourceGeneration,
        caseKey:args.caseKey||null,caseContentToken:args.caseKey?'fixture-case-token':null,catalogSignature:'a'.repeat(64),catalogEpoch:0}};
  };
  vm.runInContext(source,context,{filename:'canonical-fields.js'});
}
