import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../../frontend/case-report.js', import.meta.url), 'utf8');
const ui = source.slice(source.indexOf('  // Decorates only'), source.indexOf('  return {open,render,filename'));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const result = () => ({ blob:{}, pages:3, filename:'original-report.pdf' });
function fixture({ lateness = 0, visual = true } = {}) {
  let now = 0, serial = 0, currentCase = { id:'original', name:'Original', items:[] };
  const timers = new Map(), mounted = [], calls = [], renders = [], saves = [], toasts = [];
  const nodes = [];
  class Node {
    constructor() { this.children = []; this.hidden = false; this.textContent = ''; this.classes = new Set(); this.classList = { add:name=>this.classes.add(name), remove:name=>this.classes.delete(name) }; nodes.push(this); }
    append(child) { child.parent = this; this.children.push(child); }
    remove() { this.parent.children = this.parent.children.filter(child=>child!==this); this.parent = null; }
    get isConnected() { return this===document.body || !!this.parent?.isConnected; }
    set innerHTML(value) { this.markup=value; this.parts = Object.fromEntries(['[data-case]','[data-close]','[data-cancel]','[data-generate]','[data-status]','[data-waiting]','progress'].map(selector=>{ const child = new Node(); this.append(child); return [selector,child]; })); this.parts['[data-waiting]'].hidden=true; this.parts.progress.hidden=true; }
    querySelector(selector) { return this.parts[selector]; }
    focus() { document.activeElement=this; this.focuses=(this.focuses||0)+1; }
  }
  const document = {}; document.body = new Node(); const origin = new Node(); document.body.append(origin); document.activeElement=origin;
  const render = (snapshot, options) => { const task=deferred(); renders.push({ ...task, snapshot, options }); options.progress('Preparando a timeline…'); return task.promise; };
  const api = (command,args) => { const task=deferred(); saves.push(task); calls.push({command,args}); return task.promise; };
  const window = { ...(visual?{WaitingVisuals:{mount(host,receipt){const view={host,receipts:[receipt],destroyed:false,update(value){this.receipts.push(value);},destroy(){this.destroyed=true;}};mounted.push(view);return view;}}}:{}) };
  const context = vm.createContext({ window, document, AbortController, DOMException, structuredClone, performance:{now:()=>now},
    activeCase:()=>currentCase, el:()=>new Node(), toast:(...args)=>toasts.push(args), render, api,
    toDataURL:async()=> 'data:application/pdf;base64,proof',
    check:signal=>{if(signal?.aborted)throw new DOMException('Geração cancelada','AbortError');},
    setTimeout:(fn,ms)=>{const id=++serial;timers.set(id,{fn,at:now+Math.trunc(ms)+(id===1?lateness:0)});return id;}, clearTimeout:id=>timers.delete(id)
  });
  vm.runInContext(`let dialog, waitingSerial=0; ${ui}; window.ReportUI={open,renderWaiting};`,context);
  const advance = to => { for (;;) { const next=[...timers].filter(([,task])=>task.at<=to).sort((a,b)=>a[1].at-b[1].at)[0];if(!next)break;timers.delete(next[0]);now=next[1].at;next[1].fn(); } now=to; };
  const open = () => { window.ReportUI.open(); return document.body.children.at(-1); };
  return { open, advance, mounted, timers, renders, saves, calls, toasts, origin, document, changeCase:value=>{currentCase=value;} };
}
const flush = async () => { for(let i=0;i<8;i++)await Promise.resolve(); };

test('instant report has no decorative flash, retained result or extra native command', async () => {
  const f=fixture(), dialog=f.open(), run=dialog.querySelector('[data-generate]').onclick();
  f.renders[0].resolve(result()); await flush();
  assert.equal(f.mounted.length,0); assert.equal(f.timers.size,0);
  assert.equal(f.calls.length,1); assert.equal(f.calls[0].command,'export_timeline');
  assert.equal(dialog.querySelector('[data-status]').textContent,'Salvando PDF…');
  f.saves[0].resolve({saved:false});await run;f.advance(10000);
  assert.equal(f.mounted.length,0);assert.equal(dialog.querySelector('[data-status]').textContent,'Salvamento cancelado.');
  assert.equal(dialog.querySelector('progress').hidden,true);assert.equal(dialog.querySelector('[data-generate]').disabled,false);
});

