/* Lógica da bancada de investigação técnica. */
"use strict";

const { invoke } = window.__TAURI__.core;
const dialogApi = window.__TAURI__.dialog;

// ------------------------------------------------------------------ constantes
const STANDARD = ["timestamp", "source", "level", "code", "name", "description", "message"];
const COL_LABELS = {
  _all: "Todo o evento",
  comentario: "Comentário",
  timestamp: "Data/hora",
  source: "Origem",
  level: "Nível",
  code: "Código",
  name: "Nome",
  description: "Descrição",
  message: "Mensagem",
};

const OPS = [
  ["contains", "contém"],
  ["not_contains", "não contém"],
  ["equals", "igual a"],
  ["not_equals", "diferente de"],
  ["equals_exact", "exatamente igual a"],
  ["not_equals_exact", "diferente do valor exato"],
  ["starts_with", "começa com"],
  ["regex", "regex"],
  ["pattern", "padrão"],
  ["gt", ">"],
  ["gte", "≥"],
  ["lt", "<"],
  ["lte", "≤"],
  ["between", "entre"],
  ["empty", "vazio"],
  ["not_empty", "não vazio"],
];
const OP_SYMBOL = {
  contains: "~", not_contains: "!~", equals: "=", not_equals: "≠",
  equals_exact: "=", not_equals_exact: "≠",
  starts_with: "^", regex: "/…/", gt: ">", gte: "≥", lt: "<", lte: "≤",
  between: "↔", empty: "vazio", not_empty: "preenchido",
};

const AGG_FUNCS = [
  ["count", "contar"],
  ["count_distinct", "contar distintos"],
  ["sum", "somar"],
  ["avg", "média"],
  ["min", "mínimo"],
  ["max", "máximo"],
  ["string_agg", "juntar textos"],
];

const LEVEL_COLOR = {
  "Crítico": "var(--lv-critico)",
  "Erro": "var(--lv-erro)",
  "Aviso": "var(--lv-aviso)",
  "Informação": "var(--lv-informacao)",
  "Depuração": "var(--lv-depuracao)",
  "Rastreio": "var(--lv-rastreio)",
};
const levelColor = (lv) => LEVEL_COLOR[lv] || "var(--text-2)";
const workspaceScope = () => window.WorkspaceContext?.scope() || "dataset";

// ------------------------------------------------------------------ estado
const state = {
  columns: [...STANDARD],
  visibleCols: ["timestamp", "level", "code", "name", "message"],
  filters: [],
  sortCol: "timestamp",
  sortDir: "desc",
  page: 0,
  pageSize: 100,
  total: 0,
  rows: [],
  groupCol: "level",
  aggs: [{ func: "count", column: "*", alias: "qtd" }],
  loaded: false,
  quick: "", // filtro rápido de mensagem
  detailId: null,
  cases: { active: null, cases: [] }, // casos de análise persistidos
  analysisView: "overview", // visão persistida do Caso
  currentOrigin: "", // origem dos eventos carregados (estação ou fonte)
  currentArtifact: null,
  artifactSessions: new Map(), // fontes abertas, separadas por Caso e mantidas apenas nesta sessão
  artifactSwitchVersion: 0,
  activeContext: "artifact",
  activeStationId: null,
  stationAnalyticsId: null,
  dataPeriod: null,
  refreshVersion: 0,
  activeOperation: null,
  tsSources: [], // fontes de data/hora selecionadas (ordem)
  currentDetailEv: null, // evento aberto no drawer
  datasetDashboard: null,
  datasetCube: null,
  datasetProfiles: null,
  facetData: null, // contagens vivas de nível/fonte/código do recorte atual
  caseTreeProfiles: {}, // cache de perfis por Caso (árvore de exploração)
  caseProfilesLoading: false,
  treeAgg: { dataset: null, case: null }, // contagens vivas por coluna da árvore (recorte atual)
  derivedFields: [], // campos customizados (regex) definidos pelo usuário
  treeAggSig: { dataset: null, case: null }, // assinatura do último cálculo da árvore
  colWidths: {}, // larguras das colunas da tabela, por coluna (px)
  loadOverlay: false,
  driveCollapsed: new Set(), // pastas de artefatos recolhidas na lista de arquivos
  treeCollapsed: new Set(), // nós recolhidos da árvore de exploração
  caseProfiles: {},
  dashboardCompact: localStorage.getItem("investigation.dashboardCompact") === "1",
  analyticsScope: "dataset", // dataset = eventos carregados; case = itens do caso
  activeDatasetTab: "table",
};

let chart = null;
let debounceTimer = null;

// ------------------------------------------------------------------ util
const $ = (sel) => document.querySelector(sel);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

function setWorkbar(label, detail = "", progress = null, cancellable = false) {
  const bar = $("#workbar");
  const progressEl = $("#workbar-progress");
  const fill = $("#workbar-progress-fill");
  const active = progress !== null;
  bar.classList.toggle("active", active);
  $("#workbar-idle-icon").hidden = active;
  $("#workbar-spin").hidden = !active;
  $("#workbar-label").textContent = label;
  $("#workbar-detail").textContent = detail;
  progressEl.hidden = progress === null;
  if (progress !== null) fill.style.width = `${Math.max(0, Math.min(100, progress))}%`;
  $("#workbar-cancel").hidden = !cancellable;
  if (state.loadOverlay) mirrorLoadOverlay(label, detail, progress);
}

// ------------------------------------------------------------------ overlay de carga
let loadStepCount = 0;

function showLoadOverlay(firstStep = "Validando a fonte") {
  state.loadOverlay = true;
  loadStepCount = 0;
  $("#load-steps").innerHTML = "";
  $("#load-bar-fill").style.width = "4%";
  $("#load-phase").textContent = "Preparando…";
  $("#load-volume").textContent = "";
  $("#load-eta").textContent = "";
  progressSamples.length = 0;
  pushLoadStep(firstStep);
  $("#load-overlay").hidden = false;
}

function hideLoadOverlay(ok = true) {
  state.loadOverlay = false;
  if (ok) {
    document.querySelectorAll("#load-steps li").forEach((li) => {
      li.classList.add("done");
      li.querySelector(".load-step-ico").innerHTML = '<i class="fas fa-check"></i>';
    });
    $("#load-bar-fill").style.width = "100%";
    $("#load-phase").textContent = "Pronto!";
    setTimeout(() => { $("#load-overlay").hidden = true; }, 450);
  } else {
    $("#load-overlay").hidden = true;
  }
}

function pushLoadStep(label) {
  const steps = $("#load-steps");
  steps.querySelectorAll("li").forEach((li) => {
    li.classList.add("done");
    li.querySelector(".load-step-ico").innerHTML = '<i class="fas fa-check"></i>';
  });
  const li = el("li");
  li.innerHTML = `<span class="load-step-ico"><span class="load-step-spin"></span></span><span>${esc(label)}</span>`;
  steps.appendChild(li);
  loadStepCount++;
  // mantém só os últimos passos visíveis
  while (steps.children.length > 4) steps.firstChild.remove();
}

function mirrorLoadOverlay(label, detail, progress) {
  if (!state.loadOverlay) return;
  if (label && label !== $("#load-phase").textContent) pushLoadStep(label);
  $("#load-phase").textContent = label;
  $("#load-volume").textContent = detail || "";
  if (progress != null) $("#load-bar-fill").style.width = `${Math.max(4, Math.min(100, progress))}%`;
  // taxa e tempo estimado a partir das últimas amostras de progresso
  const etaEl = $("#load-eta");
  const s = progressSamples;
  if (s.length >= 2) {
    const a = s[0], b = s[s.length - 1];
    const dt = (b.at - a.at) / 1000;
    const rate = dt > 0.05 ? (b.completed - a.completed) / dt : 0;
    const remaining = b.total > 0 ? Math.max(0, b.total - b.completed) : 0;
    if (rate > 0 && remaining > 0) {
      const secs = Math.ceil(remaining / rate);
      etaEl.textContent = `${fmtNum(Math.round(rate))}/s · faltam ${fmtNum(remaining)} · ≈ ${secs}s`;
    } else if (b.total > 0 && remaining === 0) {
      etaEl.textContent = "concluindo…";
    }
  }
}

function startOperation(kind, label, detail = "") {
  state.activeOperation = { kind, cancelled: false };
  setWorkbar(label, detail, 8, true);
}

function updateOperation(label, detail = "", progress = 52) {
  if (!state.activeOperation) return;
  setWorkbar(label, detail, progress, true);
}

function finishOperation(label, detail = "") {
  state.activeOperation = null;
  setWorkbar(label, detail, null, false);
}

const progressSamples = []; // últimas amostras p/ taxa e ETA

window.__TAURI__.event?.listen("operation-progress", ({ payload }) => {
  if (!payload) return;
  const total = Number(payload.total || 0);
  const completed = Number(payload.completed || 0);
  progressSamples.push({ completed, total, at: Date.now() });
  if (progressSamples.length > 6) progressSamples.shift();
  const progress = total > 0 ? (completed / total) * 100 : 12;
  const volume = total > 0
    ? `${fmtNum(completed)} de ${fmtNum(total)} ${payload.unit || "itens"}`
    : (payload.unit || "");
  setWorkbar(payload.phase || "Processando", volume, progress, !!payload.cancellable);
}).catch(() => {});

// live-refresh quando uma tool MCP muta o estado do backend
window.__TAURI__.event?.listen("mcp-state-changed", ({ payload }) => {
  state.explorerCache = null;
  if (!payload?.kind) return;
  handleMcpStateChanged(payload.kind).catch(() => {});
}).catch(() => {});

function artifactIdFromSource(source) {
  if (source.kind === "bundle") return `bundle:${source.members.map(artifactIdFromSource).join("|")}`;
  if (source.kind === "file") return `file:${(source.paths?.length ? source.paths : [source.path]).join("+")}`;
  return `eventlog:${source.channel}`;
}

function sourceSpecFromControls() {
  if (currentSource() === "file") {
    const paths = $("#file-path").value.split(";").map((s) => s.trim()).filter(Boolean);
    return { kind: "file", path: paths[0] || "", paths, format: $("#file-format").value || "auto" };
  }
  return {
    kind: "eventlog",
    channel: $("#channel").value,
    maxEvents: parseInt($("#max-events").value, 10) || 5000,
  };
}

function sourceSpecFromArtifact(artifact) {
  if (artifact?.source?.kind) return { ...artifact.source };
  if (artifact?.kind === "file") return { kind: "file", path: artifact.path || "", format: "auto" };
  return { kind: "eventlog", channel: artifact?.path || "Application", maxEvents: 5000 };
}

function applySourceSpec(source) {
  if (source.kind === "bundle") { applySourceSpec(source.members.at(-1)); return; }
  setSource(source.kind);
  if (source.kind === "file") {
    const paths = source.paths?.length ? source.paths : [source.path].filter(Boolean);
    $("#file-path").value = paths.join("; ");
    if (source.format) $("#file-format").value = source.format;
  } else {
    const channel = $("#channel");
    if (![...channel.options].some((option) => option.value === source.channel)) {
      channel.appendChild(el("option", "", source.channel || "Application")).value = source.channel || "Application";
    }
    channel.value = source.channel || "Application";
    $("#max-events").value = String(source.maxEvents || 5000);
  }
}

function artifactSessionFor(caseId = state.cases.active) {
  if (!caseId) return null;
  let session = state.artifactSessions.get(caseId);
  if (session) return session;
  const c = state.cases.cases.find((item) => item.id === caseId);
  session = { activeId: c?.activeArtifactId || null, artifacts: new Map() };
  for (const artifact of c?.artifacts || []) {
    if (!artifact?.id || !artifact.path) continue;
    session.artifacts.set(artifact.id, { ...artifact, source: sourceSpecFromArtifact(artifact) });
  }
  state.artifactSessions.set(caseId, session);
  return session;
}

const baseName = (p) => String(p || "").split(/[\\/]/).pop() || String(p || "");

// persiste as colunas visíveis no registro do artefato (por Caso)
function saveVisibleCols() {
  if (workspaceScope() === "case") { saveCases(); return; }
  const artifact = currentCaseArtifact();
  if (!artifact) return;
  artifact.visibleCols = [...state.visibleCols];
  artifact.colWidths = { ...state.colWidths };
  saveCases();
}

async function removeArtifact(artifactId) {
  const c = activeCase();
  if (!c) return;
  const session = artifactSessionFor(c.id);
  session?.artifacts.delete(artifactId);
  c.artifacts = (c.artifacts || []).filter((a) => a.id !== artifactId);
  saveCases();
  renderArtifactBar();
  toast("Artefato removido do Caso.", "ok");
  // se era o artefato aberto, limpa a visão
  if (state.currentArtifact?.id === artifactId) {
    await clearData();
    switchView("source");
  }
}

function renderArtifactBar() {
  // drive view: arquivos do Caso em tabela detalhada (estilo explorer)
  const tbody = $("#drive-table tbody");
  if (!tbody) return;
  tbody.innerHTML = "";
  const session = artifactSessionFor();
  const artifacts = session ? [...session.artifacts.values()].sort((a, b) => (b.loadedAt || 0) - (a.loadedAt || 0)) : [];
  $("#drive-empty").hidden = artifacts.length > 0;
  $("#drive-count").textContent = artifacts.length
    ? `${artifacts.length} ${artifacts.length === 1 ? "artefato" : "artefatos"}`
    : "";
  const fmtWhen = (ts) => {
    if (!ts) return "—";
    const d = new Date(ts), p = (n) => String(n).padStart(2, "0");
    return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
  };
  const openArtifact = async (id) => {
    await activateArtifact(id);
    if (state.loaded) switchView("viz");
  };
  for (const artifact of artifacts) {
    const files = artifact.source?.kind === "eventlog"
      ? [artifact.source?.channel || artifact.path]
      : (artifact.source?.paths?.length ? artifact.source.paths : [artifact.path].filter(Boolean));
    const isFolder = files.length > 1;
    const tr = el("tr", isFolder ? "drive-folder" : "drive-file");
    if (artifact.id === session.activeId) tr.classList.add("active");
    const caretTd = el("td", "drive-caret");
    if (isFolder) {
      const open = !state.driveCollapsed.has(artifact.id);
      const toggleFolder = () => {
        if (!state.driveCollapsed.delete(artifact.id)) state.driveCollapsed.add(artifact.id);
        renderArtifactBar();
      };
      const caret = el("button", "icon-btn drive-caret-btn");
      caret.innerHTML = `<i class="fas fa-angle-${open ? "down" : "right"}"></i>`;
      caret.title = open ? "Recolher" : "Expandir";
      // stopPropagation: sem ele o clique cai no toggle da linha e anula o recolher
      caret.onclick = (e) => { e.stopPropagation(); toggleFolder(); };
      caretTd.appendChild(caret);
      // clique em qualquer ponto da linha da pasta expande/recolhe (estilo explorer)
      tr.onclick = (e) => { if (!e.target.closest(".drive-actions")) toggleFolder(); };
      tr.title = "Clique para expandir/recolher · duplo clique para detalhar";
    }
    const nameTd = el("td", "drive-name");
    nameTd.innerHTML = `<i class="fas ${isFolder ? "fa-folder" : (artifact.source?.kind === "eventlog" ? "fa-server" : "fa-file-lines")}"></i><span>${esc(isFolder ? (artifact.label || "Arquivos unidos") : baseName(files[0]))}</span>`;
    nameTd.title = files.join("\n");
    tr.append(
      caretTd, nameTd,
      el("td", "drive-tipo", artifact.source?.kind === "eventlog" ? "Event Log" : (isFolder ? "Pasta (unidos)" : "Arquivo")),
      el("td", "drive-num", isFolder ? String(files.length) : "1"),
      el("td", "drive-num", artifact.count ? fmtNum(artifact.count) : "—"),
      el("td", "drive-when", fmtWhen(artifact.loadedAt)),
    );
    const actTd = el("td", "drive-actions");
    const open = el("button", "btn ghost small", "Detalhar");
    open.onclick = () => openArtifact(artifact.id);
    const remove = el("button", "icon-btn drive-remove");
    remove.innerHTML = '<i class="fas fa-trash-can"></i>';
    remove.title = "Remover artefato do Caso";
    remove.onclick = () => removeArtifact(artifact.id);
    actTd.append(open, remove);
    tr.appendChild(actTd);
    tr.ondblclick = () => openArtifact(artifact.id);
    tbody.appendChild(tr);
    // arquivos da pasta (expansível)
    if (isFolder && !state.driveCollapsed.has(artifact.id)) {
      for (const f of files) {
        const child = el("tr", "drive-child");
        child.appendChild(el("td", ""));
        const cn = el("td", "drive-name child");
        cn.innerHTML = `<i class="fas fa-file-lines"></i><span>${esc(baseName(f))}</span>`;
        cn.title = f;
        child.append(cn, el("td", "drive-tipo", "Arquivo"), el("td", ""), el("td", ""), el("td", ""), el("td", ""));
        tbody.appendChild(child);
      }
    }
  }
}

function storeCurrentArtifactInSession() {
  const c = activeCase();
  if (!c || !state.currentArtifact) return;
  const session = artifactSessionFor(c.id);
  session.artifacts.set(state.currentArtifact.id, {
    ...state.currentArtifact,
    source: { ...state.currentArtifact.source },
  });
  session.activeId = state.currentArtifact.id;
  registerCurrentArtifact(c);
  saveCases();
  renderArtifactBar();
}

async function activateArtifact(artifactId) {
  const caseId = state.cases.active;
  const session = artifactSessionFor(caseId);
  const artifact = session?.artifacts.get(artifactId);
  if (!caseId || !artifact) return;
  session.activeId = artifact.id;
  state.currentArtifact = null;
  state.currentOrigin = "";
  renderArtifactBar();
  updateContextBar();
  const version = ++state.artifactSwitchVersion;
  await loadData(sourceSpecFromArtifact(artifact), { caseId, version, restoreCurrentFilters: true });
}

async function syncActiveCaseArtifacts() {
  const session = artifactSessionFor();
  renderArtifactBar();
  if (session?.activeId && session.artifacts.has(session.activeId)) {
    await activateArtifact(session.activeId);
    return;
  }
  if (state.currentArtifact || state.loaded) await clearData();
  else updateContextBar();
}

function updateContextBar() {
  if (state.queryError) { $("#context-summary").textContent = "Consulta não concluída"; return; }
  // contexto de Caso: os números são sempre do conjunto do Caso, nunca do artefato
  if (state.activeContext !== "artifact" && activeCase()) {
    const events = caseEvents(), summary = caseEventsCache.summary;
    const bits = [activeCase().name, `${fmtNum(events.length)} eventos no Caso`];
    if (summary.start != null) bits.push(`${fmtTs(summary.start)} — ${fmtTs(summary.end)}`);
    if (state.filters.length || state.quick.trim()) bits.push("recorte filtrado");
    if (state.stationAnalyticsId) {
      const station = caseStations().find((item) => item.id === state.stationAnalyticsId);
      if (station) bits.unshift(`Estação: ${station.name}`);
    }
    $("#context-summary").textContent = bits.join(" · ");
    return;
  }
  const artifact = state.currentArtifact;
  if (!artifact) {
    $("#context-summary").textContent = activeCase()
      ? "Adicione ou selecione um artefato deste Caso."
      : "Crie ou selecione um Caso para começar.";
    return;
  }
  const bits = [fmtNum(state.total ?? artifact.count ?? 0) + " eventos"];
  if (state.dataPeriod?.min != null && state.dataPeriod?.max != null) {
    bits.push(`${fmtTs(state.dataPeriod.min)} — ${fmtTs(state.dataPeriod.max)}`);
  }
  if (state.stationAnalyticsId) {
    const station = caseStations().find((item) => item.id === state.stationAnalyticsId);
    if (station) bits.unshift(`Estação: ${station.name}`);
  }
  $("#context-summary").textContent = bits.join(" · ");
}
const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c])
  );
const escRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function toast(msg, type = "info") {
  const t = el("div", `toast ${type}`, msg);
  $("#toast-area").appendChild(t);
  setTimeout(() => t.remove(), 4200);
}

// ------------------------------------------------------------------ indicador global
// Qualquer invoke não-silent que demore mais que ~320ms acende um spinner discreto
// na topbar; chamadas silent (tree_aggs, profile_fields de fundo, cases_save…) são
// tratadas como bastidor e nunca acendem o indicador.
const activity = { count: 0, timer: null };
const ACTIVITY_DELAY = 320;

function activityShow() {
  const ind = $("#activity-indicator");
  if (!ind) return;
  ind.hidden = false;
  requestAnimationFrame(() => ind.classList.add("show"));
}

function activityHide() {
  const ind = $("#activity-indicator");
  if (!ind) return;
  ind.classList.remove("show");
  setTimeout(() => { if (!activity.count) ind.hidden = true; }, 180);
}

async function api(cmd, args = {}, opts = {}) {
  const track = !opts.silent;
  if (track) {
    activity.count++;
    if (activity.count === 1) activity.timer = setTimeout(activityShow, ACTIVITY_DELAY);
  }
  try {
    if (args.filters?.length) await invoke("validate_filters", { filters: args.filters });
    if (/^(load_|clear_|set_|save_|delete_|harvest_)/.test(cmd)) state.explorerCache = null;
    return await invoke(cmd, args);
  } catch (e) {
    if (!opts.silent) toast(String(e), "err");
    throw e;
  } finally {
    if (track) {
      activity.count--;
      if (activity.count === 0) {
        clearTimeout(activity.timer);
        activity.timer = null;
        activityHide();
      }
    }
  }
}

// ------------------------------------------------------------------ helpers de espera
// botão em estado "trabalhando": desabilita e mostra spinner; o retorno restaura
function btnBusy(btn, text) {
  if (!btn) return () => {};
  const prev = { html: btn.innerHTML, disabled: btn.disabled };
  btn.disabled = true;
  btn.innerHTML = `<i class="fas fa-circle-notch spin"></i> ${esc(text)}`;
  return () => { btn.disabled = prev.disabled; btn.innerHTML = prev.html; };
}

// overlay de espera sobre uma área; aparece só se a operação passar de ~250ms
function areaLoading(container, text = "Consultando…") {
  if (!container) return { done() {} };
  let ov = null;
  const timer = setTimeout(() => {
    ov = el("div", "area-loading");
    ov.innerHTML = `<i class="fas fa-circle-notch spin"></i><span>${esc(text)}</span>`;
    container.appendChild(ov);
    requestAnimationFrame(() => ov?.classList.add("show"));
  }, 250);
  return {
    done() {
      clearTimeout(timer);
      if (!ov) return;
      ov.classList.remove("show");
      const elRef = ov;
      ov = null;
      setTimeout(() => elRef.remove(), 160);
    },
  };
}

// spinner discreto no título "Explorar" enquanto as contagens da árvore são recalculadas
function treeSpin(on) {
  document.querySelectorAll(".explore-block .side-title").forEach((title) => {
    title.querySelector(".tree-spin")?.remove();
    if (on) {
      const s = el("span", "tree-spin");
      s.innerHTML = '<i class="fas fa-circle-notch spin"></i>';
      title.appendChild(s);
    }
  });
}

