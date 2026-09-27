/* Offline evaluation of analyst-labelled findings. Never executes log content. */
import { readFile, writeFile } from "node:fs/promises";
import assert from "node:assert/strict";

export function wilson(successes,total) {
  if (!total) return null;
  const z=1.95996398454, p=successes/total, denominator=1+z*z/total;
  const center=(p+z*z/(2*total))/denominator;
  const radius=z*Math.sqrt(p*(1-p)/total+z*z/(4*total*total))/denominator;
  return [Math.max(0,center-radius),Math.min(1,center+radius)];
}
export function evaluate(corpus) {
  if (!Array.isArray(corpus.findings) || !Array.isArray(corpus.scenarios)) throw Error("Expected findings and scenarios arrays");
  const seen=new Set(), origins=new Map(), scenarios=new Map();
  for (const s of corpus.scenarios) {
    if (!s.id || !s.origin || !s.family || !["development","evaluation"].includes(s.split)) throw Error("Scenario requires id, origin, family and split");
    if (scenarios.has(s.id)) throw Error(`Duplicate scenario: ${s.id}`);
    if (origins.has(s.origin) && origins.get(s.origin)!==s.split) throw Error(`Origin leakage across development/evaluation: ${s.origin}`);
    scenarios.set(s.id,s); origins.set(s.origin,s.split);
  }
  for (const f of corpus.findings) {
    if (!f.id || seen.has(f.id)) throw Error("Findings must have unique stable IDs"); seen.add(f.id);
    if (!scenarios.has(f.scenario) || !Number.isInteger(f.evidence_level) || f.evidence_level<1 || f.evidence_level>5 || !["malicious","benign","unresolved"].includes(f.verdict)) throw Error(`Invalid labelled finding: ${f.id}`);
  }
  const reserved=corpus.findings.filter(f=>scenarios.get(f.scenario).split==="evaluation");
  const summarize=rows=> {
    const tp=rows.filter(f=>f.verdict==="malicious").length, fp=rows.filter(f=>f.verdict==="benign").length;
    const denominator=corpus.entity_periods;
    return {findings:rows.length,true_positives:tp,false_positives:fp,unresolved:rows.length-tp-fp,precision:tp+fp ? tp/(tp+fp):null,precision_ci95:wilson(tp,tp+fp),false_positives_per_entity_period:denominator>0 ? fp/denominator:null,review_minutes:rows.reduce((sum,f)=>sum+(Number.isFinite(f.review_minutes)?f.review_minutes:0),0)};
  };
  const byFamily={};
  for(const family of new Set([...scenarios.values()].map(s=>s.family))) {
    byFamily[family]=Object.fromEntries([1,2,3,4,5].map(level=>[`E${level}`,summarize(reserved.filter(f=>f.evidence_level===level && scenarios.get(f.scenario).family===family))]));
  }
  const cumulative=Object.fromEntries([5,4,3,2,1].map(minimum=> {
    const findings=reserved.filter(f=>f.evidence_level>=minimum);
    const eligible=[...scenarios.values()].filter(s=>s.split==="evaluation" && s.malicious===true && s.telemetry_sufficient===true);
    const detected=new Set(findings.filter(f=>f.verdict==="malicious").map(f=>f.scenario));
    return [`E${minimum}+`,{...summarize(findings),eligible_scenarios:eligible.length,recall_given_telemetry:eligible.length ? eligible.filter(s=>detected.has(s.id)).length/eligible.length:null}];
  }));
  return {corpus:corpus.name || "unnamed",policy:corpus.policy_version || "unspecified",synthetic:corpus.synthetic!==false,representative:corpus.representative===true,goals:{E5:0.99,E4:0.95},note:"Goals are evaluation targets, never probabilities of compromise. No rule is promoted automatically. Unresolved findings are counted separately.",levels:Object.fromEntries([1,2,3,4,5].map(level=>[`E${level}`,summarize(reserved.filter(f=>f.evidence_level===level))])),cumulative,by_family:byFamily};
}

if (process.argv.includes("--self-test")) {
  const sample={name:"synthetic-test",scenarios:[{id:"a",origin:"holdout-1",family:"web",split:"evaluation",malicious:true,telemetry_sufficient:true}],findings:[{id:"f",scenario:"a",evidence_level:4,verdict:"malicious"}]};
  const result=evaluate(sample);
  assert.equal(result.levels.E5.precision,null); assert.equal(result.levels.E4.precision,1); assert.ok(result.levels.E4.precision_ci95[0]<0.3);
  assert.equal(result.cumulative["E4+"].recall_given_telemetry,1); assert.equal(result.cumulative["E5+"].recall_given_telemetry,0);
  assert.throws(()=>evaluate({...sample,scenarios:[...sample.scenarios,{...sample.scenarios[0],id:"b",split:"development"}]}),/leakage/);
  console.log("Evidence evaluation self-test passed");
} else if (process.argv[2]) {
  const report=evaluate(JSON.parse(await readFile(process.argv[2],"utf8")));
  const output=JSON.stringify(report,null,2)+"\n";
  if(process.argv[3]) await writeFile(process.argv[3],output,"utf8"); else process.stdout.write(output);
} else {
  console.log("node scripts/security/evaluate.mjs labelled-corpus.json [report.json]");
}
