/* Real field menus with an opt-in native-text transport fixture. Native Rust
   tests own serialization/comparator correctness; this checks browser wiring. */
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { launchBrowser } from "./browser.mjs";
import { captureFailure } from "./diagnostics.mjs";

const output = resolve("output/playwright");
mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, reducedMotion: "reduce" });
page.setDefaultTimeout(20_000);
const errors = [], results = { evidence: "Real browser menus against an opt-in native-text preview fixture; not native engine verification", cells: {}, details: {} };
let phase = "startup";
page.on("pageerror", error => errors.push(error.message));
await page.addInitScript(() => {
  window.__mockCanonicalFieldsEnabled = true;
  window.__canonicalClipboardWrites = [];
  // Observe the actual copy action without requiring clipboard permissions.
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
    writeText: async text => { window.__canonicalClipboardWrites.push(text); },
  } });
});

async function composerFrom(menuTarget, column, expected, operator = "equals_exact") {
  await menuTarget.click({ button: "right" });
  await page.locator(".ctx-menu").getByRole("menuitem", { name: `Criar filtro: ${column}`, exact: true }).click();
  await page.locator("#filter-pop").waitFor({ state: "visible" });
  assert.equal(await page.locator("#fp-col").inputValue(), column);
  assert.equal(await page.locator("#fp-op").inputValue(), operator);
  assert.equal(await page.locator("#fp-val").inputValue(), expected);
  return { column, operator, value: expected };
}

async function cancelEditableDraft(expected) {
  await page.locator("#fp-val").fill(`${expected} edited`);
  assert.equal(await page.locator("#fp-val").inputValue(), `${expected} edited`, "native text stays editable");
  await page.locator("#fp-cancel").click();
  assert.equal(await page.locator("#filter-pop").isVisible(), false);
  assert.deepEqual(await page.evaluate(() => state.filters), [], "cancelled drafts never run numeric comparisons in the mock");
}

async function copyFrom(menuTarget, expected) {
  const before = await page.evaluate(() => window.__canonicalClipboardWrites.length);
  await menuTarget.click({ button: "right" });
  await page.locator(".ctx-menu").getByRole("menuitem", { name: "Copiar valor", exact: true }).click();
  await page.waitForFunction(count => window.__canonicalClipboardWrites.length === count + 1, before);
  const copied = await page.evaluate(() => window.__canonicalClipboardWrites.at(-1));
  assert.equal(copied, expected);
  return copied;
}

async function labelledScreenshot(name, label) {
  await page.evaluate(text => {
    const label = document.createElement("div");
    label.id = "canonical-fixture-label";
    label.textContent = text;
    Object.assign(label.style, { position: "fixed", left: "12px", bottom: "12px", zIndex: "99999", padding: "10px 14px",
      background: "#142139", color: "#fff", border: "1px solid #9ac7ff", borderRadius: "6px", font: "13px system-ui", pointerEvents: "none" });
    document.body.appendChild(label);
  }, label);
  try { await page.screenshot({ path: resolve(output, name) }); }
  finally { await page.locator("#canonical-fixture-label").evaluate(node => node.remove()); }
}