function fmtTs(ms) {
  if (ms == null) return "";
  const d = new Date(ms);
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}
function fmtTsFull(ms) {
  if (ms == null) return "";
  const d = new Date(ms);
  const p = (n, width = 2) => String(n).padStart(width, "0");
  return `${p(d.getDate())}/${p(d.getMonth() + 1)}/${d.getFullYear()} · ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

function fmtNum(n) {
  return Number(n).toLocaleString("pt-BR");
}

function countLabel(value, singular, plural = `${singular}s`) {
  return `${fmtNum(value)} ${Number(value) === 1 ? singular : plural}`;
}

function colLabel(col) {
  return COL_LABELS[col] || col;
}

function eventComment(ev) {
  const c = activeCase();
  const artifactId = state.currentArtifact?.id;
  if (!c || !artifactId) return "";
  return c.comments?.[artifactId]?.[ev.id] || "";
}

function setEventComment(ev, text) {
  const c = ensureCase();
  if (!c) return;
  const artifactId = state.currentArtifact?.id;
  if (!artifactId) return;
  c.comments = c.comments || {};
  c.comments[artifactId] = c.comments[artifactId] || {};
  const trimmed = String(text || "").trim();
  if (trimmed) c.comments[artifactId][ev.id] = trimmed;
  else delete c.comments[artifactId][ev.id];
  saveCases();
  // coluna disponível e visível assim que existe o primeiro comentário
  if (trimmed && !state.columns.includes("comentario")) {
    state.columns.push("comentario");
    fillColumnControls();
  }
  if (trimmed && !state.visibleCols.includes("comentario")) {
    state.visibleCols.push("comentario");
    saveVisibleCols();
  }
  renderTable({ total: state.total, rows: state.rows });
  renderExploreTree();
}

let commentEv = null;
function openCommentModal(ev) {
  commentEv = ev;
  const existing = eventComment(ev);
  $("#cm-text").value = existing;
  $("#cm-remove").hidden = !existing;
  $("#comment-modal").hidden = false;
  $("#cm-text").focus();
}

function cellValue(ev, col) {
  switch (col) {
    case "comentario": return eventComment(ev);
    case "timestamp": return fmtTs(ev.timestamp);
    case "source": return ev.source;
    case "level": return ev.level;
    case "code": return ev.code;
    case "name": return ev.name;
    case "description": return ev.description;
    case "message": return ev.message;
    default: {
      const v = ev.fields ? ev.fields[col] : undefined;
      if (v === undefined || v === null) return "";
      return typeof v === "object" ? JSON.stringify(v) : String(v);
    }
  }
}

function autoVisibleCols() {
  const hasValue = (col) => state.rows.some((ev) => String(cellValue(ev, col) ?? "").trim() !== "");
  const preferred = ["timestamp", "level", "source", "message"];
  const visible = preferred.filter((col) => state.columns.includes(col) && hasValue(col));
  // data/hora é sempre a primeira coluna, mesmo ainda não configurada (células vazias)
  if (!visible.includes("timestamp") && state.columns.includes("timestamp")) visible.unshift("timestamp");
  const extra = state.columns
    .filter((col) => !preferred.includes(col) && hasValue(col))
    .slice(0, 2);
  state.visibleCols = visible;
  if (!state.visibleCols.length && state.columns.includes("message")) state.visibleCols = ["message"];
  if (!state.visibleCols.length) state.visibleCols = state.columns.slice(0, 1);
  fillColumnControls();
  renderTable({ total: state.total, rows: state.rows });
}

// ------------------------------------------------------------------ tema
function initTheme() {
  const saved = localStorage.getItem("li-theme") || "dark";
  document.documentElement.dataset.theme = saved;
  updateThemeIcon();
}
function toggleTheme() {
  const cur = document.documentElement.dataset.theme === "light" ? "dark" : "light";
  document.documentElement.dataset.theme = cur;
  localStorage.setItem("li-theme", cur);
  updateThemeIcon();
  if (state.loaded) refresh(); // redesenha o gráfico com as cores do tema
}
function updateThemeIcon() {
  const light = document.documentElement.dataset.theme === "light";
  $("#btn-theme").innerHTML = `<i class="fas fa-${light ? "moon" : "sun"}"></i>`;
}
const isLight = () => document.documentElement.dataset.theme === "light";

// ------------------------------------------------------------------ fonte
function setSource(which) {
  const isFile = which === "file";
  $("#src-btn-file").classList.toggle("active", isFile);
  $("#src-btn-eventlog").classList.toggle("active", !isFile);
  $("#src-file").hidden = !isFile;
  $("#src-eventlog").hidden = isFile;
}
const currentSource = () => ($("#src-file").hidden ? "eventlog" : "file");

async function browseFile() {
  const selected = await dialogApi.open({ multiple: true, directory: false });
  if (!selected) return;
  const paths = (Array.isArray(selected) ? selected : [selected]).filter(Boolean);
  if (paths.length) $("#file-path").value = paths.join("; ");
}

async function refreshChannels() {
  const sel = $("#channel");
  sel.innerHTML = "";
  try {
    const channels = await api("list_channels");
    const preferred = ["Application", "System", "Security", "Setup"];
    channels.sort((a, b) => {
      const ia = preferred.indexOf(a), ib = preferred.indexOf(b);
      return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib) || a.localeCompare(b);
    });
    for (const c of channels) sel.appendChild(el("option", "", c));
    sel.value = "Application";
  } catch { /* toast já exibido */ }
}

function skeletonRows() {
  const tbody = $("#events-table tbody");
  tbody.innerHTML = "";
  for (let i = 0; i < 14; i++) {
    const tr = el("tr", "sk-row");
    for (let j = 0; j < state.visibleCols.length; j++) {
      const td = el("td");
      td.appendChild(el("span", "sk"));
      tr.appendChild(td);
    }
    tbody.appendChild(tr);
  }
}

async function loadData(requestedSource = null, options = {}) {
  await window.WorkspaceContext?.waitForSource();
  if (window.WorkspaceContext?.scope() === "case") await window.WorkspaceContext.setScope("dataset", { animate: false });
  const btn = $("#btn-load");
  btn.disabled = true;
  btn.innerHTML = '<i class="fas fa-circle-notch spin"></i> Carregando…';
  const status = $("#load-status");
  status.className = "load-status";
  startOperation("load", "Preparando artefato", "Validando fonte");
  showLoadOverlay();
  skeletonRows();
  try {
    let source = requestedSource ? { ...requestedSource } : sourceSpecFromControls();
    if (source.kind === "file") {
      source.paths = source.paths?.filter(Boolean)?.length
        ? source.paths
        : (source.path ? [source.path] : []);
      if (!source.paths.length) {
        await browseFile();
        source.paths = $("#file-path").value.split(";").map((s) => s.trim()).filter(Boolean);
        source.path = source.paths[0] || "";
        if (!source.paths.length) return;
      }
    }
    if (source.kind === "eventlog" && !source.channel) source.channel = "Application";
    const c = options.caseId
      ? state.cases.cases.find((item) => item.id === options.caseId)
      : ensureCase();
    if (!c || state.cases.active !== c.id) return;
    const version = options.version ?? ++state.artifactSwitchVersion;
    let merge = !!options.merge;
    if (merge && state.currentArtifact?.source) {
      const currentMembers = state.currentArtifact.source.kind === "bundle"
        ? state.currentArtifact.source.members
        : [state.currentArtifact.source];
      const newMembers = source.kind === "bundle" ? source.members : [source];
      const rawMembers = [...currentMembers, ...newMembers];
      const uniqueMembers = [];
      const seen = new Set();
      for (const m of rawMembers) {
        const key = m.kind === "eventlog"
          ? `eventlog:${m.channel}`
          : `file:${(m.paths?.length ? m.paths : [m.path]).map(p => String(p).toLowerCase()).sort().join(";")}`;
        if (!seen.has(key)) {
          seen.add(key);
          uniqueMembers.push(m);
        }
      }
      source = { kind: "bundle", members: uniqueMembers, path: uniqueMembers[0]?.path || uniqueMembers[0]?.channel || "" };
      merge = false;
    }
    applySourceSpec(source);
    let summary;
    if (source.kind === "bundle") {
      summary = await api("load_bundle", { members: source.members.map(s => s.kind === "file" ? { ...s, paths: s.paths?.length ? s.paths : [s.path], format: s.format || "auto" } : { ...s, maxEvents: s.maxEvents || 5000 }) }, { silent: true });
    } else if (source.kind === "file") {
      summary = source.paths.length > 1
        ? await api("load_files", { paths: source.paths, format: source.format || "auto", merge }, { silent: true })
        : await api("load_file", { path: source.path, format: source.format || "auto", merge }, { silent: true });
    } else {
      summary = await api("load_event_log", {
        channel: source.channel,
        maxEvents: source.maxEvents || 5000,
        merge,
      }, { silent: true });
    }
    if (version !== state.artifactSwitchVersion || state.cases.active !== c.id) return;
    updateOperation("Artefato carregado", `${fmtNum(summary.count)} eventos indexados`, 82);
    state.columns = summary.columns;
    const savedArtifact = c.artifacts?.find((a) => a.id === artifactIdFromSource(source));
    state.visibleCols = savedArtifact?.visibleCols?.filter((col) => summary.columns.includes(col));
    if (!state.visibleCols?.length) state.visibleCols = ["timestamp", "level", "source", "message"];
    state.colWidths = { ...(savedArtifact?.colWidths || {}) };
    if (options.restoreCurrentFilters) restoreCurrentSavedFilter();
    else {
      state.filters = [];
      state.quick = "";
      $("#quick-search").value = "";
    }
    // Neste ponto o estado ja representa o recorte certo: um novo artefato
    // parte limpo; uma reabertura recupera o ultimo recorte persistido.
    syncCurrentSavedFilter();
    state.page = 0;
    state.loaded = true;
    fillColumnControls();
    renderChips();
    status.textContent = merge ? `${fmtNum(summary.count)} eventos (fontes unidas)` : `${fmtNum(summary.count)} eventos`;
    status.classList.add("ok");
    $("#btn-merge").disabled = false;
    // origem: estação associada ao arquivo (se houver) ou a própria fonte
    state.currentOrigin = summary.source_desc;
    state.currentArtifact = {
      id: artifactIdFromSource(source),
      label: summary.source_desc,
      kind: source.kind,
      path: source.kind === "eventlog" ? source.channel : source.path,
      count: summary.count,
      loadedAt: Date.now(),
      source,
    };
    storeCurrentArtifactInSession();
    if (source.kind === "file") {
      const path = source.path;
      const linked = currentCaseArtifact();
      const st = linked ? caseStations().find((s) => s.id === linked.stationId) : null;
      if (st) state.currentOrigin = st.name;
      await loadTsConfig(path);
    }
    state.datasetDashboard = null;
    state.datasetCube = null;
    state.datasetProfiles = null;
    state.caseProfiles = {};
    // coluna Comentário disponível quando o artefato tem comentários salvos
    const hasComments = Object.keys(c.comments?.[state.currentArtifact?.id] || {}).length > 0;
    if (hasComments && !state.columns.includes("comentario")) state.columns.push("comentario");
    if (hasComments && !state.visibleCols.includes("comentario")) state.visibleCols.push("comentario");
    await refresh();
    // perfis dos campos alimentam os nós de valores/faixas da árvore de exploração
    api("profile_fields", { filters: [] }, { silent: true })
      .then((profiles) => { state.datasetProfiles = profiles; renderExploreTree(); })
      .catch(() => {});
    if (!savedArtifact?.visibleCols?.length) autoVisibleCols();
    updateTsExample();
    updateContextBar();
    finishOperation("Artefato pronto", `${fmtNum(state.total)} eventos disponíveis`);
    hideLoadOverlay(true);
    await window.Workspace?.loaded();
    if (document.body.dataset.page === "summary") await window.Workspace?.showPage("summary");
    return true;
  } catch (e) {
    if (String(e).includes("ELEVATION_REQUIRED")) {
      status.textContent = "Este canal exige permissão de administrador.";
      status.classList.add("err");
      const btn = el("button", "btn primary small elevate-btn");
      btn.innerHTML = '<i class="fas fa-shield-halved"></i> Executar como administrador';
      btn.onclick = async () => {
        btn.disabled = true;
        try { await api("relaunch_elevated"); } catch { btn.disabled = false; }
      };
      status.appendChild(btn);
    } else {
      status.textContent = "Falha ao carregar.";
      status.classList.add("err");
      toast(String(e), "err");
    }
    if (state.loaded) await refresh();
    else renderTable({ total: 0, rows: [] });
    finishOperation("Falha ao carregar artefato", "Tente revisar a fonte ou o formato.");
    return false;
  } finally {
    if (state.loadOverlay) hideLoadOverlay(false); // cobre cancelamento/erro
    btn.disabled = false;
    btn.innerHTML = '<i class="fas fa-play"></i> Carregar';
  }
}

async function clearData({ removeCurrent = false } = {}) {
  const artifact = state.currentArtifact;
  await api("clear_events");
  if (removeCurrent && artifact) {
    const session = artifactSessionFor();
    if (session) {
      if (session.activeId === artifact.id) session.activeId = null;
    }
  }
  Object.assign(state, {
    loaded: false, filters: [], quick: "", page: 0, total: 0, rows: [],
    datasetDashboard: null, datasetCube: null, datasetProfiles: null, caseProfiles: {},
    currentArtifact: null, dataPeriod: null,
  });
  $("#quick-search").value = "";
  $("#load-status").textContent = "";
  $("#btn-merge").disabled = true;
  renderChips();
  renderTable({ total: 0, rows: [] });
  renderChart({ buckets: [], levels: [] });
  state.facetData = null;
  state.treeAgg.dataset = null;
  updateAnalysisBadge();
  $("#result-count").textContent = "";
  closeDrawer();
  renderExploreTree();
  renderArtifactBar();
  updateContextBar();
  if (state.activeDatasetTab === "cube" && state.analyticsScope === "dataset") runCube();
  finishOperation("Nenhuma fonte carregada");
}

// ------------------------------------------------------------------ filtros / chips
function allFilters() {
  const fs = [...state.filters];
  if (state.quick.trim()) {
    fs.unshift({ column: "message", op: "contains", value: state.quick.trim(), value2: null, _quick: true });
  }
  return fs;
}
const backendFilters = () =>
  allFilters().map((f) => ({ column: f.column, op: f.op, value: f.value, value2: f.value2 || null }));

function addFilter(f) {
  state.filters.push(f);
  state.page = 0;
  filtersChanged();
}

function removeFilter(i) {
  state.filters.splice(i, 1);
  state.page = 0;
  filtersChanged();
}

// formata valor de filtro numérico com a unidade do perfil do campo (bytes, ms, %…)
function fmtFilterSideValue(column, raw) {
  if (column === "timestamp") return fmtTs(Number(raw) || raw);
  const p = profileFor(column);
  const n = Number(raw);
  if (p && RANGE_KINDS.has(p.kind) && raw !== "" && !Number.isNaN(n)) return fmtKindValue(p.kind, n);
  return raw;
}

function chipLabel(f) {
  const col = colLabel(f.column);
  const sym = OP_SYMBOL[f.op] || f.op;
  if (f.op === "empty" || f.op === "not_empty") return `${col} ${sym}`;
  if (f.op === "between") {
    return `${col}: ${fmtFilterSideValue(f.column, f.value)} → ${fmtFilterSideValue(f.column, f.value2)}`;
  }
  if (["gt", "gte", "lt", "lte"].includes(f.op)) return `${col} ${sym} ${fmtFilterSideValue(f.column, f.value)}`;
  return `${col} ${sym} ${f.value}`;
}

function invertFilter(index) {
  const f = state.filters[index];
  if (!f) return;
  const INVERT_OPS = {
    contains: "not_contains",
    not_contains: "contains",
    equals: "not_equals",
    not_equals: "equals",
    equals_exact: "not_equals_exact",
    not_equals_exact: "equals_exact",
    gt: "lte",
    lte: "gt",
    gte: "lt",
    lt: "gte",
    empty: "not_empty",
    not_empty: "empty",
  };
  if (INVERT_OPS[f.op]) {
    f.op = INVERT_OPS[f.op];
  } else if (f.op === "starts_with") {
    f.op = "regex";
    f.value = `^(?!${escRe(f.value)})`;
  } else if (f.op === "regex") {
    if (f.value.startsWith("^(?!") && f.value.endsWith(")")) {
      f.value = f.value.slice(4, -1);
    } else {
      f.value = `^(?!.*(?:${f.value}))`;
    }
  } else if (f.op === "pattern") {
    f.op = "not_contains";
  } else {
    toast(`Não é possível inverter o operador "${f.op}".`, "info");
    return;
  }
  state.page = 0;
  filtersChanged();
  toast(`Filtro invertido: ${chipLabel(f)}`, "ok");
}

function renderChips() {
  const boxes = document.querySelectorAll(".chips-sync");
  boxes.forEach((box) => (box.innerHTML = ""));
  state.filters.forEach((f, i) => {
    boxes.forEach((box) => {
      const chip = el("span", "chip");
      chip.title = `${chipLabel(f)} (Botão direito: inverter ou editar)`;
      chip.appendChild(el("span", "", chipLabel(f)));
      const x = el("button", "x");
      x.innerHTML = '<i class="fas fa-xmark"></i>';
      x.title = "Remover filtro";
      x.onclick = (e) => { e.stopPropagation(); removeFilter(i); };
      chip.appendChild(x);

      chip.oncontextmenu = (e) => {
        e.preventDefault();
        e.stopPropagation();
        showCtxMenu(e.clientX, e.clientY, [
          {
            icon: "fa-arrows-rotate",
            label: "Inverter filtro",
            onClick: () => invertFilter(i),
          },
          {
            icon: "fa-pen-to-square",
            label: "Editar filtro",
            onClick: () => openFilterPop(chip, i),
          },
          { sep: true },
          {
            icon: "fa-trash-can",
            label: "Remover filtro",
            danger: true,
            onClick: () => removeFilter(i),
          },
        ]);
      };

      box.appendChild(chip);
    });
  });
  if (state.filters.length > 1) {
    boxes.forEach((box) => {
      const clear = el("button", "chip chip-clear", "limpar tudo");
      clear.onclick = () => { state.filters = []; state.page = 0; filtersChanged(); };
      box.appendChild(clear);
    });
  }
  const hasActive = state.filters.length > 0 || !!state.quick.trim();
  $("#btn-clear-filters").hidden = !hasActive;
  // moldura global: tudo na janela passa a refletir apenas a realidade filtrada
  document.body.classList.toggle("filters-active", hasActive);
  renderFilterTabs();
}

// ponto único de reação a mudanças de filtro: chips, árvore e a tela corrente
function filtersChanged() {
  // O recorte atual e sempre uma visualizacao persistida do Caso. Assim o
  // usuario pode continuar de onde parou sem precisar clicar em "salvar".
  syncCurrentSavedFilter();
  renderChips();
  renderExploreTree();
  if (state.activeContext === "artifact" || !document.querySelector(".shell").hidden) {
    state.page = 0;
    refresh();
    return;
  }
  // contexto de Caso: árvore, linhas do tempo, painéis e cubo recalculam sobre o recorte
  updateContextBar();
  refreshTreeAggs("case");
  if (!$("#view-analysis").hidden) renderAnalysis();
  if (!$("#view-dashboard").hidden) renderDashboard("case");
  else if (!$("#view-cube").hidden) runCube();
  else window.Workspace?.onFiltersChanged?.();
}

// ------------------------------------------------------------------ árvore de exploração
const FIELD_KIND_ICONS = { time: "fa-clock", number: "fa-hashtag", bytes: "fa-database", bits: "fa-tower-broadcast", duration: "fa-stopwatch", category: "fa-tag", bool: "fa-toggle-on", ip: "fa-network-wired", percent: "fa-percent" };
const RANGE_KINDS = new Set(["number", "bytes", "bits", "duration", "percent"]);
const FACET_VALUE_LIMIT = 8; // campos com até N valores distintos viram nó de valores

function profileFor(column) {
  return (scopeProfiles(workspaceScope()) || []).find((p) => p.name === column) || null;
}

function fmtDurationMs(ms) {
  const r1 = (v) => Math.round(v * 10) / 10;
  if (ms >= 3_600_000) return `${r1(ms / 3_600_000)} h`;
  if (ms >= 60_000) return `${r1(ms / 60_000)} min`;
  if (ms >= 1_000) return `${r1(ms / 1_000)} s`;
  return `${Math.round(ms)} ms`;
}

function fmtKindValue(kind, n) {
  if (kind === "bytes") return fmtBytes(n);
  if (kind === "bits") return `${fmtBytes(n)}b`;
  if (kind === "duration") return fmtDurationMs(n);
  if (kind === "percent") return `${Math.round(n * 100) / 100}%`;
  return fmtNum(Math.round(n * 100) / 100);
}

function treeNode({ id, icon, label, meta, kids, active }) {
  const node = el("div", "xnode");
  if (state.treeCollapsed.has(id)) node.classList.add("collapsed");
  const head = el("button", "xnode-head");
  if (active) head.classList.add("has-filter");
  head.innerHTML = `<i class="fas fa-angle-down xnode-caret"></i><i class="fas ${icon} xnode-icon"></i>`;
  head.appendChild(el("span", "xnode-label", label));
  if (meta) head.appendChild(el("span", "xnode-meta", meta));
  head.title = label;
  head.onclick = () => {
    if (state.treeCollapsed.delete(id)) node.classList.remove("collapsed");
    else { state.treeCollapsed.add(id); node.classList.add("collapsed"); }
  };
  const box = el("div", "xnode-kids");
  kids.forEach((k) => box.appendChild(k));
  node.append(head, box);
  return node;
}

function facetValueItem(column, value, count, dotColor, mono) {
  const item = el("button", "facet-item");
  const empty = value == null, label = empty ? "(vazio)" : value === "(vazio)" ? "“(vazio)”" : String(value);
  const filter = (exclude = false) => ({ column, op: empty ? (exclude ? "not_empty" : "empty") : (exclude ? "not_equals_exact" : "equals_exact"), value: empty ? "" : String(value), value2: null });
  if (state.filters.some(f => f.column === column && (empty ? f.op === "empty" : ["equals", "equals_exact"].includes(f.op) && f.value === String(value)))) item.classList.add("active");
  if (dotColor) {
    const d = el("span", "facet-dot");
    d.style.background = dotColor;
    item.appendChild(d);
  }
  const v = el("span", empty ? "fv muted" : "fv", label);
  if (mono) v.classList.add("t-code");
  v.title = label;
  item.append(v, el("span", "fc", fmtNum(count)));
  item.onclick = () => toggleFacet(column, value);
  item.oncontextmenu = (e) => {
    e.preventDefault();
    showCtxMenu(e.clientX, e.clientY, [
      { icon: "fa-filter", label: `Filtrar: ${colLabel(column)} = ${trunc(label)}`, onClick: () => addFilter(filter()) },
      { icon: "fa-filter-circle-xmark", label: `Excluir: ${colLabel(column)} ≠ ${trunc(label)}`, onClick: () => addFilter(filter(true)) },
      { sep: true },
      { icon: "fa-microscope", label: "Enviar ao caso", onClick: () => addGroupToAnalysis(column, empty ? "" : String(value), "", empty ? "empty" : "equals_exact") },
      ...(!empty ? [{
        icon: "fa-microscope",
        label: `Enviar todos com ${colLabel(column)} preenchido ao caso`,
        onClick: () => addGroupToAnalysis(column, "", "", "not_empty"),
      }] : []),
    ]);
  };
  return item;
}

// degraus "redondos" (1, 2, 2.5, 5 × 10^n) para cortes de faixa
function niceStep(raw) {
  const mag = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * mag >= raw) return m * mag;
  return 10 * mag;
}

// ~6 opções: "< c1", "c1 – c2", ..., "≥ ck", derivadas do intervalo observado
function rangeOptions(profile) {
  const { min, max, kind } = profile;
  if (min == null || max == null || !(max > min)) return [];
  // trabalha na unidade de exibição dominante (MB, s, …) para cortes "redondos"
  const scale = displayScaleFor(kind, max);
  const minS = min / scale, maxS = max / scale;
  let step = niceStep((maxS - minS) / 5);
  let cuts = [];
  for (;;) {
    cuts = [];
    for (let i = Math.floor(minS / step) + 1; i * step < maxS; i++) cuts.push(Number((i * step).toPrecision(12)));
    if (cuts.length <= 7 || step >= maxS - minS) break;
    step *= 2;
  }
  if (!cuts.length) return [];
  const s = (n) => String(Number((n * scale).toPrecision(12)));
  const f = (n) => fmtKindValue(kind, n * scale);
  const opts = [{ label: `< ${f(cuts[0])}`, op: "lt", value: s(cuts[0]), value2: null }];
  for (let i = 0; i + 1 < cuts.length; i++)
    opts.push({ label: `${f(cuts[i])} – ${f(cuts[i + 1])}`, op: "between", value: s(cuts[i]), value2: s(cuts[i + 1]) });
  opts.push({ label: `≥ ${f(cuts[cuts.length - 1])}`, op: "gte", value: s(cuts[cuts.length - 1]), value2: null });
  return opts;
}

// unidade em que o valor é exibido (1024^n para bytes/bits; ms→s/min/h para duração)
function displayScaleFor(kind, max) {
  if (kind === "bytes" || kind === "bits") {
    let scale = 1;
    while (max / scale >= 1024 && scale < 1024 ** 4) scale *= 1024;
    return scale;
  }
  if (kind === "duration") {
    if (max >= 3_600_000) return 3_600_000;
    if (max >= 60_000) return 60_000;
    if (max >= 1_000) return 1_000;
  }
  return 1;
}

function sameRangeFilter(f, column, opt) {
  return f.column === column && f.op === opt.op
    && String(f.value) === String(opt.value) && String(f.value2 ?? "") === String(opt.value2 ?? "");
}

// opção "(vazio)": aparece quando o campo tem valores e vazios ao mesmo tempo
function emptyValueItem(column, count) {
  const item = el("button", "facet-item");
  if (state.filters.some((f) => f.column === column && f.op === "empty")) item.classList.add("active");
  item.append(el("span", "fv muted", "(vazio)"), el("span", "fc", fmtNum(count)));
  item.title = `${colLabel(column)}: (vazio)`;
  item.onclick = () => {
    const i = state.filters.findIndex((f) => f.column === column && f.op === "empty");
    if (i >= 0) { removeFilter(i); return; }
    state.filters = state.filters.filter((f) => !(f.column === column && f.op === "equals"));
    addFilter({ column, op: "empty", value: "", value2: null });
  };
  return item;
}

function rangeItem(column, opt, count = null) {
  const item = el("button", "facet-item");
  if (state.filters.some((f) => sameRangeFilter(f, column, opt))) item.classList.add("active");
  item.appendChild(el("span", "fv", opt.label));
  if (count != null) item.appendChild(el("span", "fc", fmtNum(count)));
  item.title = `${colLabel(column)}: ${opt.label}`;
  item.onclick = () => {
    const i = state.filters.findIndex((f) => sameRangeFilter(f, column, opt));
    if (i >= 0) { removeFilter(i); return; }
    // substitui a faixa anterior do mesmo campo — combinar faixas zeraria o recorte
    state.filters = state.filters.filter((f) => !(f.column === column && ["lt", "lte", "gt", "gte", "between"].includes(f.op)));
    addFilter({ column, op: opt.op, value: opt.value, value2: opt.value2 ?? null });
  };
  return item;
}

// perfis dos eventos do Caso (não filtrados), com invalidação por assinatura dos itens
function caseTreeProfiles() {
  const c = activeCase();
  if (!c) return null;
  const sig = caseSig();
  const cached = state.caseTreeProfiles[c.id];
  if (cached && cached.sig === sig) return cached.profiles;
  if (!state.caseProfilesLoading) {
    state.caseProfilesLoading = true;
    api("profile_fields", { filters: [], caseEvents: caseEvents() }, { silent: true })
      .then((profiles) => { state.caseTreeProfiles[c.id] = { sig, profiles }; })
      .catch(() => { state.caseTreeProfiles[c.id] = { sig, profiles: [] }; })
      .finally(() => { state.caseProfilesLoading = false; if (workspaceScope() === "case") refreshTreeAggs("case"); });
  }
  return null;
}

async function loadDerivedFields() {
  try { state.derivedFields = (await api("list_derived_fields", {}, { silent: true })) || []; }
  catch { state.derivedFields = []; }
  renderExploreTree();
}

function renderExploreTree() {
  document.querySelectorAll(".explore-tree-sync").forEach((box) => {
    if (box.closest("[hidden]")) return;
    renderExploreTreeInto(box, box.dataset.treeScope || "dataset");
  });
}

// ------------------------------------------------------------------ matcher local
// Espelha o matcher do backend (query.rs) para filtrar listas que já estão no
// cliente (ex.: eventos dos itens do Caso nas linhas do tempo).
function jsColNum(ev, col) {
  if (col === "timestamp") return ev.timestamp ?? null;
  if (col === "id") return ev.id;
  return jsParseNumUnit(cellValue(ev, col));
}
function jsValNum(col, s) {
  const t = String(s ?? "").trim();
  if (/^-?\d+(\.\d+)?$/.test(t)) return parseFloat(t);
  if (col === "timestamp") { const dt = Date.parse(t); return Number.isNaN(dt) ? null : dt; }
  return jsParseNumUnit(t);
}
function jsMatchFilter(ev, f) {
  if (f.op === "equals_exact" || f.op === "not_equals_exact") {
    if (f.column === "_all") return false;
    const standard = ["id", "source", "level", "code", "name", "description", "message", "raw"];
    const present = f.column === "timestamp" ? ev.timestamp != null : standard.includes(f.column) || Object.hasOwn(ev.fields || {}, f.column);
    const value = f.column === "timestamp" && present ? new Date(ev.timestamp).toISOString().replace(/\.000Z$/, "+00:00").replace(/Z$/, "+00:00")
      : standard.includes(f.column) ? String(ev[f.column] ?? "")
      : typeof ev.fields?.[f.column] === "string" ? ev.fields[f.column] : JSON.stringify(ev.fields?.[f.column]);
    const equal = present && value === String(f.value ?? "");
    return f.op === "equals_exact" ? equal : !equal;
  }
  const hay = f.column === "_all" ? `${ev.message || ""}\n${ev.raw || ""}` : cellValue(ev, f.column);
  const low = hay.toLowerCase();
  const needle = String(f.value ?? "").toLowerCase();
  if (f.op === "regex") {
    try { return new RegExp(f.value).test(hay); } catch { return false; }
  }
  switch (f.op) {
    case "pattern": return hay.split("\n")[0].replace(/\b[0-9a-f]{8}-[0-9a-f-]{27,}\b|\b(?:\d{1,3}\.){3}\d{1,3}\b|\b0x[0-9a-f]+\b|\b\d+(?:[.,]\d+)?\b/gi, "‹…›").slice(0,400) === f.value;
    case "contains": return low.includes(needle);
    case "not_contains": return !low.includes(needle);
    case "equals": return low === String(f.value ?? "").trim().toLowerCase();
    case "not_equals": return low !== String(f.value ?? "").trim().toLowerCase();
    case "starts_with": return low.startsWith(needle);
    case "empty": return !hay.trim();
    case "not_empty": return !!hay.trim();
    case "gt": case "gte": case "lt": case "lte": {
      const a = jsColNum(ev, f.column), b = jsValNum(f.column, f.value);
      if (a == null || b == null) return false;
      return f.op === "gt" ? a > b : f.op === "gte" ? a >= b : f.op === "lt" ? a < b : a <= b;
    }
    case "between": {
      const a = jsColNum(ev, f.column), lo = jsValNum(f.column, f.value), hi = jsValNum(f.column, f.value2 ?? "");
      return a != null && lo != null && hi != null && a >= lo && a <= hi;
    }
    default: return true;
  }
}
const rowPassesFilters = (ev) => allFilters().every((f) => jsMatchFilter(ev, f));

// campos que viram grupos de valores ou de faixas, a partir dos perfis
function treeGroupProfiles(profiles) {
  const covered = new Set(["timestamp", "level", "source", "code"]);
  const extra = (profiles || []).filter((p) => !covered.has(p.name));
  const categories = extra
    .filter((p) => !RANGE_KINDS.has(p.kind) && !["time", "id"].includes(p.kind)
      && p.cardinality > 0 && p.cardinality <= FACET_VALUE_LIMIT)
    .sort((a, b) => a.cardinality - b.cardinality);
  const ranges = extra
    .filter((p) => RANGE_KINDS.has(p.kind) && p.min != null && p.max != null && p.max > p.min)
    .sort((a, b) => colLabel(a.name).localeCompare(colLabel(b.name)));
  return { categories, ranges };
}

function caseTreeProfilesPeek() {
  const c = activeCase();
  const cached = c && state.caseTreeProfiles[c.id];
  return cached ? cached.profiles : null;
}

// interpreta "20 MB", "120 ms", "5.2" como número (espelha parse_num_unit do Rust)
function jsParseNumUnit(s) {
  const m = String(s).trim().toLowerCase().replace(",", ".")
    .match(/^(-?\d+(?:\.\d+)?)\s*(gbps|mbps|kbps|bps|tb|gb|mb|kb|b|min|ms|h|s)?$/);
  if (!m) return null;
  let n = parseFloat(m[1]);
  switch (m[2]) {
    case "kb": n *= 1024; break;
    case "mb": n *= 1024 ** 2; break;
    case "gb": n *= 1024 ** 3; break;
    case "tb": n *= 1024 ** 4; break;
    case "kbps": n *= 1e3; break;
    case "mbps": n *= 1e6; break;
    case "gbps": n *= 1e9; break;
    case "s": n *= 1000; break;
    case "min": n *= 60000; break;
    case "h": n *= 3600000; break;
  }
  return n;
}

// contagens por faixa, a partir dos valores vivos da coluna
function rangeCounts(opts, entries) {
  const counts = opts.map(() => 0);
  for (const [raw, n] of entries) {
    const v = jsParseNumUnit(raw);
    if (v == null) continue;
    for (let i = 0; i < opts.length; i++) {
      const o = opts[i];
      const hit = o.op === "lt" ? v < Number(o.value)
        : o.op === "gte" ? v >= Number(o.value)
        : v >= Number(o.value) && v <= Number(o.value2);
      if (hit) { counts[i] += n; break; }
    }
  }
  return counts;
}

const treeAggVersion = { dataset: 0, case: 0 };

// contagens vivas da árvore em UMA chamada consolidada (backend agrega cada coluna
// com os filtros das outras, em paralelo). Memoizada: mesma assinatura → sem recálculo.
async function refreshTreeAggs(scope, { force = false } = {}) {
  const context = scope === "case" ? caseSig() : `${state.currentArtifact?.id}:${state.currentArtifact?.loadedAt}`;
  if ((scope === "dataset" && !state.loaded) || (scope === "case" && !activeCase())) {
    state.treeAgg[scope] = null;
    state.treeAggSig[scope] = null;
    renderExploreTree();
    return;
  }
  if (!force && document.querySelector(".shell")?.classList.contains("side-collapsed")) {
    state.treeAggSig[scope] = null;
    return;
  }
  const profiles = scope === "case" ? caseTreeProfilesPeek() : (state.datasetProfiles || []);
  const { categories, ranges } = treeGroupProfiles(profiles);
  const cols = ["level", "source", "code", ...categories.map((p) => p.name), ...ranges.map((p) => p.name)];
  const filters = backendFilters();
  const sig = [scope, JSON.stringify(filters), cols.join(" "), scope === "case" ? caseSig() : (state.currentArtifact?.id || "")].join("|");
  if (!force && state.treeAggSig[scope] === sig && state.treeAgg[scope]) {
    renderExploreTree();
    return;
  }
  const version = ++treeAggVersion[scope];
  const spinTimer = setTimeout(() => treeSpin(true), 250);
  const spinDone = () => { clearTimeout(spinTimer); treeSpin(false); };
  let res;
  try {
    res = await api("tree_aggs", {
      columns: cols,
      filters,
      ...(scope === "case" ? { caseEvents: caseEvents() } : {}),
    }, { silent: true });
  } catch { spinDone(); return; }
  spinDone();
  if (version !== treeAggVersion[scope] || workspaceScope() !== scope || context !== (scope === "case" ? caseSig() : `${state.currentArtifact?.id}:${state.currentArtifact?.loadedAt}`)) return;
  const map = {};
  for (const [col, agg] of res || []) {
    map[col] = agg.rows.map((r, index) => {
      const key = Object.keys(r).find((k) => k !== "n");
      const raw = Array.isArray(agg.group_values) && agg.group_values.length === agg.rows.length ? agg.group_values[index] : (r[key] === "(vazio)" ? null : r[key]);
      return [raw == null ? null : String(raw), Number(r.n) || 0];
    });
  }
  state.treeAgg[scope] = map;
  state.treeAggSig[scope] = sig;
  renderExploreTree();
}

function renderExploreTreeInto(box, scope) {
  box.innerHTML = "";
  if (scope === "dataset" && !state.loaded) {
    box.innerHTML = '<p class="muted small">Carregue uma fonte para explorar.</p>';
    return;
  }
  if (scope === "case" && !activeCase()) {
    box.innerHTML = '<p class="muted small">Selecione um Caso para explorar.</p>';
    return;
  }
  const hasColFilter = (col) => state.filters.some((f) => f.column === col);

  let columns, profiles;
  if (scope === "case") {
    profiles = caseTreeProfiles();
    if (!profiles) {
      box.innerHTML = '<p class="muted small">Calculando perfis do Caso…</p>';
      return;
    }
    columns = profiles.map((p) => p.name);
  } else {
    profiles = state.datasetProfiles || [];
    columns = state.columns;
  }
  const live = state.treeAgg[scope] || {};
  const byName = Object.fromEntries((profiles || []).map((p) => [p.name, p]));

  // nó raiz: todos os campos organizados em árvore hierárquica por ponto (.)
  const favorites = state.favoriteFields || [];

  const createFieldRow = (column, labelOverride = null, isParent = false, toggleBtn = null) => {
    const profile = byName[column];
    const kind = profile?.kind || (column === "timestamp" ? "time" : "text");
    const row = el("div", `field-row${isParent ? " is-parent" : ""}`);
    row.dataset.field = `${column} ${colLabel(column)}`.toLocaleLowerCase();
    row.dataset.column = column;
    if (hasColFilter(column)) row.classList.add("has-filter");
    if (toggleBtn) row.appendChild(toggleBtn);
    const main = el("button", "field-item");
    const displayLabel = labelOverride || colLabel(column);
    main.innerHTML = `<i class="fas ${FIELD_KIND_ICONS[kind] || "fa-font"}"></i><span>${esc(displayLabel)}</span><small>${profile?.cardinality ? fmtNum(profile.cardinality) : ""}</small>`;
    main.title = `Inspecionar campo: ${colLabel(column)}${profile?.sampled_events ? ` · perfil de ${fmtNum(profile.sampled_events)} eventos amostrados` : ""}`;
    main.onclick = () => showFieldInspector(column);
    const adv = el("button", "icon-btn field-adv-btn");
    adv.innerHTML = '<i class="fas fa-filter"></i>';
    adv.title = `Filtro avançado: ${colLabel(column)}`;
    adv.onclick = (e) => {
      e.stopPropagation();
      openFilterPop(adv);
      $("#fp-col").value = column;
    };
    row.append(main, adv);
    const favorite = el("button", "icon-btn field-favorite");
    favorite.innerHTML = `<i class="${favorites.includes(column) ? "fas" : "far"} fa-star"></i>`;
    favorite.setAttribute("aria-label", `${favorites.includes(column) ? "Desafixar" : "Fixar"} ${colLabel(column)}`);
    favorite.title = favorite.getAttribute("aria-label");
    favorite.onclick = () => {
      state.favoriteFields = favorites.includes(column) ? favorites.filter(c => c !== column) : [...favorites, column];
      localStorage.setItem("workspace.fields", JSON.stringify(state.favoriteFields));
      renderExploreTree();
    };
    row.append(favorite);
    return row;
  };

  const buildTree = (cols) => {
    const root = { name: "", fullPath: "", isField: false, children: new Map() };
    for (const column of cols) {
      const parts = column.split(".");
      let cur = root;
      let pathAcc = "";
      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        pathAcc = pathAcc ? `${pathAcc}.${part}` : part;
        if (!cur.children.has(part)) {
          cur.children.set(part, {
            name: part,
            fullPath: pathAcc,
            isField: false,
            children: new Map(),
          });
        }
        cur = cur.children.get(part);
        if (i === parts.length - 1) cur.isField = true;
      }
    }
    return root;
  };

  const countDescendants = (node) => {
    let count = node.isField ? 1 : 0;
    for (const child of node.children.values()) {
      count += countDescendants(child);
    }
    return count;
  };

  const renderNode = (node, depth = 0) => {
    if (node.children.size === 0) {
      return createFieldRow(node.fullPath, depth > 0 ? node.name : null);
    }

    const container = el("div", "field-tree-node");
    const treeId = `field-tree-${scope}-${node.fullPath}`;
    container.dataset.treeId = treeId;
    container.dataset.field = `${node.fullPath} ${node.name}`.toLocaleLowerCase();
    if (state.treeCollapsed.has(treeId)) container.classList.add("collapsed");

    const toggle = el("button", "field-toggle-btn");
    toggle.innerHTML = '<i class="fas fa-angle-down"></i>';
    toggle.setAttribute("aria-label", "Recolher / expandir subcampos");
    toggle.onclick = (e) => {
      e.stopPropagation();
      if (state.treeCollapsed.delete(treeId)) container.classList.remove("collapsed");
      else { state.treeCollapsed.add(treeId); container.classList.add("collapsed"); }
    };

    if (node.isField) {
      container.appendChild(createFieldRow(node.fullPath, depth > 0 ? node.name : null, true, toggle));
    } else {
      const groupRow = el("div", "field-group-row");
      groupRow.dataset.field = `${node.fullPath} ${node.name}`.toLocaleLowerCase();
      const head = el("button", "field-group-head");
      head.innerHTML = `<i class="fas fa-angle-down xnode-caret"></i><i class="fas fa-folder-tree xnode-icon"></i><span class="xnode-label">${esc(node.name)}</span><span class="xnode-meta">${countDescendants(node)}</span>`;
      head.onclick = () => {
        if (state.treeCollapsed.delete(treeId)) container.classList.remove("collapsed");
        else { state.treeCollapsed.add(treeId); container.classList.add("collapsed"); }
      };
      groupRow.appendChild(head);
      container.appendChild(groupRow);
    }

    const kids = el("div", "field-tree-kids");
    const sortedChildren = [...node.children.values()].sort((a, b) => {
      const aFav = a.isField && favorites.includes(a.fullPath);
      const bFav = b.isField && favorites.includes(b.fullPath);
      if (aFav !== bFav) return Number(bFav) - Number(aFav);
      return a.name.localeCompare(b.name);
    });
    for (const child of sortedChildren) {
      kids.appendChild(renderNode(child, depth + 1));
    }
    container.appendChild(kids);
    return container;
  };

  const treeRoot = buildTree(columns);
  const sortedRoots = [...treeRoot.children.values()].sort((a, b) => {
    const aFav = a.isField && favorites.includes(a.fullPath);
    const bFav = b.isField && favorites.includes(b.fullPath);
    if (aFav !== bFav) return Number(bFav) - Number(aFav);
    return a.name.localeCompare(b.name);
  });
  const fieldKids = sortedRoots.map((rootNode) => renderNode(rootNode, 0));

  const searchFields = el("input", "field-search");
  searchFields.type = "search";
  searchFields.placeholder = "Buscar campo…";
  searchFields.setAttribute("aria-label", "Buscar campo");

  searchFields.oninput = () => {
    const q = searchFields.value.trim().toLocaleLowerCase();
    const updateVisibility = (domEl) => {
      if (!q) {
        domEl.hidden = false;
        const treeId = domEl.dataset.treeId;
        if (treeId) {
          if (state.treeCollapsed.has(treeId)) domEl.classList.add("collapsed");
          else domEl.classList.remove("collapsed");
        }
        for (const child of domEl.querySelectorAll(":scope > .field-tree-kids > .field-tree-node, :scope > .field-tree-kids > .field-row")) {
          updateVisibility(child);
        }
        return true;
      }
      const selfMatch = (domEl.dataset.field || "").includes(q);
      let childMatch = false;
      for (const child of domEl.querySelectorAll(":scope > .field-tree-kids > .field-tree-node, :scope > .field-tree-kids > .field-row")) {
        if (updateVisibility(child)) childMatch = true;
      }
      const visible = selfMatch || childMatch;
      domEl.hidden = !visible;
      if (childMatch) domEl.classList.remove("collapsed");
      return visible;
    };
    fieldKids.forEach(r => updateVisibility(r));
  };
  box.appendChild(treeNode({ id: `fields-${scope}`, icon: "fa-table-columns", label: "Campos", meta: String(columns.length), kids: [searchFields, ...fieldKids] }));

  // campos customizados (regex): gerenciáveis, com edição
  if (state.derivedFields?.length) {
    const derivedKids = state.derivedFields.map((def) => {
      const row = el("div", "field-row");
      const main = el("button", "field-item");
      main.innerHTML = `<i class="fas fa-wand-magic-sparkles"></i><span>${esc(def.name)}</span><small>${esc(colLabel(def.source))}</small>`;
      main.title = `${def.name} ← ${colLabel(def.source)} · /${def.pattern}/`;
      main.onclick = () => openDeriveEdit(def);
      const edit = el("button", "icon-btn field-adv-btn");
      edit.innerHTML = '<i class="fas fa-pen"></i>';
      edit.title = `Editar campo ${def.name}`;
      edit.onclick = (e) => { e.stopPropagation(); openDeriveEdit(def); };
      row.append(main, edit);
      return row;
    });
    box.appendChild(treeNode({
      id: `derived-${scope}`, icon: "fa-wand-magic-sparkles", label: "Campos customizados",
      meta: String(state.derivedFields.length), kids: derivedKids,
    }));
  }

  // grupo de valores: contagens vivas (recorte sem o filtro da própria coluna) quando disponíveis
  const stdIcons = { level: "fa-signal", source: "fa-server", code: "fa-barcode" };
  const valueGroup = (column, fallbackItems, meta, emptyCount = 0) => {
    let entries = live[column];
    if (!entries) {
      entries = (fallbackItems || []).map(([v, n]) => [v == null ? null : String(v), n]);
      if (emptyCount > 0) entries.push([null, emptyCount]);
    }
    entries = entries.slice().sort((a, b) => b[1] - a[1]);
    // valores com filtro ativo permanecem mesmo se ausentes da agregação
    if (hasColFilter(column)) {
      for (const f of state.filters.filter((f) => f.column === column && ["equals", "equals_exact"].includes(f.op)))
        if (!entries.some(([v]) => v === f.value)) entries.push([f.value, 0]);
      if (state.filters.some((f) => f.column === column && f.op === "empty")
        && !entries.some(([v]) => v == null)) entries.push([null, 0]);
    }
    if (entries.length < 2 && !hasColFilter(column)) return; // nada a escolher
    const kids = entries.slice(0, FACET_VALUE_LIMIT).map(([value, count]) =>
      facetValueItem(column, value, count, value != null && column === "level" ? levelColor(value) : null, column === "code"));
    box.appendChild(treeNode({
      id: `facet-${scope}-${column}`,
      icon: stdIcons[column] || FIELD_KIND_ICONS[byName[column]?.kind] || "fa-tag",
      label: colLabel(column), meta: String(meta ?? entries.length), kids, active: hasColFilter(column),
    }));
  };

  // grupos padrão (nível/fonte/código)
  const facetData = scope === "dataset" ? (state.facetData || {}) : {};
  valueGroup("level", facetData.levels || byName.level?.top, byName.level?.cardinality, byName.level?.empty || 0);
  valueGroup("source", facetData.sources || byName.source?.top, byName.source?.cardinality, byName.source?.empty || 0);
  valueGroup("code", facetData.codes || byName.code?.top, byName.code?.cardinality, byName.code?.empty || 0);

  // campos de baixa cardinalidade
  const { categories, ranges } = treeGroupProfiles(profiles);
  for (const p of categories) valueGroup(p.name, p.top, p.cardinality, p.empty || 0);

  // campos numéricos: faixas estáveis (perfil) com contagens vivas; faixas zeradas somem
  for (const p of ranges) {
    const opts = rangeOptions(p);
    if (!opts.length) continue;
    const counts = live[p.name] ? rangeCounts(opts, live[p.name]) : null;
    const kids = [];
    opts.forEach((opt, i) => {
      const n = counts ? counts[i] : null;
      const active = state.filters.some((f) => sameRangeFilter(f, p.name, opt));
      if (counts && n === 0 && !active) return; // faixa vazia no recorte: não oferecer
      kids.push(rangeItem(p.name, opt, n));
    });
    if (kids.length < 2 && !hasColFilter(p.name)) continue;
    box.appendChild(treeNode({
      id: `range-${scope}-${p.name}`, icon: FIELD_KIND_ICONS[p.kind] || "fa-hashtag", label: colLabel(p.name),
      meta: `${fmtKindValue(p.kind, p.min)} – ${fmtKindValue(p.kind, p.max)}`, kids, active: hasColFilter(p.name),
    }));
  }
  if (!box.children.length) box.innerHTML = '<p class="muted small">Sem dados para explorar.</p>';
}

function hasFacetFilter(column, value) {
  return state.filters.some((f) => f.column === column && f.op === "equals" && f.value === value);
}

function toggleFacet(column, value) {
  const empty = value == null;
  const i = state.filters.findIndex((f) => f.column === column && (empty ? f.op === "empty" : ["equals", "equals_exact"].includes(f.op) && f.value === String(value)));
  if (i >= 0) { removeFilter(i); return; }
  // substitui a seleção anterior do mesmo campo — combinar valores zeraria o recorte
  state.filters = state.filters.filter((f) => !(f.column === column && ["equals", "equals_exact", "empty"].includes(f.op)));
  addFilter({ column, op: empty ? "empty" : "equals_exact", value: empty ? "" : String(value), value2: null });
}

let currentEditFilterIndex = null;

// popover de novo filtro ou edição
function openFilterPop(anchor = null, editIndex = null) {
  currentEditFilterIndex = editIndex;
  const pop = $("#filter-pop");
  const colSel = $("#fp-col");
  const opSel = $("#fp-op");
  const titleEl = pop.querySelector(".pop-title");
  if (titleEl) titleEl.textContent = editIndex != null ? "Editar filtro" : "Novo filtro";

  colSel.innerHTML = "";
  colSel.appendChild(el("option", "", colLabel("_all"))).value = "_all";
  for (const c of state.columns) colSel.appendChild(el("option", "", colLabel(c))).value = c;
  opSel.innerHTML = "";
  for (const [v, l] of OPS) opSel.appendChild(el("option", "", l)).value = v;

  if (editIndex != null && state.filters[editIndex]) {
    const f = state.filters[editIndex];
    colSel.value = f.column;
    opSel.value = f.op;
    $("#fp-val").value = f.value ?? "";
    $("#fp-val2").value = f.value2 ?? "";
    $("#fp-val2").hidden = f.op !== "between";
  } else {
    $("#fp-val").value = "";
    $("#fp-val2").value = "";
    $("#fp-val2").hidden = true;
  }
  opSel.onchange = () => { $("#fp-val2").hidden = opSel.value !== "between"; };
  pop.hidden = false;
  positionPop(pop, anchor || $("#btn-add-filter"));
  $("#fp-val").focus();
}
function applyFilterPop() {
  const column = $("#fp-col").value;
  const op = $("#fp-op").value;
  const value = $("#fp-val").value;
  const value2 = $("#fp-val2").value;
  if (!["empty", "not_empty"].includes(op) && !value.trim()) {
    toast("Informe um valor para o filtro.", "info");
    return;
  }
  if (currentEditFilterIndex != null && state.filters[currentEditFilterIndex]) {
    state.filters[currentEditFilterIndex] = { column, op, value, value2: value2 || null };
    currentEditFilterIndex = null;
    state.page = 0;
    filtersChanged();
    toast("Filtro atualizado.", "ok");
  } else {
    addFilter({ column, op, value, value2: value2 || null });
  }
  $("#filter-pop").hidden = true;
}

function positionPop(pop, anchor) {
  const r = anchor.getBoundingClientRect();
  const margin = 12;
  const gap = 6;
  // O popover ja esta visivel quando esta funcao e chamada, entao as medidas
  // refletem seu conteudo real (inclusive o pop de nome da visualizacao).
  const w = pop.offsetWidth || 260;
  const h = pop.offsetHeight || 120;
  const opensAbove = r.bottom + gap + h > innerHeight - margin && r.top - gap >= h + margin;
  const top = opensAbove ? r.top - h - gap : Math.min(r.bottom + gap, innerHeight - h - margin);
  const left = Math.max(margin, Math.min(r.left, innerWidth - w - margin));
  pop.style.top = `${Math.max(margin, top)}px`;
  pop.style.left = `${left}px`;
}

// ------------------------------------------------------------------ refresh
async function refresh({ analytics = true } = {}) {
  const scope = workspaceScope();
  if (scope === "dataset" && !state.loaded) return;
  const filters = backendFilters();
  const version = ++state.refreshVersion;
  // evita cliques duplos na paginação enquanto a consulta está no ar
  $("#pg-prev").disabled = true;
  $("#pg-next").disabled = true;
  const loading = areaLoading($("#tab-table"), "Consultando…");
  startOperation("explore", "Atualizando exploração", "Lendo eventos e calculando recortes");
  let snapshot;
  try {
    const cacheKey = JSON.stringify([scope, scope === "case" ? caseSig() : state.currentArtifact?.loadedAt, filters]);
    const cached = state.explorerCache?.key === cacheKey ? state.explorerCache.snapshot : null;
    const result = await api(cached ? "query_events" : "explore_snapshot", {
      filters,
      ...(scope === "case" ? { caseEvents: caseEvents() } : {}),
      sortColumn: state.sortCol,
      sortDir: state.sortDir,
      offset: state.page * state.pageSize,
      limit: state.pageSize,
    });
    snapshot = cached ? { ...cached, query: result } : result;
    if (version === state.refreshVersion && snapshot.query.total > 0 && state.page * state.pageSize >= snapshot.query.total) {
      state.page = Math.floor((snapshot.query.total - 1) / state.pageSize);
      snapshot = { ...snapshot, query: await api("query_events", { filters, ...(scope === "case" ? { caseEvents: caseEvents() } : {}), sortColumn: state.sortCol, sortDir: state.sortDir, offset: state.page * state.pageSize, limit: state.pageSize }) };
    }
    if (version === state.refreshVersion) state.explorerCache = { key: cacheKey, snapshot };
  } catch (e) {
    loading.done();
    if (version === state.refreshVersion) {
      state.queryError = String(e); state.rows = []; state.total = 0; state.dataPeriod = null; state.explorerCache = null;
      renderTable({ rows: [], total: 0 });
      $("#empty-state p").textContent = "Não foi possível consultar. Revise os filtros ou tente novamente.";
      const retry = el("button", "btn ghost small", "Tentar novamente"); retry.dataset.retry = "true"; retry.onclick = () => refresh(); $("#empty-state").append(retry);
      $("#pg-prev").disabled = true; $("#pg-next").disabled = true; $("#result-count").textContent = "Consulta não concluída";
      if (chart) { chart.destroy(); chart = null; } $("#chart").replaceChildren();
      updateContextBar();
      finishOperation("Falha ao atualizar", String(e));
    }
    return false;
  }
  if (version !== state.refreshVersion) { loading.done(); return; }
  const { query: qr, stats, sources: srcAgg, codes: codeAgg } = snapshot;
  state.queryError = null;
  if (!qr.total) state.page = 0;
  state.total = qr.total;
  state.rows = qr.rows;
  if (stats.buckets?.length) {
    state.dataPeriod = {
      min: stats.buckets[0][0],
      max: stats.buckets[stats.buckets.length - 1][0] + (stats.bucketMs ?? stats.bucket_ms ?? 0),
    };
  } else state.dataPeriod = null;
  renderTable(qr);
  renderChart(stats);
  renderChips();

  const toRows = (agg) =>
    agg.rows
      .map((r) => {
        const key = Object.keys(r).find((k) => k !== "n");
        return [r[key], r.n];
      })
      .sort((a, b) => b[1] - a[1]);
  state.facetData = { levels: stats.levels, sources: toRows(srcAgg), codes: toRows(codeAgg) };
  refreshTreeAggs(scope);
  updateContextBar();
  // recalcula a aba analítica aberta para refletir o novo recorte
  if (analytics && state.activeDatasetTab === "dashboard") renderDashboard(scope);
  else if (analytics && state.activeDatasetTab === "cube") runCube();
  else if (analytics && state.activeDatasetTab === "group") runGroup();
  loading.done();
  finishOperation("Pronto", `${fmtNum(qr.total)} eventos no recorte`);
  window.Workspace?.onRefresh();
  return true;
}

function scheduleRefresh() {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => filtersChanged(), 350);
}

// ------------------------------------------------------------------ menu de contexto
let ctxEl = null;

function closeCtxMenu() {
  if (ctxEl) { ctxEl.remove(); ctxEl = null; }
}

function showCtxMenu(x, y, items) {
  closeCtxMenu();
  const m = el("div", "ctx-menu");
  for (const it of items) {
    if (it.sep) { m.appendChild(el("div", "ctx-sep")); continue; }
    const b = el("button", "ctx-item" + (it.danger ? " danger" : ""));
    b.innerHTML = `<i class="fas ${it.icon}"></i><span>${esc(it.label)}</span>`;
    if (it.color) b.querySelector("i").style.color = it.color;
    b.onclick = () => { closeCtxMenu(); it.onClick(); };
    m.appendChild(b);
  }
  document.body.appendChild(m);
  ctxEl = m;
  const r = m.getBoundingClientRect();
  m.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 8))}px`;
  m.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 8))}px`;
}

const trunc = (s, n = 32) => {
  s = String(s);
  return s.length > n ? s.slice(0, n) + "…" : s;
};

// ------------------------------------------------------------------ formatos
async function loadFormatOptions(selectId) {
  const sel = $("#file-format");
  const current = selectId || sel.value || "auto";
  sel.innerHTML = "";
  try {
    const formats = await api("list_formats", {}, { silent: true });
    for (const f of formats) sel.appendChild(el("option", "", f.name)).value = f.id;
  } catch {
    sel.appendChild(el("option", "", "Automático (inferir)")).value = "auto";
  }
  sel.value = [...sel.options].some((o) => o.value === current) ? current : "auto";
}

async function saveNewFormat() {
  const name = $("#fmt-name").value.trim();
  const kind = document.querySelector("#fmt-kind-seg .seg-btn.active")?.dataset.kind || "regex";
  const pattern = $("#fmt-pattern").value;
  const separator = $("#fmt-separator").value;
  const fields = $("#fmt-fields").value.split(",").map((s) => s.trim()).filter(Boolean);
  try {
    await api("save_custom_format", { name, kind, pattern, separator, fields }, { silent: true });
    $("#format-modal").hidden = true;
    $("#fmt-name").value = "";
    $("#fmt-pattern").value = "";
    $("#fmt-fields").value = "";
    await loadFormatOptions(`custom:${name}`);
    toast(`Formato "${name}" salvo.`, "ok");
  } catch (e) {
    toast(String(e), "err");
  }
}

async function testNewFormat() {
  const kind = document.querySelector("#fmt-kind-seg .seg-btn.active")?.dataset.kind || "regex";
  const pattern = $("#fmt-pattern").value;
  const separator = $("#fmt-separator").value;
  const fields = $("#fmt-fields").value.split(",").map((s) => s.trim()).filter(Boolean);
  const sample = $("#fmt-sample").value;
  const box = $("#fmt-test-result");
  box.innerHTML = "";
  if (!sample.trim()) { box.innerHTML = '<span class="muted small">Cole linhas de exemplo.</span>'; return; }
  try {
    const events = await api("test_parse", { kind, pattern, separator, fields, sample }, { silent: true });
    if (!events.length) { box.innerHTML = '<span class="muted small">Nenhuma linha reconhecida.</span>'; return; }
    const ev = events[0];
    for (const k of ["timestamp", "level", "code", "source", "message"]) {
      const row = el("div", "tr-row");
      row.innerHTML = `<span class="tr-k">${k}</span><span class="tr-pair">${esc(ev[k] ?? "")}</span>`;
      box.appendChild(row);
    }
    for (const [k, v] of Object.entries(ev.fields || {})) {
      const row = el("div", "tr-row");
      row.innerHTML = `<span class="tr-k">${esc(k)}</span><span class="tr-pair">${esc(String(v))}</span>`;
      box.appendChild(row);
    }
  } catch (e) {
    box.innerHTML = `<span class="tr-pair bad">${esc(String(e))}</span>`;
  }
}

// ------------------------------------------------------------------ data/hora
const TS_FORMATS = [
  ["%Y-%m-%dT%H:%M:%S%.f", "ISO 8601 (2024-01-31T08:00:01.123)"],
  ["%Y-%m-%d %H:%M:%S%.f", "2024-01-31 08:00:01,123"],
  ["%Y-%m-%d %H:%M:%S", "2024-01-31 08:00:01"],
  ["%d/%m/%Y %H:%M:%S", "31/01/2024 08:00:01"],
  ["%d/%b/%Y:%H:%M:%S %z", "31/Jan/2024:08:00:01 -0300"],
  ["%b %d %H:%M:%S", "Jan 31 08:00:01 (syslog)"],
  ["%H:%M:%S%.f", "08:00:01,123 (só hora)"],
  ["epoch_ms", "epoch em ms"],
  ["epoch_s", "epoch em s"],
  ["custom", "Personalizado…"],
];

function fillTsFormats() {
  const sel = $("#ts-format");
  sel.innerHTML = "";
  for (const [v, l] of TS_FORMATS) sel.appendChild(el("option", "", l)).value = v;
  sel.value = TS_FORMATS[1][0];
}

function renderTsSources() {
  const box = $("#ts-sources");
  box.innerHTML = "";
  const options = ["arquivo", "caminho", "linha", ...state.columns];
  for (const opt of options) {
    const chip = el("button", "ts-src");
    chip.type = "button";
    const idx = state.tsSources.indexOf(opt);
    chip.classList.toggle("active", idx >= 0);
    chip.textContent = (idx >= 0 ? `${idx + 1}· ` : "") + colLabel(opt);
    chip.onclick = () => {
      const i = state.tsSources.indexOf(opt);
      if (i >= 0) state.tsSources.splice(i, 1);
      else state.tsSources.push(opt);
      renderTsSources();
    };
    box.appendChild(chip);
  }
  renderTsExample();
}

// Texto de exemplo ao vivo: mostra os campos concatenados e o que a regex captura.
let tsExampleEv = null;

async function updateTsExample() {
  if (!state.loaded || !state.rows.length || currentSource() !== "file") {
    tsExampleEv = null;
    renderTsExample();
    return;
  }
  try {
    tsExampleEv = await api("event_detail", { id: state.rows[0].id }, { silent: true });
  } catch {
    tsExampleEv = null;
  }
  renderTsExample();
}

// espelha apply_ts_config do backend: mesmas fontes, mesma ordem, separadas por espaço
function tsSourceValue(ev, s) {
  if (s === "linha") return ev.raw || cellValue(ev, "message");
  if (s === "timestamp") return ev.timestamp != null ? new Date(ev.timestamp).toISOString() : "";
  return cellValue(ev, s); // "arquivo"/"caminho" vêm dos campos do evento
}
function tsJoinedExample(ev) {
  return state.tsSources.map((s) => tsSourceValue(ev, s)).join(" ");
}

function tsRuleBlock(rule = {}) {
  const block = el("div", "dv-rule");
  const head = el("div", "dv-rule-head");
  head.appendChild(el("span", "dv-rule-title"));
  const del = el("button", "icon-btn dv-rule-del");
  del.innerHTML = '<i class="fas fa-xmark"></i>';
  del.title = "Remover regra";
  del.onclick = () => { block.remove(); tsRenumberRules(); renderTsExample(); };
  head.appendChild(del);
  const pat = el("input");
  pat.className = "dv-rule-pattern";
  pat.type = "text";
  pat.spellcheck = false;
  pat.placeholder = "Regex (opcional) ex.: (\d{4}-\d{2}-\d{2}).*?(\d{2}:\d{2}:\d{2})";
  pat.value = rule.regex || "";
  const tpl = el("input");
  tpl.className = "dv-rule-template";
  tpl.type = "text";
  tpl.spellcheck = false;
  tpl.placeholder = "Montagem com grupos (opcional, ex.: $2 $1)";
  tpl.value = rule.template || "";
  const result = el("div", "dv-rule-result");
  pat.oninput = tpl.oninput = renderTsExample;
  block.append(head, pat, tpl, result);
  return block;
}

function tsRenumberRules() {
  const blocks = document.querySelectorAll("#ts-rules .dv-rule");
  blocks.forEach((b, i) => {
    b.querySelector(".dv-rule-title").textContent = `Regra ${i + 1}`;
    b.querySelector(".dv-rule-del").hidden = blocks.length === 1;
  });
}

function tsAddRule(rule = {}) {
  const block = tsRuleBlock(rule);
  $("#ts-rules").appendChild(block);
  tsRenumberRules();
  return block;
}

function collectTsRules() {
  const rules = [];
  document.querySelectorAll("#ts-rules .dv-rule").forEach((b) => {
    const regex = b.querySelector(".dv-rule-pattern").value.trim();
    const template = b.querySelector(".dv-rule-template").value.trim();
    if (!regex && !template) return;
    rules.push({ regex: regex || null, template: template || null });
  });
  return rules;
}

function renderTsExample() {
  const box = $("#ts-example");
  const joined = tsExampleEv && state.tsSources.length ? tsJoinedExample(tsExampleEv) : null;
  let winner = null; // { m, tpl } da primeira regra que casa
  document.querySelectorAll("#ts-rules .dv-rule").forEach((b) => {
    const pat = b.querySelector(".dv-rule-pattern").value.trim();
    const tpl = b.querySelector(".dv-rule-template").value.trim();
    const out = b.querySelector(".dv-rule-result");
    b.classList.remove("matched");
    if (joined == null) { out.textContent = ""; return; }
    if (!pat) {
      out.textContent = `→ ${joined.trim() || "(vazio)"}`;
      if (!winner && joined.trim()) winner = { m: null, tpl };
      return;
    }
    let re;
    try { re = new RegExp(pat, "d"); } catch (e) { out.textContent = `regex inválida: ${e.message}`; return; }
    const m = re.exec(joined);
    if (!m) { out.textContent = "(sem correspondência)"; return; }
    out.textContent = `→ ${tpl ? dvExpandTemplate(tpl, m) : (m.length >= 3 ? `${m[1]} ${m[2]}` : (m[1] ?? m[0]))}`;
    b.classList.add("matched");
    if (!winner) winner = { m, tpl };
  });
  if (joined == null) {
    box.textContent = state.tsSources.length ? "—" : "Selecione as fontes acima.";
    return;
  }
  if (!winner || !winner.m || !winner.m.indices) { box.innerHTML = esc(joined); return; }
  // marca grupos 1..3 da regra vencedora com cores distintas
  const m = winner.m;
  const spans = [];
  for (let gi = 1; gi <= Math.min(3, m.indices.length - 1); gi++) {
    const range = m.indices[gi];
    if (range) spans.push({ start: range[0], end: range[1], cls: `tsg${gi}` });
  }
  spans.sort((a, b) => a.start - b.start);
  let html = "";
  let pos = 0;
  for (const sp of spans) {
    if (sp.start < pos) continue;
    html += esc(joined.slice(pos, sp.start));
    html += `<mark class="${sp.cls}">${esc(joined.slice(sp.start, sp.end))}</mark>`;
    pos = sp.end;
  }
  html += esc(joined.slice(pos));
  box.innerHTML = html;
}

async function resetTsConfig() {
  const paths = tsConfigPaths();
  state.tsSources = [];
  $("#ts-rules").innerHTML = "";
  tsAddRule();
  $("#ts-complement").value = "";
  fillTsFormats();
  $("#ts-format-custom").hidden = true;
  renderTsSources();
  for (const path of paths) {
    await api("set_ts_config", { path, config: null });
  }
  if (paths.length) {
    toast("Configuração removida — voltou à inferência automática.", "ok");
    refresh();
  }
}

function tsFormatValue() {
  const v = $("#ts-format").value;
  return v === "custom" ? $("#ts-format-custom").value.trim() : v;
}

function buildTsConfig() {
  return {
    timezone_offset_minutes: $("#ts-zone").value === "" ? null : Number($("#ts-zone").value),
    clock_adjustment_ms: Number($("#ts-clock").value || 0) * 1000,
    sources: state.tsSources,
    rules: collectTsRules(),
    format: tsFormatValue(),
    complement: $("#ts-complement").value.trim() || null,
  };
}

async function loadTsConfig(path) {
  $("#ts-zone").value = ""; $("#ts-clock").value = "0";
  state.tsSources = [];
  $("#ts-regex").value = "";
  $("#ts-complement").value = "";
  try {
    const cfg = await api("get_ts_config", { path }, { silent: true });
    if (cfg) {
      $("#ts-zone").value = cfg.timezone_offset_minutes == null ? "" : String(cfg.timezone_offset_minutes);
      $("#ts-clock").value = String((cfg.clock_adjustment_ms || 0) / 1000);
      state.tsSources = cfg.sources || [];
      $("#ts-rules").innerHTML = "";
      const rules = cfg.rules?.length ? cfg.rules : [{ regex: cfg.regex, template: cfg.template }];
      for (const r of rules) tsAddRule(r);
      if (!rules.length) tsAddRule();
      $("#ts-complement").value = cfg.complement || "";
      if (TS_FORMATS.some(([v]) => v === cfg.format)) $("#ts-format").value = cfg.format;
      else {
        $("#ts-format").value = "custom";
        $("#ts-format-custom").hidden = false;
        $("#ts-format-custom").value = cfg.format || "";
      }
    }
  } catch { /* sem config salva */ }
  renderTsSources();
}

async function testTsConfig() {
  const box = $("#ts-test-result");
  box.innerHTML = "";
  try {
    const rows = await api("test_ts_config", { config: buildTsConfig(), path: tsConfigPath() }, { silent: true });
    for (const [entrada, resultado] of rows) {
      const row = el("div", "tr-row");
      const ok = !resultado.includes("não reconhecido");
      row.innerHTML = `<span class="tr-pair">${esc(entrada.slice(0, 60))} → <span class="${ok ? "ok" : "bad"}">${esc(resultado)}</span></span>`;
      box.appendChild(row);
    }
    if (!rows.length) box.innerHTML = '<span class="muted small">Sem linhas para testar.</span>';
  } catch (e) {
    box.innerHTML = `<span class="tr-pair bad">${esc(String(e))}</span>`;
  }
}

// todos os arquivos do conjunto atual (união), ou o caminho do campo
function tsConfigPath() { return state.tsEditingPath || state.currentArtifact?.path || $("#file-path").value.split(";")[0].trim(); }
function tsConfigPaths() {
  if (state.tsEditingPath) return [state.tsEditingPath];
  const members = state.currentArtifact?.source?.members;
  if (members) return members.flatMap(s => s.kind === "file" ? (s.paths?.length ? s.paths : [s.path]) : []);
  const fromSource = state.currentArtifact?.source?.paths?.filter(Boolean);
  if (fromSource?.length) return fromSource;
  const single = tsConfigPath();
  return single ? [single] : [];
}

async function applyTsConfig() {
  const paths = tsConfigPaths();
  if (!paths.length) { toast("Carregue um arquivo primeiro.", "info"); return; }
  const cfg = buildTsConfig();
  const empty = cfg.sources.length === 0 || !cfg.format;
  const done = btnBusy($("#ts-apply"), "Aplicando…");
  // status detalhado: passos + progresso por linha + resultado
  showLoadOverlay("Aplicando configuração de data/hora");
  try {
    // cada arquivo do conjunto guarda a config pela própria chave (caminho)
    for (const path of paths) {
      await api("set_ts_config", { path, config: empty ? null : cfg });
    }
    hideLoadOverlay(true);
    toast(empty ? "Configuração de data/hora removida." : "Data/hora aplicada aos eventos.", "ok");
    refresh();
  } catch (e) {
    hideLoadOverlay(false);
    toast(`Falha ao aplicar data/hora: ${e}`, "err");
  } finally {
    done();
  }
}

// ------------------------------------------------------------------ campo derivado
let dvCtx = null;

const DV_DELIMITERS = new Set(["\r", "\n", "\t", '"', "'", ";", "|", ",", ".", ")", "]", "}"]);

function dvDelimiterLabel(char) {
  if (char === "\r" || char === "\n") return "quebra de linha";
  if (char === "\t") return "tab";
  if (char === '"') return "aspas";
  if (char === "'") return "apóstrofo";
  return `“${char}”`;
}

function inferDvSuggestions(selected, sourceValue) {
  const out = [];
  const seen = new Set();
  const add = (label, pattern, detail = "") => {
    if (!pattern || seen.has(pattern)) return;
    seen.add(pattern);
    out.push({ label, pattern, detail });
  };
  const value = String(sourceValue || "");
  const index = value.indexOf(selected);

  // Usa o texto antes e depois da seleção como contexto, mas captura apenas
  // o trecho entre eles. Em linhas com a mesma estrutura, extrai o valor variável.
  if (index >= 0) {
    const before = value.slice(0, index);
    const after = value.slice(index + selected.length);
    add(
      "Trecho entre antes e depois",
      `^${escRe(before)}([\\s\\S]*?)${escRe(after)}$`,
      "Captura exatamente a seleção atual usando o restante do valor como contexto.",
    );

    // Alternativa para quando a seleção é uma chave/âncora: pega o próximo
    // valor até o primeiro delimitador efetivo visto naquele mesmo texto.
    const afterSelection = value.slice(index + selected.length);
    const leading = afterSelection.match(/^[ \t]*/)?.[0] || "";
    const start = leading.length;
    const first = afterSelection[start];
    const prefix = value.slice(0, index + selected.length) + leading;
    if (first === '"' || first === "'") {
      const closing = afterSelection.indexOf(first, start + 1);
      if (closing > start + 1) {
        add(
          `Após a seleção, entre ${dvDelimiterLabel(first)}`,
          `^${escRe(prefix + first)}([^${escRe(first)}]*)${escRe(first)}`,
          `Extrai o próximo valor delimitado por ${dvDelimiterLabel(first)}.`,
        );
      }
    } else {
      let delimiterIndex = -1;
      for (let i = start; i < afterSelection.length; i++) {
        if (DV_DELIMITERS.has(afterSelection[i])) { delimiterIndex = i; break; }
      }
      if (delimiterIndex > start && afterSelection.slice(start, delimiterIndex).trim()) {
        const delimiter = afterSelection[delimiterIndex];
        add(
          `Após a seleção, até ${dvDelimiterLabel(delimiter)}`,
          `^${escRe(prefix)}([\\s\\S]*?)${escRe(delimiter)}`,
          `Extrai o próximo valor até ${dvDelimiterLabel(delimiter)}.`,
        );
      }
    }
  }

  if (/^\d+$/.test(selected)) add("Número", "(\\d+)");
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(selected)) add("IP", "(\\d{1,3}(?:\\.\\d{1,3}){3})");
  if (/^\d+[.,]\d+$/.test(selected)) add("Decimal", "(\\d+[.,]\\d+)");
  if (/^[A-Za-z_][\w.-]*$/.test(selected) && !/^\d+$/.test(selected)) add("Identificador", "([\\w.-]+)");
  if (/^\d{2}:\d{2}(:\d{2})?/.test(selected)) add("Hora", "(\\d{2}:\\d{2}(?::\\d{2})?(?:[.,]\\d+)?)");
  add("Próximo token", "(\\S+)", "Extrai um valor contínuo sem espaços.");
  return out;
}

function renderDvSuggestions() {
  const sug = $("#dv-suggestions");
  sug.innerHTML = "";
  if (!dvCtx) return;
  for (const suggestion of inferDvSuggestions(dvCtx.selected, dvCtx.sourceValue)) {
    const button = el("button", "dv-sug", suggestion.label);
    button.type = "button";
    button.title = `${suggestion.detail}\nRegex: ${suggestion.pattern}`.trim();
    button.onclick = () => {
      const last = [...document.querySelectorAll("#dv-rules .dv-rule-pattern")].at(-1);
      if (last) { last.value = suggestion.pattern; updateDvPreview(); }
    };
    sug.appendChild(button);
  }
}

function dvRuleBlock(rule = {}) {
  const block = el("div", "dv-rule");
  const head = el("div", "dv-rule-head");
  head.appendChild(el("span", "dv-rule-title"));
  const del = el("button", "icon-btn dv-rule-del");
  del.innerHTML = '<i class="fas fa-xmark"></i>';
  del.title = "Remover regra";
  del.onclick = () => { block.remove(); dvRenumberRules(); updateDvPreview(); };
  head.appendChild(del);
  const pat = el("input");
  pat.className = "dv-rule-pattern";
  pat.type = "text";
  pat.spellcheck = false;
  pat.placeholder = "Regex (grupos $1, $2…)";
  pat.value = rule.pattern || "";
  const tpl = el("input");
  tpl.className = "dv-rule-template";
  tpl.type = "text";
  tpl.spellcheck = false;
  tpl.placeholder = "Modelo (opcional): ex.: $1 - $2 · vazio = 1º grupo";
  tpl.value = rule.template || "";
  const frow = el("div", "dv-filter-row");
  const fcol = el("select");
  fcol.className = "dv-rule-fcol";
  fcol.appendChild(el("option", "", "sempre aplicar")).value = "";
  for (const c of state.columns) fcol.appendChild(el("option", "", colLabel(c))).value = c;
  fcol.value = rule.filter?.column || "";
  const fop = el("select");
  fop.className = "dv-rule-fop";
  for (const [v, l] of OPS.filter(([v]) => ["contains", "not_contains", "equals", "not_equals", "starts_with", "regex", "empty", "not_empty"].includes(v)))
    fop.appendChild(el("option", "", l)).value = v;
  fop.value = rule.filter?.op || "contains";
  const fval = el("input");
  fval.className = "dv-rule-fval";
  fval.type = "text";
  fval.spellcheck = false;
  fval.placeholder = "valor da condição (opcional)";
  fval.value = rule.filter?.value || "";
  frow.append(fcol, fop, fval);
  const result = el("div", "dv-rule-result");
  for (const input of [pat, tpl, fval]) input.oninput = updateDvPreview;
  for (const s of [fcol, fop]) s.onchange = updateDvPreview;
  block.append(head, pat, tpl, frow, result);
  return block;
}

function dvRenumberRules() {
  const blocks = document.querySelectorAll("#dv-rules .dv-rule");
  blocks.forEach((b, i) => {
    b.querySelector(".dv-rule-title").textContent = `Regra ${i + 1}`;
    b.querySelector(".dv-rule-del").hidden = blocks.length === 1;
  });
}

function dvAddRule(rule = {}) {
  const block = dvRuleBlock(rule);
  $("#dv-rules").appendChild(block);
  dvRenumberRules();
  return block;
}

function dvBlockToRule(b) {
  const column = b.querySelector(".dv-rule-fcol").value;
  const op = b.querySelector(".dv-rule-fop").value;
  const value = b.querySelector(".dv-rule-fval").value;
  return {
    pattern: b.querySelector(".dv-rule-pattern").value,
    template: b.querySelector(".dv-rule-template").value.trim() || null,
    filter: column && (["empty", "not_empty"].includes(op) || value.trim())
      ? { column, op, value, value2: null }
      : null,
  };
}

function collectDvRules() {
  const rules = [];
  document.querySelectorAll("#dv-rules .dv-rule").forEach((b) => {
    const rule = dvBlockToRule(b);
    if (rule.pattern.trim()) rules.push(rule);
  });
  return rules;
}

// expande $1, $2… com os grupos capturados (espelha cap.expand do backend)
function dvExpandTemplate(template, m) {
  return template.replace(/\$(\d+)/g, (_, gi) => m[Number(gi)] ?? "");
}

// resultado de uma regra contra o valor atual
function dvRuleResult(rule) {
  if (rule.filter && dvCtx?.ev && !jsMatchFilter(dvCtx.ev, rule.filter)) {
    return { text: "(condição não atendida → próxima regra)", matched: false };
  }
  try {
    const m = new RegExp(rule.pattern).exec(dvCtx?.sourceValue || "");
    if (!m) return { text: "(sem correspondência)", matched: false };
    return { text: rule.template ? dvExpandTemplate(rule.template, m) : (m[1] ?? m[0]), matched: true };
  } catch (e) {
    return { text: `regex inválida: ${e.message}`, matched: false };
  }
}

function updateDvPreview() {
  let winner = null;
  document.querySelectorAll("#dv-rules .dv-rule").forEach((b) => {
    const rule = dvBlockToRule(b);
    const out = b.querySelector(".dv-rule-result");
    if (!rule.pattern.trim()) { out.textContent = ""; b.classList.remove("matched"); return; }
    const r = dvRuleResult(rule);
    out.textContent = r.text;
    b.classList.toggle("matched", r.matched);
    if (!winner && r.matched) winner = r;
  });
  const preview = $("#dv-preview");
  preview.textContent = winner
    ? winner.text
    : (document.querySelector("#dv-rules .dv-rule") ? "(nenhuma regra capturou — campo vazio)" : "—");
}

function openDeriveModal(selected, sourceCol, sourceValue) {
  dvCtx = { selected, sourceCol, sourceValue, editName: null };
  $("#derive-delete").hidden = true;
  $("#dv-name").disabled = false;
  $("#dv-selected").textContent = selected;
  $("#dv-source-label").textContent = colLabel(sourceCol);
  const sel = $("#dv-source");
  sel.innerHTML = "";
  for (const c of state.columns) sel.appendChild(el("option", "", colLabel(c))).value = c;
  sel.value = sourceCol;
  sel.onchange = () => {
    dvCtx.sourceCol = sel.value;
    dvCtx.sourceValue = cellValue(dvCtx.ev, sel.value);
    $("#dv-source-label").textContent = colLabel(sel.value);
    renderDvSuggestions();
    updateDvPreview();
  };
  dvCtx.ev = state.currentDetailEv;
  $("#dv-name").value = "";
  $("#dv-rules").innerHTML = "";
  dvAddRule();
  renderDvSuggestions();
  updateDvPreview();
  $("#derive-modal").hidden = false;
  $("#dv-name").focus();
}

function openDeriveEdit(def, { appendRule = false } = {}) {
  const ev = state.rows.find((r) => String(cellValue(r, def.source) || "").trim()) || state.rows[0] || null;
  state.currentDetailEv = ev;
  openDeriveModal("", def.source, ev ? cellValue(ev, def.source) : "");
  dvCtx.editName = def.name;
  dvCtx.ev = ev;
  $("#dv-name").value = def.name;
  $("#dv-name").disabled = true; // renomear criaria outro campo; nome é a chave
  $("#dv-source").value = def.source;
  $("#dv-rules").innerHTML = "";
  for (const rule of def.rules || []) dvAddRule(rule);
  if (appendRule || !(def.rules || []).length) dvAddRule(); // "Incrementar regra" ou campo sem regra válida
  $("#derive-delete").hidden = false;
  updateDvPreview();
}

async function saveDerivedField() {
  const name = $("#dv-name").value.trim();
  const rules = collectDvRules();
  if (!rules.length) { toast("Informe ao menos uma regra (regex).", "info"); return; }
  try {
    await api("save_derived_field", { name, source: dvCtx.sourceCol, rules });
    $("#derive-modal").hidden = true;
    loadDerivedFields();
    toast(`Campo "${name}" salvo.`, "ok");
    // disponibiliza a coluna imediatamente (árvore, seletor de colunas, agrupamentos)
    if (!state.columns.includes(name)) {
      state.columns.push(name);
      fillColumnControls();
    }
    state.datasetProfiles = null;
    api("profile_fields", { filters: [] }, { silent: true })
      .then((profiles) => { state.datasetProfiles = profiles; refreshTreeAggs(state.activeContext === "artifact" ? "dataset" : "case"); })
      .catch(() => {});
    refresh();
  } catch { /* toast já exibido */ }
}

// ------------------------------------------------------------------ analisar trilha
// 15 antes + o evento + 15 depois, com "carregar mais" nas duas pontas.
// Estado persistido no Caso (reabrir volta à mesma visualização).
function openTrail(ev) {
  state.trail = {
    centerId: ev.id,
    centerLabel: `${fmtTs(ev.timestamp)} · ${ev.source} · #${ev.code}${ev.name ? " " + ev.name : ""}`,
    before: 15,
    after: 15,
    onlyCase: false,
    unfiltered: false,
  };
  persistTrail();
  switchView("trail");
}

