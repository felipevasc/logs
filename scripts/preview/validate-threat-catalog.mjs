/* Static Rust-regex validation using installed ripgrep. Does not execute or
   replace the application's native tests. All samples are inert search input. */
import { readFileSync, writeFileSync, mkdirSync, unlinkSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

const directory=resolve('output/threat-catalog');mkdirSync(directory,{recursive:true});
const patterns=resolve(directory,`validation-pattern-${process.pid}.txt`);
const catalog=JSON.parse(readFileSync('src-tauri/resources/threat-rules.json','utf8'));
const fixtures=JSON.parse(readFileSync('src-tauri/resources/threat-examples.json','utf8'));
const byId=new Map(catalog.rules.map(rule=>[rule.id,rule]));
const failures=[];
function run(pattern,input='neutral fixture text',combined=false){
  writeFileSync(patterns,pattern+'\n');
  const result=spawnSync('rg',['--multiline','--no-unicode','--regex-size-limit',combined?'32M':'256K','--dfa-size-limit',combined?'16M':'64K','-f',patterns,'-'],{input,encoding:'utf8',shell:false});
  const error=(result.stderr||(result.error?String(result.error):'')).split('\n').map(line=>line.trim()).filter(Boolean).slice(-2).join('\n').slice(-1000);
  return {status:result.status,error};
}
for(const rule of catalog.rules){const result=run(rule.pattern);if(![0,1].includes(result.status))failures.push({id:rule.id,...result});}
const all=catalog.rules.filter(rule=>rule.enabled).map(rule=>rule.pattern).join('\n');
const combined=run(all,'neutral fixture text',true);
if(![0,1].includes(combined.status))failures.push({id:'<combined>',...combined});
function normalize(original){
  let result=original,previous=original;
  for(let i=0;i<2;i++){
    const decoded=previous.replace(/(?:%[0-9a-f]{2})+/gi,encoded=>new TextDecoder().decode(Uint8Array.from(encoded.match(/%[0-9a-f]{2}/gi),hex=>parseInt(hex.slice(1),16)))).replace(/\\u([0-9a-f]{4})/gi,(original,hex)=>{const code=parseInt(hex,16);return code>=0xd800&&code<=0xdfff?original:String.fromCharCode(code);});
    if(decoded===previous)break;result+='\n'+decoded;previous=decoded;
  }
  return result;
}
let positive=0,benign=0;
for(const sample of [...fixtures.positive,...fixtures.normalizedPositive]){
  const rule=byId.get(sample.id);
  if(!rule){failures.push({id:sample.id,error:'Fixture references missing rule'});continue;}
  const result=run(rule.pattern,normalize(sample.text));
  if(result.status!==0)failures.push({id:sample.id,fixture:true,...result});else positive++;
}
for(const [index,text]of fixtures.benign.entries()){
  const result=run(all,normalize(text),true);
  if(result.status!==1)failures.push({id:`benign:${index}`,fixture:true,...result});else benign++;
}
const result={count:catalog.rules.length,enabled:catalog.rules.filter(rule=>rule.enabled).length,positive,benign,failures,combined,note:'Static ripgrep byte-regex check (ASCII default), not application unit tests.'};
writeFileSync(resolve(directory,'regex-rg-result.json'),JSON.stringify(result,null,2));
console.log(JSON.stringify(result,null,2));
unlinkSync(patterns);
if(failures.length)process.exitCode=1;
