import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('../../frontend/case-timeline.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
function helpers(){const context=vm.createContext({window:{},document:{addEventListener(){}}});vm.runInContext(source.replace('return { render, rows, image };','return { render, rows, image, sampledTime, nativeCallbacks, noteResolver };'),context);return{context,...context.window.CaseTimeline};}

test('horizontal ruler and manual sampling keep singleton dates at both admitted Date boundaries',()=>{
 const f=helpers();for(const boundary of [-8640000000000000,8640000000000000])for(const fraction of [0,.25,.5,.75,1]){const time=f.sampledTime(boundary,boundary,fraction);assert.equal(time,boundary);assert.doesNotThrow(()=>new Intl.DateTimeFormat('pt-BR').format(new Date(time)));}
 assert.match(source,/const time = sampledTime\(start, end, i \/ steps\)/);assert.match(source,/if \(horizontal\) return Math.round\(sampledTime\(start, end,/);
});

test('retained native move and manual removal menu callbacks validate before changing authored metadata',()=>{
 const f=helpers();let current=true,choices,saves=0;const notices=[],config={layout:{},annotations:[{id:'note',anchor:'m:m'}]},c={manual:[{id:'m'}]},entry={id:'m:m',manual:c.manual[0]};
 Object.assign(f.context,{config,c,entry,side:'left',offset:3,callbacksSave:()=>{saves++;}});
 const move=source.match(/label: "Mover para o outro lado", onClick: (\(\) => \{[^\n]+?\}) \}/)[1];
 const label=source.indexOf('label: "Remover marco"'),start=source.indexOf('onClick: () => {',label)+9,end=source.indexOf('\n            } });',start)+14;
 const remove=source.slice(start,end);vm.runInContext(`this.move=${move};this.remove=${remove};`,f.context);
 const callbacks=f.nativeCallbacks({nativeCurrent:()=>current,notify:text=>notices.push(text),menu:(_x,_y,value)=>{choices=value;}});callbacks.menu(0,0,[{onClick:f.context.move},{onClick:f.context.remove}]);current=false;const before=plain({config,c});assert.equal(choices[0].onClick(),false);assert.equal(choices[1].onClick(),false);assert.deepEqual(plain({config,c}),before);assert.equal(saves,0);assert.equal(notices.length,2);
 current=true;choices[0].onClick();assert.equal(config.layout['m:m'].side,'right');assert.equal(saves,1);choices[1].onClick();assert.equal(c.manual.length,0);assert.equal(saves,2);
});

test('native note chains resolve with one first-ID index and memoized traversal, including cycles',()=>{
 const f=helpers();let reads=0;const notes=Array.from({length:300},(_,index)=>({id:`note-${index}`,get anchor(){reads++;return index?`note-${index-1}`:'entry';}}));notes.push({id:'note-0',anchor:'missing'},{id:'cycle-a',anchor:'cycle-b'},{id:'cycle-b',anchor:'cycle-a'});
 const resolve=f.noteResolver(notes,new Map([['entry',{index:0}]]));for(let index=299;index>=0;index--)assert.equal(resolve(`note-${index}`).index,0);assert.equal(resolve('cycle-a'),null);assert.equal(resolve('missing'),null);assert.ok(reads<=300,`${reads} parent reads`);
});

test('curve gestures remain local until a current pointerup and a stale gesture never changes authored notes',()=>{
 for(const stale of [false,true]){const f=helpers(),annotation={id:'note',curve:{matrix:{dx:2,dy:3}}};let current=true,saves=0;const board={getBoundingClientRect:()=>({left:0,top:0}),hasPointerCapture:()=>false};Object.assign(f.context,{board,curveDrag:{kind:'midpoint',link:{annotation},previous:{dx:2,dy:3},midX:1,midY:1,pointerId:7},horizontal:true,mayMutate:()=>current,redrawLinks(){},callbacksSave:()=>{saves++;},nearestSide:()=> 'left'});const a=source.indexOf('    board.onpointermove = event => {'),b=source.indexOf('\n    const noteNodes',a);vm.runInContext(source.slice(a,b),f.context);
 board.onpointermove({pointerId:7,clientX:30,clientY:40});assert.deepEqual(annotation.curve.matrix,{dx:2,dy:3});if(stale)current=false;board.onpointerup({pointerId:7});assert.deepEqual(plain(annotation.curve.matrix),stale?{dx:2,dy:3}:{dx:29,dy:39});assert.equal(saves,stale?0:1);}
});