function persistTrail() {
  const c = activeCase();
  if (!c || !state.trail) return;
  c.trail = state.trail;
  saveCases();
}

async function renderTrail() {
  const t = state.trail;
  const list = $("#trail-list");
  if (!t) { list.innerHTML = '<p class="muted small">Nenhum evento selecionado.</p>'; return; }
  $("#trail-only-case").checked = !!t.onlyCase;
  $("#trail-unfiltered").checked = !!t.unfiltered;
  $("#trail-info").textContent = t.centerLabel || "";
  startOperation("trail", "Analisando trilha", "Buscando vizinhança do evento");
  let res;
  try {
    res = await api("trail_events", {
      centerId: t.centerId,
      before: t.before,
      after: t.after,
      filters: t.unfiltered ? [] : backendFilters(),
      ...(t.onlyCase ? { caseEvents: caseEvents() } : {}),
    });
  } catch (e) {
    finishOperation("Falha ao analisar trilha", String(e));
    return;
  }
  finishOperation("Possível trilha pronta", `${res.events.length} eventos na vizinhança`);
  list.innerHTML = "";
  // formato de timeline vertical: o evento analisado destacado no meio do fluxo
  const wrap = el("div", "vtl trail-vtl");
  let lastDay = null;
  for (const ev of res.events) {
    const day = fmtDay(ev.timestamp);
    if (day !== lastDay) {
      lastDay = day;
      wrap.appendChild(el("div", "vtl-day", day));
    }
    const isCenter = ev.id === t.centerId;
    const row = el("div", "vtl-row" + (isCenter ? " trail-center" : ""));
    row.appendChild(el("div", "vtl-time", fmtTime(ev.timestamp)));
    const dot = el("div", "vtl-dot");
    dot.style.background = levelColor(ev.level);
    row.appendChild(dot);
    const card = el("div", "vtl-card");
    card.appendChild(el("span", "vtl-src", `${ev.source} · #${ev.code}${ev.name ? " " + ev.name : ""}`));
    card.appendChild(el("span", "vtl-msg", ev.message || ""));
    if (isCenter) card.appendChild(el("span", "trail-center-mark", "◀ evento analisado"));
    row.appendChild(card);
    row.title = "Ver detalhes";
    row.onclick = () => openDetail(ev.id);
    wrap.appendChild(row);
  }
  list.appendChild(wrap);
  const before = $("#trail-more-before"), after = $("#trail-more-after");
  before.hidden = res.before_available === 0;
  after.hidden = res.after_available === 0;
  before.textContent = "";
  after.textContent = "";
  before.insertAdjacentHTML("beforeend", `<i class="fas fa-angle-up"></i> Carregar mais antes (${fmtNum(Math.min(15, res.before_available))} de ${fmtNum(res.before_available)})`);
  after.insertAdjacentHTML("beforeend", `<i class="fas fa-angle-down"></i> Carregar mais depois (${fmtNum(Math.min(15, res.after_available))} de ${fmtNum(res.after_available)})`);
}

// ------------------------------------------------------------------ filtros salvos
// Abas inferiores: alternar entre filtros nomeados, com contagens caso/artefato.
const CURRENT_FILTER_ID = "__current__";
function savedFilters() {
  const c = activeCase();
  if (!c) return [];
  if (workspaceScope() === "case") { c.workspace ||= defaultCaseWorkspace(); return c.workspace.savedFilters ||= []; }
  c.savedFilters = c.savedFilters || [];
  return c.savedFilters;
}

function sameFilters(a, b) {
  const norm = (fs) => JSON.stringify((fs || []).map((f) => [f.column, f.op, f.value, f.value2 || null]).sort());
  return norm(a) === norm(b);
}

function copyCurrentFilters() {
  return state.filters.map((filter) => ({ ...filter }));
}

function copyCurrentFilterState() {
  return { filters: copyCurrentFilters(), quick: state.quick.trim() };
}

function sameSavedFilterState(saved) {
  return sameFilters(saved?.filters, state.filters) && (saved?.quick || "").trim() === state.quick.trim();
}

function savedBackendFilters(saved) {
  const filters = (saved?.filters || []).map((filter) => ({ ...filter }));
  if (saved?.quick?.trim()) filters.unshift({ column: "message", op: "contains", value: saved.quick.trim(), value2: null });
  return filters;
}

function restoreCurrentSavedFilter() {
  const current = savedFilters().find((filter) => filter.id === CURRENT_FILTER_ID);
  state.filters = (current?.filters || []).map((filter) => ({ ...filter }));
  state.quick = current?.quick || "";
  $("#quick-search").value = state.quick;
}

function syncCurrentSavedFilter() {
  const c = activeCase();
  if (!c) return null;
  const saved = savedFilters();
  let current = saved.find((filter) => filter.id === CURRENT_FILTER_ID);
  let changed = false;

  if (!current) {
    current = { id: CURRENT_FILTER_ID, name: "Visualizacao atual", ...copyCurrentFilterState() };
    saved.unshift(current);
    changed = true;
  } else {
    if (!current.name?.trim()) { current.name = "Visualizacao atual"; changed = true; }
    if (!sameSavedFilterState(current)) {
      Object.assign(current, copyCurrentFilterState());
      changed = true;
    }
    // A visualizacao que acompanha a ultima acao fica sempre como primeira aba.
    if (saved[0] !== current) {
      saved.splice(saved.indexOf(current), 1);
      saved.unshift(current);
      changed = true;
    }
  }
  if (changed) saveCases();
  return current;
}

function applySavedFilter(saved) {
  state.filters = (saved.filters || []).map((f) => ({ ...f }));
  state.quick = saved.quick || "";
  $("#quick-search").value = state.quick;
  state.page = 0;
  filtersChanged();
}

function renderFilterTabs() {
  const bar = $("#filter-tabs");
  bar.innerHTML = "";
  const c = activeCase();
  if (!c) {
    bar.hidden = true;
    updateFilterTabsLayout();
    return;
  }
  const saved = savedFilters();
  bar.hidden = false;
  for (const f of saved) {
    const tab = el("button", "filter-tab" + (sameSavedFilterState(f) ? " active" : ""));
    tab.dataset.fid = f.id;
    tab.innerHTML = '<i class="fas fa-filter"></i>';
    tab.appendChild(el("span", "filter-tab-name", f.name));
    tab.appendChild(el("span", "filter-tab-counts", "…"));
    tab.title = `${f.name} — clique para aplicar · botão direito para opções`;
    tab.onclick = () => applySavedFilter(f);
    tab.oncontextmenu = (e) => {
      e.preventDefault();
      const actions = [
        { icon: "fa-pen", label: "Renomear visualizacao", onClick: () => openNamePop(tab, (name) => { f.name = name; saveCases(); renderFilterTabs(); }, { title: "Renomear visualizacao", initialValue: f.name }) },
      ];
      if (f.id !== CURRENT_FILTER_ID) {
        actions.push(
          { icon: "fa-arrows-rotate", label: "Atualizar com o recorte atual", onClick: () => { Object.assign(f, copyCurrentFilterState()); saveCases(); renderFilterTabs(); refreshFilterTabCounts(); } },
          { sep: true },
          { icon: "fa-trash-can", label: "Excluir visualizacao", danger: true, onClick: () => { const index = saved.findIndex((x) => x.id === f.id); if (index >= 0) saved.splice(index, 1); saveCases(); renderFilterTabs(); } },
        );
      }
      showCtxMenu(e.clientX, e.clientY, actions);
    };
    bar.appendChild(tab);
  }
  const add = el("button", "filter-tab filter-tab-add");
  add.innerHTML = '<i class="fas fa-plus"></i>';
  add.appendChild(el("span", "filter-tab-name", "Nova visualizacao"));
  add.title = "Criar uma visualizacao a partir do recorte atual";
  add.onclick = (e) => {
    e.stopPropagation(); // evita o fechamento imediato pelo handler de documento
    openNamePop(add, (name) => {
      savedFilters().push({ id: "f" + Date.now().toString(36), name, ...copyCurrentFilterState() });
      saveCases();
      renderFilterTabs();
      refreshFilterTabCounts();
      toast(`Visualizacao "${name}" criada.`, "ok");
    }, { title: "Nova visualizacao", placeholder: "ex.: Somente erros" });
  };
  bar.appendChild(add);
  updateFilterTabsLayout();
  refreshFilterTabCounts();
}

// As abas ficam fixas acima da barra de status. Reserva-se a mesma altura no
// espaço de trabalho para que elas nunca cubram tabela, botões ou rolagem.
function updateFilterTabsLayout() {
  const bar = $("#filter-tabs");
  const height = bar && !bar.hidden ? Math.ceil(bar.getBoundingClientRect().height) : 0;
  document.documentElement.style.setProperty("--filter-tabs-h", `${height}px`);
}

let filterCountsTimer = null;
function refreshFilterTabCounts() {
  clearTimeout(filterCountsTimer);
  filterCountsTimer = setTimeout(async () => {
    const saved = savedFilters();
    if (!saved.length) return;
    const caseEvs = caseEvents();
    await Promise.all(saved.map(async (f) => {
      const tab = document.querySelector(`.filter-tab[data-fid="${f.id}"] .filter-tab-counts`);
      if (!tab) return;
      try {
        const [artifact, caso] = await Promise.all([
          api("count_filtered", { filters: savedBackendFilters(f) }, { silent: true }),
          caseEvs.length ? api("count_filtered", { filters: savedBackendFilters(f), caseEvents: caseEvs }, { silent: true }) : Promise.resolve(0),
        ]);
        tab.textContent = `${fmtNum(caso)} · ${fmtNum(artifact)}`;
        tab.title = `${fmtNum(caso)} no Caso · ${fmtNum(artifact)} no Artefato`;
      } catch { tab.textContent = "—"; }
    }));
  }, 250);
}

// ------------------------------------------------------------------ estações
function caseStations() {
  const c = activeCase();
  if (!c) return [];
  c.stations = c.stations || [];
  return c.stations;
}

function caseArtifacts(c = activeCase()) {
  if (!c) return [];
  c.artifacts = c.artifacts || [];
  return c.artifacts;
}

