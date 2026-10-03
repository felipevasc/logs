import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../../frontend/entity-menu.js', import.meta.url),'utf8');
const presets=[];let applies=0;
const context=vm.createContext({window:{},state:{columns:['request_path','source','code']},document:{addEventListener(){}},
  valueFilterMenuItem:(column,value,anchor,options)=>({label:`Criar filtro: ${column}`,onClick:()=>presets.push({column,value,...options})}),
  colLabel:String,workspaceScope:()=> 'dataset',toast(){},navigator:{clipboard:{writeText:async()=>{}}},
});
context.window.Workspace={applyFilters(){applies++;},search(){applies++;}};
vm.runInContext(source,context);
const cases=[
  [{value:'10.2.3.4'},'_all','contains'],
  [{value:'https://nginx.example/path'},'_all','contains'],
  [{value:'a'.repeat(64)},'_all','contains'],
  [{column:'@dst_ip',value:'10.2.3.4'},'@dst_ip','equals_exact'],
  [{column:'@src_ip',value:'10.2.3.4'},'@src_ip','equals_exact'],
  [{column:'request_path',value:'/api?a=b'},'request_path','equals_exact'],
  [{column:'unseen_column',value:'anything'},'_all','contains'],
];
for(const [entity,column,op] of cases){
  const items=context.window.EntityMenu.items(entity);
  assert.ok(items.some(item=>item.label==='Filtrar por este valor'),'existing quick action remains');
  const composer=items.find(item=>item.label?.startsWith('Criar filtro'));assert.ok(composer);composer.onClick();
  assert.deepEqual(presets.at(-1),{column,value:entity.value,op});
  if(column==='_all')assert.match(composer.label,/todo o evento/);
}
assert.equal(applies,0,'opening an entity composer does not apply a quick action');
console.log('Entity composer preserves known roles and never invents a source field for highlighted literals');
