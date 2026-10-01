import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const app=readFileSync(new URL('../../frontend/app.js',import.meta.url),'utf8');
const nodes=new Map(),plots=[],saved=[];let requests=0;
const node=id=>{if(!nodes.has(id))nodes.set(id,{style:{},clientWidth:800,classList:{toggle(){}},appendChild(){},closest:()=>node('.hist-panel')});return nodes.get(id);};
const document={documentElement:{dataset:{theme:'dark'}}};
const context=vm.createContext({document,window:{},state:{loaded:true,dashboardCompact:false},chart:null,dashCharts:{},$:node,el:()=>node('holder'),
  localStorage:{setItem:(...args)=>saved.push(args)},getComputedStyle:()=>({getPropertyValue:()=>document.documentElement.dataset.theme==='light'?'#123456':'#abcdef'}),
  fmtNum:String,fmtTs:String,fmtVal:String,LEVEL_COLOR:{Erro:'var(--lv-erro)'},CHART_COLORS:['#eeeeee'],
  refresh(){requests++;},api(){requests++;},addFilter(){throw Error('Repaint must not invoke filter hooks');},
  uPlot:class{constructor(options,data,box){this.options=options;this.data=data;this.box=box;this.width=options.width;this.height=options.height;this.scales={x:{min:7,max:8}};this.cursor={left:12};this.paints=[];plots.push(this);} redraw(...args){this.paints.push({args,axis:this.options.axes[0].stroke(),grid:this.options.axes[1].grid.stroke(),series:this.options.series[1].stroke()});}setData(data){this.data=data;}setSize(){}destroy(){throw Error('Theme must preserve plot instances');}},
});
vm.runInContext(app.slice(app.indexOf('function toggleTheme()'),app.indexOf('// ------------------------------------------------------------------ fonte')),context);
vm.runInContext(app.slice(app.indexOf('function renderChart('),app.indexOf('// ------------------------------------------------------------------ colunas')),context);
vm.runInContext(app.slice(app.indexOf('function renderLineChart('),app.indexOf('// ---------- editor de gráfico')),context);
context.renderChart({buckets:[[1000,3],[2000,5]]});
context.renderLineChart(node('dashboard'),{x:[1000,2000],series:[{name:'Erro',points:[3,5]}],unit:'count'},'example');
const histogram=context.chart,dashboard=context.dashCharts.example,data=histogram.data,scales=histogram.scales,cursor=histogram.cursor;
context.toggleTheme();assert.equal(document.documentElement.dataset.theme,'light');
for(const plot of plots){assert.equal(plot.paints.at(-1).axis,'#5b6678');assert.equal(plot.paints.at(-1).grid,'rgba(19,81,180,0.08)');assert.equal(plot.paints.at(-1).series,'#123456');assert.deepEqual(Array.from(plot.paints.at(-1).args),[false,true]);}
context.toggleTheme();assert.equal(document.documentElement.dataset.theme,'dark');
for(const plot of plots){assert.equal(plot.paints.at(-1).axis,'#6b7690');assert.equal(plot.paints.at(-1).series,'#abcdef');}
assert.equal(context.chart,histogram);assert.equal(context.dashCharts.example,dashboard);assert.equal(histogram.data,data);assert.equal(histogram.scales,scales);assert.equal(histogram.cursor,cursor);
assert.equal(requests,0);assert.equal(saved.length,2);
context.chart=null;context.dashCharts={};context.toggleTheme();assert.equal(requests,0,'theme works without a loaded plot');
console.log('Theme changes repaint existing histogram/dashboard colors without querying or replacing data/scales');