function registerCurrentArtifact(c = activeCase()) {
  if (!c || !state.currentArtifact) return null;
  c.activeArtifactId = state.currentArtifact.id; // restaura o artefato aberto ao reabrir
  const artifacts = caseArtifacts(c);
  const record = { ...state.currentArtifact }; // inclui source (paths) — sem ele o conjunto multi-arquivo se perde
  let artifact = artifacts.find((a) => a.id === state.currentArtifact.id);
  if (!artifact) {
    artifact = { ...record, stationId: null };
    artifacts.push(artifact);
  } else {
    Object.assign(artifact, record, { stationId: artifact.stationId || null });
  }
  return artifact;
}

function currentCaseArtifact() {
  const c = activeCase();
  if (!c || !state.currentArtifact) return null;
  return caseArtifacts(c).find((a) => a.id === state.currentArtifact.id) || null;
}

function activeStation() {
  return caseStations().find((s) => s.id === state.activeStationId) || null;
}

function stationItems(stationId) {
  return (activeCase()?.items || []).filter((item) => item.stationId === stationId);
}

function stationArtifacts(stationId) {
  return caseArtifacts().filter((artifact) => artifact.stationId === stationId);
}

function renderStationShortcuts() {
  const box = $("#station-list-side");
  box.innerHTML = "";
  const stations = caseStations();
  if (!stations.length) {
    box.innerHTML = '<p class="muted small">Associe uma estação ao adicionar um item ao Caso.</p>';
    return;
  }
  for (const station of stations) {
    const item = el("button", "station-shortcut");
    const count = stationItems(station.id).length;
    item.innerHTML = `<i class="fas fa-server"></i><span>${esc(station.name)}</span><small>${count}</small>`;
    item.onclick = () => openStation(station.id);
    box.appendChild(item);
  }
}

function openStation(stationId) {
  state.activeStationId = stationId;
  state.activeContext = "station";
  switchView("estacoes");
  renderStations();
  updateContextBar();
}

function renderStations() {
  const box = $("#stations-list");
  box.innerHTML = "";
  const c = activeCase();
  const stations = caseStations();
  $("#stations-count").textContent = c
    ? `${stations.length} ${stations.length === 1 ? "estação" : "estações"}`
    : "Sem caso ativo";
  if (!c) {
    box.innerHTML = `<div class="analysis-empty"><i class="fas fa-server"></i>Selecione ou crie um caso primeiro.</div>`;
    return;
  }
  const selected = activeStation();
  if (selected) {
    const items = stationItems(selected.id);
    const artifacts = stationArtifacts(selected.id);
    const toolbar = el("div", "analysis-toolbar");
    const back = el("button", "btn ghost small");
    back.innerHTML = '<i class="fas fa-arrow-left"></i> Todas as estações';
    back.onclick = () => {
      state.activeStationId = null;
      saveCaseWorkspace("estacoes");
      renderStations();
      updateContextBar();
    };
    toolbar.append(back, el("span", "spacer"));
    const dash = el("button", "btn ghost small");
    dash.innerHTML = '<i class="fas fa-chart-pie"></i> Painéis do recorte';
    dash.onclick = () => { state.stationAnalyticsId = selected.id; switchView("case-dashboard"); };
    const cube = el("button", "btn primary small");
    cube.innerHTML = '<i class="fas fa-cube"></i> Cubo do recorte';
    cube.onclick = () => { state.stationAnalyticsId = selected.id; switchView("case-cube"); };
    toolbar.append(dash, cube);
    box.appendChild(toolbar);
    const summary = el("div", "station-summary");
    const cards = [
      ["Estação", selected.name],
      ["Host / IP", selected.host || "Não informado"],
      ["Artefatos associados", fmtNum(artifacts.length)],
      ["Itens técnicos no Caso", fmtNum(items.length)],
    ];
    for (const [label, value] of cards) {
      const card = el("div", "station-stat");
      card.append(el("small", "", label), el("strong", "", value));
      summary.appendChild(card);
    }
    box.appendChild(summary);
    if (selected.notes) box.appendChild(el("p", "note-preview", selected.notes));
    const artifactsTitle = el("div", "side-title", "Artefatos relacionados");
    box.appendChild(artifactsTitle);
    if (!artifacts.length) box.appendChild(el("p", "muted small", "Nenhum artefato associado ainda."));
    for (const artifact of artifacts) {
      const row = el("div", "analysis-item-head");
      row.appendChild(el("span", "label", artifact.label || artifact.path || "Artefato"));
      row.appendChild(el("span", "spacer"));
      row.appendChild(el("span", "count", `${fmtNum(artifact.count || 0)} eventos`));
      box.appendChild(row);
    }
    const itemTitle = el("div", "side-title", "Itens relacionados no Caso");
    box.appendChild(itemTitle);
    if (!items.length) box.appendChild(el("p", "muted small", "Nenhum item técnico associado a esta estação."));
    for (const item of items) {
      const row = el("div", "analysis-item-head");
      row.appendChild(el("span", "kind", item.kind));
      row.appendChild(el("span", "label", item.label));
      row.appendChild(el("span", "spacer"));
      row.appendChild(el("span", "count", `${fmtNum(item.includedCount ?? item.rows?.length ?? 0)} eventos`));
      box.appendChild(row);
    }
    return;
  }
  if (!stations.length) {
    box.innerHTML = `<div class="analysis-empty">
      <i class="fas fa-server"></i>
      Nenhuma estação neste caso.<br>
      <span class="small">Estações representam as máquinas de origem dos logs do caso.</span>
    </div>`;
    return;
  }
  stations.forEach((st, i) => {
    const card = el("div", "analysis-item");
    const head = el("div", "analysis-item-head");
    head.appendChild(el("span", "kind", "estação"));
    const label = el("button", "label", st.name);
    label.onclick = () => openStation(st.id);
    head.appendChild(label);
    if (st.host) head.appendChild(el("span", "station-meta", st.host));
    head.appendChild(el("span", "spacer"));
    const info = el("button", "icon-btn");
    info.innerHTML = '<i class="fas fa-circle-info"></i>';
    info.title = "Inspecionar estação";
    info.onclick = () => showStationInspector(st);
    const del = el("button", "icon-btn");
    del.innerHTML = '<i class="fas fa-trash-can"></i>';
    del.title = "Excluir estação";
    del.onclick = () => {
      for (const artifact of caseArtifacts()) {
        if (artifact.stationId === st.id) artifact.stationId = null;
      }
      for (const item of activeCase()?.items || []) {
        if (item.stationId === st.id) item.stationId = null;
      }
      caseStations().splice(i, 1);
      if (state.activeStationId === st.id) state.activeStationId = null;
      if (state.stationAnalyticsId === st.id) state.stationAnalyticsId = null;
      saveCaseWorkspace("estacoes");
      renderStations();
      updateAnalysisBadge();
    };
    head.append(info, del);
    card.appendChild(head);

    const body = el("div", "station-files");
    body.appendChild(el("span", "muted small", `${stationArtifacts(st.id).length} artefatos · ${stationItems(st.id).length} itens no Caso`));
    if (st.notes) body.appendChild(el("p", "muted small", st.notes));
    card.appendChild(body);
    box.appendChild(card);
  });
  renderStationShortcuts();
}

function createStation({ name, host = "", notes = "" }) {
  const station = {
    id: "s" + Date.now().toString(36) + Math.floor(Math.random() * 1e4),
    name: name.trim(), host: host.trim(), notes: notes.trim(),
  };
  caseStations().push(station);
  return station;
}

// ------------------------------------------------------------------ casos de análise
let casesSaveQueue = Promise.resolve();
let caseSaveErrorShown = false;

let caseSaveTimer = null;
let caseSaveWaiters = [];
function saveCases() {
  clearTimeout(caseSaveTimer);
  const result = new Promise((resolve, reject) => caseSaveWaiters.push({ resolve, reject }));
  // Autosave is coalesced, while callers can still await durable persistence.
  caseSaveTimer = setTimeout(() => {
    const waiters = caseSaveWaiters.splice(0);
    const snapshot = JSON.parse(JSON.stringify({ ...state.cases, schemaVersion: 2 }));
    casesSaveQueue = casesSaveQueue.catch(() => {}).then(() => api("cases_save", { data: { ...snapshot, revision: state.cases.revision } }, { silent: true }))
      .then(result => { if (result?.revision != null) state.cases.revision = result.revision; caseSaveErrorShown = false; waiters.forEach(w => w.resolve(true)); })
      .catch(error => {
        if (!caseSaveErrorShown) { caseSaveErrorShown = true; toast(`Não foi possível salvar: ${error}`, "err"); }
        waiters.forEach(w => w.resolve(false));
      });
  }, 200);
  return result;
}

function defaultCaseWorkspace() {
  return {
    view: "caso",
    analysisView: "overview",
    activeStationId: null,
    stationAnalyticsId: null,
    expandedItemIds: [],
    updatedAt: 0,
  };
}

function normalizeCaseWorkspace(raw) {
  const base = defaultCaseWorkspace();
  const source = raw && typeof raw === "object" ? raw : {};
  const allowedViews = new Set(["caso", "estacoes", "case-dashboard", "case-cube", "trail"]);
  const allowedAnalysisViews = new Set(["overview", "items", "timeline", "vtimeline", "timeline-table", "data"]);
  return {
    ...base,
    ...source,
    view: allowedViews.has(source.view) ? source.view : base.view,
    analysisView: allowedAnalysisViews.has(source.analysisView) ? source.analysisView : base.analysisView,
    activeStationId: typeof source.activeStationId === "string" ? source.activeStationId : null,
    stationAnalyticsId: typeof source.stationAnalyticsId === "string" ? source.stationAnalyticsId : null,
    expandedItemIds: Array.isArray(source.expandedItemIds) ? source.expandedItemIds.filter((id) => typeof id === "string") : [],
    updatedAt: Number.isFinite(source.updatedAt) ? source.updatedAt : 0,
  };
}

function syncAnalysisViewButtons() {
  document.querySelectorAll(".small-seg .seg-btn").forEach((button) => {
    button.classList.toggle("active", button.dataset.view === state.analysisView);
  });
}

function setAnalysisView(view) {
  if (!["overview", "items", "timeline", "vtimeline", "timeline-table", "data"].includes(view)) return;
  state.analysisView = view;
  syncAnalysisViewButtons();
}

function saveCaseWorkspace(view = null) {
  const c = activeCase();
  if (!c) return;
  const workspace = normalizeCaseWorkspace(c.workspace);
  const stationIds = new Set((c.stations || []).map((station) => station.id));
  c.workspace = {
    ...workspace,
    view: view || workspace.view,
    analysisView: state.analysisView,
    activeStationId: stationIds.has(state.activeStationId) ? state.activeStationId : null,
    stationAnalyticsId: stationIds.has(state.stationAnalyticsId) ? state.stationAnalyticsId : null,
    updatedAt: Date.now(),
  };
  saveCases();
}

function restoreCaseWorkspace() {
  const c = activeCase();
  if (!c) return false;
  const workspace = normalizeCaseWorkspace(c.workspace);
  const stationIds = new Set((c.stations || []).map((station) => station.id));
  c.workspace = workspace;
  state.activeStationId = stationIds.has(workspace.activeStationId) ? workspace.activeStationId : null;
  state.stationAnalyticsId = stationIds.has(workspace.stationAnalyticsId) ? workspace.stationAnalyticsId : null;
  setAnalysisView(workspace.analysisView);
  if (workspace.view === "trail" && c.trail) state.trail = c.trail;
  switchView(workspace.view);
  return true;
}

function normalizeCaseStore(loaded) {
  const stored = Array.isArray(loaded?.cases) ? loaded.cases : [];
  const cases = stored.filter((item) => item && typeof item === "object").map((raw, index) => ({
    ...raw,
    id: raw.id || `c-recuperado-${index}-${Date.now().toString(36)}`,
    name: raw.name || `Caso ${index + 1}`,
    createdAt: raw.createdAt || Date.now(),
    items: Array.isArray(raw.items) ? raw.items.map((item) => ({
      ...item,
      createdAt: item?.createdAt || raw.createdAt || Date.now(),
      rows: Array.isArray(item?.rows) ? item.rows : [],
      sourceFilters: Array.isArray(item?.sourceFilters) ? item.sourceFilters : [],
      tags: Array.isArray(item?.tags) ? item.tags : [],
      note: typeof item?.note === "string" ? item.note : "",
      summary: typeof item?.summary === "string" ? item.summary : "",
      details: typeof item?.details === "string" ? item.details : (typeof item?.note === "string" ? item.note : ""),
      attachments: window.CaseContent?.attachments(item) || [],
      relevance: item?.relevance || "normal",
    })) : [],
    caseTrails: Array.isArray(raw.caseTrails) ? raw.caseTrails.filter(trail => trail && typeof trail.id === "string").map(trail => ({ ...trail, title: typeof trail.title === "string" ? trail.title : "Trilha", summary: typeof trail.summary === "string" ? trail.summary : "", details: typeof trail.details === "string" ? trail.details : "", itemIds: [...new Set(Array.isArray(trail.itemIds) ? trail.itemIds.filter(id => typeof id === "string") : [])], attachments: window.CaseContent?.attachments(trail) || [] })) : [],
    manual: Array.isArray(raw.manual) ? raw.manual.map((item) => ({ ...item, createdAt: item?.createdAt || raw.createdAt || Date.now() })) : [],
    stations: Array.isArray(raw.stations) ? raw.stations : [],
    artifacts: Array.isArray(raw.artifacts) ? raw.artifacts : [],
    workspace: normalizeCaseWorkspace(raw.workspace),
  }));
  const active = cases.some((item) => item.id === loaded?.active) ? loaded.active : (cases[0]?.id || null);
  return { active, cases, schemaVersion: loaded?.schemaVersion || 2, revision: loaded?.revision };
}
function activeCase() {
  return state.cases.cases.find((c) => c.id === state.cases.active) || null;
}
function ensureCase() {
  let c = activeCase();
  if (!c) { newCase(undefined, { keepArtifact: true }); c = activeCase(); }
  return c;
}
function newCase(name, { keepArtifact = false, contextSnapshot = null } = {}) {
  if (window.WorkspaceContext?.sourceBusy) { toast("Aguarde a atualização das fontes para criar um Caso.", "info"); return activeCase(); }
  const context = contextSnapshot || window.WorkspaceContext?.beforeCaseCreation();
  const c = {
    id: "c" + Date.now().toString(36) + Math.floor(Math.random() * 1e4),
    name: name || `Caso ${state.cases.cases.length + 1}`,
    createdAt: Date.now(),
    items: [],
    manual: [],
    caseTrails: [],
    stations: [],
    artifacts: context?.artifacts || [],
    activeArtifactId: context?.activeArtifactId || null,
    savedFilters: [{ id: CURRENT_FILTER_ID, name: "Visualizacao atual", filters: [], quick: "" }],
    workspace: defaultCaseWorkspace(),
  };
  state.cases.cases.push(c);
  state.cases.active = c.id;
  state.activeStationId = null;
  state.stationAnalyticsId = null;
  setAnalysisView("overview");
  saveCases();
  renderCaseBar();
  updateAnalysisBadge();
  renderAnalysis();
  if (context) void window.WorkspaceContext.afterCaseCreation(context);
  else if (!keepArtifact) void syncActiveCaseArtifacts();
  return c;
}
function caseItems() {
  const c = activeCase();
  return c ? c.items : [];
}

function updateAnalysisBadge() {
  const n = caseItems().length;
  const b = $("#analysis-badge");
  b.hidden = n === 0;
  b.textContent = n;
  const c = activeCase();
  $("#analysis-count").textContent = c
    ? `${countLabel(n, "item", "itens")} · ${countLabel((c.manual || []).length, "marco manual", "marcos manuais")}`
    : "Sem caso ativo";
  renderStationShortcuts();
  window.WorkspaceContext?.refreshMembership?.();
  updateContextBar();
}

function renderCaseBar() {
  renderFilterTabs();
  const sel = $("#case-select");
  sel.innerHTML = "";
  if (state.cases.cases.length) {
    for (const c of state.cases.cases) sel.appendChild(el("option", "", c.name)).value = c.id;
    sel.value = state.cases.active || "";
  } else {
    const empty = el("option", "", "Sem Caso ativo");
    empty.value = "";
    empty.disabled = true;
    empty.selected = true;
    sel.appendChild(empty);
  }
  sel.disabled = state.cases.cases.length === 0;
  $("#btn-case-menu").disabled = !activeCase();
  renderArtifactBar();
  updateContextBar();
}

let caseInputMode = null;
function closeCaseNameInput() {
  $("#case-name-input").hidden = true;
  $("#case-select").hidden = false;
  $("#btn-new-case").hidden = false;
  $("#btn-case-menu").hidden = false;
  caseInputMode = null;
}
function showCaseNameInput(mode) {
  if (mode === "rename" && !activeCase()) return;
  caseInputMode = mode;
  const inp = $("#case-name-input");
  inp.value = mode === "rename" && activeCase() ? activeCase().name : "";
  $("#case-select").hidden = true;
  $("#btn-new-case").hidden = true;
  $("#btn-case-menu").hidden = true;
  inp.hidden = false;
  inp.focus();
}
function commitCaseNameInput() {
  const v = $("#case-name-input").value.trim();
  const mode = caseInputMode;
  closeCaseNameInput();
  if (!v) return;
  if (mode === "rename" && activeCase()) {
    activeCase().name = v;
    saveCases();
    renderCaseBar();
  } else {
    newCase(v);
  }
}
async function deleteActiveCase() {
  const c = activeCase();
  if (!c) return;
  if (window.WorkspaceContext?.ready) { await window.WorkspaceContext.deleteCase(c); toast(`Caso "${c.name}" excluído.`, "ok"); return; }
  state.artifactSessions.delete(c.id);
  state.cases.cases = state.cases.cases.filter((x) => x.id !== c.id);
  state.cases.active = state.cases.cases[0]?.id || null;
  state.activeStationId = null;
  state.stationAnalyticsId = null;
  setAnalysisView("overview");
  saveCases();
  renderCaseBar();
  updateAnalysisBadge();
  if (activeCase()) {
    restoreCaseWorkspace();
    await syncActiveCaseArtifacts();
  } else {
    switchView("source");
    await syncActiveCaseArtifacts();
  }
  toast(`Caso "${c.name}" excluído.`, "ok");
}

let pendingCaseAdd = null;

function openCaseAdd(request) {
  if (workspaceScope() === "case") { toast("Esses registros já pertencem ao Caso.", "info"); return; }
  const c = ensureCase();
  const artifact = currentCaseArtifact();
  pendingCaseAdd = request;
  const select = $("#case-add-station");
  select.innerHTML = '<option value="">Sem estação</option>';
  for (const station of caseStations()) select.appendChild(el("option", "", station.name)).value = station.id;
  select.value = artifact?.stationId || "";
  $("#case-add-summary").textContent = request.kind === "event"
    ? "O evento selecionado será incluído no Caso ativo."
    : `O grupo "${chipLabel({ column: request.column, op: request.op || "equals", value: request.value, value2: null })}" será incluído com a quantidade disponível.`;
  $("#case-add-station-form").hidden = true;
  $("#case-add-station-name").value = "";
  $("#case-add-station-host").value = "";
  $("#case-add-station-notes").value = "";
  $("#case-add-modal").hidden = false;
}

function addEventToAnalysis(evId) {
  openCaseAdd({ kind: "event", evId });
}

function addGroupToAnalysis(column, value, name, op = "equals_exact") {
  openCaseAdd({ kind: "group", column, value, name: name || "", op });
}

function caseItemBase(kind, stationId, foundCount, includedCount) {
  const artifact = registerCurrentArtifact();
  if (artifact) artifact.stationId = stationId || null;
  return {
    id: "i" + Date.now().toString(36) + Math.floor(Math.random() * 1e4),
    createdAt: Date.now(),
    kind,
    artifactId: artifact?.id || null,
    stationId: stationId || null,
    origin: state.currentOrigin,
    foundCount,
    includedCount,
    note: "",
    tags: [],
    relevance: "normal",
  };
}

async function confirmCaseAdd() {
  if (!pendingCaseAdd) return;
  const c = ensureCase();
  let stationId = $("#case-add-station").value || null;
  if (!$("#case-add-station-form").hidden) {
    const name = $("#case-add-station-name").value.trim();
    if (!name) { toast("Informe o nome da nova estação.", "info"); return; }
    stationId = createStation({
      name,
      host: $("#case-add-station-host").value,
      notes: $("#case-add-station-notes").value,
    }).id;
  }
  const request = pendingCaseAdd;
  $("#case-add-modal").hidden = true;
  pendingCaseAdd = null;
  startOperation("case", "Adicionando ao Caso", "Organizando o contexto técnico");
  try {
    if (request.kind === "visible") {
      c.items.push({
        ...caseItemBase("grupo", stationId, request.rows.length, request.rows.length),
        label: `${fmtNum(request.rows.length)} eventos visíveis`,
        total: request.rows.length,
        rows: request.rows,
        sourceFilters: backendFilters(),
      });
      toast(`${fmtNum(request.rows.length)} eventos adicionados ao Caso (duplicados ignorados).`, "ok");
      saveCases();
      updateAnalysisBadge();
      renderAnalysis();
      renderStations();
      finishOperation("Caso atualizado", "Eventos visíveis associados ao contexto.");
      return;
    }
    if (request.kind === "event") {
      const ev = await api("event_detail", { id: request.evId });
      if (!ev) return;
      c.items.push({
        ...caseItemBase("evento", stationId, 1, 1),
        label: `${fmtTs(ev.timestamp)} · ${ev.source} · #${ev.code}${ev.name ? " " + ev.name : ""}`,
        rows: [ev],
        sourceFilters: [],
      });
      toast("Evento adicionado ao Caso.", "ok");
    } else {
      const filters = [
        ...backendFilters(),
        { column: request.column, op: request.op || "equals", value: request.value, value2: null },
      ];
      const qr = await api("query_events", {
        filters, sortColumn: "timestamp", sortDir: "desc", offset: 0, limit: 500,
      });
      c.items.push({
        ...caseItemBase("grupo", stationId, qr.total, qr.rows.length),
        named: !!request.name,
        label: request.name || chipLabel({ column: request.column, op: request.op || "equals", value: request.value, value2: null }),
        total: qr.total,
        rows: qr.rows,
        sourceFilters: filters,
      });
      toast(`${fmtNum(qr.rows.length)} de ${fmtNum(qr.total)} eventos adicionados ao Caso.`, "ok");
    }
    saveCases();
    updateAnalysisBadge();
    renderAnalysis();
    renderStations();
    finishOperation("Caso atualizado", "Item técnico associado ao contexto selecionado.");
  } catch (e) {
    finishOperation("Falha ao adicionar ao Caso", String(e));
    toast(String(e), "err");
  }
}

// popover genérico para nomear um grupo antes de enviar à análise
let namePopCb = null;
function openNamePop(anchor, cb, { title = "Nome do agrupamento", placeholder = "ex.: Janela de falhas do gateway", initialValue = "" } = {}) {
  namePopCb = cb;
  $("#name-pop .pop-title").textContent = title;
  $("#np-val").placeholder = placeholder;
  $("#np-val").value = initialValue;
  const pop = $("#name-pop");
  pop.hidden = false;
  positionPop(pop, anchor);
  $("#np-val").focus();
}
function commitNamePop() {
  const v = $("#np-val").value.trim();
  $("#name-pop").hidden = true;
  const cb = namePopCb;
  namePopCb = null;
  if (v && cb) cb(v);
}

let editingCaseItem = null;
function openCaseItemContext(item) {
  editingCaseItem = item;
  $("#case-item-relevance").value = item.relevance || "normal";
  $("#case-item-tags").value = (item.tags || []).join(", ");
  $("#case-item-note").value = CaseContent.narrative(item).details;
  $("#case-item-modal").hidden = false;
}

function saveCaseItemContext() {
  if (!editingCaseItem) return;
  editingCaseItem.relevance = $("#case-item-relevance").value;
  editingCaseItem.tags = $("#case-item-tags").value.split(",").map((tag) => tag.trim()).filter(Boolean);
  editingCaseItem.details = $("#case-item-note").value.trim();
  editingCaseItem.note = editingCaseItem.details;
  saveCases();
  $("#case-item-modal").hidden = true;
  editingCaseItem = null;
  renderAnalysis();
}

function caseItemIncludedCount(item) {
  return item.includedCount ?? item.rows?.length ?? 0;
}

function caseItemFoundCount(item) {
  return item.foundCount ?? item.total ?? caseItemIncludedCount(item);
}

function caseStoredEventCount(c) {
  return (c.items || []).reduce((total, item) => total + caseItemIncludedCount(item), 0);
}

function caseItemStation(c, item) {
  return (c.stations || []).find((station) => station.id === item.stationId) || null;
}

function caseItemArtifact(c, item) {
  return (c.artifacts || []).find((artifact) => artifact.id === item.artifactId) || null;
}

function caseExpandedItemIds(c = activeCase()) {
  const workspace = normalizeCaseWorkspace(c?.workspace);
  if (c) c.workspace = workspace;
  return new Set(workspace.expandedItemIds);
}

function setCaseItemExpanded(itemId, expanded) {
  const c = activeCase();
  if (!c) return;
  const workspace = normalizeCaseWorkspace(c.workspace);
  const ids = new Set(workspace.expandedItemIds);
  if (expanded) ids.add(itemId);
  else ids.delete(itemId);
  c.workspace = { ...workspace, expandedItemIds: [...ids] };
  saveCaseWorkspace("caso");
  renderAnalysis();
}

function openCaseItemInItems(itemId) {
  const c = activeCase();
  if (!c) return;
  const workspace = normalizeCaseWorkspace(c.workspace);
  const ids = new Set(workspace.expandedItemIds);
  ids.add(itemId);
  c.workspace = { ...workspace, expandedItemIds: [...ids] };
  setAnalysisView("items");
  saveCaseWorkspace("caso");
  renderAnalysis();
}

function appendCaseItemMeta(host, c, item) {
  const meta = el("div", "case-item-meta");
  const station = caseItemStation(c, item);
  const artifact = caseItemArtifact(c, item);
  if (station) {
    const stationButton = el("button", "case-meta-link", station.name);
    stationButton.title = "Abrir estação";
    stationButton.onclick = () => openStation(station.id);
    meta.appendChild(stationButton);
  }
  if (artifact) meta.appendChild(el("span", "case-meta-token", artifact.label || artifact.path || "Artefato"));
  if (item.origin) meta.appendChild(el("span", "case-meta-token", item.origin));
  if (item.relevance && item.relevance !== "normal") meta.appendChild(el("span", `case-meta-token relevance-${item.relevance}`, item.relevance));
  for (const tag of item.tags || []) meta.appendChild(el("span", "case-meta-token", tag));
  if (meta.children.length) host.appendChild(meta);
}

function appendCaseItemBody(card, c, item) {
  const body = el("div", "case-item-body");
  appendCaseItemMeta(body, c, item);
  const narrative = CaseContent.narrative(item);
  if (narrative.summary) body.appendChild(el("p", "case-narrative-summary", narrative.summary));
  if (narrative.details) body.appendChild(el("p", "case-item-details", narrative.details));
  if (item.attachments?.length) { const images = el("div"); body.append(images); CaseContent.mountAttachments(images, item); }

  if (item.sourceFilters?.length) {
    const filters = document.createElement("details");
    filters.className = "case-item-detail";
    const summary = el("summary", "", `Filtro de origem (${item.sourceFilters.length})`);
    filters.appendChild(summary);
    const list = el("div", "case-filter-list");
    for (const filter of item.sourceFilters) list.appendChild(el("span", "case-meta-token", chipLabel(filter)));
    filters.appendChild(list);
    body.appendChild(filters);
  }

  const rows = item.rows || [];
  if (rows.length) {
    const events = document.createElement("details");
    events.className = "case-item-detail";
    events.appendChild(el("summary", "", `Eventos preservados (${fmtNum(rows.length)})`));
    const hint = caseItemFoundCount(item) > rows.length
      ? `${fmtNum(rows.length)} de ${countLabel(caseItemFoundCount(item), "evento")} foram preservados neste item.`
      : `${countLabel(rows.length, "evento preservado", "eventos preservados")} neste item.`;
    events.appendChild(el("p", "muted small", hint));
    const scroll = el("div", "table-scroll case-item-events");
    const table = el("table");
    const thead = el("thead");
    const trh = el("tr");
    for (const col of state.visibleCols) trh.appendChild(el("th", "", colLabel(col)));
    thead.appendChild(trh);
    const tbody = el("tbody");
    for (const event of rows.slice(0, 100)) tbody.appendChild(buildEventRow(event));
    table.append(thead, tbody);
    scroll.appendChild(table);
    events.appendChild(scroll);
    if (rows.length > 100) events.appendChild(el("p", "muted small", `Mostrando os primeiros 100 de ${fmtNum(rows.length)} eventos preservados.`));
    body.appendChild(events);
  }
  card.appendChild(body);
}

function renderCaseItems(box, c) {
  if (!(c.items || []).length) {
    box.innerHTML = `<div class="analysis-empty">
      <i class="fas fa-layer-group"></i>
      Nenhum item técnico ainda.<br>
      <span class="small">Envie um evento, grupo ou faceta ao Caso a partir do artefato.</span>
    </div>`;
    return;
  }
  const expanded = caseExpandedItemIds(c);
  const list = el("div", "case-item-list");
  c.items.forEach((item, index) => {
    const card = el("article", "case-item-card");
    const isOpen = expanded.has(item.id);
    card.classList.toggle("expanded", isOpen);
    const header = el("div", "case-item-summary");
    const open = el("button", "case-item-open");
    open.type = "button";
    open.setAttribute("aria-expanded", String(isOpen));
    const kind = el("span", "case-item-kind", item.kind || "item");
    const content = el("span", "case-item-title");
    const label = el("strong", "", item.label || "Item técnico");
    label.title = item.label || "Item técnico";
    content.appendChild(label);
    const station = caseItemStation(c, item);
    const artifact = caseItemArtifact(c, item);
    const subtitle = [station?.name, artifact?.label || artifact?.path, item.origin].filter(Boolean).join(" · ");
    if (subtitle) content.appendChild(el("small", "", subtitle));
    open.append(kind, content);
    open.onclick = () => setCaseItemExpanded(item.id, !isOpen);
    header.appendChild(open);

    const volume = el("span", "case-item-volume");
    volume.append(
      el("strong", "", fmtNum(caseItemIncludedCount(item))),
      el("span", "", `de ${fmtNum(caseItemFoundCount(item))}`),
    );
    volume.title = "Eventos preservados / eventos encontrados na origem";
    header.appendChild(volume);
    const actions = el("div", "case-item-actions");
    const edit = el("button", "icon-btn");
    edit.innerHTML = '<i class="fas fa-note-sticky"></i>';
    edit.title = "Anotações e relevância";
    edit.onclick = () => openCaseItemContext(item);
    const del = el("button", "icon-btn");
    del.innerHTML = '<i class="fas fa-trash-can"></i>';
    del.title = "Remover item do Caso";
    del.onclick = () => {
      c.items.splice(index, 1);
      const workspace = normalizeCaseWorkspace(c.workspace);
      workspace.expandedItemIds = workspace.expandedItemIds.filter((id) => id !== item.id);
      c.workspace = workspace;
      saveCases();
      updateAnalysisBadge();
      renderAnalysis();
    };
    const explain = el("button", "icon-btn"); explain.innerHTML = '<i class="fas fa-pen-to-square"></i>'; explain.title = "Explicação e imagens"; explain.setAttribute("aria-label", explain.title); explain.onclick = () => CaseContent.editItem(item.id);
    actions.append(explain, edit, del);
    header.appendChild(actions);
    card.appendChild(header);
    if (isOpen) appendCaseItemBody(card, c, item);
    list.appendChild(card);
  });
  box.appendChild(list);
}

function overviewMetric(icon, value, label, action) {
  const button = el("button", "case-overview-metric");
  button.type = "button";
  button.append(el("i", `fas ${icon}`), el("strong", "", fmtNum(value)), el("span", "", label));
  if (action) button.onclick = action;
  return button;
}

function renderCaseOverview(box, c) {
  const overview = el("div", "case-overview");
  const head = el("div", "case-overview-head");
  const copy = el("div", "");
  copy.append(el("span", "eyebrow", "Resumo do Caso"), el("h2", "", c.name));
  const updated = c.workspace?.updatedAt ? `Atualizado ${fmtTs(c.workspace.updatedAt)}` : `Criado ${fmtTs(c.createdAt)}`;
  copy.appendChild(el("p", "muted", updated));
  head.appendChild(copy);
  overview.appendChild(head);

  const metrics = el("div", "case-overview-metrics");
  metrics.append(
    overviewMetric("fa-layer-group", (c.items || []).length, (c.items || []).length === 1 ? "item técnico" : "itens técnicos", () => { setAnalysisView("items"); saveCaseWorkspace("caso"); renderAnalysis(); }),
    overviewMetric("fa-table-list", caseStoredEventCount(c), caseStoredEventCount(c) === 1 ? "evento preservado" : "eventos preservados", () => { setAnalysisView("items"); saveCaseWorkspace("caso"); renderAnalysis(); }),
    overviewMetric("fa-server", (c.stations || []).length, (c.stations || []).length === 1 ? "estação" : "estações", () => switchView("estacoes")),
    overviewMetric("fa-file-lines", (c.artifacts || []).length, (c.artifacts || []).length === 1 ? "artefato" : "artefatos", () => switchView("source")),
    overviewMetric("fa-thumbtack", (c.manual || []).length, (c.manual || []).length === 1 ? "marco manual" : "marcos manuais", () => { setAnalysisView("timeline"); saveCaseWorkspace("caso"); renderAnalysis(); }),
  );
  overview.appendChild(metrics);

  const recentSection = el("section", "case-overview-section");
  recentSection.appendChild(el("h3", "", "Itens técnicos"));
  const items = [...(c.items || [])].sort((a, b) => (b.createdAt || 0) - (a.createdAt || 0)).slice(0, 6);
  if (!items.length) {
    recentSection.appendChild(el("p", "muted", "Ainda não há recortes técnicos no Caso."));
  } else {
    const recentList = el("div", "case-overview-items");
    for (const item of items) {
      const row = el("button", "case-overview-item");
      row.type = "button";
      row.appendChild(el("span", "case-item-kind", item.kind || "item"));
      const itemCopy = el("span", "");
      itemCopy.append(el("strong", "", item.label || "Item técnico"), el("small", "", countLabel(caseItemIncludedCount(item), "evento preservado", "eventos preservados")));
      row.appendChild(itemCopy);
      row.appendChild(el("i", "fas fa-chevron-right"));
      row.onclick = () => openCaseItemInItems(item.id);
      recentList.appendChild(row);
    }
    recentSection.appendChild(recentList);
  }
  overview.appendChild(recentSection);
  box.appendChild(overview);
}

function caseDataSection(title, count, open = false) {
  const section = document.createElement("details");
  section.className = "case-data-section";
  section.open = open;
  const summary = el("summary", "");
  summary.append(el("strong", "", title));
  if (count != null) summary.appendChild(el("span", "", fmtNum(count)));
  section.appendChild(summary);
  return section;
}

function caseDataRow(label, value) {
  const row = el("div", "case-data-row");
  row.append(el("span", "", label), el("strong", "", value || "—"));
  return row;
}

function renderCaseData(box, c) {
  const data = el("div", "case-data");
  const info = caseDataSection("Informações do Caso", null, true);
  const infoRows = el("div", "case-data-rows");
  infoRows.append(
    caseDataRow("Nome", c.name),
    caseDataRow("Criado", fmtTsFull(c.createdAt)),
    caseDataRow("Última alteração", c.workspace?.updatedAt ? fmtTsFull(c.workspace.updatedAt) : "Ainda não alterado"),
    caseDataRow("Itens técnicos", String((c.items || []).length)),
    caseDataRow("Eventos preservados", String(caseStoredEventCount(c))),
  );
  info.appendChild(infoRows);
  data.appendChild(info);

  const artifacts = caseDataSection("Artefatos", (c.artifacts || []).length);
  if (!(c.artifacts || []).length) artifacts.appendChild(el("p", "muted", "Nenhum artefato vinculado a este Caso."));
  for (const artifact of c.artifacts || []) {
    const row = el("div", "case-data-record");
    row.append(el("strong", "", artifact.label || artifact.path || "Artefato"));
    const station = (c.stations || []).find((item) => item.id === artifact.stationId);
    row.appendChild(el("span", "", `${artifact.kind === "eventlog" ? "Event Log" : "Arquivo"} · ${countLabel(artifact.count || 0, "evento")}${station ? ` · ${station.name}` : ""}`));
    if (artifact.path) row.appendChild(el("small", "", artifact.path));
    artifacts.appendChild(row);
  }
  data.appendChild(artifacts);

  const stations = caseDataSection("Estações", (c.stations || []).length);
  if (!(c.stations || []).length) stations.appendChild(el("p", "muted", "Nenhuma estação associada."));
  for (const station of c.stations || []) {
    const row = el("div", "case-data-record");
    row.append(el("strong", "", station.name), el("span", "", station.host || "Host/IP não informado"));
    const linkedArtifacts = (c.artifacts || []).filter((item) => item.stationId === station.id).length;
    const linkedItems = (c.items || []).filter((item) => item.stationId === station.id).length;
    const linked = `${countLabel(linkedArtifacts, "artefato")} · ${countLabel(linkedItems, "item")}`;
    row.appendChild(el("small", "", linked));
    if (station.notes) row.appendChild(el("small", "", station.notes));
    stations.appendChild(row);
  }
  data.appendChild(stations);

  const items = caseDataSection("Itens técnicos", (c.items || []).length);
  if (!(c.items || []).length) items.appendChild(el("p", "muted", "Nenhum item técnico salvo."));
  for (const item of c.items || []) {
    const row = el("div", "case-data-record");
    row.append(el("strong", "", item.label || "Item técnico"));
    const station = caseItemStation(c, item);
    const artifact = caseItemArtifact(c, item);
    row.appendChild(el("span", "", `${item.kind || "item"} · ${fmtNum(caseItemIncludedCount(item))} de ${countLabel(caseItemFoundCount(item), "evento")}`));
    row.appendChild(el("small", "", [station?.name, artifact?.label || artifact?.path, item.origin].filter(Boolean).join(" · ") || "Sem associação adicional"));
    if (item.relevance !== "normal" || item.tags?.length || item.note) row.appendChild(el("small", "", [item.relevance !== "normal" ? item.relevance : "", ...(item.tags || []), item.note].filter(Boolean).join(" · ")));
    if (item.sourceFilters?.length) row.appendChild(el("small", "", `Filtros: ${item.sourceFilters.map(chipLabel).join(" · ")}`));
    items.appendChild(row);
  }
  data.appendChild(items);

  const manual = caseDataSection("Marcos manuais", (c.manual || []).length);
  if (!(c.manual || []).length) manual.appendChild(el("p", "muted", "Nenhum marco manual salvo."));
  for (const item of c.manual || []) {
    const row = el("div", "case-data-record");
    row.append(el("strong", "", item.name), el("span", "", item.end ? `${fmtTs(item.start)} → ${fmtTs(item.end)}` : fmtTs(item.start)));
    if (item.description) row.appendChild(el("small", "", item.description));
    manual.appendChild(row);
  }
  data.appendChild(manual);

  const visuals = caseDataSection("Visualizações salvas", (c.caseDashboard || []).length + (c.caseCube?.tables?.length || 0) + (c.caseCube?.charts?.length || 0));
  const visualRows = el("div", "case-data-rows");
  visualRows.append(
    caseDataRow("Painéis", String((c.caseDashboard || []).length)),
    caseDataRow("Tabelas do Cubo", String(c.caseCube?.tables?.length || 0)),
    caseDataRow("Gráficos do Cubo", String(c.caseCube?.charts?.length || 0)),
  );
  visuals.appendChild(visualRows);
  for (const chart of c.caseDashboard || []) {
    visuals.appendChild(el("div", "case-data-record", `${chart.title || "Gráfico"} · Painel`));
  }
  for (const table of c.caseCube?.tables || []) {
    visuals.appendChild(el("div", "case-data-record", `${table.name || "Tabela"} · Cubo`));
  }
  for (const chart of c.caseCube?.charts || []) {
    visuals.appendChild(el("div", "case-data-record", `${chart.name || "Gráfico"} · Gráfico do Cubo`));
  }
  data.appendChild(visuals);
  box.appendChild(data);
}