try {
  await page.goto(process.argv[2] || "http://127.0.0.1:4174");
  await page.waitForFunction(() => window.WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded
    && !state.loadOverlay && document.querySelector("#load-overlay").hidden);
  await page.getByRole("button", { name: "Explorar", exact: true }).click();
  await page.waitForFunction(() => explorerAnalytics.get(explorerKey())?.status === "done"
    && document.querySelector("#events-table").getAttribute("aria-busy") === "false");
  const fixture = await page.evaluate(() => window.__mockCanonicalFixture);
  assert.ok(fixture?.eventRef, "the canonical fixture is explicitly enabled for this scenario only");
  results.fixture = fixture;
  await page.evaluate(() => {
    state.visibleCols = ["timestamp", "native_float", "native_object", "native_u64", "native_null", "native_missing"];
    saveVisibleCols();
    renderTable({ total: state.total, rows: state.rows });
  });
  results.decoded = await page.evaluate(id => {
    const event = state.rows.find(row => row.id === id);
    return Object.fromEntries(["native_float", "native_object", "native_u64"].map(column => [column, cellValue(event, column)]));
  }, fixture.id);
  for (const column of ["native_float", "native_object", "native_u64"]) {
    assert.notEqual(results.decoded[column], fixture.values[column], `${column} must expose a real JavaScript/native-text mismatch`);
  }
  const row = page.locator(`#events-table tbody tr[data-event-id="${fixture.id}"]`);
  for (const column of ["native_float", "native_object", "native_u64", "native_null"]) {
    phase = `row menu ${column}`;
    const expected = fixture.values[column], cell = row.locator(`td[data-column="${column}"]`);
    const draft = await composerFrom(cell, column, expected);
    if (column === "native_u64") await labelledScreenshot("canonical-field-row-u64-1440.png",
      "Preview transport fixture · Row menu preserves native u64: 18446744073709551615 · Not native engine verification");
    await cancelEditableDraft(expected);
    results.cells[column] = { draft, copied: await copyFrom(cell, expected) };
  }

  phase = "missing field row menu";
  const missing = row.locator('td[data-column="native_missing"]');
  results.cells.native_missing = await composerFrom(missing, "native_missing", "", "empty");
  await page.locator("#fp-cancel").click();
  assert.deepEqual(await page.evaluate(() => state.filters), []);

  phase = "detail menus";
  await row.locator('td[data-column="native_float"]').click();
  await page.waitForFunction(id => state.currentDetailEv?.id === id && !document.querySelector("#drawer").hidden, fixture.id);
  for (const column of ["native_float", "native_object", "native_u64", "native_null"]) {
    phase = `detail menu ${column}`;
    const expected = fixture.values[column];
    const value = page.locator(`#pane-overview .detail-tree-row[data-col="${column}"] > .detail-tree-value-line > .detail-tree-value`);
    const draft = await composerFrom(value, column, expected);
    if (column === "native_object") await labelledScreenshot("canonical-field-detail-object-1440.png",
      'Preview transport fixture · Detail menu preserves native key order: {"10":"ten","2":"two"} · Not native engine verification');
    await cancelEditableDraft(expected);
    results.details[column] = { draft, copied: await copyFrom(value, expected) };
  }
  await page.locator("#dr-close").click();

  phase = "mixed native line endings";
  const rawMultiline = "alpha\r\nbravo\ncharlie\rdelta", encodedMultiline = JSON.stringify(rawMultiline);
  assert.equal(fixture.values.native_multiline, rawMultiline);
  const initialTotal = await page.evaluate(() => state.total);
  await page.evaluate(() => {
    state.visibleCols = ["timestamp", "native_multiline"];
    saveVisibleCols(); renderTable({ total: state.total, rows: state.rows });
    window.__canonicalMultilineReads = []; window.__canonicalMultilineQueries = [];
    window.__canonicalMultilineApi = api;
    api = async (command, args, options) => {
      if (command === "query_page" && args.filters?.some(filter => filter.column === "native_multiline"))
        window.__canonicalMultilineQueries.push(structuredClone(args.filters));
      const result = await window.__canonicalMultilineApi(command, args, options);
      if (command === "analysis_field_text" && args.column === "native_multiline") window.__canonicalMultilineReads.push(structuredClone(result));
      return result;
    };
  });
  const multilineCell = row.locator('td[data-column="native_multiline"]'), input = page.locator("#fp-val");
  await composerFrom(multilineCell, "native_multiline", encodedMultiline);
  assert.equal(await input.evaluate(node => node.tagName), "TEXTAREA");
  assert.equal(await input.getAttribute("data-value-format"), "json-string");
  assert.equal(await page.locator("#fp-text-hint").isVisible(), true);
  assert.match(await page.locator("#fp-text-hint").textContent(), /texto JSON entre aspas/);
  // Insert a newline inside the quoted string: the editor must insert the
  // visible escape, not an invalid literal line break into JSON-string mode.
  await input.press("End"); await input.press("ArrowLeft"); await input.press("Shift+Enter");
  const jsonWithNewline = encodedMultiline.slice(0, -1) + '\\n"';
  assert.equal(await input.inputValue(), jsonWithNewline);
  assert.equal(JSON.parse(await input.inputValue()), rawMultiline + "\n");
  assert.deepEqual(await page.evaluate(() => state.filters), []);
  await input.fill('"unterminated'); await input.press("Enter");
  assert.equal(await page.locator("#filter-pop").isVisible(), true, "invalid JSON keeps the draft open");
  assert.equal(await input.inputValue(), '"unterminated');
  assert.deepEqual(await page.evaluate(() => state.filters), []);
  await input.fill(JSON.stringify(rawMultiline + " edited")); await page.locator("#fp-cancel").click();
  assert.deepEqual(await page.evaluate(() => state.filters), [], "editing then cancelling preserves the previous analysis");

  await composerFrom(multilineCell, "native_multiline", encodedMultiline);
  await labelledScreenshot("canonical-field-multiline-json-1440.png",
    "Preview transport fixture · JSON string editor preserves mixed CRLF, LF and CR · Not native engine verification");
  await input.press("Enter");
  await page.waitForFunction(id => state.total === 1 && state.rows.length === 1 && state.rows[0].id === id
    && document.querySelector("#events-table").getAttribute("aria-busy") === "false", fixture.id);
  const exactMultilineFilter = { column: "native_multiline", op: "equals_exact", value: rawMultiline, value2: null };
  assert.deepEqual(await page.evaluate(() => state.filters), [exactMultilineFilter]);
  assert.equal(await page.locator("#filter-pop").isVisible(), false);
  assert.equal(await copyFrom(multilineCell, rawMultiline), rawMultiline);
  const editMultilineChip = async () => {
    await page.locator(".chips-sync:visible .chip").filter({ hasText: "native_multiline" }).first().click({ button: "right" });
    await page.locator(".ctx-menu").getByRole("menuitem", { name: "Editar filtro", exact: true }).click();
    await page.locator("#filter-pop").waitFor({ state: "visible" });
  };
  await editMultilineChip();
  assert.equal(await input.inputValue(), encodedMultiline, "filter-chip editing restores the exact JSON representation");
  await input.fill(JSON.stringify(rawMultiline + " changed")); await page.locator("#fp-cancel").click();
  assert.deepEqual(await page.evaluate(() => state.filters), [exactMultilineFilter], "cancelled chip edits preserve raw stored line endings");
  await editMultilineChip(); assert.equal(await input.inputValue(), encodedMultiline); await input.press("Enter");
  await page.waitForFunction(() => state.total === 1 && document.querySelector("#events-table").getAttribute("aria-busy") === "false");
  assert.deepEqual(await page.evaluate(() => state.filters), [exactMultilineFilter], "unchanged chip edits round-trip the original mixed line endings");
  results.multiline = await page.evaluate(() => ({ reads: window.__canonicalMultilineReads, queries: window.__canonicalMultilineQueries,
    filters: structuredClone(state.filters), matchedRows: state.rows.map(row => row.id) }));
  assert.equal(results.multiline.reads.length, 3, "two composer opens and one copy each resolve the native string");
  for (const response of results.multiline.reads) assert.equal(response.canonicalText, rawMultiline);
  assert.ok(results.multiline.queries.length > 0);
  for (const filters of results.multiline.queries) assert.deepEqual(filters, [exactMultilineFilter]);
  results.multiline.jsonInput = encodedMultiline; results.multiline.shiftEnterJson = jsonWithNewline;
  await page.locator("#btn-clear-filters").click();
  await page.waitForFunction(total => state.total === total && document.querySelector("#events-table").getAttribute("aria-busy") === "false", initialTotal);

  phase = "plain LF preset and Shift+Enter";
  const plainMultiline = "plain\nline", plainFilter = { column: "native_multiline", op: "equals_exact", value: plainMultiline, value2: null };
  await page.locator("#btn-add-filter").click();
  await page.locator("#fp-col").selectOption("native_multiline"); await page.locator("#fp-op").selectOption("equals_exact");
  await input.fill(plainMultiline); await input.press("Enter");
  await page.waitForFunction(() => state.total === 0 && document.querySelector("#events-table").getAttribute("aria-busy") === "false");
  assert.deepEqual(await page.evaluate(() => state.filters), [plainFilter]);
  await editMultilineChip();
  assert.equal(await input.inputValue(), plainMultiline, "LF-only presets remain plain multiline text");
  assert.equal(await input.getAttribute("data-value-format"), "text");
  await input.press("End"); await input.press("Shift+Enter");
  assert.equal(await input.inputValue(), plainMultiline + "\n", "Shift+Enter inserts an actual LF in plain mode");
  assert.equal(await page.locator("#filter-pop").isVisible(), true, "Shift+Enter never applies the draft");
  await page.locator("#fp-cancel").click();
  assert.deepEqual(await page.evaluate(() => state.filters), [plainFilter]);
  results.multiline.plainPreset = plainMultiline; results.multiline.shiftEnterPlain = plainMultiline + "\n";
  await page.locator("#btn-clear-filters").click();
  await page.waitForFunction(total => state.total === total && document.querySelector("#events-table").getAttribute("aria-busy") === "false", initialTotal);
  await page.evaluate(() => { api = window.__canonicalMultilineApi; delete window.__canonicalMultilineApi; });

  phase = "receipt and absence contract";
  results.presence = await page.evaluate(async fixture => {
    const owner = AnalysisContexts.capture(), responses = {};
    for (const column of ["native_null", "native_missing"]) responses[column] = await window.__TAURI__.core.invoke("analysis_field_text", {
      id: fixture.id, eventRef: fixture.eventRef, column, analysisContext: owner.identity,
      sourceGeneration: owner.sourceGeneration, caseKey: null, operationId: `canonical-contract-${column}`,
    });
    return responses;
  }, fixture);
  assert.equal(results.presence.native_null.presence, "null");
  assert.equal(results.presence.native_null.valueType, "null");
  assert.equal(results.presence.native_null.canonicalText, "null");
  assert.equal(results.presence.native_missing.presence, "missing");
  assert.equal(results.presence.native_missing.valueType, null);
  assert.equal(results.presence.native_missing.canonicalText, null);
  assert.equal(results.presence.native_null.receipt.caseContentToken, null);
  assert.equal(results.presence.native_missing.receipt.caseContentToken, null);
  const requests = await page.evaluate(() => window.__mockRequests.filter(request => request.cmd === "analysis_field_text"));
  assert.equal(requests.length, 22, "row/detail actions, three multiline actions and two contract reads each resolve native text");
  for (const request of requests) {
    assert.equal(request.id, fixture.id);
    assert.equal(request.eventRef, fixture.eventRef);
    assert.equal(request.hasCaseEvents, false, "native field commands carry identity/receipt context, never projected row arrays");
    assert.ok(request.analysisContext?.analysisId);
    assert.ok(Number.isSafeInteger(request.sourceGeneration));
  }
  results.requests = requests;
  assert.deepEqual(await page.evaluate(() => state.filters), []);
  assert.deepEqual(errors, []);
  results.ok = true;
  writeFileSync(resolve(output, "canonical-field-actions.json"), JSON.stringify(results, null, 2));
  console.log(JSON.stringify(results, null, 2));
} catch (error) {
  await captureFailure(page, "canonical-field-actions", error, { phase, errors, results });
  throw error;
} finally {
  await browser.close();
}