test('real render status/counts replace receipts and only a measured threshold selects the long scene', async () => {
  const f=fixture({lateness:.6}),dialog=f.open(),run=dialog.querySelector('[data-generate]').onclick();
  f.advance(251);const view=f.mounted[0];
  assert.equal(view.receipts[0].phaseId,'command:case_report_render');assert.equal(view.receipts[0].elapsedMs,250.6);
  assert.equal(view.receipts[0].total,undefined);assert.equal(dialog.querySelector('[data-status]').hidden,true);assert.equal(dialog.querySelector('progress').hidden,true);
  f.renders[0].options.progress('Organizando timeline: 26 / 70…',{label:'Organizando timeline…',completed:26,total:70,unit:'ocorrências'});
  assert.equal(view.receipts.at(-1).completed,26);assert.equal(view.receipts.at(-1).total,70);assert.equal(view.receipts.at(-1).label,'Organizando timeline…');
  const count=view.receipts.length;f.advance(3999);assert.equal(view.receipts.length,count);f.advance(4001);
  assert.ok(view.receipts.at(-1).elapsedMs>=4000);assert.ok(view.receipts.at(-1).elapsedMs<4001);assert.equal(view.receipts.length,count+1);
  f.advance(10000);assert.equal(view.receipts.length,count+1);assert.equal(f.timers.size,0,'no recurring timer or polling');
  f.renders[0].options.progress('Incluindo imagem: Print…');
  assert.equal(view.receipts.at(-1).total,undefined);assert.equal(view.receipts.at(-1).completed,undefined);
  assert.ok(view.receipts.every(receipt=>receipt.operationId===view.receipts[0].operationId));
  f.renders[0].resolve(result());await flush();assert.equal(view.destroyed,true,'composition is gone before the save promise');
  assert.equal(dialog.querySelector('[data-waiting]').hidden,true);assert.equal(dialog.querySelector('[data-status]').hidden,false);
  assert.equal(dialog.querySelector('progress').hidden,false);f.saves[0].resolve({saved:true});await run;
  assert.equal(f.toasts.length,1);assert.equal(f.origin.focuses,1);
});

test('close aborts the exact render immediately; late progress cannot animate or save', async () => {
  for(const afterMount of [false,true]) {
    const f=fixture(),dialog=f.open(),run=dialog.querySelector('[data-generate]').onclick();
    if(afterMount)f.advance(300);dialog.querySelector('[data-cancel]').onclick();
    const oldStatus=dialog.querySelector('[data-status]').textContent;
    assert.equal(f.renders[0].options.signal.aborted,true);assert.equal(dialog.isConnected,false);assert.equal(f.timers.size,0);
    if(afterMount)assert.equal(f.mounted[0].destroyed,true);
    f.renders[0].options.progress('This stale callback must not change the dialog');f.renders[0].resolve(result());await run;f.advance(10000);
    assert.equal(dialog.querySelector('[data-status]').textContent,oldStatus);assert.equal(f.calls.length,0);assert.equal(f.toasts.length,0);
  }
});

test('error keeps original text and retry starts one distinct operation; double click is inert', async () => {
  const f=fixture(),dialog=f.open(),button=dialog.querySelector('[data-generate]'),run=button.onclick();await button.onclick();
  assert.equal(f.renders.length,1);f.advance(300);const first=f.mounted[0];f.renders[0].reject(new Error('Não foi possível carregar a fonte.'));await run;
  assert.equal(first.destroyed,true);assert.equal(dialog.querySelector('[data-status]').textContent,'Não foi possível carregar a fonte.');
  assert.ok(dialog.querySelector('[data-status]').classes.has('case-report-error'));assert.equal(f.timers.size,0);assert.equal(button.disabled,false);
  const retry=button.onclick();f.advance(600);assert.equal(f.mounted.length,2);
  assert.notEqual(f.mounted[1].receipts[0].operationId,first.receipts[0].operationId);assert.equal(dialog.querySelector('[data-status]').classes.has('case-report-error'),false);
  dialog.querySelector('[data-close]').onclick();f.renders[1].reject(new DOMException('cancelled','AbortError'));await retry;
  assert.equal(f.calls.length,0);assert.equal(f.timers.size,0);
});

test('report snapshot survives a different active Case and saving keeps its existing close guard', async () => {
  const f=fixture(),dialog=f.open(),run=dialog.querySelector('[data-generate]').onclick();
  f.changeCase({id:'new-case',name:'New Case',items:[{}]});assert.equal(f.renders[0].snapshot.id,'original');
  f.advance(300);f.renders[0].resolve(result());await flush();dialog.querySelector('[data-cancel]').onclick();
  assert.equal(dialog.isConnected,true,'existing save cannot be interrupted or falsely undone');assert.equal(f.mounted[0].destroyed,true);
  assert.equal(f.calls[0].args.filename,'original-report.pdf');f.saves[0].resolve({saved:false});await run;dialog.querySelector('[data-cancel]').onclick();
  assert.equal(dialog.isConnected,false);assert.equal(f.timers.size,0);
});