function renderAnalysis() {
  const box = $("#analysis-list");
  box.innerHTML = "";
  box.classList.remove("case-timeline-host");
  $("#view-analysis").classList.toggle("case-timeline-active", ["timeline", "vtimeline", "timeline-table"].includes(state.analysisView));
  const c = activeCase();
  if (!c) {
    box.innerHTML = `<div class="analysis-empty">
      <i class="fas fa-briefcase"></i>
      Nenhum Caso selecionado.<br>
      <span class="small">Crie um Caso no topo e adicione artefatos ou itens técnicos.</span>
    </div>`;
    return;
  }
  // árvore e barra de contexto acompanham o conjunto atual do Caso
  updateContextBar();
  refreshTreeAggs("case"); // memoizado: só recalcula se a assinatura mudou
  if (state.analysisView === "overview") return renderCaseOverview(box, c);
  if (state.analysisView === "timeline") return renderTimeline(box, c);
  if (state.analysisView === "vtimeline") return renderVTimeline(box, c);
  if (state.analysisView === "timeline-table") return window.CaseTimeline.render(box, c, "table", caseTimelineCallbacks);
  if (state.analysisView === "data") return renderCaseData(box, c);
  renderCaseItems(box, c);
}

// ------------------------------------------------------------------ timeline
const ITEM_COLORS = ["#2f6fed", "#3fb950", "#d29922", "#bc8cff", "#ff7b72", "#58a6ff"];
const itemColor = (i) => ITEM_COLORS[i % ITEM_COLORS.length];

function fmtTick(ms, spanMs) {
  const d = new Date(ms);
  const p = (n) => String(n).padStart(2, "0");
  if (spanMs < 24 * 3600e3) return `${p(d.getHours())}:${p(d.getMinutes())}`;
  if (spanMs < 60 * 24 * 3600e3)
    return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
  return `${p(d.getDate())}/${p(d.getMonth() + 1)}/${String(d.getFullYear()).slice(2)}`;
}

const fmtDay = (ms) => {
  const d = new Date(ms);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getDate())}/${p(d.getMonth() + 1)}/${d.getFullYear()}`;
};
const fmtTime = (ms) => {
  const d = new Date(ms);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
};

let tlPop = null;
function closeTlPop() {
  if (tlPop) { tlPop.remove(); tlPop = null; }
}
function showBucketPop(x, y, evs) {
  closeTlPop();
  const pop = el("div", "tl-pop");
  for (const ev of evs.slice(0, 30)) {
    const b = el("button", "tl-pop-item");
    const ts = el("span", "ts", fmtTs(ev.timestamp));
    b.appendChild(ts);
    b.appendChild(document.createTextNode(trunc(ev.message || ev.name || `#${ev.code}`, 60)));
    b.onclick = () => { closeTlPop(); showDetail(ev); };
    pop.appendChild(b);
  }
  if (evs.length > 30) pop.appendChild(el("div", "muted small", `… e mais ${evs.length - 30}`));
  document.body.appendChild(pop);
  tlPop = pop;
  const r = pop.getBoundingClientRect();
  pop.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 8))}px`;
  pop.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 8))}px`;
}

function renderTimeline(box, c) {
  window.CaseTimeline.render(box, c, "horizontal", caseTimelineCallbacks);
}
// ------------------------------------------------------------------ timeline vertical
function renderVTimeline(box, c) {
  window.CaseTimeline.render(box, c, "vertical", caseTimelineCallbacks);
}

const caseTimelineCallbacks = {
  passes: rowPassesFilters,
  detail: showDetail,
  bucket: showBucketPop,
  menu: showCtxMenu,
  notify: message => toast(message, "info"),
  save: () => { saveCases(); updateAnalysisBadge(); renderAnalysis(); },
  createAt: timestamp => {
    const date = new Date(timestamp);
    $("#mf-start").value = new Date(timestamp - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
    $("#manual-form").hidden = false;
    $("#mf-name").focus();
  },
};
function saveManualEvent() {
  const name = $("#mf-name").value.trim();
  const start = $("#mf-start").value;
  if (!name || !start) { toast("Informe nome e início do evento.", "info"); return; }
  const c = ensureCase();
  c.manual = c.manual || [];
  c.manual.push({
    id: "m" + Date.now().toString(36),
    createdAt: Date.now(),
    name,
    description: $("#mf-desc").value.trim(),
    start: new Date(start).getTime(),
    end: $("#mf-end").value ? new Date($("#mf-end").value).getTime() : null,
  });
  saveCases();
  updateAnalysisBadge();
  renderAnalysis();
  $("#manual-form").hidden = true;
  $("#mf-name").value = "";
  $("#mf-desc").value = "";
  $("#mf-start").value = "";
  $("#mf-end").value = "";
}

// menu de contexto de uma célula de evento
function eventCellMenu(ev, col, value) {
  const hasVal = value !== undefined && value !== null && String(value).trim() !== "";
  const items = [
    { icon: "fa-eye", label: "Ver detalhes", onClick: () => openDetail(ev.id) },
    { icon: "fa-route", label: "Investigar possível trilha", onClick: () => openTrail(ev) },
  ];
  if (hasVal) {
    items.push({ sep: true });
    items.push({
      icon: "fa-filter",
      label: `Filtrar: ${colLabel(col)} = ${trunc(value)}`,
      onClick: () => addFilter({ column: col, op: "equals", value: String(value), value2: null }),
    });
    items.push({
      icon: "fa-filter-circle-xmark",
      label: `Excluir: ${colLabel(col)} ≠ ${trunc(value)}`,
      onClick: () => addFilter({ column: col, op: "not_equals", value: String(value), value2: null }),
    });
  }
  items.push({ sep: true });
  items.push({
    icon: "fa-microscope",
    label: "Enviar evento ao caso",
    onClick: () => addEventToAnalysis(ev.id),
  });
  if (hasVal) {
    items.push({
      icon: "fa-microscope",
      label: `Enviar todos com ${trunc(value)} ao caso`,
      onClick: () => addGroupToAnalysis(col, String(value)),
    });
    items.push({
      icon: "fa-microscope",
      label: `Enviar todos com ${colLabel(col)} preenchido ao caso`,
      onClick: () => addGroupToAnalysis(col, "", "", "not_empty"),
    });
  }
  if (hasVal) {
    items.push({ sep: true });
    items.push({
      icon: "fa-copy",
      label: "Copiar valor",
      onClick: () => { navigator.clipboard.writeText(String(value)); toast("Copiado.", "ok"); },
    });
  }
  items.push({ sep: true });
  items.push({
    icon: "fa-comment-dots",
    label: eventComment(ev) ? "Editar comentário" : "Adicionar comentário",
    onClick: () => openCommentModal(ev),
  });
  items.push({
    icon: "fa-briefcase",
    label: "Enviar todos visíveis ao caso",
    onClick: sendVisibleToCase,
  });
  const selectedList = (state.selectedEventRows?.has(ev.id) && state.selectedEventRows.size > 1)
    ? Array.from(state.selectedEventRows.values())
    : [ev];

  items.push({ sep: true });
  items.push({
    icon: "fa-route",
    label: selectedList.length > 1
      ? `Jogar ${selectedList.length} eventos para uma trilha...`
      : "Jogar evento para uma trilha...",
    onClick: () => openSendToTrailModal(selectedList),
  });

  if (workspaceScope() === "case") {
    const caseItems = items.filter(item => !["fa-microscope", "fa-briefcase"].includes(item.icon));
    caseItems.push({ sep: true });
    caseItems.push({
      icon: "fa-trash-can",
      label: "Remover este registro do Caso",
      danger: true,
      onClick: () => removeEventFromCase(ev),
    });
    return caseItems;
  }
  return items;
}

async function removeEventFromCase(ev) {
  const c = activeCase();
  if (!c) return;
  const key = caseRecordKey(ev, state.currentArtifact?.id, state.currentOrigin);
  let removed = false;
  for (let i = (c.items || []).length - 1; i >= 0; i--) {
    const item = c.items[i];
    if (item.rows && item.rows.length) {
      const matchIdx = item.rows.findIndex(r => caseRecordKey(r, item.artifactId, item.origin) === key || r.id === ev.id);
      if (matchIdx >= 0) {
        if (item.rows.length === 1) {
          c.items.splice(i, 1);
        } else {
          item.rows.splice(matchIdx, 1);
          item.includedCount = item.rows.length;
        }
        removed = true;
        break;
      }
    }
  }
  if (removed) {
    await saveCases();
    window.WorkspaceContext?.refreshMembership();
    filtersChanged();
    toast("Registro removido do Caso.", "ok");
  } else {
    toast("Registro não encontrado no Caso.", "info");
  }
}

function openSendToTrailModal(events) {
  if (!events || !events.length) return;
  const c = ensureCase();
  if (!c) return;
  const existingTrails = Array.isArray(c.caseTrails) ? c.caseTrails : [];

  let overlay = document.querySelector("#send-trail-modal");
  if (overlay) overlay.remove();

  overlay = el("div", "modal-overlay");
  overlay.id = "send-trail-modal";
  const modal = el("section", "modal");
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-label", "Jogar para trilha");

  const head = el("header", "modal-head");
  head.innerHTML = `<div><h3>Jogar para trilha</h3><p>${fmtNum(events.length)} evento(s) selecionado(s)</p></div><button class="icon-btn" type="button" id="stm-close" aria-label="Fechar"><i class="fas fa-xmark"></i></button>`;

  const body = el("div", "modal-body");
  const optionsWrap = el("div", "trail-pick-options");
  optionsWrap.style.display = "flex";
  optionsWrap.style.flexDirection = "column";
  optionsWrap.style.gap = "12px";

  let selectExistingHtml = "";
  if (existingTrails.length > 0) {
    selectExistingHtml = `
      <label class="radio-label" style="display:flex;align-items:center;gap:8px;cursor:pointer;">
        <input type="radio" name="stm-mode" value="existing" checked>
        <strong>Trilha existente:</strong>
      </label>
      <select id="stm-existing-select" class="form-select" style="margin-left:24px;width:calc(100% - 24px);padding:6px 10px;border-radius:4px;border:1px solid var(--border);">
        ${existingTrails.map(t => `<option value="${t.id}">${esc(t.title || "Trilha sem título")} (${fmtNum(t.itemIds?.length || 0)} itens)</option>`).join("")}
      </select>
    `;
  }

  const defaultTitle = events.length === 1
    ? (events[0].message ? events[0].message.slice(0, 60) : `Evento ${events[0].id}`)
    : `Trilha · ${events.length} eventos (${fmtTs(events[0].timestamp || Date.now())})`;

  const newTrailHtml = `
    <label class="radio-label" style="display:flex;align-items:center;gap:8px;cursor:pointer;margin-top:${existingTrails.length ? '8px' : '0'};">
      <input type="radio" name="stm-mode" value="new" ${existingTrails.length === 0 ? "checked" : ""}>
      <strong>Nova trilha:</strong>
    </label>
    <div style="margin-left:24px;width:calc(100% - 24px);">
      <input type="text" id="stm-new-title" class="form-input" style="width:100%;padding:6px 10px;border-radius:4px;border:1px solid var(--border);" placeholder="Título da nova trilha" value="${esc(defaultTitle)}">
    </div>
  `;

  optionsWrap.innerHTML = selectExistingHtml + newTrailHtml;
  body.appendChild(optionsWrap);

  const footer = el("div", "modal-actions");
  footer.style.display = "flex";
  footer.style.justifyContent = "flex-end";
  footer.style.gap = "8px";
  footer.style.marginTop = "16px";
  footer.innerHTML = `
    <button class="btn ghost small" id="stm-cancel" type="button">Cancelar</button>
    <button class="btn primary small" id="stm-confirm" type="button"><i class="fas fa-check"></i> Confirmar</button>
  `;

  modal.append(head, body, footer);
  overlay.appendChild(modal);
  document.body.appendChild(overlay);

  const close = () => { overlay.remove(); };
  overlay.querySelector("#stm-close").onclick = close;
  overlay.querySelector("#stm-cancel").onclick = close;
  overlay.onclick = (e) => { if (e.target === overlay) close(); };

  const confirmBtn = overlay.querySelector("#stm-confirm");
  confirmBtn.onclick = async () => {
    confirmBtn.disabled = true;
    try {
      const mode = overlay.querySelector('input[name="stm-mode"]:checked')?.value || "new";
      const artifact = registerCurrentArtifact();
      const groupItem = {
        ...caseItemBase("grupo", artifact?.stationId || null, events.length, events.length),
        label: events.length === 1 ? (events[0].message?.slice(0, 100) || "Evento") : `${events.length} eventos da exploração`,
        rows: structuredClone(events),
        sourceFilters: structuredClone(state.filters),
        sourceSpec: structuredClone(state.currentArtifact?.source),
        summary: `Eventos enviados da exploração (${events.length} registros)`,
        details: "",
        attachments: []
      };
      c.items = c.items || [];
      c.items.push(groupItem);

      let trailTitle = "";
      if (mode === "existing") {
        const selId = overlay.querySelector("#stm-existing-select")?.value;
        const trail = c.caseTrails?.find(t => t.id === selId);
        if (!trail) throw Error("Trilha não encontrada.");
        trail.itemIds = trail.itemIds || [];
        if (!trail.itemIds.includes(groupItem.id)) trail.itemIds.push(groupItem.id);
        trail.updatedAt = Date.now();
        trailTitle = trail.title || "Trilha";
      } else {
        const titleInput = overlay.querySelector("#stm-new-title")?.value?.trim() || defaultTitle;
        const newTrail = {
          id: `ct-${nid()}`,
          title: titleInput,
          summary: `Criada com ${events.length} evento(s) da exploração`,
          details: "",
          attachments: [],
          itemIds: [groupItem.id],
          createdAt: Date.now(),
          updatedAt: Date.now(),
        };
        c.caseTrails = c.caseTrails || [];
        c.caseTrails.push(newTrail);
        trailTitle = newTrail.title;
      }

      await saveCases();
      updateAnalysisBadge();
      close();
      toast(`${fmtNum(events.length)} evento(s) adicionado(s) à trilha "${trailTitle}".`, "ok");
    } catch (err) {
      toast(String(err.message || err), "err");
      confirmBtn.disabled = false;
    }
  };
}

function toggleRowSelect(ev, forceState = null) {
  state.selectedEventRows = state.selectedEventRows || new Map();
  const next = forceState !== null ? forceState : !state.selectedEventRows.has(ev.id);
  if (next) {
    state.selectedEventRows.set(ev.id, ev);
    state.lastSelectedRowId = ev.id;
  } else {
    state.selectedEventRows.delete(ev.id);
  }
  updateRowSelectionStyles();
}

function selectRowRange(targetId, keepExisting = false) {
  if (!keepExisting) {
    state.selectedEventRows = new Map();
  } else {
    state.selectedEventRows = state.selectedEventRows || new Map();
  }
  const ids = (state.rows || []).map(r => r.id);
  const idxA = ids.indexOf(state.lastSelectedRowId);
  const idxB = ids.indexOf(targetId);
  if (idxA >= 0 && idxB >= 0) {
    const min = Math.min(idxA, idxB);
    const max = Math.max(idxA, idxB);
    for (let i = min; i <= max; i++) {
      state.selectedEventRows.set(state.rows[i].id, state.rows[i]);
    }
  } else {
    const targetEv = state.rows?.find(r => r.id === targetId);
    if (targetEv) state.selectedEventRows.set(targetEv.id, targetEv);
    state.lastSelectedRowId = targetId;
  }
  updateRowSelectionStyles();
}

function updateRowSelectionStyles() {
  const tbody = $("#events-table tbody");
  if (!tbody) return;
  tbody.querySelectorAll("tr").forEach(tr => {
    const id = Number(tr.dataset.eventId);
    const selected = state.selectedEventRows?.has(id);
    tr.classList.toggle("row-multi-selected", !!selected);
  });
}

// envia a página visível ao Caso, sem duplicar o que já está lá
function sendVisibleToCase() {
  if (workspaceScope() === "case") { toast("Esses registros já pertencem ao Caso.", "info"); return; }
  const c = ensureCase();
  if (!c) return;
  const existing = new Set();
  for (const item of c.items || []) {
    for (const r of item.rows || []) {
      existing.add(caseRecordKey(r, item.artifactId, item.origin));
    }
  }
  const rows = state.rows.filter((ev) => !existing.has(caseRecordKey(ev, state.currentArtifact?.id, state.currentOrigin)));
  if (!rows.length) { toast("Todos os eventos visíveis já estão no Caso.", "info"); return; }
  openCaseAdd({ kind: "visible", rows });
}

// ------------------------------------------------------------------ tabela
function buildEventRow(ev) {
  const quick = state.quick.trim();
  const quickRe = quick ? new RegExp(`(${escRe(esc(quick))})`, "gi") : null;
  const row = el("tr");
  row.dataset.eventId = ev.id;
  if (workspaceScope() === "dataset" && window.WorkspaceContext?.isIncluded(ev)) { row.classList.add("event-in-case"); row.title = "Este registro já está no Caso"; }
  if (ev.id === state.detailId) row.classList.add("selected");
  if (state.selectedEventRows?.has(ev.id)) row.classList.add("row-multi-selected");

  row.onclick = (e) => {
    if (e.target.closest("input, button, a")) return;
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      toggleRowSelect(ev);
    } else if (e.shiftKey && state.lastSelectedRowId != null) {
      e.preventDefault();
      selectRowRange(ev.id);
    } else {
      state.selectedEventRows = new Map([[ev.id, ev]]);
      state.lastSelectedRowId = ev.id;
      updateRowSelectionStyles();
      openDetail(ev.id);
    }
  };

  for (const col of state.visibleCols) {
    const td = el("td");
    if (col === "level") {
      const wrap = el("span", "lv-cell");
      const dot = el("span", "lv-dot");
      dot.style.background = levelColor(ev.level);
      dot.style.color = levelColor(ev.level);
      if (ev.level === "Crítico") dot.classList.add("pulse");
      wrap.append(dot, el("span", "", ev.level));
      td.appendChild(wrap);
    } else if (col === "timestamp") {
      td.className = "t-mono t-ts";
      td.textContent = cellValue(ev, col);
    } else if (col === "code") {
      td.className = "t-code";
      td.textContent = cellValue(ev, col);
    } else if (col === "name") {
      td.className = "t-name";
      td.textContent = cellValue(ev, col);
    } else if (col === "message") {
      td.className = "t-msg";
      if (quickRe) td.innerHTML = esc(ev.message).replace(quickRe, "<mark>$1</mark>");
      else td.textContent = ev.message;
    } else {
      td.textContent = cellValue(ev, col);
    }
    td.title = col === "level" ? ev.level : cellValue(ev, col);
    td.oncontextmenu = (e) => {
      e.preventDefault();
      if (!state.selectedEventRows?.has(ev.id)) {
        state.selectedEventRows = new Map([[ev.id, ev]]);
        state.lastSelectedRowId = ev.id;
        updateRowSelectionStyles();
      }
      const value = col === "level" ? ev.level : cellValue(ev, col);
      showCtxMenu(e.clientX, e.clientY, eventCellMenu(ev, col, value));
    };
    row.appendChild(td);
  }
  return row;
}

function renderTable(qr) {
  $("#empty-state [data-retry]")?.remove();
  const thead = $("#events-table thead");
  const tbody = $("#events-table tbody");
  thead.innerHTML = "";
  tbody.innerHTML = "";

  // colgroup com as larguras salvas (redimensionamento manual)
  const table = $("#events-table");
  table.querySelector("colgroup")?.remove();
  const colgroup = document.createElement("colgroup");
  for (const col of state.visibleCols) {
    const colEl = document.createElement("col");
    if (state.colWidths[col]) colEl.style.width = `${state.colWidths[col]}px`;
    colgroup.appendChild(colEl);
  }
  table.prepend(colgroup);

  const tr = el("tr");
  let lastColumnDropAt = 0;
  for (const col of state.visibleCols) {
    const th = el("th", "", colLabel(col));
    // arrastar para reordenar as colunas exibidas
    th.draggable = true;
    th.ondragstart = (e) => { e.dataTransfer.setData("text/col", col); th.classList.add("dragging"); };
    th.ondragend = () => th.classList.remove("dragging");
    th.ondragover = (e) => { e.preventDefault(); th.classList.add("drag-over"); };
    th.ondragleave = () => th.classList.remove("drag-over");
    th.ondrop = (e) => {
      e.preventDefault();
      th.classList.remove("drag-over");
      const from = e.dataTransfer.getData("text/col");
      if (!from || from === col) return;
      const cols = state.visibleCols.filter((x) => x !== from);
      cols.splice(cols.indexOf(col), 0, from); // solta antes da coluna alvo
      state.visibleCols = cols;
      lastColumnDropAt = Date.now(); // impede o clique residual de reordenar a tabela
      saveVisibleCols();
      renderTable({ total: state.total, rows: state.rows });
    };
    th.title = "Clique para ordenar. Arraste para reordenar. Botão direito: adicionar ao Cubo.";
    th.onclick = () => {
      if (Date.now() - lastColumnDropAt < 400) return;
      if (state.sortCol === col) state.sortDir = state.sortDir === "asc" ? "desc" : "asc";
      else { state.sortCol = col; state.sortDir = col === "timestamp" ? "desc" : "asc"; }
      state.page = 0;
      refresh();
    };
    th.oncontextmenu = (event) => {
      event.preventDefault();
      showCtxMenu(event.clientX, event.clientY, [
        {
          icon: "fa-arrow-down",
          label: "Adicionar ao Cubo em Linhas",
          onClick: () => { cubeAdd("rows", col); switchTab("cube"); },
        },
        {
          icon: "fa-arrow-right",
          label: "Adicionar ao Cubo em Colunas",
          onClick: () => { cubeAdd("cols", col); switchTab("cube"); },
        },
        {
          icon: "fa-sigma",
          label: "Adicionar ao Cubo em Valores",
          onClick: () => { cubeAdd("values", col); switchTab("cube"); },
        },
      ]);
    };
    if (state.sortCol === col) {
      th.appendChild(el("span", "sort-arrow", state.sortDir === "asc" ? "▲" : "▼"));
    }
    if (col === "timestamp" && workspaceScope() === "dataset") {
      const cfg = el("button", "icon-btn th-cfg");
      cfg.innerHTML = '<i class="fas fa-clock"></i>';
      cfg.title = "Configurar data/hora do artefato";
      cfg.onclick = (e) => { e.stopPropagation(); openTsModal(); };
      th.appendChild(cfg);
    }
    // alça de redimensionamento da coluna (arrastar com o mouse)
    const grip = el("span", "col-grip");
    grip.title = "Arraste para redimensionar";
    grip.onmousedown = (e) => {
      e.preventDefault();
      e.stopPropagation();
      const startX = e.clientX;
      const startW = th.getBoundingClientRect().width;
      const onMove = (ev) => {
        const w = Math.max(48, Math.round(startW + (ev.clientX - startX)));
        th.style.width = `${w}px`;
        colgroup.children[state.visibleCols.indexOf(col)].style.width = `${w}px`;
      };
      const onUp = (ev) => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        const w = Math.max(48, Math.round(startW + (ev.clientX - startX)));
        state.colWidths[col] = w;
        saveVisibleCols();
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    };
    th.appendChild(grip);
    tr.appendChild(th);
  }
  thead.appendChild(tr);

  for (const ev of qr.rows) {
    tbody.appendChild(buildEventRow(ev));
  }

  const empty = $("#empty-state");
  empty.hidden = qr.rows.length > 0;
  empty.querySelector("p").textContent = state.loaded || workspaceScope() === "case"
    ? "Nenhum evento encontrado."
    : "Selecione uma fonte de dados.";

  const pages = Math.max(1, Math.ceil(qr.total / state.pageSize));
  $("#pg-label").textContent = `${state.page + 1} / ${pages}`;
  $("#pg-prev").disabled = state.page === 0;
  $("#pg-next").disabled = state.page >= pages - 1;
  const from = qr.rows.length ? state.page * state.pageSize + 1 : 0;
  const to = state.page * state.pageSize + qr.rows.length;
  $("#result-count").textContent = state.loaded ? `${fmtNum(from)}–${fmtNum(to)} de ${fmtNum(qr.total)}` : "";
}

// ------------------------------------------------------------------ histograma
function renderChart(stats) {
  const box = $("#chart");
  const panel = box.closest(".hist-panel");
  panel.hidden = !state.loaded;


  if (!stats.buckets || stats.buckets.length === 0) {
    if (chart) { chart.destroy(); chart = null; }
    box.innerHTML = "";
    const p = el("div", "hint-empty", "Sem dados temporais.");
    box.appendChild(p);
    return;
  }

  const xs = stats.buckets.map(([t]) => t / 1000);
  const ys = stats.buckets.map(([, c]) => c);
  const axisColor = isLight() ? "#5b6678" : "#6b7690";
  const gridColor = isLight() ? "rgba(19,81,180,0.08)" : "rgba(255,255,255,0.06)";

  if (chart) {
    chart.setData([xs, ys]);
    const width = Math.max(280, box.clientWidth - 4);
    if (Math.abs(chart.width - width) > 2) chart.setSize({ width, height: 96 });
    return;
  }
  box.innerHTML = "";

  chart = new uPlot(
    {
      width: Math.max(280, box.clientWidth - 4),
      height: 96,
      legend: { show: false },
      cursor: { drag: { x: true, y: false, setScale: false }, focus: { prox: 24 } },
      scales: { x: { time: true } },
      axes: [
        { stroke: axisColor, grid: { show: false }, ticks: { show: false }, size: 20, font: "10px " + "sans-serif", values: (u, vals) => vals.map(v => new Date(v * 1000).toLocaleTimeString("pt-BR", {hour:"2-digit",minute:"2-digit"})) },
        { stroke: axisColor, grid: { stroke: gridColor }, ticks: { show: false }, size: 30 },
      ],
      series: [
        {},
        { stroke: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim(), width: 1.5, fill: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() + "1a", points: { show: false } },
      ],
      hooks: {
        setSelect: [
          (u) => {
            if (u.select.width < 8) return;
            const t0 = Math.round(u.posToVal(u.select.left, "x") * 1000);
            const t1 = Math.round(u.posToVal(u.select.left + u.select.width, "x") * 1000);
            u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
            // substitui um filtro de tempo anterior vindo do gráfico
            state.filters = state.filters.filter((f) => !(f.column === "timestamp" && f.op === "between"));
            addFilter({ column: "timestamp", op: "between", value: String(t0), value2: String(t1) });
          },
        ],
      },
    },
    [xs, ys],
    box
  );
}

// ------------------------------------------------------------------ colunas
function fillColumnControls() {
  const g = $("#group-col");
  g.innerHTML = "";
  for (const c of state.columns) g.appendChild(el("option", "", colLabel(c))).value = c;
  g.value = state.columns.includes(state.groupCol) ? state.groupCol : state.columns[0];
  renderAggs();
}

function openTsModal(path = null) {
  state.tsEditingPath = typeof path === "string" ? path : null;
  if (!document.querySelector("#ts-rules .dv-rule")) tsAddRule();
  updateTsExample();
  $("#ts-modal").hidden = false;
}

function openColPop() {
  const pop = $("#col-pop");
  const list = $("#col-list");
  list.innerHTML = "";
  for (const col of state.columns) {
    const item = el("label", "col-item");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = state.visibleCols.includes(col);
    if (col === "timestamp") {
      cb.checked = true;
      cb.disabled = true;
      item.title = "Data/hora é sempre a primeira coluna";
    }
    cb.onchange = () => {
      const set = new Set(state.visibleCols);
      cb.checked ? set.add(col) : set.delete(col);
      state.visibleCols = state.columns.filter((c) => set.has(c));
      saveVisibleCols();
      // as linhas da página já estão no cliente: re-render local basta
      renderTable({ total: state.total, rows: state.rows });
    };
    item.append(cb, el("span", "", colLabel(col)));
    list.appendChild(item);
  }
  pop.hidden = false;
  positionPop(pop, $("#btn-colpicker"));
}

// ------------------------------------------------------------------ drawer
function currentIndex() {
  return state.rows.findIndex((r) => r.id === state.detailId);
}

let detailRequest = 0;
async function openDetail(id) {
  const request = ++detailRequest;
  showDetailLoading();
  try {
    const ev = workspaceScope() === "case" ? caseEvents().find(event => event.id === id) : await api("event_detail", { id });
    if (request === detailRequest && ev) showDetail(ev);
  } catch (error) {
    if (request === detailRequest) $("#pane-overview").textContent = `Não foi possível abrir o registro: ${String(error)}`;
  }
}

// abre o drawer imediatamente com estado de espera (o conteúdo chega via event_detail)
function showDetailLoading() {
  state.detailId = null; state.currentDetailEv = null; state.detailSourceSpec = null;
  const actions = $("#drawer .detail-quick-actions"); if (actions) actions.hidden = true;
  for (const id of ["dr-prev", "dr-next", "dr-copy"]) $("#" + id).hidden = true;
  $("#drawer-badges").innerHTML = "";
  const wait = el("div", "loading-inline drawer-loading");
  wait.innerHTML = '<i class="fas fa-circle-notch spin"></i> Carregando…';
  $("#pane-overview").innerHTML = "";
  $("#pane-overview").appendChild(wait);
  $("#pane-json").hidden = true;
  $("#pane-raw").hidden = true;
  $("#drawer").hidden = false;
  $("#drawer-scrim").hidden = false;
  $("#btn-right-inspect").classList.add("active");
  switchDetailTab("overview");
}

function openContextInspector(title, subtitle, overview) {
  detailRequest++;
  state.detailId = null;
  state.currentDetailEv = null;
  state.detailSourceSpec = null;
  const actions = $("#drawer .detail-quick-actions"); if (actions) actions.hidden = true;
  $("#drawer-badges").innerHTML = "";
  $("#drawer-badges").append(el("span", "badge code", title));
  if (subtitle) $("#drawer-badges").append(el("span", "badge", subtitle));
  $("#pane-overview").innerHTML = "";
  $("#pane-overview").appendChild(overview);
  $("#pane-json").hidden = true;
  $("#pane-raw").hidden = true;
  document.querySelectorAll("#drawer .dtab").forEach((tab) => {
    tab.hidden = tab.dataset.pane !== "overview";
    tab.classList.toggle("active", tab.dataset.pane === "overview");
  });
  $("#dr-prev").hidden = true;
  $("#dr-next").hidden = true;
  $("#dr-copy").hidden = true;
  $("#drawer").hidden = false;
  $("#drawer-scrim").hidden = false;
  $("#btn-right-inspect").classList.add("active");
}

function showFieldInspector(column) {
  const overview = el("div", "kv");
  const inCase = state.activeContext !== "artifact";
  const pool = inCase ? caseEvents() : state.rows;
  const values = pool.map((event) => cellValue(event, column)).filter(Boolean);
  const counts = new Map();
  for (const value of values) counts.set(value, (counts.get(value) || 0) + 1);
  const rows = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12);
  const description = el("p", "muted small", inCase
    ? `${fmtNum(values.length)} valores observados no conjunto do Caso.`
    : `${fmtNum(values.length)} valores observados na página atual.`);
  overview.appendChild(description);
  const include = el("button", "btn ghost small", "Filtrar valores deste campo");
  include.onclick = () => { openFilterPop(); $("#fp-col").value = column; };
  overview.appendChild(include);
  for (const [value, count] of rows) {
    const row = el("button", "kv-row");
    row.append(el("span", "kv-v mono", value), el("span", "fc", fmtNum(count)));
    row.onclick = () => { addFilter({ column, op: "equals", value, value2: null }); closeDrawer(); };
    overview.appendChild(row);
  }
  openContextInspector("Campo", colLabel(column), overview);
}

function showStationInspector(station) {
  const overview = el("div", "kv");
  const entries = [
    ["Host / IP", station.host || "Não informado"],
    ["Artefatos", fmtNum(stationArtifacts(station.id).length)],
    ["Itens técnicos", fmtNum(stationItems(station.id).length)],
    ["Notas", station.notes || "Nenhuma nota"],
  ];
  for (const [key, value] of entries) {
    const row = el("div", "kv-row");
    row.append(el("span", "kv-k", key), el("span", "kv-v", value));
    overview.appendChild(row);
  }
  const open = el("button", "btn primary small", "Abrir estação");
  open.onclick = () => { closeDrawer(); openStation(station.id); };
  overview.appendChild(open);
  openContextInspector("Estação", station.name, overview);
}

function showDetail(ev, sourceSpec = null) {
  detailRequest++;
  state.detailId = ev.id;
  state.currentDetailEv = ev;
  state.detailSourceSpec = sourceSpec;
  const actions = $("#drawer .detail-quick-actions"); if (actions) actions.hidden = false;
  const follow = $("#ws-detail-follow"), context = $("#ws-detail-context");
  if (follow) follow.hidden = !["trace_id", "trace.id", "request_id", "requestId", "correlation_id", "session_id"].some(key => ev.fields?.[key]);
  if (context) context.hidden = ev.timestamp == null;

  $("#dr-prev").hidden = !!sourceSpec;
  $("#dr-next").hidden = !!sourceSpec;
  $("#dr-copy").hidden = false;
  document.querySelectorAll("#drawer .dtab").forEach((tab) => { tab.hidden = false; });

  $("#drawer-badges").innerHTML = "";
  const lv = el("span", "badge");
  lv.style.background = "color-mix(in srgb, " + levelColor(ev.level) + " 18%, transparent)";
  lv.style.color = levelColor(ev.level);
  lv.textContent = ev.level;
  const badges = $("#drawer-badges");
  badges.appendChild(lv);
  if (ev.code) badges.appendChild(el("span", "badge code", `#${ev.code}`));
  if (ev.name) badges.appendChild(el("span", "badge code", ev.name));

  const rows = [];
  const push = (k, v, mono, filterValue = v) => rows.push({ k, v: v ?? "", mono, filterValue });
  push("timestamp", fmtTsFull(ev.timestamp), true, ev.timestamp);
  push("source", ev.source);
  push("level", ev.level);
  push("code", ev.code, true);
  push("name", ev.name);
  push("description", ev.description);
  push("message", ev.message, true);
  const fieldEntries = Object.entries(ev.fields || {}).sort(([a], [b]) => a.localeCompare(b));
  for (const [k, v] of fieldEntries) {
    push(k, typeof v === "object" ? JSON.stringify(v) : String(v), true);
  }

  const allFieldKeys = new Set(Object.keys(ev.fields || {}));
  const kv = el("div", "kv");
  for (const r of rows) {
    const row = el("div", "kv-row");
    const colKey = r.k;
    row.dataset.col = colKey;

    const lastDot = colKey.lastIndexOf(".");
    const parentKey = lastDot > 0 ? colKey.slice(0, lastDot) : null;
    const isChild = parentKey && (allFieldKeys.has(parentKey) || rows.some(other => other.k === parentKey));

    if (isChild) {
      row.classList.add("kv-row-child");
      const kDiv = el("div", "kv-k");
      const lastSegment = colKey.slice(lastDot + 1);
      kDiv.innerHTML = `<span class="kv-tree-guide">└─</span> <span class="kv-child-name">${esc(lastSegment)}</span>`;
      kDiv.title = `${colLabel(colKey)} (filho de ${colLabel(parentKey)})`;
      row.appendChild(kDiv);
    } else {
      row.appendChild(el("div", "kv-k", colLabel(colKey)));
    }

    const v = el("div", `kv-v${r.mono ? " mono" : ""}`, String(r.v));
    row.appendChild(v);
    if (!["raw"].includes(colKey) && r.filterValue != null && String(r.filterValue).trim() !== "") {
      const f = el("button", "kv-filter");
      f.innerHTML = '<i class="fas fa-filter"></i>';
      f.title = `Filtrar: ${colLabel(colKey)} = ${String(r.v).slice(0, 40)}`;
      f.onclick = (e) => {
        e.stopPropagation();
        addFilter({ column: colKey, op: colKey === "timestamp" ? "between" : "equals_exact", value: String(r.filterValue), value2: colKey === "timestamp" ? String(r.filterValue) : null });
        toast("Filtro adicionado.", "ok");
      };
      row.appendChild(f);
    }
    kv.appendChild(row);
  }
  $("#pane-overview").innerHTML = "";
  $("#pane-overview").appendChild(kv);
  $("#pane-json").innerHTML = highlightJson(ev);
  $("#pane-raw").textContent = ev.raw || "(sem conteúdo bruto)";

  $("#drawer").hidden = false;
  $("#drawer-scrim").hidden = false;
  $("#btn-right-inspect").classList.add("active");
  switchDetailTab("overview");
  updateDetailNav();

  // marca a linha selecionada na tabela
  document.querySelectorAll("#events-table tbody tr").forEach((tr) => tr.classList.remove("selected"));
  const idx = currentIndex();
  const tr = $("#events-table tbody").children[idx];
  if (tr) tr.classList.add("selected");
}

function updateDetailNav() {
  const i = currentIndex();
  $("#dr-prev").disabled = i <= 0;
  $("#dr-next").disabled = i === -1 || i >= state.rows.length - 1;
}

function detailStep(dir) {
  const i = currentIndex();
  const n = i + dir;
  if (n >= 0 && n < state.rows.length) openDetail(state.rows[n].id);
}

function closeDrawer() {
  detailRequest++;
  state.detailId = null;
  $("#drawer").hidden = true;
  $("#drawer-scrim").hidden = true;
  $("#btn-right-inspect").classList.remove("active");
  document.querySelectorAll("#events-table tbody tr").forEach((tr) => tr.classList.remove("selected"));
}

function switchDetailTab(which) {
  document.querySelectorAll("#drawer .dtab").forEach((t) =>
    t.classList.toggle("active", t.dataset.pane === which)
  );
  for (const pane of ["overview", "json", "raw"]) {
    $(`#pane-${pane}`).hidden = pane !== which;
  }
}

function highlightJson(obj) {
  return esc(JSON.stringify(obj, null, 2)).replace(
    /("(\\u[a-f0-9]{4}|\\[^u]|[^\\"])*")(\s*:)?|\b(true|false|null)\b|-?\d+(\.\d+)?([eE][+-]?\d+)?/g,
    (m) => {
      let cls = "j-num";
      if (m.startsWith('"')) cls = m.endsWith(":") ? "j-key" : "j-str";
      else if (/true|false|null/.test(m)) cls = "j-bool";
      return `<span class="${cls}">${m}</span>`;
    }
  );
}

