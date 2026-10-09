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
  await page.waitForFunction(() => !!window.__remoteMock && !WorkspaceContext.changing && document.body.dataset.page === "connections");
  assert.equal(await page.locator("#rs-list").isVisible(), false, "Global vault requires an explicit opening");
  await page.locator("#rs-reload").click();
  await page.waitForFunction(() => document.querySelector("#rs-secret-note").textContent.includes("Desmarcado"));
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
  await page.waitForFunction(() => !!document.querySelector("#rs-url").value);
  assert.equal(await page.locator("#rs-list").isVisible(), false);
  await page.locator("#rs-reload").click();
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
  const originalPaths = await page.evaluate(() => {
    const files = source => source?.kind === "bundle" ? source.members.flatMap(files) : [source?.path].filter(Boolean);
    return files(state.currentArtifact?.source);
  });
  assert.ok(originalPaths.length > 0, "Import fixture starts with an existing source");
  await page.locator("#rs-import").click();
  // Import adds a local snapshot to the open dataset; the bundle path belongs to its first member.
  await page.waitForFunction(() => {
    const files = source => source?.kind === "bundle" ? source.members.flatMap(files) : [source].filter(Boolean);
    return !state.loadOverlay && document.body.dataset.page === "summary" && files(state.currentArtifact?.source).some(source => source.path?.includes("remote-"));
  }, null, { timeout: 30000 });
  result.snapshot = await page.evaluate(() => {
    const files = source => source?.kind === "bundle" ? source.members.flatMap(files) : [source].filter(Boolean);
    const members = files(state.currentArtifact?.source), remote = members.find(source => source.path?.includes("remote-"));
    return { kind: remote?.kind, format: remote?.format, bundleKind: state.currentArtifact.source.kind,
      paths: members.map(source => source.path), page: document.body.dataset.page,
      hasRemotePasswordInCase: JSON.stringify(state.cases).includes("fixture-secret-only") };
  });
  assert.equal(result.snapshot.kind, "file"); assert.equal(result.snapshot.format, "jsonl");
  assert.equal(result.snapshot.bundleKind, "bundle");
  for (const path of originalPaths) assert.ok(result.snapshot.paths.includes(path), "Import preserves each previously open source");
  assert.equal(result.snapshot.page, "summary"); assert.equal(result.snapshot.hasRemotePasswordInCase, false);
  result.importRequest = await page.evaluate(() => {
    const request = window.__remoteMock.requests.filter(item => item.command === "remote_import").at(-1);
    return { from: request.from, to: request.to, maxRecords: request.connection.maxRecords, query: request.connection.query, passwordProvided: request.passwordProvided };
  });
  assert.equal(result.importRequest.maxRecords, 100000); assert.ok(result.importRequest.from.endsWith("Z")); assert.deepEqual(result.importRequest.query, { match: { "service.name": "api" } }); assert.equal(result.importRequest.passwordProvided, false);

  await openConnections();
  await page.waitForFunction(() => !document.querySelector("#rs-delete").hidden);
  await page.locator("#rs-reload").click();
  await page.locator("#rs-delete").click(); await idle();
  assert.equal(await page.locator("#rs-list .remote-connection").count(), 0);
  result.deleteKeepsSnapshot = await page.evaluate(paths => {
    const files = source => source?.kind === "bundle" ? source.members.flatMap(files) : [source?.path].filter(Boolean);
    const actual = files(state.currentArtifact?.source);
    return paths.every(path => actual.includes(path));
  }, result.snapshot.paths); assert.equal(result.deleteKeepsSnapshot, true);
  await page.locator("#rs-kind").selectOption("kibana");
  assert.match(await page.locator("#rs-url-hint").textContent(), /Console/); assert.match(await page.locator("#rs-url-hint").textContent(), /SSO/);
  await page.evaluate(() => { window.__remoteMock.persistentSecrets = false; });
  await page.locator("#rs-reload").click();
  await page.waitForFunction(() => document.querySelector("#rs-remember").disabled);
  result.sessionOnlyOnUnsupportedOS = await page.locator("#rs-remember").isDisabled(); assert.equal(result.sessionOnlyOnUnsupportedOS, true);
  phase = "SSH and WinRM collection controls";
  await page.locator("#rs-new").click();
  await page.locator("#rs-kind").selectOption("ssh");
  assert.equal(await page.locator("#rs-files").isVisible(), true);
  assert.equal(await page.locator("#rs-password").isVisible(), false);
  await page.locator("#rs-presets").getByRole("button", { name: "+ Autenticação", exact: true }).click();
  assert.match(await page.locator("#rs-paths").inputValue(), /\/var\/log\/auth\.log/);
  await page.locator("#rs-name").fill("Linux da investigação");
  await page.locator("#rs-url").fill("ssh://linux.example.test:22");
  await page.locator("#rs-username").fill("analyst");
  await page.locator("#rs-test").click(); await idle();
  assert.equal(await page.evaluate(() => window.__remoteMock.requests.at(-1).connection.kind), "ssh");
  assert.deepEqual(await page.evaluate(() => window.__remoteMock.requests.at(-1).connection.paths), ["/var/log/auth.log", "/var/log/secure"]);
  await page.locator("#rs-kind").selectOption("winrm");
  await page.locator("#rs-paths").fill("");
  await page.locator("#rs-presets").getByRole("button", { name: "+ EVTX principais", exact: true }).click();
  assert.match(await page.locator("#rs-paths").inputValue(), /Security\.evtx/);
  assert.equal(await page.locator("#rs-password").isVisible(), true);
  assert.equal(await page.locator("#rs-query-options").isVisible(), false);
  result.fileCollectionControls = "SSH/Linux and WinRM/EVTX paths are Case-owned";

  phase = "Wazuh indexer and server API";
  const lastRequest = command => page.evaluate(name => window.__remoteMock.requests.filter(item => item.command === name).at(-1), command);
  await page.locator("#rs-new").click();
  await page.locator("#rs-kind").selectOption("wazuh");
  assert.equal(await page.locator("#rs-wazuh-data-wrap").isVisible(), true);
  assert.equal(await page.locator("#rs-tls").isVisible(), true);
  // An index authored in this Case is never replaced by switching services.
  assert.equal(await page.locator("#rs-index").inputValue(), "logs-api-*");
  assert.equal(await page.locator("#rs-wazuh-data").inputValue(), "");
  await page.locator("#rs-wazuh-data").selectOption("wazuh-findings-v5*");
  assert.equal(await page.locator("#rs-time-field").inputValue(), "@timestamp");
  await page.locator("#rs-wazuh-data").selectOption("wazuh-alerts-*");
  assert.equal(await page.locator("#rs-index").inputValue(), "wazuh-alerts-*");
  assert.equal(await page.locator("#rs-time-field").inputValue(), "timestamp");
  await page.locator("#rs-name").fill("Wazuh produção");
  await page.locator("#rs-url").fill("https://wazuh-indexer.example.test:9200");
  assert.match(await page.locator("#rs-url-hint").textContent(), /9200/);
  await page.locator("#rs-username").fill("leitor");
  await page.locator("#rs-password").fill("fixture-secret-only");
  await page.locator("#rs-ca").fill("C:\\certs\\root-ca.pem");
  await page.locator("#rs-test").click(); await idle();
  await page.locator("#rs-url").evaluate(node => node.scrollIntoView({ block: "start" }));
  await page.screenshot({ path: resolve(output, "remote-wazuh-1024.png") });
  let request = await lastRequest("remote_test");
  assert.deepEqual([request.connection.kind, request.connection.index, request.connection.timeField, request.connection.auth, request.connection.caPath, request.connection.insecureTls, request.connection.username],
    ["wazuh", "wazuh-alerts-*", "timestamp", "basic", "C:\\certs\\root-ca.pem", false, "leitor"]);
  await page.locator("#rs-auth").selectOption("token");
  assert.equal(await page.locator("#rs-username").isVisible(), false);
  assert.equal(await page.locator("#rs-password-label").textContent(), "Token");
  await page.locator("#rs-insecure").check();
  await page.locator("#rs-test").click(); await idle();
  request = await lastRequest("remote_test");
  assert.deepEqual([request.connection.auth, request.connection.username, request.connection.insecureTls, request.passwordProvided], ["token", "", true, true]);

  // The server API offers fixed data sets, without index, Query DSL or period.
  await page.locator("#rs-kind").selectOption("wazuhapi");
  for (const id of ["#rs-index", "#rs-time-field", "#rs-from", "#rs-to", "#rs-query-advanced"]) assert.equal(await page.locator(id).isVisible(), false, id);
  assert.equal(await page.locator("#rs-wazuh-data").inputValue(), "agents");
  assert.match(await page.locator("#rs-url-hint").textContent(), /55000/);
  await page.locator("#rs-auth").selectOption("basic");
  await page.locator("#rs-username").fill("");
  await page.locator("#rs-test").click();
  assert.match(await page.locator("#rs-status-text").textContent(), /usuário da API/);
  await page.locator("#rs-username").fill("wazuh-wui");
  await page.locator("#rs-url").fill("https://wazuh-server.example.test:55000");
  await page.evaluate(() => { window.__remoteMock.nextError = "Fixture: importação interrompida."; });
  await page.locator("#rs-import").click(); await idle();
  request = await lastRequest("remote_import");
  assert.deepEqual([request.connection.kind, request.connection.index, request.connection.timeField, request.connection.query, request.from, request.to],
    ["wazuhapi", "agents", "", null, undefined, undefined]);
  await page.locator("#rs-save").click(); await idle();
  request = await lastRequest("remote_save");
  assert.deepEqual([request.connection.index, request.connection.timeField, request.connection.auth], ["agents", "", "basic"]);
  assert.ok((await page.locator("#rs-list .remote-connection-kind").allTextContents()).includes("Wazuh API"));
  await page.locator("#rs-kind").selectOption("elasticsearch");
  assert.equal(await page.locator("#rs-index").inputValue(), "logs-*");
  assert.equal(await page.locator("#rs-auth-wrap").isVisible(), false);
  result.wazuh = "Indexer presets, token/CA options and server API data sets";
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