test('removed host and optional-art fallback keep real status without mounting a detached scene', async () => {
  for(const visual of [false,true]) {
    const f=fixture({visual}),dialog=f.open(),run=dialog.querySelector('[data-generate]').onclick();
    if(visual)dialog.remove();f.advance(10000);assert.equal(f.mounted.length,0);assert.equal(f.timers.size,0);
    if(!visual)assert.equal(dialog.querySelector('[data-status]').textContent,'Preparando a timeline…');
    f.renders[0].reject(new Error('Renderer failed'));await run;assert.equal(f.timers.size,0);
  }
});

test('numeric report adapters originate at actual loop boundaries and preserve legacy callback text', () => {
  assert.match(source,/progress\(`Organizando timeline: \$\{i\+1\} \/ \$\{timeline.rows.length\}…`, \{label:'Organizando timeline…',completed:i\+1,total:timeline.rows.length,unit:'ocorrências'\}\)/);
  assert.match(source,/progress\(`Organizando trilha \$\{i\+1\} \/ \$\{trails.length\}…`, \{label:'Organizando trilhas…',completed:i,total:trails.length,unit:'trilhas concluídas'\}\)/);
  assert.doesNotMatch(ui,/setInterval|requestAnimationFrame|phaseCount|estimateMs|parseInt|\.match\(/);
});

test('real renderer preserves legacy progress text and reports only work already drawn', async () => {
  const drawn = new Set(), messages = [], counts = [];
  // Exercise the complete renderer, replacing only its browser/PDF dependencies.
  class PDF {
    constructor() { this.pages = 1; }
    addFileToVFS() {} addFont() {} setProperties() {} setFont() {} setFontSize() {}
    setTextColor() {} setDrawColor() {} setFillColor() {} rect() {} line() {} setPage() {}
    getFont() { return { metadata:{ characterToGlyph:()=>true } }; }
    getTextWidth(value) { return value.length; }
    getNumberOfPages() { return this.pages; }
    addPage() { this.pages++; }
    text(value) { drawn.add(value); }
    output() { return { size:5 }; }
  }
  const rows = Array.from({ length:30 }, (_, index) => ({ start:null, title:`ROW_${index}`, itemIds:[] }));
  const reportCase = { name:'Case', items:[], caseTrails:[
    { title:'one', summary:'DONE_TRAIL_0', itemIds:[] },
    { title:'two', summary:'DONE_TRAIL_1', itemIds:[] }
  ] };
  const window = { jspdf:{ jsPDF:PDF } };
  const context = vm.createContext({
    window, DOMException, AbortController, structuredClone, setTimeout, performance, Intl, Date, Uint8Array,
    fetch:async()=>({ ok:true, arrayBuffer:async()=>new ArrayBuffer(0) }), btoa:()=>'',
    FontFace:class { async load() { return this; } },
    document:{ fonts:{ add() {} }, createElement:()=>({ getContext:()=>({}) }) },
    CaseTimeline:{ rows:async()=>({ rows, eventCount:30, undated:0 }), image:async()=>({ blob:null }) },
    CaseContent:{ narrative:target=>({ summary:target.summary||'', details:'' }), attachments:()=>[] }
  });
  vm.runInContext(source, context);
  const rendered = await window.CaseReport.render(reportCase, { progress(value, detail) {
    messages.push(value);
    if (!detail) return;
    const isTimeline = detail.unit === 'ocorrências';
    assert.ok(isTimeline || detail.unit === 'trilhas concluídas');
    const targets = isTimeline ? rows : reportCase.caseTrails;
    const actual = targets.filter(target=>drawn.has(isTimeline ? target.title : target.summary)).length;
    assert.equal(detail.completed, actual, 'a callback must count only rows or trails already drawn');
    assert.equal(detail.total, targets.length);
    counts.push({ label:detail.label, completed:detail.completed, total:detail.total, unit:detail.unit });
  } });
  assert.deepEqual(messages, [
    'Preparando a timeline…', 'Montando a visão do Caso…',
    'Organizando timeline: 1 / 30…', 'Organizando timeline: 26 / 30…',
    'Organizando trilha 1 / 2…', 'Organizando trilha 2 / 2…', 'Finalizando páginas…'
  ]);
  assert.deepEqual(counts, [
    { label:'Organizando timeline…', completed:1, total:30, unit:'ocorrências' },
    { label:'Organizando timeline…', completed:26, total:30, unit:'ocorrências' },
    { label:'Organizando trilhas…', completed:0, total:2, unit:'trilhas concluídas' },
    { label:'Organizando trilhas…', completed:1, total:2, unit:'trilhas concluídas' }
  ]);
  assert.equal(rows.filter(row=>drawn.has(row.title)).length,30);
  assert.equal(reportCase.caseTrails.filter(trail=>drawn.has(trail.summary)).length,2);
  assert.equal(rendered.rows,30); assert.equal(rendered.trails,2); assert.equal(rendered.items,0);
});