// ------------------------------------------------------------------ agrupamento
function renderAggs() {
  const box = $("#agg-list");
  box.innerHTML = "";
  state.aggs.forEach((agg, i) => {
    const row = el("div", "agg-row");

    const selFunc = el("select");
    for (const [v, l] of AGG_FUNCS) selFunc.appendChild(el("option", "", l)).value = v;
    selFunc.value = agg.func;
    selFunc.onchange = () => { agg.func = selFunc.value; };

    const selCol = el("select");
    selCol.appendChild(el("option", "", "(evento)")).value = "*";
    for (const c of state.columns) selCol.appendChild(el("option", "", colLabel(c))).value = c;
    selCol.value = agg.column;
    selCol.onchange = () => { agg.column = selCol.value; };

    const alias = document.createElement("input");
    alias.type = "text";
    alias.placeholder = "apelido";
    alias.value = agg.alias;
    alias.oninput = () => { agg.alias = alias.value; };

    const del = el("button", "icon-btn");
    del.innerHTML = '<i class="fas fa-xmark"></i>';
    del.title = "Remover agregação";
    del.onclick = () => { state.aggs.splice(i, 1); renderAggs(); };

    row.append(selFunc, selCol, alias, del);
    box.appendChild(row);
  });
}

async function runGroup() {
  if (!state.loaded) { toast("Carregue uma fonte de dados primeiro.", "info"); return; }
  startOperation("group", "Calculando agrupamento", `Agrupando por ${colLabel(state.groupCol)}`);
  const loading = areaLoading($("#tab-group"), "Agrupando…");
  let res;
  try {
    res = await api("aggregate_events", {
      groupColumn: state.groupCol,
      aggs: state.aggs,
      filters: backendFilters(),
    });
  } catch (error) {
    loading.done();
    finishOperation("Falha ao agrupar", String(error));
    return;
  }
  loading.done();

  const thead = $("#group-table thead");
  const tbody = $("#group-table tbody");
  thead.innerHTML = "";
  tbody.innerHTML = "";

  const tr = el("tr");
  for (const c of res.columns) tr.appendChild(el("th", "", colLabel(c)));
  thead.appendChild(tr);

  // barra proporcional para a primeira agregação "count"
  const barCol = state.aggs.find((a) => a.func === "count");
  const barAlias = barCol ? (barCol.alias || `count(${barCol.column})`) : null;
  const maxN = barAlias ? Math.max(1, ...res.rows.map((r) => Number(r[barAlias]) || 0)) : 1;

  for (const row of res.rows) {
    const tr2 = el("tr");
    const key = row[state.groupCol];
    tr2.title = "Clique para filtrar este grupo";
    tr2.onclick = () => drillDown(key);
    tr2.oncontextmenu = (e) => {
      e.preventDefault();
      showCtxMenu(e.clientX, e.clientY, [
        { icon: "fa-filter", label: `Filtrar: ${colLabel(state.groupCol)} = ${trunc(key)}`, onClick: () => drillDown(key) },
        {
          icon: "fa-filter-circle-xmark",
          label: `Excluir: ${colLabel(state.groupCol)} ≠ ${trunc(key)}`,
          onClick: () => {
            state.filters.push({ column: state.groupCol, op: "not_equals", value: String(key), value2: null });
            state.page = 0;
            renderChips();
            switchTab("table");
            refresh();
          },
        },
        { sep: true },
        { icon: "fa-microscope", label: "Enviar grupo ao caso…", onClick: () => openNamePop(tr2, (name) => addGroupToAnalysis(state.groupCol, String(key), name)) },
      ]);
    };
    for (const c of res.columns) {
      const td = el("td");
      const v = row[c];
      if (c === barAlias && v !== null && v !== undefined) {
        const wrap = el("span", "agg-bar-wrap");
        const bar = el("span", "agg-bar");
        bar.style.width = `${Math.max(2, (Number(v) / maxN) * 120)}px`;
        wrap.append(bar, el("span", "", fmtNum(v)));
        td.appendChild(wrap);
      } else if (c === state.groupCol && c === "level") {
        const wrap = el("span", "lv-cell");
        const dot = el("span", "lv-dot");
        dot.style.background = levelColor(String(v));
        wrap.append(dot, el("span", "", String(v)));
        td.appendChild(wrap);
      } else {
        td.textContent = v === null || v === undefined ? "" : String(v);
      }
      tr2.appendChild(td);
    }
    tbody.appendChild(tr2);
  }
  if (res.rows.length === 0) {
    const tr2 = el("tr");
    const td = el("td", "muted", "Nenhum grupo encontrado.");
    td.colSpan = res.columns.length || 1;
    tr2.appendChild(td);
    tbody.appendChild(tr2);
  }
  finishOperation("Agrupamento atualizado", `${fmtNum(res.rows.length)} grupos calculados`);
}

function drillDown(key) {
  const f = key === "(vazio)"
    ? { column: state.groupCol, op: "empty", value: "", value2: null }
    : { column: state.groupCol, op: "equals", value: String(key), value2: null };
  state.filters.push(f);
  state.page = 0;
  renderChips();
  switchTab("table");
  refresh();
}

// ------------------------------------------------------------------ tabs
function placeAnalytics(scope) {
  const target = scope === "dataset" ? document.querySelector(".content") : document.body;
  for (const view of [$("#view-dashboard"), $("#view-cube")]) {
    if (view.parentElement !== target) target.appendChild(view);
  }
}

function placeSourcePanel() {
  const panel = $("#source-panel");
  const target = $("#artifact-source-host");
  if (!panel || !target || panel.parentElement === target) return;
  target.appendChild(panel);
}

function switchTab(which, { deferAnalytics = false } = {}) {
  placeAnalytics("dataset");
  const scope = workspaceScope(); state.analyticsScope = scope;
  state.activeDatasetTab = which;
  for (const tab of ["table", "group", "dashboard", "cube"]) {
    $(`#tabbtn-${tab}`).classList.toggle("active", which === tab);
  }
  $("#tab-table").hidden = which !== "table";
  $("#tab-group").hidden = which !== "group";
  $("#view-dashboard").hidden = which !== "dashboard";
  $("#view-cube").hidden = which !== "cube";
  $("#view-dashboard").classList.toggle("in-workspace", which === "dashboard");
  $("#view-cube").classList.toggle("in-workspace", which === "cube");
  if (!deferAnalytics && which === "group") runGroup();
  if (!deferAnalytics && which === "dashboard") openDashboard(scope);
  if (!deferAnalytics && which === "cube") openCube(scope);
}

// troca entre as telas "Visualização", "Caso" e "Estações"
// lista de arquivos (drive) x formulário de carga, na tela de fonte
function showSourceMode(mode) {
  const has = (artifactSessionFor()?.artifacts.size || 0) > 0;
  const load = mode === "load" || !has;
  $("#source-load-card").hidden = !load;
  $("#drive-card").hidden = load;
  $("#btn-source-back").hidden = !has;
}

function switchView(which, { deferAnalytics = false } = {}) {
  if (window.WorkspaceContext && !window.WorkspaceContext.changing && ["caso", "case-dashboard", "case-cube", "estacoes"].includes(which) && workspaceScope() !== "case") {
    return window.WorkspaceContext.setScope("case", { page: which === "caso" && ["timeline", "vtimeline", "timeline-table"].includes(state.analysisView) ? "case-timeline" : which === "case-dashboard" || which === "case-cube" ? "explore" : "evidence", tab: which === "case-cube" ? "cube" : which === "case-dashboard" ? "dashboard" : undefined });
  }
  window.Workspace?.onView(which);
  if (which === "source") showSourceMode("list");
  state.activeContext = workspaceScope() === "case" ? "case" : which === "estacoes" ? "station"
    : ((which === "viz" || which === "source" || which === "trail" || which === "workspace") ? "artifact" : "case");
  if (["caso", "estacoes", "case-dashboard", "case-cube", "trail"].includes(which)) {
    saveCaseWorkspace(which);
  }
  placeSourcePanel();
  document.querySelector(".shell").hidden = which !== "viz";
  $("#view-artifact-source").hidden = which !== "source";
  $("#view-analysis").hidden = which !== "caso";
  $("#view-stations").hidden = which !== "estacoes";
  if (which === "case-dashboard" || which === "case-cube") placeAnalytics("case");
  if (which !== "viz") {
    $("#tab-table").hidden = true;
    $("#tab-group").hidden = true;
    $("#view-dashboard").classList.remove("in-workspace");
    $("#view-cube").classList.remove("in-workspace");
  }
  $("#view-dashboard").hidden = which !== "case-dashboard";
  $("#view-cube").hidden = which !== "case-cube";
  $("#view-trail").hidden = which !== "trail";
  if (which === "trail") renderTrail();
  if (which === "caso") renderAnalysis();
  if (which === "estacoes") renderStations();
  if (which === "case-dashboard") openDashboard("case");
  if (which === "case-cube") openCube("case");
  if (which === "viz") { switchTab(state.activeDatasetTab, { deferAnalytics }); requestAnimationFrame(() => { const box = $("#chart"); if (chart && box.clientWidth > 0) chart.setSize({ width: box.clientWidth - 4, height: 96 }); }); }
  updateContextBar();
}

function openContextAnalytics(view) {
  const station = state.activeContext === "station" ? activeStation() : null;
  if (state.activeContext === "artifact") {
    state.stationAnalyticsId = null;
    switchView("viz");
    switchTab(view);
    return;
  }
  state.stationAnalyticsId = station?.id || null;
  switchView(view === "dashboard" ? "case-dashboard" : "case-cube");
}

function openCaseCube() {
  if (!activeCase()) {
    toast("Crie ou selecione um Caso antes de abrir o Cubo do Caso.", "info");
    return;
  }
  state.stationAnalyticsId = null;
  switchView("case-cube");
}

function openRightInspector() {
  if (!$("#drawer").hidden) {
    closeDrawer();
    return;
  }
  if (state.currentDetailEv) {
    showDetail(state.currentDetailEv);
    return;
  }
  if (activeStation()) {
    showStationInspector(activeStation());
    return;
  }
  if (state.rows[0]) {
    openDetail(state.rows[0].id);
    return;
  }
  const currentCase = activeCase();
  if (currentCase) {
    const overview = el("div", "kv");
    overview.append(
      el("div", "kv-row", `Itens técnicos: ${fmtNum(currentCase.items?.length || 0)}`),
      el("div", "kv-row", `Estações vinculadas: ${fmtNum(caseStations().length)}`),
    );
    openContextInspector("Caso", currentCase.name, overview);
    return;
  }
  toast("Selecione um evento, campo, estação ou Caso para inspecionar.", "info");
}

// ------------------------------------------------------------------ códigos
async function openCodes() {
  $("#codes-editor").value = await api("get_codes");
  $("#codes-path").textContent = await api("get_codes_path");
  updateSysCount();
  $("#codes-modal").hidden = false;
}

async function updateSysCount() {
  try {
    const n = await api("system_codes_count", {}, { silent: true });
    $("#sys-count").textContent = fmtNum(n);
  } catch { /* deixa "—" */ }
}

async function runHarvest() {
  const btn = $("#btn-harvest");
  btn.disabled = true;
  btn.innerHTML = '<i class="fas fa-circle-notch spin"></i> Extraindo…';
  try {
    const res = await api("harvest_codes", {}, { silent: true });
    $("#sys-count").textContent = fmtNum(res.count);
    toast(`${fmtNum(res.count)} códigos extraídos de ${fmtNum(res.sources)} fontes do sistema.`, "ok");
    refresh();
  } catch (e) {
    toast(String(e), "err");
  } finally {
    btn.disabled = false;
    btn.innerHTML = '<i class="fas fa-rotate"></i> Atualizar do sistema';
  }
}

async function saveCodes() {
  try {
    await api("save_codes", { text: $("#codes-editor").value });
    $("#codes-modal").hidden = true;
    toast("Catálogo salvo e reaplicado aos eventos.", "ok");
    refresh();
  } catch { /* toast de erro já exibido */ }
}

// ------------------------------------------------------------------ configurações / MCP
// tools que alteram o estado do app (exibem o selo "altera dados" na aba MCP)
const MCP_MUTATING_TOOLS = new Set([
  "load_file", "load_files", "load_event_log", "clear_events",
  "save_custom_format", "set_ts_config",
  "save_derived_field", "delete_derived_field",
  "save_codes", "harvest_codes", "cases_save",
  "threat_catalog_update", "export_events", "remote_import",
]);
const MCP_CATEGORY_ORDER = [
  "Fontes", "Consulta", "Análise", "Ameaças", "Jornadas", "Conexões", "Formatos", "Data/Hora",
  "Campos derivados", "Códigos", "Casos", "Outros",
];

function mcpToolCategory(name) {
  if (/^threat_/.test(name)) return "Ameaças";
  if (/^journey_/.test(name)) return "Jornadas";
  if (/^remote_/.test(name)) return "Conexões";
  if (/^(load_|clear_events|source_summary|list_sources|expand_paths)/.test(name)) return "Fontes";
  if (/format/.test(name)) return "Formatos";
  if (/ts_config|timestamp/.test(name)) return "Data/Hora";
  if (/derived/.test(name)) return "Campos derivados";
  if (/code|harvest/.test(name)) return "Códigos";
  if (/^cases?_/.test(name)) return "Casos";
  if (/^(query|count|event_detail|stats|explore|trail|list_channels)/.test(name)) return "Consulta";
  if (/^(aggregate|profile|compute|pivot|tree|discover_patterns|compare_periods|timeline_range|dataset_overview|export_events)/.test(name)) return "Análise";
  return "Outros";
}

function copyTextButton(text, label = "Copiar") {
  const btn = el("button", "icon-btn");
  btn.type = "button";
  btn.title = label;
  btn.innerHTML = '<i class="fas fa-copy"></i>';
  btn.onclick = async () => {
    try {
      await navigator.clipboard.writeText(text);
      toast("Copiado.", "ok");
    } catch (e) {
      toast(`Não foi possível copiar: ${e}`, "err");
    }
  };
  return btn;
}

function mcpSnippet(title, code) {
  const box = el("div", "mcp-snippet");
  const head = el("div", "mcp-snippet-head");
  head.appendChild(el("span", "", title));
  head.appendChild(copyTextButton(code));
  box.appendChild(head);
  box.appendChild(el("pre", "", code));
  return box;
}

function switchSettingsTab(tab) {
  document.querySelectorAll("#settings-modal .settings-tab").forEach((b) =>
    b.classList.toggle("active", b.dataset.settingsTab === tab));
  document.querySelectorAll("#settings-modal .settings-pane").forEach((p) => {
    p.hidden = p.id !== `settings-pane-${tab}`;
  });
}

async function openSettings(tab = "mcp") {
  $("#settings-modal").hidden = false;
  switchSettingsTab(tab);
  if (tab === "mcp") await renderMcpPane();
}

async function renderMcpPane() {
  const pane = $("#settings-pane-mcp");
  pane.innerHTML = "";
  let status = null;
  try { status = await api("mcp_status", {}, { silent: true }); } catch { status = null; }
  if (!status) {
    pane.appendChild(el("p", "muted small",
      "Não foi possível consultar o status do servidor MCP nesta versão do aplicativo."));
    return;
  }

  // ---- status
  const box = el("div", "mcp-status");
  const rowStatus = el("div", "mcp-status-row");
  rowStatus.appendChild(el("i", "fas fa-server"));
  if (status.running) {
    rowStatus.appendChild(el("span", "mcp-badge on", "Servidor ativo"));
  } else {
    rowStatus.appendChild(el("span", "mcp-badge off", status.enabled ? "Servidor parado" : "Servidor desativado"));
  }
  if (status.running && status.port) rowStatus.appendChild(el("span", "muted small", `porta ${status.port}`));
  box.appendChild(rowStatus);
  const toggle = el("button", "btn ghost small", status.enabled ? "Desativar integração" : "Ativar integração");
  toggle.onclick = async () => { toggle.disabled = true; try { await api("mcp_configure", { enabled: !status.enabled }); await renderMcpPane(); } finally { toggle.disabled = false; } };
  box.appendChild(toggle);

  if (status.running && status.url) {
    const rowUrl = el("div", "mcp-status-row");
    rowUrl.appendChild(el("span", "mcp-mono", status.url));
    rowUrl.appendChild(copyTextButton(status.url, "Copiar URL"));
    box.appendChild(rowUrl);
  }
  if (!status.enabled) {
    box.appendChild(el("p", "mcp-note",
      "Ative para conectar um cliente aos logs e às investigações deste aplicativo."));
  } else if (!status.running) {
    box.appendChild(el("p", "mcp-note",
      "O MCP está habilitado, mas o servidor não está em execução — reinicie o aplicativo."));
  } else {
    box.appendChild(el("p", "mcp-note",
      "O LogInsight precisa estar aberto para o MCP funcionar: o estado (eventos, casos e configurações) vive no aplicativo."));
  }
  pane.appendChild(box);

  pane.appendChild(el("p", "muted small",
    "MCP (Model Context Protocol) permite que assistentes de IA (Google Antigravity, VS Code/Copilot, opencode, Claude Code, Cursor) " +
    "usem o LogInsight como ferramenta: eles enxergam e operam os mesmos dados que você vê na tela — " +
    "mudanças feitas pela IA aparecem aqui na hora."));

  if (status.enabled) {
    // ---- como configurar
    const port = status.port || 39117;
    const url = status.url || `http://127.0.0.1:${port}/mcp`;
    const headers = { Authorization: `Bearer ${status.token || ""}` };
    const fldConfig = el("div", "fld");
    fldConfig.appendChild(el("label", "", "Como configurar"));
    fldConfig.appendChild(mcpSnippet("Google Antigravity (~/.gemini/config/mcp_config.json)",
      JSON.stringify({
        mcpServers: {
          loginsight: {
            serverUrl: url,
            headers,
          },
        },
      }, null, 2)));
    fldConfig.appendChild(mcpSnippet("VS Code (mcp.json)",
      JSON.stringify({ servers: { loginsight: { type: "http", url, headers } } }, null, 2)));
    fldConfig.appendChild(mcpSnippet("opencode (opencode.json)",
      JSON.stringify({ mcp: { loginsight: { type: "remote", url, headers, enabled: true } } }, null, 2)));
    fldConfig.appendChild(mcpSnippet("Claude Code (terminal)",
      `claude mcp add --transport http loginsight ${url} --header "Authorization: Bearer ${status.token || ""}"`));
    fldConfig.appendChild(el("p", "muted small",
      "No Google Antigravity, configure em ~/.gemini/config/mcp_config.json (global) ou em .agents/mcp_config.json (workspace). Para os demais clientes, use a URL local e o cabeçalho Authorization."));
    const adv = el("div", "mcp-status");
    const advRow = el("div", "mcp-status-row");
    advRow.appendChild(el("span", "muted small", "Config avançada:"));
    advRow.appendChild(el("span", "mcp-mono", status.config_path || "mcp.json"));
    if (status.config_path) advRow.appendChild(copyTextButton(status.config_path, "Copiar caminho"));
    adv.appendChild(advRow);
    adv.appendChild(el("p", "mcp-note",
      'Conteúdo: {"enabled": true, "port": 39117} — alterações no mcp.json só valem após reiniciar o aplicativo.'));
    fldConfig.appendChild(adv);
    pane.appendChild(fldConfig);

    // ---- ferramentas
    const tools = Array.isArray(status.tools) ? status.tools : [];
    const fldTools = el("div", "fld");
    fldTools.appendChild(el("label", "", `Ferramentas disponíveis (${tools.length})`));
    fldTools.appendChild(el("p", "muted small",
      'As ferramentas com o selo "altera dados" modificam o estado do app — a tela é atualizada automaticamente.'));
    const groups = new Map();
    for (const tool of tools) {
      const cat = mcpToolCategory(tool.name || "");
      if (!groups.has(cat)) groups.set(cat, []);
      groups.get(cat).push(tool);
    }
    for (const cat of MCP_CATEGORY_ORDER) {
      const items = groups.get(cat);
      if (!items?.length) continue;
      const group = el("div", "mcp-tool-group");
      group.appendChild(el("div", "mcp-tool-group-title", cat));
      for (const tool of items) {
        const row = el("div", "mcp-tool");
        const head = el("div", "mcp-tool-head");
        head.appendChild(el("span", "mcp-tool-name", tool.name || ""));
        if (MCP_MUTATING_TOOLS.has(tool.name)) head.appendChild(el("span", "mcp-tool-badge", "altera dados"));
        row.appendChild(head);
        if (tool.description) row.appendChild(el("div", "mcp-tool-desc", tool.description));
        group.appendChild(row);
      }
      fldTools.appendChild(group);
    }
    if (!tools.length) fldTools.appendChild(el("p", "muted small", "Nenhuma ferramenta registrada."));
    pane.appendChild(fldTools);
  }
}

// ------------------------------------------------------------------ live-refresh via MCP
async function handleMcpStateChanged(kind) {
  window.Discovery?.clearCache();
  caseEventsCache.sig = null;
  if (kind === "source") {
    await mcpRefreshSource();
    toast("Fonte de dados atualizada via MCP.", "info");
    return;
  }
  if (kind === "cases") {
    await mcpReloadCases();
    toast("Casos atualizados via MCP.", "info");
    return;
  }
  if (kind === "threats") {
    toast("Catálogo de ameaças atualizado via MCP.", "info");
    return;
  }
  // codes / derived / ts_config / formats: recarrega painéis abertos e reconsulta a view
  try {
    if (kind === "codes" && !$("#codes-modal").hidden) {
      $("#codes-editor").value = await api("get_codes", {}, { silent: true });
      updateSysCount();
    } else if (kind === "derived") {
      await loadDerivedFields();
    } else if (kind === "ts_config") {
      if (state.currentArtifact?.kind === "file") await loadTsConfig(state.currentArtifact.path);
      updateTsExample();
    } else if (kind === "formats") {
      await loadFormatOptions();
    }
  } catch { /* painel permanece como estava */ }
  // eventos são re-enriquecidos/re-derivados no backend: reconsulta a view ativa
  if (state.loaded) {
    await refresh();
    api("profile_fields", { filters: [] }, { silent: true })
      .then((profiles) => { state.datasetProfiles = profiles; renderExploreTree(); })
      .catch(() => {});
  }
  toast("Configurações atualizadas via MCP.", "info");
}

// a fonte de eventos mudou no backend (load/clear via MCP): refaz o pós-load lógico da UI
async function mcpRefreshSource(contextual = false) {
  if (window.WorkspaceContext?.ready && !contextual) return window.WorkspaceContext.sourceChanged(() => mcpRefreshSource(true));
  // resume a fonte atual no backend; fallback: deriva as colunas dos perfis (vazio = fonte limpa)
  let columns = [];
  let count = null;
  let sourceDesc = "";
  let profiles = null;
  let summary = null;
  try { summary = await api("source_summary", {}, { silent: true }); } catch { summary = null; }
  if (summary && typeof summary.count === "number") {
    columns = summary.columns || [];
    count = summary.count;
    sourceDesc = summary.source_desc || "";
  } else {
    try { profiles = (await api("profile_fields", { filters: [] }, { silent: true })) || []; }
    catch { profiles = []; }
    columns = profiles.map((p) => p.name);
    if (!columns.length) count = 0;
  }
  if (count === 0) {
    await clearData();
    switchView("source");
    return;
  }
  state.columns = columns;
  state.visibleCols = (state.visibleCols || []).filter((col) => state.columns.includes(col));
  if (!state.visibleCols.length) {
    state.visibleCols = ["timestamp", "level", "code", "name", "message"].filter((col) => state.columns.includes(col));
  }
  // mesmo recorte limpo de um load manual: filtros e paginação recomeçam
  state.filters = [];
  state.quick = "";
  $("#quick-search").value = "";
  state.page = 0;
  state.loaded = true;
  state.datasetDashboard = null;
  state.datasetCube = null;
  state.caseProfiles = {};
  fillColumnControls();
  renderChips();
  $("#btn-merge").disabled = false;
  // rótulo da fonte carregada pelo MCP (sem registrar no drive do Caso: a spec de origem é externa)
  if (sourceDesc) {
    if (state.currentArtifact) {
      state.currentArtifact.label = sourceDesc;
      if (count != null) state.currentArtifact.count = count;
    } else {
      state.currentArtifact = {
        id: "mcp:externo",
        label: sourceDesc,
        kind: "file",
        path: "",
        count: count || 0,
        loadedAt: Date.now(),
        source: null,
      };
    }
    state.currentOrigin = sourceDesc;
  }
  await refresh();
  $("#load-status").textContent = `${fmtNum(count ?? state.total)} eventos`;
  $("#load-status").className = "load-status ok";
  // perfis dos campos alimentam a árvore de exploração
  if (profiles) {
    state.datasetProfiles = profiles;
    renderExploreTree();
  } else {
    api("profile_fields", { filters: [] }, { silent: true })
      .then((p) => { state.datasetProfiles = p; renderExploreTree(); })
      .catch(() => {});
  }
  updateContextBar();
  // se a UI estava fora da exploração do artefato, leva o usuário aos dados
  if (state.activeContext === "artifact" && document.querySelector(".shell").hidden) switchView("viz");
}

// cases.json mudou fora do app: relê e substitui o estado em memória (sem regravar)
async function mcpReloadCases() {
  let loaded = null;
  try { loaded = await api("cases_load", {}, { silent: true }); } catch { return; }
  if (!loaded || !Array.isArray(loaded.cases)) return;
  if (window.WorkspaceContext?.ready) { await window.WorkspaceContext.replaceCases(normalizeCaseStore(loaded)); return; }
  state.cases = normalizeCaseStore(loaded);
  // sessões de artefatos foram derivadas do estado anterior dos casos
  state.artifactSessions = new Map();
  renderCaseBar();
  updateAnalysisBadge();
  if (activeCase()) {
    restoreCaseWorkspace();
    await syncActiveCaseArtifacts();
  }
}

// ------------------------------------------------------------------ teclado
function bindKeyboard() {
  document.addEventListener("keydown", (e) => {
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || "");
    if (e.key === "/" && !typing) {
      e.preventDefault();
      $("#quick-search").focus();
    } else if (e.key === "Escape") {
      closeCtxMenu();
      closeTlPop();
      closeDrawer();
      $("#filter-pop").hidden = true;
      $("#col-pop").hidden = true;
      $("#name-pop").hidden = true;
      $("#codes-modal").hidden = true;
      $("#settings-modal").hidden = true;
      $("#format-modal").hidden = true;
      $("#derive-modal").hidden = true;
      $("#chart-modal").hidden = true;
      $("#ts-modal").hidden = true;
      $("#manual-form").hidden = true;
      closeCaseNameInput();
      $("#case-add-modal").hidden = true;
      $("#case-item-modal").hidden = true;
      pendingCaseAdd = null;
      editingCaseItem = null;
    } else if (!$("#drawer").hidden && !typing) {
      if (e.key === "ArrowLeft") detailStep(-1);
      else if (e.key === "ArrowRight") detailStep(1);
    } else if ((e.ctrlKey || e.metaKey) && (e.key === "a" || e.key === "A") && !typing) {
      if (state.rows?.length && !$("#view-explore")?.hidden) {
        e.preventDefault();
        state.selectedEventRows = new Map();
        for (const ev of state.rows) state.selectedEventRows.set(ev.id, ev);
        updateRowSelectionStyles();
      }
    }
  });
  // fecha o menu de contexto ao clicar/usar botão direito fora dele
  document.addEventListener("click", (e) => {
    if (ctxEl && !e.target.closest(".ctx-menu")) closeCtxMenu();
    if (tlPop && !e.target.closest(".tl-pop")) closeTlPop();
  }, true);
  document.addEventListener("contextmenu", (e) => {
    if (ctxEl && !e.target.closest(".ctx-menu")) closeCtxMenu();
    if (tlPop && !e.target.closest(".tl-pop")) closeTlPop();
  }, true);
}

// ------------------------------------------------------------------ init
function bind() {
  $("#src-btn-file").onclick = () => setSource("file");
  $("#src-btn-eventlog").onclick = () => setSource("eventlog");
  $("#btn-browse").onclick = browseFile;
  $("#btn-refresh-channels").onclick = refreshChannels;
  $("#btn-load").onclick = async () => { await loadData(null, { merge: state.loaded }); if (state.loaded) switchView("viz"); };
  $("#btn-merge").onclick = async () => { await loadData(null, { merge: true }); if (state.loaded) switchView("viz"); };
  $("#btn-clear").onclick = () => clearData({ removeCurrent: true });
  $("#btn-back-drive").onclick = () => switchView("source");
  $("#cm-close").onclick = () => { $("#comment-modal").hidden = true; };
  $("#cm-cancel").onclick = () => { $("#comment-modal").hidden = true; };
  $("#comment-modal").addEventListener("click", (e) => { if (e.target === $("#comment-modal")) $("#comment-modal").hidden = true; });
  $("#cm-save").onclick = () => {
    setEventComment(commentEv, $("#cm-text").value);
    $("#comment-modal").hidden = true;
    toast("Comentário salvo.", "ok");
  };
  $("#cm-remove").onclick = () => {
    setEventComment(commentEv, "");
    $("#comment-modal").hidden = true;
    toast("Comentário removido.", "ok");
  };
  $("#btn-trail-back").onclick = () => switchView(state.loaded ? "viz" : "source");
  $("#trail-more-before").onclick = () => { state.trail.before += 15; persistTrail(); renderTrail(); };
  $("#trail-more-after").onclick = () => { state.trail.after += 15; persistTrail(); renderTrail(); };
  $("#trail-only-case").onchange = (e) => { state.trail.onlyCase = e.target.checked; persistTrail(); renderTrail(); };
  $("#trail-unfiltered").onchange = (e) => { state.trail.unfiltered = e.target.checked; persistTrail(); renderTrail(); };
  $("#btn-drive-add").onclick = () => showSourceMode("load");
  $("#btn-drive-add-empty").onclick = () => showSourceMode("load");
  $("#btn-source-back").onclick = () => showSourceMode("list");

  $("#quick-search").addEventListener("input", (e) => {
    state.quick = e.target.value;
    scheduleRefresh();
  });

  $("#btn-add-filter").onclick = (e) => {
    e.stopPropagation();
    $("#filter-pop").hidden ? openFilterPop() : ($("#filter-pop").hidden = true);
  };
  $("#fp-apply").onclick = applyFilterPop;
  $("#fp-cancel").onclick = () => { $("#filter-pop").hidden = true; };
  $("#fp-val").addEventListener("keydown", (e) => { if (e.key === "Enter") applyFilterPop(); });
  $("#np-ok").onclick = commitNamePop;
  $("#np-cancel").onclick = () => { $("#name-pop").hidden = true; namePopCb = null; };
  $("#np-val").addEventListener("keydown", (e) => { if (e.key === "Enter") commitNamePop(); });

  $("#btn-colpicker").onclick = (e) => {
    e.stopPropagation();
    $("#col-pop").hidden ? openColPop() : ($("#col-pop").hidden = true);
  };
  document.addEventListener("click", (e) => {
    if (!$("#filter-pop").hidden && !e.target.closest("#filter-pop") && !e.target.closest("#btn-add-filter"))
      $("#filter-pop").hidden = true;
    if (!$("#col-pop").hidden && !e.target.closest("#col-pop") && !e.target.closest("#btn-colpicker"))
      $("#col-pop").hidden = true;
    // .ctx-menu isento: o item que abriu o popover não pode fechá-lo no mesmo clique
    if (!$("#name-pop").hidden && !e.target.closest("#name-pop") && !e.target.closest(".ctx-menu"))
      $("#name-pop").hidden = true;
  });

  $("#page-size").onchange = () => {
    state.pageSize = parseInt($("#page-size").value, 10);
    state.page = 0;
    refresh();
  };
  $("#pg-prev").onclick = () => { if (state.page > 0) { state.page--; refresh(); } };
  $("#pg-next").onclick = () => { state.page++; refresh(); };

  $("#tabbtn-table").onclick = () => switchTab("table");
  $("#tabbtn-group").onclick = () => switchTab("group");
  $("#tabbtn-dashboard").onclick = () => switchTab("dashboard");
  $("#tabbtn-cube").onclick = () => switchTab("cube");
  // (abrir fonte = botão Arquivos na topbar)
  $("#btn-open-stations").onclick = () => switchView("estacoes");
  $("#btn-case-dashboard").onclick = () => { state.stationAnalyticsId = null; switchView("case-dashboard"); };
  $("#btn-right-inspect").onclick = openRightInspector;
  $("#btn-right-artifact").onclick = () => {
    state.stationAnalyticsId = null;
    // com artefato aberto, volta para a visão dele; sem artefato, mostra a lista de arquivos
    switchView(state.loaded ? "viz" : "source");
  };
  $("#btn-right-dashboard").onclick = () => openContextAnalytics("dashboard");
  $("#btn-right-cube").onclick = openCaseCube;
  $("#btn-right-case").onclick = () => {
    state.stationAnalyticsId = null;
    setAnalysisView("overview");
    switchView("caso");
  };
  $("#btn-right-stations").onclick = () => switchView("estacoes");
  $("#btn-side-toggle").onclick = () => {
    const collapsed = document.querySelector(".shell").classList.toggle("side-collapsed");
    if (!collapsed) refreshTreeAggs(workspaceScope());
  };
  $("#btn-clear-filters").onclick = () => {
    state.filters = [];
    state.quick = "";
    $("#quick-search").value = "";
    state.page = 0;
    filtersChanged();
  };

  // dashboard
  $("#btn-dash-add").onclick = () => {
    openChartEditor({ id: nid(), title: "Novo gráfico", chart: "time", metric: "count", field: null, split: null, type: "line", interval_ms: null }, $("#btn-dash-add"));
  };
  $("#btn-dash-refresh").onclick = () => {
    setDashboardCharts(null);
    setScopeProfiles(null);
    openDashboard(state.analyticsScope);
  };
  $("#cp-apply").onclick = applyChartEditor;
  $("#cp-cancel").onclick = () => { $("#chart-modal").hidden = true; chartEditing = null; };
  $("#cp-close").onclick = () => { $("#chart-modal").hidden = true; chartEditing = null; };
  $("#chart-modal").addEventListener("click", (e) => {
    if (e.target === $("#chart-modal")) { $("#chart-modal").hidden = true; chartEditing = null; }
  });
  $("#btn-dash-compact").onclick = () => {
    state.dashboardCompact = !state.dashboardCompact;
    localStorage.setItem("investigation.dashboardCompact", state.dashboardCompact ? "1" : "0");
    renderDashboard();
  };
  $("#cp-type").onchange = () => syncDashboardChartEditor(true);
  $("#cp-chart").onchange = () => syncDashboardChartEditor(false);

  // cubo
  $("#btn-cube-clear").onclick = () => {
    const table = activeCube();
    table.rows = [];
    table.cols = [];
    table.values = [{ func: "count", column: "*", alias: "qtd" }];
    markCubeTableChanged();
    renderCubeZones();
    runCube();
  };
  $("#btn-cube-chart").onclick = cubeToChart;
  $("#btn-cube-add-table").onclick = addCubeTable;
  $("#cube-chart-apply").onclick = applyCubeChart;
  $("#cube-chart-cancel").onclick = () => { $("#cube-chart-modal").hidden = true; cubeChartEditing = null; };
  $("#cube-chart-close").onclick = () => { $("#cube-chart-modal").hidden = true; cubeChartEditing = null; };
  $("#cube-chart-modal").addEventListener("click", (event) => {
    if (event.target === $("#cube-chart-modal")) { $("#cube-chart-modal").hidden = true; cubeChartEditing = null; }
  });
  document.querySelectorAll("#cube-chart-type .seg-btn").forEach((button) => {
    button.onclick = () => setCubeChartType(button.dataset.type);
  });

  // casos de análise
  $("#case-select").onchange = async () => {
    if (window.WorkspaceContext) { await window.WorkspaceContext.changeCase($("#case-select").value || null); return; }
    state.cases.active = $("#case-select").value || null;
    saveCases();
    updateAnalysisBadge();
    if (activeCase()) restoreCaseWorkspace();
    else switchView("source");
    await syncActiveCaseArtifacts();
  };
  $("#btn-new-case").onclick = () => showCaseNameInput("new");
  $("#btn-case-menu").onclick = (e) => {
    e.stopPropagation();
    showCtxMenu(e.clientX, e.clientY, [
      { icon: "fa-pen", label: "Renomear caso", onClick: () => showCaseNameInput("rename") },
      { icon: "fa-trash-can", label: "Excluir caso", danger: true, onClick: deleteActiveCase },
    ]);
  };
  $("#case-name-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") commitCaseNameInput();
    else if (e.key === "Escape") { closeCaseNameInput(); e.stopPropagation(); }
  });
  document.querySelectorAll(".small-seg .seg-btn").forEach((b) => {
    b.onclick = () => {
      setAnalysisView(b.dataset.view);
      saveCaseWorkspace("caso");
      renderAnalysis();
    };
  });
  $("#btn-manual-event").onclick = () => {
    ensureCase();
    setAnalysisView("timeline");
    saveCaseWorkspace("caso");
    renderAnalysis();
    $("#manual-form").hidden = !$("#manual-form").hidden;
    if (!$("#manual-form").hidden) $("#mf-name").focus();
  };
  $("#mf-save").onclick = saveManualEvent;
  $("#mf-cancel").onclick = () => { $("#manual-form").hidden = true; };
  $("#btn-analysis-clear").onclick = () => {
    const c = activeCase();
    if (!c) return;
    c.items = [];
    saveCases();
    updateAnalysisBadge();
    renderAnalysis();
  };
  $("#case-add-close").onclick = () => { $("#case-add-modal").hidden = true; pendingCaseAdd = null; };
  $("#case-add-cancel").onclick = () => { $("#case-add-modal").hidden = true; pendingCaseAdd = null; };
  $("#case-add-confirm").onclick = confirmCaseAdd;
  $("#case-add-new-station").onclick = () => {
    $("#case-add-station-form").hidden = !$("#case-add-station-form").hidden;
    if (!$("#case-add-station-form").hidden) $("#case-add-station-name").focus();
  };
  $("#case-add-modal").addEventListener("click", (e) => {
    if (e.target === $("#case-add-modal")) { $("#case-add-modal").hidden = true; pendingCaseAdd = null; }
  });
  $("#case-item-close").onclick = () => { $("#case-item-modal").hidden = true; editingCaseItem = null; };
  $("#case-item-cancel").onclick = () => { $("#case-item-modal").hidden = true; editingCaseItem = null; };
  $("#case-item-save").onclick = saveCaseItemContext;
  $("#case-item-modal").addEventListener("click", (e) => {
    if (e.target === $("#case-item-modal")) { $("#case-item-modal").hidden = true; editingCaseItem = null; }
  });
  $("#workbar-cancel").onclick = () => {
    state.refreshVersion++;
    if (state.activeOperation) state.activeOperation.cancelled = true;
    finishOperation("Operação interrompida", "Resultados anteriores foram descartados.");
  };
  $("#group-col").onchange = () => { state.groupCol = $("#group-col").value; };
  $("#btn-add-agg").onclick = () => {
    state.aggs.push({ func: "count", column: "*", alias: "" });
    renderAggs();
  };
  $("#btn-run-group").onclick = runGroup;

  $("#dr-close").onclick = closeDrawer;
  $("#drawer-scrim").onclick = closeDrawer;
  $("#dr-prev").onclick = () => detailStep(-1);
  $("#dr-next").onclick = () => detailStep(1);
  $("#dr-copy").onclick = async () => {
    if (state.detailId == null) return;
    const ev = await api("event_detail", { id: state.detailId });
    await navigator.clipboard.writeText(JSON.stringify(ev, null, 2));
    toast("JSON copiado.", "ok");
  };
  document.querySelectorAll("#drawer .dtab").forEach((t) => {
    t.onclick = () => switchDetailTab(t.dataset.pane);
  });

  $("#btn-codes").onclick = openCodes;
  $("#codes-close").onclick = () => { $("#codes-modal").hidden = true; };
  $("#codes-cancel").onclick = () => { $("#codes-modal").hidden = true; };
  $("#codes-save").onclick = saveCodes;
  $("#btn-harvest").onclick = runHarvest;

  // formatos de log
  $("#btn-new-format").onclick = () => { $("#format-modal").hidden = false; $("#fmt-name").focus(); };
  $("#format-close").onclick = () => { $("#format-modal").hidden = true; };
  $("#format-cancel").onclick = () => { $("#format-modal").hidden = true; };
  $("#format-save").onclick = saveNewFormat;
  $("#fmt-test").onclick = testNewFormat;
  $("#fmt-help-btn").onclick = () => { $("#fmt-help").hidden = !$("#fmt-help").hidden; };
  document.querySelectorAll("#fmt-kind-seg .seg-btn").forEach((b) => {
    b.onclick = () => {
      document.querySelectorAll("#fmt-kind-seg .seg-btn").forEach((x) => x.classList.toggle("active", x === b));
      $("#fmt-regex-fld").hidden = b.dataset.kind !== "regex";
      $("#fmt-delim-fld").hidden = b.dataset.kind !== "delimited";
    };
  });
  $("#format-modal").addEventListener("click", (e) => {
    if (e.target === $("#format-modal")) $("#format-modal").hidden = true;
  });

  // data/hora
  $("#ts-open").onclick = () => openTsModal();
  $("#ts-close").onclick = () => { $("#ts-modal").hidden = true; };
  $("#ts-modal").addEventListener("click", (e) => { if (e.target === $("#ts-modal")) $("#ts-modal").hidden = true; });
  $("#ts-help-btn").onclick = () => { $("#ts-help").hidden = !$("#ts-help").hidden; };
  $("#ts-format").onchange = () => {
    $("#ts-format-custom").hidden = $("#ts-format").value !== "custom";
  };
  $("#ts-add-rule").onclick = () => { tsAddRule(); renderTsExample(); };
  $("#ts-test").onclick = testTsConfig;
  $("#ts-apply").onclick = applyTsConfig;
  $("#ts-reset").onclick = resetTsConfig;
  fillTsFormats();

  // campo derivado (seleção de texto no drawer)
  $("#drawer").addEventListener("contextmenu", (e) => {
    const sel = window.getSelection()?.toString().trim();
    if (!sel || !state.currentDetailEv) return;
    const row = e.target.closest(".kv-row");
    const sourceCol = row?.dataset.col || "message";
    e.preventDefault();
    const items = [
      {
        icon: "fa-filter",
        label: `Filtrar: ${colLabel(sourceCol)} contém "${trunc(sel, 30)}"`,
        onClick: () => addFilter({ column: sourceCol, op: "contains", value: sel, value2: null }),
      },
      {
        icon: "fa-filter-circle-xmark",
        label: `Filtrar: ${colLabel(sourceCol)} não contém "${trunc(sel, 30)}"`,
        onClick: () => addFilter({ column: sourceCol, op: "not_contains", value: sel, value2: null }),
      },
      { sep: true },
      {
        icon: "fa-square-plus",
        label: "Criar campo a partir da seleção",
        onClick: () => openDeriveModal(sel, sourceCol, cellValue(state.currentDetailEv, sourceCol)),
      },
    ];
    const derived = (state.derivedFields || []).slice(0, 6);
    if (derived.length) {
      items.push({ sep: true });
      for (const def of derived) {
        items.push({
          icon: "fa-layer-plus",
          label: `Incrementar regra: ${def.name}`,
          onClick: () => openDeriveEdit(def, { appendRule: true }),
        });
      }
    }
    showCtxMenu(e.clientX, e.clientY, items);
  });
  $("#derive-close").onclick = () => { $("#derive-modal").hidden = true; };
  $("#derive-cancel").onclick = () => { $("#derive-modal").hidden = true; };
  $("#derive-save").onclick = saveDerivedField;
  $("#dv-add-rule").onclick = () => { dvAddRule(); updateDvPreview(); };
  $("#derive-delete").onclick = async () => {
    if (!dvCtx?.editName) return;
    try {
      await api("delete_derived_field", { name: dvCtx.editName });
      $("#derive-modal").hidden = true;
      toast(`Campo "${dvCtx.editName}" removido.`, "ok");
      state.columns = state.columns.filter((c) => c !== dvCtx.editName);
      loadDerivedFields();
      refresh();
    } catch { /* toast já exibido */ }
  };
  $("#derive-modal").addEventListener("click", (e) => {
    if (e.target === $("#derive-modal")) $("#derive-modal").hidden = true;
  });

  $("#codes-modal").addEventListener("click", (e) => {
    if (e.target === $("#codes-modal")) $("#codes-modal").hidden = true;
  });

  $("#btn-settings").onclick = () => openSettings();
  $("#settings-close").onclick = () => { $("#settings-modal").hidden = true; };
  $("#settings-modal").addEventListener("click", (e) => {
    if (e.target === $("#settings-modal")) $("#settings-modal").hidden = true;
  });
  document.querySelectorAll("#settings-modal .settings-tab").forEach((b) => {
    b.onclick = () => switchSettingsTab(b.dataset.settingsTab);
  });

  $("#btn-theme").onclick = toggleTheme;

  let resizeTimer;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      updateFilterTabsLayout();
      if (state.loaded) refresh();
    }, 250);
  });

  bindKeyboard();
}

