/* UI contract test against the preview fixture; never contacts Elasticsearch/Kibana. */
import assert from "node:assert/strict";
import { launchBrowser } from "./browser.mjs";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const url = process.argv[2] || "http://127.0.0.1:4175";
const output = resolve("output/playwright"); mkdirSync(output, { recursive: true });
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1024, height: 768 }, reducedMotion: "reduce" });
const errors = [], result = {};
let phase = "initial readiness";
const openConnections = async () => {
  await page.locator('.zone-switch [data-zone="structure"]').click();
  await page.locator('.nav-pages [data-page="connections"]').click();
  await page.waitForFunction(() => document.body.dataset.page === "connections" && !WorkspaceContext.changing && document.querySelector("#ws-content .remote-modal.docked"));
};
page.on("pageerror", error => errors.push(error.message));
const idle = () => page.waitForFunction(() => document.querySelector("#rs-form").getAttribute("aria-busy") !== "true");
const counts = () => page.evaluate(() => Object.fromEntries(["remote_save", "remote_test", "remote_import", "remote_delete"].map(command => [command, window.__remoteMock.requests.filter(item => item.command === command).length])));
try {
  await page.goto(url);
  // First rows intentionally precede completion; interact only when startup restoration permits user input.
  await page.waitForFunction(() => state.loaded && state.rows.length > 0 && window.WorkspaceContext?.ready && !WorkspaceContext.changing && !state.loadOverlay, null, { timeout: 30000 });
  if(await page.evaluate(()=>!!window.WorkspaceContext)){
    await page.evaluate(()=>WorkspaceContext.setScope('case',{animate:false}));
    assert.equal(await page.locator('.nav-pages [data-page="connections"]').isVisible(),false,'Remote sources belong to Analysis');
    await page.evaluate(()=>RemoteSources.open());
    assert.equal(await page.evaluate(()=>WorkspaceContext.scope()),'dataset','Opening sources returns to Analysis');
    result.contextRouting=true;
  }else await page.getByRole("button", { name: "Conexões", exact: true }).first().click();
  await page.waitForFunction(() => !!window.__remoteMock && !WorkspaceContext.changing && document.body.dataset.page === "connections" && document.querySelector("#rs-secret-note").textContent.includes("Desmarcado"));
  phase = "connection form";
  await page.locator("#rs-name").fill("Produção · API");
  await page.locator("#rs-url").fill("https://elastic.example.test:9200");
  await page.locator("#rs-index").fill("logs-api-*");
  await page.locator("#rs-username").fill("analista");
  await page.locator("#rs-password").fill("fixture-secret-only");
  await page.locator("#rs-remember").check();
  await page.locator("#rs-test").click(); await idle();
  result.afterTest = await counts();
  assert.equal(result.afterTest.remote_save, 0); assert.equal(result.afterTest.remote_import, 0); assert.equal(result.afterTest.remote_test, 1);
  assert.match(await page.locator("#rs-status-text").textContent(), /Acesso confirmado/);
  await page.locator("#rs-save").click(); await idle();
  assert.equal(await page.locator("#rs-list .remote-connection").count(), 1);
  assert.equal(await page.locator("#rs-password").inputValue(), "");
  result.afterSave = await counts(); assert.equal(result.afterSave.remote_import, 0); assert.equal(result.afterSave.remote_save, 1);
  // Connections is docked; leaving the page, rather than a hidden modal Close, clears the secret input.
  phase = "leave and reopen connections";
  await page.locator('.nav-pages [data-page="sources"]').click();
  await page.waitForFunction(() => document.body.dataset.page === "sources");
  assert.equal(await page.locator("#rs-password").inputValue(), "");
  await openConnections();
  assert.equal(await page.locator("#rs-password").inputValue(), "");
  result.secretOutsideCase = await page.evaluate(() => !JSON.stringify(state.cases).includes("fixture-secret-only") && !JSON.stringify({ ...localStorage }).includes("fixture-secret-only"));
  assert.equal(result.secretOutsideCase, true);

  await page.getByText("Filtro avançado · Query DSL", { exact: true }).click();
  await page.locator("#rs-query").fill('{"query":');
  await page.locator("#rs-test").click();
  assert.match(await page.locator("#rs-status-text").textContent(), /JSON válido/);
  assert.equal((await counts()).remote_test, 1);
  await page.locator("#rs-query").fill('{"match":{"service.name":"api"}}');
  await page.locator("#rs-from").fill("2026-09-22T12:00");
  await page.locator("#rs-to").fill("2026-09-21T12:00");
  await page.locator("#rs-import").click();
  assert.match(await page.locator("#rs-status-text").textContent(), /anterior/); assert.equal((await counts()).remote_import, 0);
  await page.locator("#rs-to").fill("2026-09-23T12:00");
  await page.evaluate(() => { window.__remoteMock.nextError = "Acesso negado ao índice. Verifique as permissões."; });
  await page.locator("#rs-test").click(); await idle();
  assert.match(await page.locator("#rs-status-text").textContent(), /Acesso negado/);
  assert.equal(await page.locator("#rs-url").inputValue(), "https://elastic.example.test:9200");
  assert.equal(await page.locator("#rs-list .remote-connection").count(), 1);

  phase = "cancel import";
  await page.evaluate(() => { window.__remoteMock.delay = 900; });
  await page.locator("#rs-import").click();
  await page.locator("#rs-cancel").click(); await idle();
  assert.match(await page.locator("#rs-status-text").textContent(), /cancelada/);
  result.cancelPreservesConnection = await page.locator("#rs-list .remote-connection").count() === 1;
  assert.equal(result.cancelPreservesConnection, true);
  await page.evaluate(() => { window.__remoteMock.delay = 100; });
  await page.screenshot({ path: resolve(output, "remote-connection-1024.png") });
  phase = "import snapshot";
  await page.locator("#rs-import").click();
  await page.waitForFunction(() => !state.loadOverlay && document.body.dataset.page === "summary" && state.currentArtifact?.path.includes("remote-"), null, { timeout: 30000 });
  result.snapshot = await page.evaluate(() => ({ kind: state.currentArtifact.source.kind, format: state.currentArtifact.source.format, page: document.body.dataset.page, hasRemotePasswordInCase: JSON.stringify(state.cases).includes("fixture-secret-only") }));
  assert.equal(result.snapshot.kind, "file"); assert.equal(result.snapshot.format, "jsonl"); assert.equal(result.snapshot.page, "summary"); assert.equal(result.snapshot.hasRemotePasswordInCase, false);
  result.importRequest = await page.evaluate(() => {
    const request = window.__remoteMock.requests.filter(item => item.command === "remote_import").at(-1);
    return { from: request.from, to: request.to, maxRecords: request.connection.maxRecords, query: request.connection.query, passwordProvided: request.passwordProvided };
  });
  assert.equal(result.importRequest.maxRecords, 100000); assert.ok(result.importRequest.from.endsWith("Z")); assert.deepEqual(result.importRequest.query, { match: { "service.name": "api" } }); assert.equal(result.importRequest.passwordProvided, false);

  await openConnections();
  await page.locator("#rs-delete").click(); await idle();
  assert.equal(await page.locator("#rs-list .remote-connection").count(), 0);
  result.deleteKeepsSnapshot = await page.evaluate(() => state.currentArtifact?.path.includes("remote-")); assert.equal(result.deleteKeepsSnapshot, true);
  await page.locator("#rs-kind").selectOption("kibana");
  assert.match(await page.locator("#rs-url-hint").textContent(), /Console/); assert.match(await page.locator("#rs-url-hint").textContent(), /SSO/);
  await page.evaluate(() => { window.__remoteMock.persistentSecrets = false; });
  await page.locator("#rs-reload").click();
  await page.waitForFunction(() => document.querySelector("#rs-remember").disabled);
  result.sessionOnlyOnUnsupportedOS = await page.locator("#rs-remember").isDisabled(); assert.equal(result.sessionOnlyOnUnsupportedOS, true);
  assert.deepEqual(errors, []); result.pageErrors = errors;
  writeFileSync(resolve(output, "remote-validation.json"), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  const diagnostics = { phase, error: String(error.stack || error), errors, result };
  try {
    diagnostics.page = await page.evaluate(() => ({
      page: document.body.dataset.page, scope: WorkspaceContext.scope(), changing: WorkspaceContext.changing,
      ready: WorkspaceContext.ready, loaded: state.loaded, loadOverlay: state.loadOverlay,
      panelDocked: document.querySelector(".remote-modal")?.classList.contains("docked"),
      panelParent: document.querySelector(".remote-modal")?.parentElement?.id,
      rememberRect: document.querySelector("#rs-remember")?.getBoundingClientRect().toJSON(),
      status: document.querySelector("#rs-status-text")?.textContent,
      busy: document.querySelector("#rs-form")?.getAttribute("aria-busy"),
      requests: window.__remoteMock?.requests.map(({ command }) => command),
    }));
  } catch (captureError) { diagnostics.captureError = String(captureError); }
  await page.screenshot({ path: resolve(output, "remote-failure.png"), fullPage: true }).catch(() => {});
  writeFileSync(resolve(output, "remote-failure.json"), JSON.stringify(diagnostics, null, 2));
  console.error(JSON.stringify(diagnostics, null, 2));
  throw error;
} finally { await browser.close(); }
