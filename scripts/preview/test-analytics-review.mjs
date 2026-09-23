/* New interaction regressions found during the independent analytics review. */
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
const url = process.argv[2] || "http://127.0.0.1:4174", output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
const fallback = `${process.env.LOCALAPPDATA}/ms-playwright/chromium_headless_shell-1217/chrome-headless-shell-win64/chrome-headless-shell.exe`;
const browser = await chromium.launch({ executablePath: existsSync(chromium.executablePath()) ? undefined : fallback });
const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, reducedMotion: "reduce" });
const errors = [], results = {};
page.on("pageerror", error => errors.push(error.message));
try {
  await page.goto(url);
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && !state.loadOverlay && state.rows.length);
  await page.evaluate(async () => {
    await WorkspaceContext.setScope("dataset", { animate: false, page: "explore" });
    window.reviewFilters = structuredClone(state.filters); window.reviewPage = document.body.dataset.page;
    state.loadOverlay = true;
    try { await Discovery.applySelection([{column:"source",op:"equals_exact",value:"Do not apply"}], "case", true); await Journeys.open({ scope:"case", event:state.rows[0] }); }
    finally { state.loadOverlay = false; }
  });
  assert.deepEqual(await page.evaluate(() => state.filters), await page.evaluate(() => window.reviewFilters));
  assert.equal(await page.evaluate(() => document.body.dataset.page), await page.evaluate(() => window.reviewPage));
  results.refusedContextSwitch = "selection and Journey seed do not leak to the active area";
  await page.evaluate(async () => {
    const original = api; window.reviewRestoreApi = () => { api = original; };
    api = async (command,args,options) => command === "aggregate_events" ? { columns:[args.groupColumn,"Registros"], rows:Array.from({length:250},(_,i)=>({[args.groupColumn]:`Grupo ${i}`,Registros:i+1})) } : original(command,args,options);
    state.groupCol = "level"; state.aggs=[{func:"count",column:"*",alias:"Registros"}]; switchTab("group"); await runGroup();
  });
  await page.locator("#aw-group-order").selectOption("smallest");
  await page.getByRole("button",{name:"Próximos grupos",exact:true}).click();
  const groupBefore = await page.evaluate(() => WorkspaceAnalysis.capture().group);
  assert.equal(groupBefore.page,1);
  await page.evaluate(() => WorkspaceContext.setScope("case",{page:"explore",tab:"group",animate:false}));
  await page.waitForFunction(() => document.querySelector("#group-table").getAttribute("aria-busy") !== "true");
  await page.locator("#aw-group-order").selectOption("name");
  await page.evaluate(() => WorkspaceContext.setScope("dataset",{page:"explore",tab:"group",animate:false}));
  await page.waitForFunction(() => document.querySelector("#group-table").getAttribute("aria-busy") !== "true");
  assert.deepEqual(await page.evaluate(() => WorkspaceAnalysis.capture().group),groupBefore);
  assert.equal(await page.locator("#aw-group-order").inputValue(),"smallest");
  results.groupPosition = "page2 and measure ordering survive context switches";
  await page.evaluate(async () => { window.reviewRestoreApi(); switchTab("cube"); await runCube(); });
  await page.locator("#aw-pivot-search").fill("Info");
  await page.locator("#aw-pivot-order").selectOption("desc");
  await page.locator("#aw-pivot-heat").click();
  await page.evaluate(() => WorkspaceContext.setScope("case",{page:"explore",tab:"cube",animate:false}));
  await page.locator("#aw-pivot-search").fill("other");
  await page.locator("#aw-pivot-order").selectOption("asc");
  await page.evaluate(() => WorkspaceContext.setScope("dataset",{page:"explore",tab:"cube",animate:false}));
  assert.equal(await page.locator("#aw-pivot-search").inputValue(),"Info");
  assert.equal(await page.locator("#aw-pivot-order").inputValue(),"desc");
  assert.equal(await page.locator("#aw-pivot-heat").getAttribute("aria-pressed"),"false");
  results.pivotControls = "search, ordering and heat button reflect restored state";
  await page.evaluate(async () => {
    Discovery.restore({mode:"frequency",cross:{row:"source",column:"level"},heat:{row:"code",color:'["max","duracao_ms"]',border:"",bins:"48"}});
    window.reviewDiscovery = Discovery.capture();
    await WorkspaceContext.setScope("case",{animate:false});
    Discovery.restore({mode:"frequency",cross:{row:"code",column:"source"},heat:{row:"level",color:"",border:"",bins:"12"}});
    await WorkspaceContext.setScope("dataset",{animate:false});
  });
  assert.deepEqual(await page.evaluate(() => Discovery.capture()),await page.evaluate(() => window.reviewDiscovery));
  results.discoveryControls = "cross field choices and heat channels are separate per context";
  // Literal placeholder strings and column tuples with display separators remain distinct.
  await page.evaluate(async () => {
    const base=state.rows[0]||caseEvents()[0]; const rows=[
      {bucket:null,a:"A → B",b:"C"},{bucket:"(vazio)",a:"A",b:"B → C"},{bucket:" API ",a:"A",b:"C"},{bucket:"API",a:"A",b:"C"},{bucket:"api",a:"A",b:"C"}
    ].map((fields,id)=>({...base,id,event_ref:`review:${id}`,fields:{...base.fields,...fields}}));
    activeCase().items=[{id:"review-keys",kind:"events",rows}]; caseEventsCache.sig=null; updateAnalysisBadge();
    await WorkspaceContext.setScope("case",{page:"explore",tab:"group",animate:false}); state.groupCol="bucket";state.aggs=[{func:"count",column:"*",alias:"Registros"}];await runGroup();
  });
  await page.waitForFunction(() => document.querySelectorAll("#group-table tbody tr").length===5 && document.querySelector("#group-table").getAttribute("aria-busy")!=="true");
  assert.equal(await page.locator("#group-table tbody tr").count(),5);
  await page.locator("#group-table tbody tr").filter({has:page.getByText('“(vazio)”',{exact:true})}).getByRole("button",{name:"Ver registros →",exact:true}).click();
  await page.waitForFunction(() => state.rows.length===1 && state.filters.some(f=>f.column==="bucket"));
  assert.deepEqual(await page.evaluate(() => state.filters.find(f=>f.column==="bucket")),{column:"bucket",op:"equals_exact",value:"(vazio)",value2:null});
  assert.equal(await page.evaluate(() => state.rows[0].fields.bucket),"(vazio)");
  await page.evaluate(async () => {
    state.filters=[];state.quick="";switchTab("cube");const cube=activeCube();cube.rows=["bucket"];cube.cols=["a","b"];cube.values=[{func:"count",column:"*",alias:"Registros"}];markCubeTableChanged();await runCube();
  });
  await page.locator("#aw-pivot-search").fill("");
  await page.waitForFunction(() => document.querySelector("#aw-pivot-summary").textContent.includes("5 linhas"));
  const tuples=await page.evaluate(() => cubeResultForTable(activeCube()).col_values);
  assert.ok(tuples.some(v=>v[0]==="A → B"&&v[1]==="C"));assert.ok(tuples.some(v=>v[0]==="A"&&v[1]==="B → C"));
  const row=page.locator("#cube-table tbody tr").filter({has:page.getByText('“(vazio)”',{exact:true})});
  await row.locator("td.cube-value").filter({hasText:/^1$/}).click({button:"right"});
  await page.getByText("Abrir registros correspondentes",{exact:true}).click();
  await page.waitForFunction(() => state.filters.some(f=>f.column==="b") && state.rows.length===1);
  assert.deepEqual(await page.evaluate(() => state.filters.map(f=>[f.column,f.op,f.value]).sort()),[["a","equals_exact","A"],["b","equals_exact","B → C"],["bucket","equals_exact","(vazio)"]]);
  results.exactKeys = "literal(vazio), API/api/spaces and composite pivot columns produce exact filters";
  const chartChecks = await page.evaluate(() => {
    const table=activeCube(); table.values=[{func:"avg",column:"duration",alias:"Média"}];
    const snapshot={value_names:["Média"],value_functions:["avg"],col_keys:["A","B"],row_paths:[["g"]],row_values:[["g"]],cells:[[[2],[8]]],totals:[[2],[8]]};
    const chart={snapshot,sourceRows:["bucket"],sourceTableId:table.id,sourceSchemaSignature:cubeSchemaSignature(table),columnIndex:-1,measureIndex:0};
    const sum= cubeChartPoints(chart); chart.columnIndex=0; const first=cubeChartPoints(chart);
    snapshot.cells[0][0][0]=null; const missing=cubeChartPoints(chart); snapshot.cells[0][0][0]=2;
    openCubeChartModal(null,snapshot);
    const invalidSumDisabled=document.querySelector('#cube-chart-column option[value="-1"]').disabled, column=document.querySelector('#cube-chart-column').value;
    document.querySelector('#cube-chart-modal').hidden=true;
    const donut=document.createElement('div');renderDonut(donut,{x:['one'],series:[{points:[10]}]}, {},'case');
    const bars=document.createElement('div');renderHBars(bars,{x:['negative','positive'],series:[{points:[-3,2]}]}, {},'case');
    return {sum,first,missing,invalidSumDisabled,column,fullCircle:donut.querySelectorAll('circle[r="60"]').length,negativeWidth:bars.querySelector('.hbar-fill').style.width,negativeLeft:bars.querySelector('.hbar-fill').style.left};
  });
  assert.deepEqual(chartChecks.sum,[],"averages cannot be summed across columns");
  assert.deepEqual(chartChecks.first,[{label:"g",value:2}]);
  assert.deepEqual(chartChecks.missing,[],"missing numeric values never become fake zeros");
  assert.equal(chartChecks.invalidSumDisabled,true);assert.equal(chartChecks.column,"0");
  assert.equal(chartChecks.fullCircle,1);assert.equal(chartChecks.negativeWidth,"60%");assert.equal(chartChecks.negativeLeft,"0%");
  results.customCharts = "nonadditive column totals blocked; null preserved; full donut and signed bars visible";
  await page.screenshot({path:resolve(output,"analytics-review-context.png")});
  assert.deepEqual(errors,[]);
  writeFileSync(resolve(output,"analytics-review-results.json"),JSON.stringify({...results,errors},null,2));
  console.log(JSON.stringify({...results,errors},null,2));
} finally {await browser.close();}