initTheme();
bind();
refreshChannels();
loadFormatOptions();
fillColumnControls();
renderChips();
renderExploreTree();
loadDerivedFields();
renderStationShortcuts();
renderTable({ total: 0, rows: [] });
renderChart({ buckets: [], levels: [] });
setWorkbar("Nenhuma fonte carregada", "");
switchView("source");
window.workspaceBootstrap = (async () => {
  try {
    const loaded = await api("cases_load", {}, { silent: true });
    if (loaded && Array.isArray(loaded.cases)) {
      state.cases = normalizeCaseStore(loaded);
      saveCases();
    }
  } catch (error) { toast(`Não foi possível abrir as investigações: ${error}`, "err"); }
  renderCaseBar();
  updateAnalysisBadge();
  if (activeCase()) {
    setAnalysisView(activeCase().workspace?.analysisView || "vtimeline");
    await syncActiveCaseArtifacts();
    if (window.WorkspaceContext) await window.WorkspaceContext.initialize();
  } else if (window.WorkspaceContext) await window.WorkspaceContext.initialize();
})();


// ==========================================================================
// DASHBOARD
// ==========================================================================
const CHART_COLORS = ["#2f6fed", "#3fb950", "#d29922", "#bc8cff", "#ff7b72", "#58a6ff", "#f778ba", "#76e3ea"];
const nid = () => "x" + Math.random().toString(36).slice(2, 9);
const dashCharts = {}; // id → uPlot instance
const DASH_TYPE_LABELS = {
  line: "linha", bar: "barras", donut: "rosca", gauge: "velocímetro",
  radar: "radar", heatmap: "mapa de calor", kpi: "indicador",
};

function dashboardType(spec) {
  return DASH_TYPE_LABELS[spec?.type] ? spec.type : (spec?.chart === "terms" ? "bar" : "line");
}

function dashboardTypeLabel(spec) {
  return DASH_TYPE_LABELS[dashboardType(spec)];
}

function fmtBytes(v) {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return v.toFixed(v >= 100 ? 0 : 1) + " " + u[i];
}
function fmtVal(v, unit) {
  if (v === null || v === undefined || isNaN(v)) return "";
  if (unit === "bytes") return fmtBytes(v);
  if (unit === "bits") return fmtBytes(v) + "b";
  if (unit === "duration") return v >= 60000 ? (v / 60000).toFixed(1) + "min" : v >= 1000 ? (v / 1000).toFixed(1) + "s" : v.toFixed(0) + "ms";
  return fmtNum(Math.round(v * 100) / 100);
}

// Track replacement/reordering of immutable evidence rows without hashing large log bodies.
const caseObjectIds = new WeakMap();
let caseObjectId = 0;
function caseObjectKey(value) {
  if (!value || typeof value !== "object") return 0;
  if (!caseObjectIds.has(value)) caseObjectIds.set(value, ++caseObjectId);
  return caseObjectIds.get(value);
}
function caseSig() {
  const c = activeCase();
  if (!c) return "none";
  return JSON.stringify([c.id, caseObjectKey(c), state.stationAnalyticsId || "", (c.items || []).map(it => [it.id, caseObjectKey(it.rows), it.rows?.length || 0, it.stationId, it.artifactId, it.origin])]);
}

const caseEventsCache = { sig: null, events: [], summary: { start: null, end: null, columns: [] } };

function caseEvents() {
  const sig = caseSig();
  if (caseEventsCache.sig === sig) return caseEventsCache.events;
  const events = caseEventsCompute();
  caseEventsCache.sig = sig;
  caseEventsCache.events = events;
  let start = null, end = null; const columns = new Set(STANDARD);
  for (const event of events) {
    if (event.timestamp != null && Number.isFinite(event.timestamp)) { start = start == null ? event.timestamp : Math.min(start, event.timestamp); end = end == null ? event.timestamp : Math.max(end, event.timestamp); }
    for (const column of Object.keys(event.fields || {})) columns.add(column);
  }
  caseEventsCache.summary = { start, end, columns: [...columns] };
  return events;
}

function caseRecordKey(row, artifactId, origin) {
  return row.event_ref || JSON.stringify([row.fields?.caminho || artifactId || origin || "", row.id, row.timestamp, row.source, row.code, row.message]);
}

function caseEventsCompute() {
  const events = [];
  const seen = new Set();
  for (const item of activeCase()?.items || []) {
    if (state.stationAnalyticsId && item.stationId !== state.stationAnalyticsId) continue;
    for (const row of item.rows || []) {
      if (!Number.isInteger(row.id)) continue;
      const key = caseRecordKey(row, item.artifactId, item.origin);
      if (seen.has(key)) continue;
      seen.add(key);
      events.push({
        id: events.length,
        event_ref: row.event_ref || "",
        parse_status: row.parse_status || "",
        timestamp: row.timestamp ?? null,
        source: row.source || "",
        level: row.level || "",
        code: row.code || "",
        name: row.name || "",
        description: row.description || "",
        message: row.message || "",
        raw: row.raw || "",
        fields: row.fields || {},
      });
    }
  }
  return events;
}

function analyticsRequest(scope = state.analyticsScope) {
  return scope === "case"
    ? { filters: backendFilters(), caseEvents: caseEvents() }
    : { filters: backendFilters() };
}

function scopeHasEvents(scope = state.analyticsScope) {
  return scope === "case" ? caseEvents().length > 0 : state.loaded;
}

function dashboardCharts(scope = state.analyticsScope) {
  return scope === "case" ? ensureCase().caseDashboard : state.datasetDashboard;
}

function setDashboardCharts(charts, scope = state.analyticsScope) {
  if (scope === "case") {
    ensureCase().caseDashboard = charts;
    saveCases();
  } else {
    state.datasetDashboard = charts;
  }
}

function scopeProfiles(scope = state.analyticsScope) {
  if (scope !== "case") return state.datasetProfiles;
  return state.caseProfiles[activeCase()?.id] || null;
}

function setScopeProfiles(profiles, scope = state.analyticsScope) {
  if (scope === "case") {
    const caseId = activeCase()?.id;
    if (caseId) state.caseProfiles[caseId] = profiles;
  } else state.datasetProfiles = profiles;
}

async function openDashboard(scope = "dataset") {
  state.analyticsScope = scope;
  startOperation("dashboard", "Atualizando painéis", "Preparando campos e gráficos");
  if (!scopeHasEvents(scope)) {
    await renderDashboard(scope);
    finishOperation("Painéis prontos", "Sem eventos no escopo atual.");
    return;
  }
  if (!dashboardCharts(scope)) {
    $("#dash-info").textContent = "Preparando painéis…";
    $("#dash-grid").innerHTML = "";
    try {
      const profiles = await api("profile_fields", analyticsRequest(scope));
      setScopeProfiles(profiles, scope);
      setDashboardCharts(autoDashboard(profiles), scope);
    } catch {
      setDashboardCharts([], scope);
    }
  }
  await renderDashboard(scope);
  finishOperation("Painéis prontos", "Gráficos calculados no escopo atual.");
}

function autoDashboard(profiles) {
  const charts = [];
  const hasValues = (name) => profiles.some((p) => p.name === name && p.cardinality > 0);
  charts.push({ id: nid(), title: "Eventos ao longo do tempo", chart: "time", metric: "count", field: null, split: null, type: "line", interval_ms: null });
  if (hasValues("level")) {
    charts.push({ id: nid(), title: "Eventos por nível", chart: "terms", metric: "count", field: "level", split: null, type: "donut" });
    charts.push({ id: nid(), title: "Perfil de níveis", chart: "terms", metric: "count", field: "level", split: null, type: "radar" });
    charts.push({ id: nid(), title: "Nível predominante", chart: "terms", metric: "count", field: "level", split: null, type: "gauge" });
  }
  charts.push({ id: nid(), title: "Top códigos", chart: "terms", metric: "count", field: "code", split: null, type: "bar" });
  charts.push({ id: nid(), title: "Top fontes", chart: "terms", metric: "count", field: "source", split: null, type: "bar" });
  const numFields = profiles
    .filter((p) => ["bytes", "bits", "duration", "number", "percent"].includes(p.kind) && !["id", "code"].includes(p.name) && p.numeric_ratio >= 0.85)
    .slice(0, 3);
  for (const f of numFields) {
    const metric = f.kind === "duration" ? "avg" : "sum";
    charts.push({
      id: nid(),
      title: `${metric === "sum" ? "Soma" : "Média"} de ${f.name} ao longo do tempo`,
      chart: "time", metric, field: f.name, split: null, type: "line", interval_ms: null,
    });
  }
  const cat = profiles.find((p) => ["category", "bool", "ip"].includes(p.kind) && p.cardinality >= 2 && p.cardinality <= 12 && !["level", "code", "source"].includes(p.name));
  if (cat) {
    charts.push({ id: nid(), title: `Eventos por ${cat.name} ao longo do tempo`, chart: "time", metric: "count", field: null, split: cat.name, type: "line", interval_ms: null });
    charts.push({ id: nid(), title: `${cat.name} no tempo`, chart: "time", metric: "count", field: null, split: cat.name, type: "heatmap", interval_ms: null });
  }
  return charts.filter((chart) => !chart.field || hasValues(chart.field));
}

async function renderDashboard(scope = state.analyticsScope) {
  state.analyticsScope = scope;
  const charts = dashboardCharts(scope) || [];
  const grid = $("#dash-grid");
  grid.classList.toggle("dash-compact", state.dashboardCompact);
  $("#btn-dash-compact").setAttribute("aria-pressed", String(state.dashboardCompact));
  for (const id of Object.keys(dashCharts)) { dashCharts[id].destroy(); delete dashCharts[id]; }
  grid.innerHTML = "";
  const selected = caseEvents().length;
  const station = state.stationAnalyticsId
    ? caseStations().find((item) => item.id === state.stationAnalyticsId)
    : null;
  $("#dash-info").textContent = scope === "case"
    ? `${charts.length} ${charts.length === 1 ? "gráfico" : "gráficos"} · ${selected} eventos${station ? ` da estação ${station.name}` : " selecionados no Caso"}${state.filters.length ? " (filtros aplicados)" : ""}`
    : `${charts.length} ${charts.length === 1 ? "gráfico" : "gráficos"} · dados carregados${state.filters.length ? " (filtros aplicados)" : ""}`;
  if (!scopeHasEvents(scope)) {
    grid.innerHTML = scope === "case"
      ? '<div class="analysis-empty"><i class="fas fa-chart-pie"></i>Selecione eventos e envie-os ao caso para gerar painéis.</div>'
      : '<div class="analysis-empty"><i class="fas fa-chart-pie"></i>Carregue uma fonte de dados para gerar painéis.</div>';
    return;
  }
  const tasks = [];
  for (const spec of charts) {
    const type = dashboardType(spec);
    const card = el("article", `dash-card dash-card-${type}`);
    card.classList.toggle("is-wide", type === "line" || type === "heatmap");
    const head = el("div", "dash-card-head");
    head.appendChild(el("span", "dash-card-title", spec.title));
    head.appendChild(el("span", "dash-card-type", dashboardTypeLabel(spec)));
    const edit = el("button", "icon-btn");
    edit.innerHTML = '<i class="fas fa-pen"></i>';
    edit.title = "Configurar gráfico";
    edit.onclick = (e) => { e.stopPropagation(); openChartEditor(spec, card, scope); };
    const del = el("button", "icon-btn");
    del.innerHTML = '<i class="fas fa-xmark"></i>';
    del.title = "Remover gráfico";
    del.onclick = () => {
      setDashboardCharts(charts.filter((x) => x.id !== spec.id), scope);
      renderDashboard(scope);
    };
    head.append(edit, del);
    card.appendChild(head);
    const body = el("div", "dash-card-body");
    const wait = el("span", "loading-inline");
    wait.innerHTML = '<i class="fas fa-circle-notch spin"></i> calculando…';
    body.appendChild(wait);
    card.appendChild(body);
    grid.appendChild(card);
    tasks.push(renderChartCard(body, spec, scope).catch(() => { body.innerHTML = '<span class="muted small">erro ao calcular</span>'; }));
  }
  await Promise.allSettled(tasks);
}

async function renderChartCard(body, spec, scope = state.analyticsScope) {
  const res = await api("compute_series", {
    ...analyticsRequest(scope),
    spec: {
      chart: spec.chart, metric: spec.metric, field: spec.field,
      interval_ms: spec.interval_ms, split: spec.split, limit: 12, unit: "auto",
    },
  });
  if (!body.isConnected) return;
  body.innerHTML = "";
  if (!res.x.length || !res.series?.length) { body.innerHTML = '<span class="muted small">Sem dados.</span>'; return; }
  const type = dashboardType(spec);
  if (type === "gauge") {
    renderGauge(body, res, spec);
  } else if (type === "radar") {
    renderRadar(body, res, spec);
  } else if (type === "heatmap") {
    renderHeatmap(body, res, spec);
  } else if (type === "kpi") {
    renderKpi(body, res, spec);
  } else if (type === "donut") {
    renderDonut(body, res, spec, scope);
  } else if (type === "bar" || spec.chart === "terms") {
    renderHBars(body, res, spec, scope);
  } else {
    renderLineChart(body, res, spec.id);
  }
}

function showChartValueActions(event, spec, value, scope) {
  if (!spec.field || spec.chart !== "terms") return;
  event.preventDefault();
  const filter = { column: spec.field, op: value == null ? "empty" : "equals_exact", value: value == null ? "" : String(value), value2: null };
  const items = [
    { icon: "fa-filter", label: `Filtrar: ${colLabel(spec.field)} = ${trunc(value ?? "(vazio)")}`, onClick: () => window.Discovery.applySelection([filter], scope) },
    { icon: "fa-table-list", label: "Abrir eventos correspondentes", onClick: () => window.Discovery.applySelection([filter], scope, true) },
    { icon: "fa-circle-info", label: "Inspecionar campo", onClick: () => showFieldInspector(spec.field) },
  ];
  if (scope === "dataset") items.push({ icon: "fa-briefcase", label: "Adicionar grupo ao Caso", onClick: () => addGroupToAnalysis(spec.field, filter.value, undefined, filter.op) });
  showCtxMenu(event.clientX, event.clientY, items);
}

function renderHBars(box, res, spec, scope) {
  const wrap = el("div", "hbar");
  const min = Math.min(0, ...res.series[0].points), max = Math.max(0, ...res.series[0].points), span = Math.max(1e-9, max - min), zero = -min / span * 100;
  res.x.forEach((label, i) => {
    const v = res.series[0].points[i];
    const row = el("div", "hbar-row");
    row.title = spec.field ? "Clique com o botão direito para ações" : "";
    const value = res.x_values?.length === res.x.length ? res.x_values[i] : label === "(vazio)" ? null : label;
    row.oncontextmenu = (event) => showChartValueActions(event, spec, value, scope);
    const lab = el("span", "hbar-label", String(label));
    lab.title = String(label);
    const track = el("div", "hbar-track");
    const fill = el("div", "hbar-fill");
    track.style.position = "relative"; fill.style.position = "absolute";
    fill.style.left = `${Math.min(zero, (v - min) / span * 100)}%`; fill.style.width = `${Math.abs(v) / span * 100}%`;
    if (v < 0) fill.style.background = "var(--lv-erro)";
    if (min < 0 && max > 0) { const axis = el("i"); axis.style.cssText = `position:absolute;left:${zero}%;height:100%;width:1px;background:var(--text-2)`; track.append(axis); }
    track.appendChild(fill);
    row.append(lab, track, el("span", "hbar-val", fmtVal(v, res.unit)));
    wrap.appendChild(row);
  });
  box.appendChild(wrap);
}

function renderDonut(box, res, spec, scope) {
  if (res.series[0].points.some(value => value < 0)) { renderHBars(box, res, spec, scope); box.append(el("p", "muted small", "Valores negativos são mostrados em barras.")); return; }
  const total = res.series[0].points.reduce((a, b) => a + b, 0) || 1;
  const R = 60, CX = 70, CY = 70;
  let angle = -Math.PI / 2;
  const svgParts = [];
  res.x.forEach((label, i) => {
    const v = res.series[0].points[i];
    const frac = v / total;
    const a2 = angle + frac * Math.PI * 2;
    const large = frac > 0.5 ? 1 : 0;
    const x1 = CX + R * Math.cos(angle), y1 = CY + R * Math.sin(angle);
    const x2 = CX + R * Math.cos(a2), y2 = CY + R * Math.sin(a2);
    if (frac >= 1 - 1e-9) {
      svgParts.push(`<circle cx="${CX}" cy="${CY}" r="${R}" fill="${CHART_COLORS[i % CHART_COLORS.length]}"/>`);
    } else if (frac > 0.001) {
      svgParts.push(`<path d="M ${CX} ${CY} L ${x1.toFixed(1)} ${y1.toFixed(1)} A ${R} ${R} 0 ${large} 1 ${x2.toFixed(1)} ${y2.toFixed(1)} Z" fill="${CHART_COLORS[i % CHART_COLORS.length]}"/>`);
    }
    angle = a2;
  });
  const wrap = el("div", "donut-wrap");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "140");
  svg.setAttribute("height", "140");
  svg.innerHTML = svgParts.join("") + `<circle cx="${CX}" cy="${CY}" r="34" fill="var(--bg-2)"/>`;
  wrap.appendChild(svg);
  const legend = el("div", "donut-legend");
  res.x.slice(0, 8).forEach((label, i) => {
    const li = el("div", "li");
    li.title = spec.field ? "Clique com o botão direito para ações" : "";
    const value = res.x_values?.length === res.x.length ? res.x_values[i] : label === "(vazio)" ? null : label;
    li.oncontextmenu = (event) => showChartValueActions(event, spec, value, scope);
    const sw = el("span", "sw");
    sw.style.background = CHART_COLORS[i % CHART_COLORS.length];
    li.append(sw, el("span", "", String(label)), el("span", "v", fmtVal(res.series[0].points[i], res.unit)));
    legend.appendChild(li);
  });
  wrap.appendChild(legend);
  box.appendChild(wrap);
}

function sumSeriesAt(res, index) {
  return (res.series || []).reduce((sum, series) => sum + (Number(series.points?.[index]) || 0), 0);
}

function dashboardValues(res) {
  return (res.series || []).flatMap((series) => series.points || []).map((value) => Number(value) || 0);
}

function dashTimeLabel(value) {
  const d = new Date(Number(value));
  if (Number.isNaN(d.getTime())) return String(value);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function polarPoint(cx, cy, radius, angle) {
  return [cx + radius * Math.cos(angle), cy + radius * Math.sin(angle)];
}

function renderGauge(box, res, spec) {
  const primary = res.series?.[0]?.points || [];
  const isTerms = res.kind === "terms" || spec.chart === "terms";
  const values = isTerms ? primary : res.x.map((_, index) => sumSeriesAt(res, index));
  const value = isTerms ? Math.max(0, ...values) : (values.at(-1) || 0);
  const total = isTerms ? values.reduce((sum, item) => sum + item, 0) : Math.max(0, ...values);
  const ratio = total > 0 ? Math.max(0, Math.min(1, value / total)) : 0;
  const labelIndex = values.indexOf(value);
  const label = isTerms
    ? String(res.x[labelIndex] ?? "Maior valor")
    : "Último intervalo";
  const start = Math.PI, end = 0;
  const centerX = 120, centerY = 112, radius = 82;
  const valueAngle = start + (end - start) * ratio;
  const [needleX, needleY] = polarPoint(centerX, centerY, radius - 17, valueAngle);
  const outerStart = polarPoint(centerX, centerY, radius, start);
  const outerEnd = polarPoint(centerX, centerY, radius, end);
  const filledEnd = polarPoint(centerX, centerY, radius, valueAngle);
  const large = ratio > 0.5 ? 1 : 0;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.classList.add("gauge-svg");
  svg.setAttribute("viewBox", "0 0 240 138");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", `${label}: ${fmtVal(value, res.unit)}`);
  svg.innerHTML = `
    <path d="M ${outerStart[0]} ${outerStart[1]} A ${radius} ${radius} 0 0 1 ${outerEnd[0]} ${outerEnd[1]}" fill="none" stroke="var(--bg-3)" stroke-width="15" stroke-linecap="round"/>
    <path d="M ${outerStart[0]} ${outerStart[1]} A ${radius} ${radius} 0 ${large} 1 ${filledEnd[0]} ${filledEnd[1]}" fill="none" stroke="var(--accent)" stroke-width="15" stroke-linecap="round"/>
    <line x1="${centerX}" y1="${centerY}" x2="${needleX.toFixed(1)}" y2="${needleY.toFixed(1)}" stroke="var(--text-0)" stroke-width="3" stroke-linecap="round"/>
    <circle cx="${centerX}" cy="${centerY}" r="6" fill="var(--accent)" stroke="var(--bg-2)" stroke-width="3"/>
    <text x="25" y="132" fill="var(--text-2)" font-size="10">0%</text>
    <text x="195" y="132" fill="var(--text-2)" font-size="10">100%</text>`;
  const text = el("div", "gauge-value");
  text.append(
    el("strong", "", fmtVal(value, res.unit)),
    el("span", "", label),
    el("small", "", `${Math.round(ratio * 100)}% ${isTerms ? "do total mostrado" : "do maior intervalo"}`),
  );
  const wrap = el("div", "gauge-wrap");
  wrap.append(svg, text);
  box.appendChild(wrap);
}

function renderRadar(box, res, spec) {
  const primary = res.series?.[0]?.points || [];
  const isTerms = res.kind === "terms" || spec.chart === "terms";
  const labels = (isTerms ? res.x : res.x.slice(-8).map(dashTimeLabel)).slice(0, 8).map(String);
  const values = (isTerms ? primary : primary.slice(-8)).slice(0, labels.length);
  if (labels.length < 3) {
    renderHBars(box, res, spec, state.analyticsScope);
    return;
  }
  const max = Math.max(1e-9, ...values);
  const cx = 128, cy = 118, radius = 82;
  const start = -Math.PI / 2;
  const pointFor = (ratio, index) => polarPoint(cx, cy, radius * ratio, start + (Math.PI * 2 * index) / labels.length);
  const grid = [0.25, 0.5, 0.75, 1].map((ratio) => labels.map((_, index) => pointFor(ratio, index).map((v) => v.toFixed(1)).join(",")).join(" "));
  const axes = labels.map((_, index) => {
    const [x, y] = pointFor(1, index);
    return `<line x1="${cx}" y1="${cy}" x2="${x.toFixed(1)}" y2="${y.toFixed(1)}" stroke="var(--border)" stroke-width="1"/>`;
  }).join("");
  const points = values.map((value, index) => pointFor(Math.max(0, value) / max, index).map((v) => v.toFixed(1)).join(",")).join(" ");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.classList.add("radar-svg");
  svg.setAttribute("viewBox", "0 0 256 236");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "Gráfico radar");
  const labelNodes = labels.map((label, index) => {
    const [x, y] = pointFor(1.2, index);
    return `<text x="${x.toFixed(1)}" y="${(y + 3).toFixed(1)}" fill="var(--text-2)" text-anchor="middle" font-size="9">${esc(trunc(label, 16))}</text>`;
  }).join("");
  svg.innerHTML = `${grid.map((pointsText) => `<polygon points="${pointsText}" fill="none" stroke="var(--border)" stroke-width="1"/>`).join("")}${axes}<polygon points="${points}" fill="rgba(47,111,237,0.22)" stroke="var(--accent)" stroke-width="2"/>${labelNodes}`;
  const legend = el("div", "radar-legend");
  labels.forEach((label, index) => {
    const row = el("div", "radar-legend-row");
    row.append(el("span", "radar-legend-dot"), el("span", "", label), el("span", "", fmtVal(values[index], res.unit)));
    legend.appendChild(row);
  });
  const wrap = el("div", "radar-wrap");
  wrap.append(svg, legend);
  box.appendChild(wrap);
}

function heatColor(ratio) {
  const alpha = 0.12 + Math.max(0, Math.min(1, ratio)) * 0.82;
  return `rgba(47, 111, 237, ${alpha.toFixed(3)})`;
}

function renderHeatmap(box, res, spec) {
  const xValues = res.x || [];
  const rows = (res.series || []).slice(0, 8);
  const max = Math.max(1e-9, ...dashboardValues(res));
  const scroll = el("div", "heatmap-scroll");
  const grid = el("div", "heatmap-grid");
  grid.style.gridTemplateColumns = `100px repeat(${xValues.length}, 30px)`;
  grid.appendChild(el("span", "heatmap-corner", ""));
  const labelStride = Math.max(1, Math.ceil(xValues.length / 8));
  xValues.forEach((value, index) => {
    const fullLabel = res.kind === "time" ? dashTimeLabel(value) : String(value);
    const label = el("span", "heatmap-xlabel", index % labelStride === 0 ? fullLabel : "");
    label.title = fullLabel;
    grid.appendChild(label);
  });
  rows.forEach((series, rowIndex) => {
    grid.appendChild(el("span", "heatmap-ylabel", series.name || `Série ${rowIndex + 1}`));
    xValues.forEach((value, columnIndex) => {
      const current = Number(series.points?.[columnIndex]) || 0;
      const cell = el("span", "heatmap-cell");
      cell.style.background = heatColor(current / max);
      cell.title = `${series.name || "Série"} · ${res.kind === "time" ? dashTimeLabel(value) : value}: ${fmtVal(current, res.unit)}`;
      grid.appendChild(cell);
    });
  });
  scroll.appendChild(grid);
  const wrap = el("div", "heatmap-wrap");
  wrap.append(scroll, el("span", "heatmap-caption", `${rows.length} ${rows.length === 1 ? "série" : "séries"} · intensidade relativa ao maior valor`));
  box.appendChild(wrap);
}

function renderKpi(box, res, spec) {
  const isTerms = res.kind === "terms" || spec.chart === "terms";
  const values = isTerms ? dashboardValues(res) : res.x.map((_, index) => sumSeriesAt(res, index));
  const value = isTerms ? values.reduce((sum, current) => sum + current, 0) : (values.at(-1) || 0);
  const reference = Math.max(0, ...values);
  const label = isTerms ? "Total dos valores apresentados" : "Último intervalo";
  const wrap = el("div", "kpi-wrap");
  wrap.append(
    el("strong", "", fmtVal(value, res.unit)),
    el("span", "", label),
    el("small", "", `Maior valor no recorte: ${fmtVal(reference, res.unit)}`),
  );
  box.appendChild(wrap);
}

function renderLineChart(box, res, id) {
  const xs = res.x.map((t) => Number(t) / 1000);
  const data = [xs, ...res.series.map((s) => s.points)];
  const holder = el("div");
  box.appendChild(holder);
  const axisColor = isLight() ? "#5b6678" : "#6b7690";
  dashCharts[id] = new uPlot(
    {
      width: Math.max(280, box.clientWidth - 8),
      height: state.dashboardCompact ? 112 : 170,
      legend: { show: res.series.length > 1 },
      cursor: { show: true, drag: { x: true, y: false, setScale: false } },
      scales: { x: { time: true } },
      axes: [
        { stroke: axisColor, grid: { show: false }, ticks: { show: false }, size: 22,
          values: (u, vals) => vals.map(v => new Date(v * 1000).toLocaleString("pt-BR", Number(res.x.at(-1)) - Number(res.x[0]) < 86400000 ? {hour:"2-digit",minute:"2-digit"} : {day:"2-digit",month:"2-digit",hour:"2-digit",minute:"2-digit"})) },
        {
          stroke: axisColor, grid: { stroke: isLight() ? "rgba(19,81,180,0.08)" : "rgba(255,255,255,0.06)" },
          ticks: { show: false }, size: 44,
          values: (u, vals) => vals.map((v) => fmtVal(v, res.unit)),
        },
      ],
      series: [
        { label: "Horário", value: (u, v) => v == null ? "—" : fmtTs(v * 1000) },
        ...res.series.map((s, i) => ({
          label: s.name,
          stroke: LEVEL_COLOR[s.name] ? getComputedStyle(document.documentElement).getPropertyValue(LEVEL_COLOR[s.name].slice(4, -1)).trim() : CHART_COLORS[i % CHART_COLORS.length],
          value: (u, v) => v == null ? "—" : fmtVal(v, res.unit),
          width: 1.6,
          fill: res.series.length === 1 ? "rgba(47,111,237,0.18)" : undefined,
          points: { show: false },
        })),
      ],
    },
    data,
    holder
  );
}

// ---------- editor de gráfico ----------
let chartEditing = null;
let chartEditingScope = "dataset";

function openChartEditor(spec, anchor, scope = state.analyticsScope) {
  chartEditing = spec;
  chartEditingScope = scope;
  $("#cp-title").value = spec.title;
  $("#cp-chart").value = spec.chart;
  $("#cp-metric").value = spec.metric;
  const field = $("#cp-field"), split = $("#cp-split");
  field.innerHTML = "";
  field.appendChild(el("option", "", "(contagem de eventos)")).value = "";
  for (const c of state.columns) field.appendChild(el("option", "", colLabel(c))).value = c;
  field.value = spec.field || "";
  split.innerHTML = "";
  split.appendChild(el("option", "", "(nenhum)")).value = "";
  for (const c of state.columns) split.appendChild(el("option", "", colLabel(c))).value = c;
  split.value = spec.split || "";
  $("#cp-interval").value = spec.interval_ms ? String(spec.interval_ms) : "";
  $("#cp-type").value = dashboardType(spec);
  syncDashboardChartEditor(false);
  $("#chart-modal").hidden = false;
}

function syncDashboardChartEditor(preferCompatibleBase) {
  const type = $("#cp-type").value;
  const chart = $("#cp-chart");
  const preferredBase = {
    bar: "terms", donut: "terms", gauge: "terms", radar: "terms", heatmap: "time",
  }[type];
  if (preferCompatibleBase && preferredBase) chart.value = preferredBase;
  const hints = {
    line: "Série temporal para acompanhar a evolução dos eventos.",
    bar: "Ranking para comparar os valores mais frequentes.",
    donut: "Participação de cada valor no total mostrado.",
    gauge: "Destaque para o maior valor e sua participação no total mostrado.",
    radar: "Comparação visual entre até oito valores do ranking.",
    heatmap: "Distribuição no tempo; use “Agrupar por” para criar uma linha por categoria.",
    kpi: "Um indicador principal para leitura rápida do recorte.",
  };
  $("#cp-type-hint").textContent = hints[type] || "";
}

function applyChartEditor() {
  if (!chartEditing) return;
  const charts = dashboardCharts(chartEditingScope) || [];
  const isNew = !charts.some((x) => x.id === chartEditing.id);
  Object.assign(chartEditing, {
    title: $("#cp-title").value.trim() || "Gráfico",
    chart: $("#cp-chart").value,
    metric: $("#cp-metric").value,
    field: $("#cp-field").value || null,
    split: $("#cp-split").value || null,
    interval_ms: $("#cp-interval").value ? parseInt($("#cp-interval").value, 10) : null,
    type: $("#cp-type").value,
  });
  if (isNew) charts.push(chartEditing);
  setDashboardCharts(charts, chartEditingScope);
  $("#chart-modal").hidden = true;
  chartEditing = null;
  renderDashboard(chartEditingScope);
}

// ==========================================================================
// CUBO (OLAP)
// ==========================================================================
const cubeState = {
  collapsed: new Set(), result: null, requestVersion: 0, dragField: null,
  results: new Map(),
};
let cubeChartEditing = null;

function newCubeTable(index = 1, seed = null) {
  return {
    id: `cube-table-${nid()}`,
    name: `Tabela ${index}`,
    rows: [...(seed?.rows || ["level"])],
    cols: [...(seed?.cols || [])],
    values: (seed?.values || [{ func: "count", column: "*", alias: "qtd" }]).map((value) => ({ ...value })),
    lastSchemaSignature: null,
    lastDataSignature: null,
  };
}

function normalizeCubeWorkspace(raw) {
  if (!raw?.tables || !Array.isArray(raw.tables)) {
    const table = newCubeTable(1, raw);
    return { tables: [table], charts: [], activeTableId: table.id, activeView: { type: "table", id: table.id } };
  }
  // Keep table identity: pending calculations and chart editors hold this reference.
  raw.tables = raw.tables.filter((table) => table && typeof table === "object").map((table, index) => Object.assign(table, {
    ...newCubeTable(index + 1, table), ...table,
    id: table.id || `cube-table-${nid()}`,
    name: table.name || `Tabela ${index + 1}`,
    rows: Array.isArray(table.rows) ? table.rows : [],
    cols: Array.isArray(table.cols) ? table.cols : [],
    values: Array.isArray(table.values) && table.values.length ? table.values : [{ func: "count", column: "*", alias: "qtd" }],
  }));
  if (!raw.tables.length) raw.tables.push(newCubeTable(1));
  raw.charts = Array.isArray(raw.charts) ? raw.charts.filter((chart) => chart && chart.snapshot) : [];
  if (!raw.tables.some((table) => table.id === raw.activeTableId)) raw.activeTableId = raw.tables[0].id;
  if (!raw.activeView || !["table", "chart"].includes(raw.activeView.type)) raw.activeView = { type: "table", id: raw.activeTableId };
  return raw;
}

function cubeWorkspace(scope = state.analyticsScope) {
  if (scope === "case") {
    const c = ensureCase();
    c.caseCube = normalizeCubeWorkspace(c.caseCube);
    return c.caseCube;
  }
  state.datasetCube = normalizeCubeWorkspace(state.datasetCube);
  return state.datasetCube;
}

function activeCube(scope = state.analyticsScope) {
  const workspace = cubeWorkspace(scope);
  let table = workspace.tables.find((item) => item.id === workspace.activeTableId);
  if (!table) {
    table = workspace.tables[0];
    workspace.activeTableId = table.id;
  }
  return table;
}

function cubeResultKey(scope, tableId) {
  return `${scope}:${tableId}`;
}

function cubeSchemaSignature(table) {
  return JSON.stringify({ rows: table.rows, cols: table.cols, values: table.values.map((value) => ({ func: value.func, column: value.column, alias: value.alias || "" })) });
}

function cubeFingerprint(value) {
  const text = JSON.stringify(value);
  let hash = 2166136261;
  for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619);
  return (hash >>> 0).toString(36);
}

function cubeDataSignature(result) {
  return cubeFingerprint({ rows: result.row_values || result.row_paths, cols: result.col_values || result.col_keys, values: result.value_names, cells: result.cells, totals: result.totals, incompatible: result.incompatible_units, units: result.value_units });
}

function cloneCubeResult(result) {
  return JSON.parse(JSON.stringify(result));
}

function saveActiveCube(scope = state.analyticsScope) {
  if (scope === "case") saveCases();
}

function markCubeTableChanged() {
  const table = activeCube();
  table.lastSchemaSignature = null;
  table.lastDataSignature = null;
  cubeState.lastComputedSignature = null;
  cubeState.results.delete(cubeResultKey(state.analyticsScope, table.id));
  saveActiveCube();
}

const FIELD_ICONS = {
  time: "fa-clock", number: "fa-hashtag", bytes: "fa-database",
  bits: "fa-tower-broadcast", duration: "fa-stopwatch", category: "fa-tag", text: "fa-font",
  bool: "fa-toggle-on", ip: "fa-network-wired", percent: "fa-percent", id: "fa-fingerprint",
};

async function openCube(scope = "dataset", { force = false } = {}) {
  const opening = Symbol("cube-opening");
  cubeState.openingRequest = opening;
  const contextKey = () => JSON.stringify([workspaceScope(), activeCase()?.id, scope === "case" ? caseSig() : [state.currentArtifact?.id, state.currentArtifact?.loadedAt]]);
  const openedContext = contextKey();
  const current = () => cubeState.openingRequest === opening && state.analyticsScope === scope && contextKey() === openedContext && !$("#view-cube").hidden;
  startOperation("cube", "Atualizando Cubo", "Preparando dimensões e medidas");
  if (state.analyticsScope !== scope) {
    cubeState.collapsed.clear();
    cubeState.result = null;
    cubeState.lastComputedSignature = null;
  }
  state.analyticsScope = scope;
  if (!scopeProfiles(scope) && scopeHasEvents(scope)) {
    try {
      const profiles = await api("profile_fields", analyticsRequest(scope));
      if (!current()) return;
      setScopeProfiles(profiles, scope);
    } catch {
      if (!current()) return;
      setScopeProfiles([], scope);
    }
  }
  if (!current()) return;
  renderCubeFields();
  renderCubeZones();
  renderCubeViews();

  const cube = activeCube(scope);
  const currentSig = JSON.stringify([scope, cube.id, cubeSchemaSignature(cube), backendFilters(), scope === "case" ? caseSig() : state.currentArtifact?.loadedAt, state.derivedFields]);
  if (!force && cubeState.result && cubeState.lastComputedSignature === currentSig && $("#cube-table tbody tr").length > 0) {
    finishOperation("Cubo pronto", "Recorte exibido do cache.");
    return;
  }
  await runCube({ force });
  if (current()) finishOperation("Cubo pronto", "Recorte calculado no escopo atual.");
}

function renderCubeFields() {
  const station = state.stationAnalyticsId
    ? caseStations().find((item) => item.id === state.stationAnalyticsId)
    : null;
  $("#cube-info").textContent = state.analyticsScope === "case"
    ? `${caseEvents().length} eventos${station ? ` da estação ${station.name}` : " selecionados no Caso"}`
    : state.loaded ? "Eventos da fonte carregada" : "Nenhuma fonte carregada";
  if (state.analyticsScope === "case") {
    $("#cube-info").textContent = `Caso: ${caseEvents().length} eventos enviados${station ? ` da estação ${station.name}` : ""}`;
  } else {
    $("#cube-info").textContent = state.loaded
      ? "Artefato aberto: eventos carregados"
      : "Artefato aberto: nenhuma fonte carregada";
  }
  const box = $("#cube-field-list");
  box.innerHTML = "";
  const kinds = Object.fromEntries((scopeProfiles() || []).map((p) => [p.name, p.kind]));
  for (const c of state.columns) {
    const f = el("div", "cube-field");
    f.draggable = true;
    f.dataset.field = c;
    const kind = kinds[c] || (c === "timestamp" ? "time" : "text");
    f.innerHTML = `<i class="fas ${FIELD_ICONS[kind] || "fa-font"}"></i><span class="fn" title="${esc(c)}">${esc(colLabel(c))}</span>`;
    const add = el("button", "cube-field-add");
    add.type = "button";
    add.innerHTML = '<i class="fas fa-plus"></i>';
    add.title = `Adicionar ${colLabel(c)} ao Cubo`;
    add.onclick = (event) => showCubeFieldActions(event, c);
    f.appendChild(add);
    f.onclick = (event) => {
      if (event.target.closest("button")) return;
      showCubeFieldActions(event, c);
    };
    f.ondragstart = (event) => beginCubeDrag(event, c);
    f.ondragend = endCubeDrag;
    f.oncontextmenu = (event) => showCubeFieldActions(event, c);
    box.appendChild(f);
  }
}

function showCubeFieldActions(event, field) {
  event.preventDefault();
  event.stopPropagation();
  showCtxMenu(event.clientX, event.clientY, [
    { icon: "fa-arrow-down", label: "Adicionar a Linhas", onClick: () => cubeAdd("rows", field) },
    { icon: "fa-arrow-right", label: "Adicionar a Colunas", onClick: () => cubeAdd("cols", field) },
    { icon: "fa-sigma", label: "Adicionar a Valores", onClick: () => cubeAdd("values", field) },
  ]);
}

function beginCubeDrag(event, field) {
  cubeState.dragField = field;
  event.currentTarget.classList.add("dragging");
  event.dataTransfer.effectAllowed = "copy";
  event.dataTransfer.setData("application/x-investigation-field", field);
  event.dataTransfer.setData("text/plain", field);
  document.body.classList.add("cube-dragging");
}

function endCubeDrag() {
  cubeState.dragField = null;
  document.body.classList.remove("cube-dragging");
  document.querySelectorAll(".cube-field.dragging").forEach((field) => field.classList.remove("dragging"));
  document.querySelectorAll(".cube-zone.dragover").forEach((zone) => zone.classList.remove("dragover"));
}

function dragField(event) {
  return event.dataTransfer?.getData("application/x-investigation-field")
    || event.dataTransfer?.getData("text/plain")
    || cubeState.dragField;
}

function cubeAdd(zone, field) {
  if (!field) return;
  const cube = activeCube();
  if (zone === "values") {
    if (cube.values.some((value) => value.func === "count" && value.column === field)) {
      toast(`${colLabel(field)} já está em Valores.`, "info");
      return;
    }
    cube.values.push({ func: "count", column: field, alias: "" });
  } else {
    if (cube[zone].includes(field)) {
      toast(`${colLabel(field)} já está em ${zone === "rows" ? "Linhas" : "Colunas"}.`, "info");
      return;
    }
    cube[zone].push(field);
  }
  markCubeTableChanged();
  renderCubeZones();
  runCube();
}

function cubeRemove(zone, idx) {
  const cube = activeCube();
  cube[zone].splice(idx, 1);
  markCubeTableChanged();
  renderCubeZones();
  runCube();
}

function renderCubeZones() {
  const cube = activeCube();
  for (const zone of ["rows", "cols", "values"]) {
    const box = $(`#cz-${zone}`);
    box.innerHTML = "";
    const items = zone === "values" ? cube.values : cube[zone];
    items.forEach((item, i) => {
      const chip = el("span", "cube-chip");
      const label = zone === "values"
        ? (item.alias || `${item.func}(${item.column === "*" ? "eventos" : item.column})`)
        : colLabel(item);
      chip.appendChild(el("span", "", label));
      chip.title = zone === "values" ? "Clique para editar" : colLabel(item);
      const x = el("button", "x");
      x.innerHTML = '<i class="fas fa-xmark"></i>';
      x.onclick = (e) => { e.stopPropagation(); cubeRemove(zone, i); };
      chip.appendChild(x);
      if (zone === "values") {
        chip.onclick = (e) => {
          e.stopPropagation();
          showCtxMenu(e.clientX, e.clientY,
            ["count", "count_distinct", "sum", "avg", "min", "max", "string_agg"].map((f) => ({
              icon: "fa-sigma",
              label: f,
              onClick: () => { item.func = f; markCubeTableChanged(); renderCubeZones(); runCube(); },
            }))
          );
        };
      }
      box.appendChild(chip);
    });
    if (!items.length) {
      box.appendChild(el("span", "cube-zone-empty", "Solte um campo aqui"));
    }
    // Área de soltura: aceita campos do painel e mantém o estado durante todo o arraste.
    const zoneEl = document.querySelector(`.cube-zone[data-zone="${zone}"]`);
    zoneEl.ondragenter = (event) => {
      event.preventDefault();
      zoneEl.classList.add("dragover");
    };
    zoneEl.ondragover = (event) => {
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      zoneEl.classList.add("dragover");
    };
    zoneEl.ondragleave = (event) => {
      if (!zoneEl.contains(event.relatedTarget)) zoneEl.classList.remove("dragover");
    };
    zoneEl.ondrop = (event) => {
      event.preventDefault();
      zoneEl.classList.remove("dragover");
      const field = dragField(event);
      endCubeDrag();
      if (field) cubeAdd(zone, field);
    };
  }
}

async function runCube({ force = false } = {}) {
  const version = ++cubeState.requestVersion;
  const scope = state.analyticsScope;
  const cube = activeCube(scope);
  const resultKey = cubeResultKey(scope, cube.id);
  const filters = backendFilters();
  const currentSig = JSON.stringify([scope, cube.id, cubeSchemaSignature(cube), filters, scope === "case" ? caseSig() : state.currentArtifact?.loadedAt, state.derivedFields]);

  if (!force && cubeState.result && cubeState.lastComputedSignature === currentSig && $("#cube-table tbody tr").length > 0) {
    return { status: "cached" };
  }

  const table = $("#cube-table");
  table.querySelector("thead").innerHTML = "";
  table.querySelector("tbody").innerHTML = "";
  if (!scopeHasEvents(scope)) {
    cubeState.result = null;
    cubeState.lastComputedSignature = null;
    cubeState.results.delete(resultKey);
    renderCubeViews();
    return { status: "empty" };
  }
  const loading = areaLoading(document.querySelector(".cube-output"), "Calculando Cubo…");
  try {
    updateOperation("Calculando Cubo", "Agregando dimensões e valores", 58);
    const res = await api("pivot", {
      ...analyticsRequest(scope),
      spec: { rows: cube.rows, cols: cube.cols, values: cube.values, limit_rows: 2000 },
    });
    if (version !== cubeState.requestVersion || scope !== state.analyticsScope || cube.id !== activeCube(scope).id) return { status: "stale" };
    if (res.complete === false) toast(`Resultado parcial: ${fmtNum(res.processed_events)} eventos analisados. Reduza as dimensões ou o período.`, "info");
    cubeState.result = res;
    cubeState.lastComputedSignature = currentSig;
    cubeState.results.set(resultKey, res);
    cube.lastSchemaSignature = cubeSchemaSignature(cube);
    cube.lastDataSignature = cubeDataSignature(res);
    saveActiveCube(scope);
    renderCubeTable(cube, res);
    renderCubeViews();
    finishOperation("Cruzamento atualizado");
    return { status: "success" };
  } catch (error) {
    if (version !== cubeState.requestVersion || scope !== state.analyticsScope || cube.id !== activeCube(scope).id) return { status: "stale" };
    cubeState.result = null;
    cubeState.lastComputedSignature = null;
    cubeState.results.delete(resultKey);
    finishOperation("Falha ao calcular Cubo", String(error));
    return { status: "error", error: String(error) };
  } finally {
    loading.done();
  }
}

function cubeResultForTable(table, scope = state.analyticsScope) {
  return cubeState.results.get(cubeResultKey(scope, table.id)) || null;
}

function cubeChartNeedsRefresh(chart, workspace = cubeWorkspace()) {
  const table = workspace.tables.find((item) => item.id === chart.sourceTableId);
  return !!table
    && table.lastSchemaSignature === chart.sourceSchemaSignature
    && table.lastDataSignature
    && table.lastDataSignature !== chart.sourceDataSignature;
}

function renderCubeViews() {
  const workspace = cubeWorkspace();
  if (!workspace.activeView || (workspace.activeView.type === "chart" && !workspace.charts.some((chart) => chart.id === workspace.activeView.id))) {
    workspace.activeView = { type: "table", id: workspace.activeTableId };
  }
  const list = $("#cube-view-list");
  list.innerHTML = "";

  workspace.tables.forEach((table) => {
    const button = el("button", "cube-view-btn");
    button.type = "button";
    button.title = table.name;
    button.setAttribute("aria-label", table.name);
    button.classList.toggle("active", workspace.activeView.type === "table" && workspace.activeView.id === table.id);
    button.innerHTML = '<i class="fas fa-table"></i>';
    button.onclick = () => openCubeTable(table.id);
    button.oncontextmenu = (event) => {
      event.preventDefault();
      showCtxMenu(event.clientX, event.clientY, [
        { icon: "fa-trash-can", label: "Remover esta tabela", danger: true, onClick: () => removeCubeTable(table.id) },
      ]);
    };
    list.appendChild(button);
  });

  workspace.charts.forEach((chart) => {
    const button = el("button", "cube-view-btn");
    button.type = "button";
    button.title = chart.name;
    button.setAttribute("aria-label", chart.name);
    button.classList.toggle("active", workspace.activeView.type === "chart" && workspace.activeView.id === chart.id);
    button.innerHTML = `<i class="fas ${chart.type === "donut" ? "fa-chart-pie" : "fa-chart-column"}"></i>`;
    if (cubeChartNeedsRefresh(chart, workspace)) button.classList.add("needs-refresh");
    button.onclick = () => openCubeChartView(chart.id);
    list.appendChild(button);
  });

  const tableView = $("#cube-table-view");
  const chartView = $("#cube-chart-view");
  const activeChart = workspace.activeView.type === "chart"
    ? workspace.charts.find((chart) => chart.id === workspace.activeView.id)
    : null;
  tableView.hidden = !!activeChart;
  chartView.hidden = !activeChart;
  if (activeChart) renderCubeChart(activeChart, chartView);
}

async function openCubeTable(tableId) {
  const workspace = cubeWorkspace();
  const table = workspace.tables.find((item) => item.id === tableId);
  if (!table) return;
  workspace.activeTableId = table.id;
  workspace.activeView = { type: "table", id: table.id };
  cubeState.collapsed.clear();
  renderCubeFields();
  renderCubeZones();
  renderCubeViews();
  const result = cubeResultForTable(table);
  if (result) {
    cubeState.result = result;
    renderCubeTable(table, result);
  } else {
    await runCube();
  }
  saveActiveCube();
}

function openCubeChartView(chartId) {
  const workspace = cubeWorkspace();
  if (!workspace.charts.some((chart) => chart.id === chartId)) return;
  workspace.activeView = { type: "chart", id: chartId };
  saveActiveCube();
  renderCubeViews();
}

function addCubeTable() {
  const workspace = cubeWorkspace();
  const table = newCubeTable(workspace.tables.length + 1, activeCube());
  workspace.tables.push(table);
  workspace.activeTableId = table.id;
  workspace.activeView = { type: "table", id: table.id };
  cubeState.collapsed.clear();
  saveActiveCube();
  renderCubeFields();
  renderCubeZones();
  renderCubeViews();
  runCube();
}

function removeCubeTable(tableId) {
  const workspace = cubeWorkspace();
  if (workspace.tables.length === 1) {
    toast("Mantenha ao menos uma tabela no Cubo.", "info");
    return;
  }
  workspace.tables = workspace.tables.filter((table) => table.id !== tableId);
  cubeState.results.delete(cubeResultKey(state.analyticsScope, tableId));
  if (workspace.activeTableId === tableId) workspace.activeTableId = workspace.tables[0].id;
  if (workspace.activeView.type === "table" && workspace.activeView.id === tableId) {
    workspace.activeView = { type: "table", id: workspace.activeTableId };
  }
  saveActiveCube();
  renderCubeFields();
  renderCubeZones();
  renderCubeViews();
  if (workspace.activeView.type === "table") openCubeTable(workspace.activeTableId);
}

function cubeChartPoints(chart) {
  const result = chart.snapshot;
  if (result.incompatible_units?.[chart.measureIndex]) return [];
  const table = cubeWorkspace().tables.find(item => item.id === chart.sourceTableId);
  const fn = result.value_functions?.[chart.measureIndex] || (table && cubeSchemaSignature(table) === chart.sourceSchemaSignature ? table.values?.[chart.measureIndex]?.func : null);
  if (chart.columnIndex === -1 && result.col_keys.length > 1 && !["count", "sum"].includes(fn)) return [];
  const dimensionCount = chart.sourceRows?.length || 0;
  const rows = dimensionCount
    ? result.row_paths.map((path, ri) => ({ path, ri })).filter((item) => item.path.length === dimensionCount)
    : [{ path: [], ri: result.row_paths.findIndex((path) => path.length === 0) }];
  const points = rows.map(({ path, ri }) => {
    const raw = chart.columnIndex === -1 ? result.cells[ri]?.map(column => column?.[chart.measureIndex]).filter(value => typeof value === "number" && Number.isFinite(value)) : [result.cells[ri]?.[chart.columnIndex]?.[chart.measureIndex]];
    const value = raw?.length && raw.every(value => typeof value === "number" && Number.isFinite(value)) ? raw.reduce((sum, value) => sum + value, 0) : null;
    const labels = result.row_values?.[ri]?.map(value => value == null ? "(vazio)" : value === "(vazio)" ? '“(vazio)”' : String(value)) || path;
    return { label: labels.length ? labels.join(" · ") : "Total", value };
  });
  return points.filter((point) => Number.isFinite(point.value)).slice(0, 32);
}

function renderCubeChart(chart, host = $("#cube-chart-view")) {
  host.innerHTML = "";
  const workspace = cubeWorkspace();
  const table = workspace.tables.find((item) => item.id === chart.sourceTableId);
  const head = el("div", "cube-chart-head");
  const title = el("div", "cube-chart-title");
  title.append(el("strong", "", chart.name), el("span", "muted small", table?.name || "Tabela removida"));
  head.appendChild(title);
  if (cubeChartNeedsRefresh(chart, workspace)) {
    const refresh = el("button", "btn primary small cube-chart-refresh", "Dados da tabela atualizados · Atualizar");
    refresh.onclick = () => refreshCubeChart(chart);
    head.appendChild(refresh);
  }
  const edit = el("button", "icon-btn");
  edit.title = "Editar gráfico";
  edit.innerHTML = '<i class="fas fa-pen"></i>';
  edit.onclick = () => openCubeChartModal(chart, chart.snapshot);
  const remove = el("button", "icon-btn");
  remove.title = "Remover visualizacao";
  remove.innerHTML = '<i class="fas fa-xmark"></i>';
  remove.onclick = () => {
    workspace.charts = workspace.charts.filter((item) => item.id !== chart.id);
    workspace.activeView = { type: "table", id: workspace.activeTableId };
    saveActiveCube();
    renderCubeViews();
  };
  head.append(edit, remove);
  host.appendChild(head);

  const body = el("div", "cube-chart-body");
  const points = cubeChartPoints(chart);
  if (!points.length) {
    const incompatible = chart.snapshot.incompatible_units?.[chart.measureIndex];
    body.appendChild(el("p", "muted", incompatible ? "Esta medida mistura unidades incompatíveis. Separe os registros por unidade e recalcule a tabela." : chart.columnIndex === -1 && chart.snapshot.col_keys.length > 1 ? "Esta medida não pode somar colunas. Edite o gráfico e escolha uma coluna para visualizar os valores corretos." : "A tabela não possui valores numéricos para este gráfico."));
  } else {
    const data = { kind: "terms", x: points.map(point => point.label), series: [{ points: points.map(point => point.value) }] };
    if (chart.type === "donut") renderDonut(body, data, {}, state.analyticsScope); else renderHBars(body, data, {}, state.analyticsScope);
    if (chart.snapshot.row_paths.filter(path => path.length === (chart.sourceRows?.length || 0)).length > points.length) body.append(el("p", "muted small", `Mostrando ${points.length} grupos com valores numéricos. O gráfico exibe até 32 grupos.`));
  }
  host.appendChild(body);
}

function renderCubeBars(host, points) {
  const max = Math.max(1, ...points.map((point) => point.value));
  const bars = el("div", "cube-bars");
  points.forEach((point) => {
    const row = el("div", "cube-bar-row");
    const label = el("span", "cube-bar-label", point.label);
    label.title = point.label;
    const track = el("div", "cube-bar-track");
    const fill = el("div", "cube-bar-fill");
    fill.style.width = `${(point.value / max) * 100}%`;
    track.appendChild(fill);
    row.append(label, track, el("strong", "cube-bar-value", fmtNum(point.value)));
    bars.appendChild(row);
  });
  host.appendChild(bars);
}

function renderCubeDonut(host, points) {
  const total = points.reduce((sum, point) => sum + point.value, 0) || 1;
  const radius = 64, cx = 76, cy = 76;
  let angle = -Math.PI / 2;
  const paths = [];
  points.forEach((point, index) => {
    const part = point.value / total;
    const next = angle + part * Math.PI * 2;
    const large = part > 0.5 ? 1 : 0;
    const x1 = cx + radius * Math.cos(angle), y1 = cy + radius * Math.sin(angle);
    const x2 = cx + radius * Math.cos(next), y2 = cy + radius * Math.sin(next);
    if (part > 0) paths.push(`<path d="M ${cx} ${cy} L ${x1.toFixed(1)} ${y1.toFixed(1)} A ${radius} ${radius} 0 ${large} 1 ${x2.toFixed(1)} ${y2.toFixed(1)} Z" fill="${CHART_COLORS[index % CHART_COLORS.length]}"/>`);
    angle = next;
  });
  const wrap = el("div", "cube-donut-wrap");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 152 152");
  svg.setAttribute("class", "cube-donut");
  svg.innerHTML = `${paths.join("")}<circle cx="${cx}" cy="${cy}" r="36" fill="var(--bg-2)"/><text x="${cx}" y="${cy + 5}" text-anchor="middle" fill="var(--text-0)" font-size="12">${fmtNum(total)}</text>`;
  const legend = el("div", "cube-chart-legend");
  points.slice(0, 12).forEach((point, index) => {
    const item = el("div", "cube-legend-item");
    const swatch = el("span", "cube-legend-swatch");
    swatch.style.background = CHART_COLORS[index % CHART_COLORS.length];
    item.append(swatch, el("span", "", point.label), el("strong", "", fmtNum(point.value)));
    legend.appendChild(item);
  });
  wrap.append(svg, legend);
  host.appendChild(wrap);
}

function setCubeChartType(type) {
  document.querySelectorAll("#cube-chart-type .seg-btn").forEach((button) => button.classList.toggle("active", button.dataset.type === type));
}

function openCubeChartModal(chart = null, sourceResult = null) {
  const workspace = cubeWorkspace();
  const table = chart
    ? workspace.tables.find((item) => item.id === chart.sourceTableId)
    : activeCube();
  const result = sourceResult || (table ? cubeResultForTable(table) : null);
  if (!table || !result) {
    toast("Calcule a tabela antes de criar um gráfico.", "info");
    return;
  }
  const refreshing = !!chart && sourceResult && sourceResult !== chart.snapshot;
  cubeChartEditing = {
    chartId: chart?.id || null,
    tableId: table.id,
    result: cloneCubeResult(result),
    sourceRows: refreshing || !chart ? [...table.rows] : [...(chart.sourceRows || table.rows)],
    sourceSchemaSignature: refreshing || !chart ? cubeSchemaSignature(table) : chart.sourceSchemaSignature,
    sourceDataSignature: refreshing || !chart ? cubeDataSignature(result) : chart.sourceDataSignature,
  };
  cubeChartEditing.result.value_functions = result.value_functions || (refreshing || !chart || cubeSchemaSignature(table) === chart.sourceSchemaSignature ? table.values.map(value => value.func) : []);
  $("#cube-chart-modal-title").textContent = chart ? "Atualizar gráfico do Cubo" : "Novo gráfico do Cubo";
  $("#cube-chart-apply").textContent = chart ? "Atualizar gráfico" : "Criar gráfico";
  $("#cube-chart-name").value = chart?.name || `${table.name} · ${result.value_names?.[0] || "Eventos"}`;
  const measure = $("#cube-chart-measure");
  measure.innerHTML = "";
  (result.value_names || ["Eventos"]).forEach((name, index) => measure.appendChild(el("option", "", name)).value = String(index));
  measure.value = String(chart?.measureIndex ?? 0);
  const column = $("#cube-chart-column");
  column.innerHTML = "";
  const allColumns = el("option", "", "Somar colunas"); allColumns.value = "-1"; column.append(allColumns);
  (result.col_keys || []).forEach((name, index) => column.appendChild(el("option", "", name === "(total)" ? "Total" : name)).value = String(index));
  column.value = String(chart?.columnIndex ?? -1);
  const validColumn = () => {
    const fn = cubeChartEditing.result.value_functions[Number(measure.value)];
    allColumns.disabled = result.col_keys.length > 1 && !["count", "sum"].includes(fn);
    if (allColumns.disabled && column.value === "-1") column.value = "0";
  };
  measure.onchange = validColumn; validColumn();
  setCubeChartType(chart?.type || "bar");
  $("#cube-chart-modal").hidden = false;
}

function applyCubeChart() {
  if (!cubeChartEditing) return;
  const workspace = cubeWorkspace();
  const table = workspace.tables.find((item) => item.id === cubeChartEditing.tableId);
  if (!table) return;
  const type = document.querySelector("#cube-chart-type .seg-btn.active")?.dataset.type || "bar";
  const current = workspace.charts.find((chart) => chart.id === cubeChartEditing.chartId);
  const chart = current || { id: `cube-chart-${nid()}` };
  Object.assign(chart, {
    name: $("#cube-chart-name").value.trim() || "Gráfico do Cubo",
    type,
    measureIndex: Number($("#cube-chart-measure").value || 0),
    columnIndex: Number($("#cube-chart-column").value || -1),
    sourceTableId: table.id,
    sourceRows: cubeChartEditing.sourceRows,
    sourceSchemaSignature: cubeChartEditing.sourceSchemaSignature,
    sourceDataSignature: cubeChartEditing.sourceDataSignature,
    snapshot: cloneCubeResult(cubeChartEditing.result),
    updatedAt: Date.now(),
  });
  if (!current) workspace.charts.push(chart);
  workspace.activeView = { type: "chart", id: chart.id };
  saveActiveCube();
  $("#cube-chart-modal").hidden = true;
  cubeChartEditing = null;
  renderCubeViews();
}

async function refreshCubeChart(chart) {
  const workspace = cubeWorkspace();
  const table = workspace.tables.find((item) => item.id === chart.sourceTableId);
  if (!table) return;
  workspace.activeTableId = table.id;
  let result = cubeResultForTable(table);
  if (!result) {
    workspace.activeView = { type: "table", id: table.id };
    renderCubeFields();
    renderCubeZones();
    await runCube();
    result = cubeResultForTable(table);
  }
  if (result) openCubeChartModal(chart, result);
}

function showCubeValueActions(event, field, value) {
  showChartValueActions(event, { chart: "terms", field }, value, state.analyticsScope);
}

function legacyRenderCubeTable(cube, res) {
  const thead = $("#cube-table thead");
  const tbody = $("#cube-table tbody");
  thead.innerHTML = "";
  tbody.innerHTML = "";
  return renderCubeTable(cube, res);

  // cabeçalho: 2 linhas se houver colunas
  const nCols = Math.max(1, res.col_keys.length);
  const tr1 = el("tr");
  const rowHeader = el("th", "rowh", cube.rows.map(colLabel).join(" › ") || "Linhas");
  rowHeader.rowSpan = res.col_keys.length && res.col_keys[0] !== "(total)" ? 2 : 1;
  tr1.appendChild(rowHeader);
  const multiCols = res.col_keys.length > 1 || res.col_keys[0] !== "(total)";
  if (multiCols) {
    for (const ck of res.col_keys) {
      const th = el("th", "", ck);
      th.colSpan = res.value_names.length;
      tr1.appendChild(th);
    }
    thead.appendChild(tr1);
    const tr2 = el("tr");
    for (let ci = 0; ci < nCols; ci++) {
      for (const vn of res.value_names) tr2.appendChild(el("th", "", vn));
    }
    thead.appendChild(tr2);
  } else {
    for (const vn of res.value_names) tr1.appendChild(el("th", "", vn));
    thead.appendChild(tr1);
  }

  // linhas (árvore com expand/collapse)
  const isHidden = (path) => {
    for (const cp of cubeState.collapsed) {
      if (path.length > cp.length && cp.every((v, i) => path[i] === v)) return true;
    }
    return false;
  };
  const childMap = new Set(res.row_paths.map((p) => p.slice(0, -1).join("\u001f")));

  res.row_paths.forEach((path, ri) => {
    if (isHidden(path)) return;
    const tr = el("tr");
    const td = el("td", "rowh");
    td.style.paddingLeft = `${(path.length - 1) * 16 + 12}px`;
    const pkey = path.join("\u001f");
    if (childMap.has(pkey)) {
      const t = el("button", "cube-toggle");
      const isCollapsed = cubeState.collapsed.has(path);
      t.innerHTML = `<i class="fas fa-angle-${isCollapsed ? "right" : "down"}"></i>`;
      t.onclick = (e) => {
        e.stopPropagation();
        const key = [...cubeState.collapsed].find((cp) => cp.join("\u001f") === pkey);
        if (key) cubeState.collapsed.delete(key);
        else cubeState.collapsed.add(path);
        renderCubeTable(cube, res);
      };
      td.appendChild(t);
    } else {
      td.appendChild(el("span", "cube-toggle"));
    }
    td.appendChild(document.createTextNode(path[path.length - 1]));
    const field = cube.rows[path.length - 1];
    const value = path[path.length - 1];
    td.oncontextmenu = (e) => showCubeValueActions(e, field, value);
    tr.appendChild(td);
    for (let ci = 0; ci < nCols; ci++) {
      for (let vi = 0; vi < res.value_names.length; vi++) {
        const v = res.cells[ri]?.[ci]?.[vi];
        const cell = el("td", "", v === null || v === undefined ? "" : fmtNum(Number(v) || v));
        cell.title = "Clique com o botão direito para ações";
        cell.oncontextmenu = (e) => showCubeValueActions(e, field, value);
        tr.appendChild(cell);
      }
    }
    tbody.appendChild(tr);
  });

  // totais
  const trt = el("tr", "total");
  trt.appendChild(el("td", "rowh", "Total"));
  for (let ci = 0; ci < nCols; ci++) {
    for (let vi = 0; vi < res.value_names.length; vi++) {
      const v = res.totals[ci]?.[vi];
      trt.appendChild(el("td", "", v === null || v === undefined ? "" : fmtNum(Number(v) || v)));
    }
  }
  tbody.appendChild(trt);
  if (res.truncated) {
    const tr = el("tr");
    const td = el("td", "muted small", res.complete === false ? `Resultado parcial: ${fmtNum(res.processed_events)} eventos analisados. Reduza as dimensões ou o período.` : "Exibição limitada a 2.000 linhas; totais calculados sobre o recorte completo.");
    td.colSpan = 1 + nCols * res.value_names.length;
    tr.appendChild(td);
    tbody.appendChild(tr);
  }
}

function renderCubeTable(cube, res) {
  const thead = $("#cube-table thead");
  const tbody = $("#cube-table tbody");
  const columnKeys = res.col_keys?.length ? res.col_keys : ["(total)"];
  const valueNames = res.value_names?.length ? res.value_names : ["Eventos"];
  const nCols = columnKeys.length;
  const nValues = valueNames.length;
  const hasColumnGroups = nCols > 1 || columnKeys[0] !== "(total)";
  const headerRows = hasColumnGroups ? 2 : 1;
  const dimensionCount = Math.max(1, cube.rows.length);

  const headerTop = el("tr", "cube-header-top");
  if (cube.rows.length) {
    cube.rows.forEach((field) => {
      const th = el("th", "rowh cube-dimension-head", colLabel(field));
      th.rowSpan = headerRows;
      headerTop.appendChild(th);
    });
  } else {
    const th = el("th", "rowh cube-dimension-head", "Eventos");
    th.rowSpan = headerRows;
    headerTop.appendChild(th);
  }

  if (hasColumnGroups) {
    columnKeys.forEach((key) => {
      const th = el("th", "cube-column-group", key === "(total)" ? "Todos" : key);
      th.colSpan = nValues;
      headerTop.appendChild(th);
    });
    thead.appendChild(headerTop);
    const headerBottom = el("tr", "cube-header-bottom");
    columnKeys.forEach(() => valueNames.forEach((name) => {
      headerBottom.appendChild(el("th", "cube-measure-head", name));
    }));
    thead.appendChild(headerBottom);
  } else {
    valueNames.forEach((name) => headerTop.appendChild(el("th", "cube-measure-head", name)));
    thead.appendChild(headerTop);
  }

  const pathKey = (path) => path.join("\u001f");
  const samePath = (a, b) => a.length === b.length && a.every((value, i) => value === b[i]);
  const pathIndex = new Map(res.row_paths.map((path, ri) => [pathKey(path), ri]));
  const leafLength = cube.rows.length;
  const leaves = leafLength
    ? res.row_paths.map((path, ri) => ({ path, ri })).filter(({ path }) => path.length === leafLength)
    : [{ path: [], ri: pathIndex.get("") ?? 0 }];

  const collapsedFor = (path) => {
    for (const prefix of cubeState.collapsed) {
      if (prefix.length && prefix.length < path.length && prefix.every((value, i) => path[i] === value)) return prefix;
    }
    return null;
  };
  const displayRows = [];
  const displayedCollapsed = new Set();
  leaves.forEach((leaf) => {
    const prefix = collapsedFor(leaf.path);
    if (!prefix) {
      displayRows.push({ type: "leaf", ...leaf });
      return;
    }
    const key = pathKey(prefix);
    if (!displayedCollapsed.has(key)) {
      displayedCollapsed.add(key);
      displayRows.push({ type: "collapsed", path: prefix, ri: pathIndex.get(key) });
    }
  });

  const toggle = (path, isCollapsed) => {
    const button = el("button", "cube-toggle");
    button.innerHTML = `<i class="fas fa-angle-${isCollapsed ? "right" : "down"}"></i>`;
    button.title = isCollapsed ? "Expandir grupo" : "Recolher grupo";
    button.onclick = (event) => {
      event.stopPropagation();
      const savedPath = [...cubeState.collapsed].find((saved) => samePath(saved, path));
      if (savedPath) cubeState.collapsed.delete(savedPath);
      else cubeState.collapsed.add([...path]);
      renderCubeTable(cube, res);
    };
    return button;
  };
  const appendValues = (tr, ri, field, value) => {
    for (let ci = 0; ci < nCols; ci++) {
      for (let vi = 0; vi < nValues; vi++) {
        const raw = res.cells[ri]?.[ci]?.[vi];
        const cell = el("td", "cube-value", raw === null || raw === undefined ? "" : fmtNum(Number(raw) || raw));
        cell.title = "Clique com o botao direito para acoes";
        if (field !== undefined) cell.oncontextmenu = (event) => showCubeValueActions(event, field, value);
        tr.appendChild(cell);
      }
    }
  };
  const sameGroupAbove = (index, dimension) => {
    const current = displayRows[index];
    const previous = displayRows[index - 1];
    return current?.type === "leaf" && previous?.type === "leaf"
      && current.path.slice(0, dimension + 1).every((value, i) => previous.path[i] === value);
  };
  const groupSpan = (index, dimension) => {
    const prefix = displayRows[index].path.slice(0, dimension + 1);
    let span = 1;
    for (let nextIndex = index + 1; nextIndex < displayRows.length; nextIndex++) {
      const next = displayRows[nextIndex];
      if (next.type !== "leaf" || !prefix.every((value, i) => next.path[i] === value)) break;
      span++;
    }
    return span;
  };

  displayRows.forEach((entry, index) => {
    const field = cube.rows[Math.max(0, entry.path.length - 1)];
    const value = entry.path.at(-1);
    if (entry.type === "collapsed") {
      const tr = el("tr", "cube-collapsed-group");
      const td = el("td", "rowh cube-collapsed-label");
      td.colSpan = dimensionCount;
      td.appendChild(toggle(entry.path, true));
      td.appendChild(document.createTextNode(`${colLabel(field)}: ${value}`));
      td.oncontextmenu = (event) => showCubeValueActions(event, field, value);
      tr.appendChild(td);
      appendValues(tr, entry.ri, field, value);
      tbody.appendChild(tr);
      return;
    }

    const tr = el("tr", "cube-leaf-row");
    if (cube.rows.length) {
      cube.rows.forEach((dimension, di) => {
        if (sameGroupAbove(index, di)) return;
        const td = el("td", "rowh cube-dimension", entry.path[di]);
        td.rowSpan = groupSpan(index, di);
        if (di < cube.rows.length - 1) td.appendChild(toggle(entry.path.slice(0, di + 1), false));
        else td.appendChild(el("span", "cube-toggle"));
        td.oncontextmenu = (event) => showCubeValueActions(event, dimension, entry.path[di]);
        tr.appendChild(td);
      });
    } else {
      tr.appendChild(el("td", "rowh cube-dimension", "Todos os eventos"));
    }
    appendValues(tr, entry.ri, field, value);
    tbody.appendChild(tr);
  });

  const total = el("tr", "total cube-grand-total");
  const totalLabel = el("td", "rowh", "Total");
  totalLabel.colSpan = dimensionCount;
  total.appendChild(totalLabel);
  for (let ci = 0; ci < nCols; ci++) {
    for (let vi = 0; vi < nValues; vi++) {
      const raw = res.totals[ci]?.[vi];
      total.appendChild(el("td", "cube-value", raw === null || raw === undefined ? "" : fmtNum(Number(raw) || raw)));
    }
  }
  tbody.appendChild(total);
  if (res.truncated) {
    const tr = el("tr");
    const td = el("td", "muted small", res.complete === false ? `Resultado parcial: ${fmtNum(res.processed_events)} eventos analisados. Reduza as dimensões ou o período.` : "Exibição limitada a 2.000 linhas; totais calculados sobre o recorte completo.");
    td.colSpan = dimensionCount + nCols * nValues;
    tr.appendChild(td);
    tbody.appendChild(tr);
  }
}

function cubeToChart() {
  openCubeChartModal();
}
