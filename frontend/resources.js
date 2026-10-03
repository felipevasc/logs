/* Native resource samples are independent of Cases and investigation requests. */
window.Resources = (() => {
  "use strict";
  const call = (command, args = {}) => window.__TAURI__.core.invoke(command, args);
  const button = document.querySelector("#btn-resources");
  const number = value => typeof value === "number" && Number.isFinite(value);
  const decimalFormat = new Intl.NumberFormat("pt-BR", { maximumFractionDigits: 1 });
  const decimal = value => number(value) ? decimalFormat.format(value) : "—";
  const percent = value => number(value) ? `${decimal(value)}%` : "Indisponível";
  const bytes = value => {
    if (!number(value) || value < 0) return "Indisponível";
    const units = ["B", "KiB", "MiB", "GiB", "TiB"], index = value > 0 ? Math.min(4, Math.floor(Math.log(value) / Math.log(1024))) : 0;
    return `${decimal(value / 1024 ** index)} ${units[index]}`;
  };
  const rate = value => number(value) ? `${bytes(value)}/s` : "Indisponível";
  const time = value => number(value) ? new Date(value).toLocaleTimeString("pt-BR") : "—";
  const badge = (kind, label) => `<span class="resource-badge" data-kind="${kind}">${label}</span>`;
  const basis = value => ({ measured: ["measured", "Medido"], estimated: ["estimate", "Estimativa"], logical: ["logical", "Tamanho lógico"], unavailable: ["unavailable", "Indisponível"] }[value] || ["unavailable", "Indisponível"]);
  let dialog = null, timer = null, version = 0, pending = null, latest = null, previousFocus = null, background = [], paused = false;

  function metric(label, value, detail, available = true) {
    return `<div class="resource-metric"><div class="resource-metric-label"><span>${esc(label)}</span>${badge(available ? latest?.preview ? "preview" : "measured" : "unavailable", available ? latest?.preview ? "Simulado" : "Medido" : "Indisponível")}</div><strong class="resource-value">${esc(value)}</strong><p class="resource-detail">${esc(detail)}</p></div>`;
  }
  function table(columns, rows, empty) {
    return rows.length ? `<table class="resources-table"><thead><tr>${columns.map(column => `<th scope="col">${esc(column)}</th>`).join("")}</tr></thead><tbody>${rows.join("")}</tbody></table>` : `<p class="resources-empty">${esc(empty)}</p>`;
  }
  function setStatus(message, error = false) {
    if (!dialog) return;
    const status = dialog.querySelector("[data-resource-status]");
    status.textContent = message; status.dataset.error = String(error);
  }
  function render(snapshot) {
    if (!dialog) return;
    const app = snapshot.app || {}, host = snapshot.host || {};
    dialog.querySelector("[data-resource-app]").innerHTML = [
      metric("CPU do aplicativo", percent(app.cpuPercent), number(app.oneCoreCpuPercent) ? `${decimal(app.oneCoreCpuPercent)}% de um núcleo · 100% = toda a máquina` : "100% = toda a capacidade da máquina", number(app.cpuPercent)),
      metric("RAM residente", bytes(app.residentBytes), `${number(app.memoryPercent) ? `${percent(app.memoryPercent)} da RAM · ` : ""}${decimal(app.processCount)} processos próprios; páginas compartilhadas podem ser contadas mais de uma vez.`, number(app.residentBytes)),
      metric("Leitura do aplicativo", rate(app.readBytesPerSec), `Acumulado nos processos próprios vivos: ${bytes(app.totalReadBytes)}`, number(app.readBytesPerSec)),
      metric("Gravação do aplicativo", rate(app.writtenBytesPerSec), `Acumulado nos processos próprios vivos: ${bytes(app.totalWrittenBytes)}`, number(app.writtenBytesPerSec)),
    ].join("");
    dialog.querySelector("[data-resource-host-description]").textContent = [host.os, host.cpuBrand].filter(Boolean).join(" · ") || "Informações do sistema indisponíveis.";
    dialog.querySelector("[data-resource-host]").innerHTML = [
      metric("CPU do sistema", percent(host.cpuPercent), "Inclui o aplicativo e os demais processos do sistema.", number(host.cpuPercent)),
      metric("RAM usada no sistema", bytes(host.usedMemoryBytes), `${bytes(host.availableMemoryBytes)} disponíveis de ${bytes(host.totalMemoryBytes)}`, number(host.usedMemoryBytes)),
      metric("Swap / paginação", bytes(host.usedSwapBytes), `Capacidade informada pelo sistema: ${bytes(host.totalSwapBytes)}`, number(host.usedSwapBytes)),
      metric("Processadores lógicos", decimal(host.logicalCpus), `${number(host.physicalCores) ? `${decimal(host.physicalCores)} núcleos físicos` : "Núcleos físicos indisponíveis"}${number(host.cpuFrequencyMhz) ? ` · ${decimal(host.cpuFrequencyMhz)} MHz` : ""}`, number(host.logicalCpus)),
    ].join("");
    dialog.querySelector("[data-resource-processes]").innerHTML = table(["Processo próprio", "CPU", "RAM residente", snapshot.memoryVirtualLabel || "Memória virtual", "Leitura", "Gravação"], (snapshot.processes || []).map(process => `<tr><td>${esc(process.name || "Processo")}<small>PID ${esc(process.pid)} · ${esc(process.role || "Aplicativo")}${number(process.parentPid) ? ` · pai ${esc(process.parentPid)}` : ""}</small></td><td>${esc(percent(process.cpuPercent))}</td><td>${esc(bytes(process.residentBytes))}</td><td>${esc(bytes(process.virtualBytes))}</td><td>${esc(rate(process.readBytesPerSec))}</td><td>${esc(rate(process.writtenBytesPerSec))}</td></tr>`), "Nenhum processo próprio disponível nesta amostra.");
    dialog.querySelector("[data-resource-disks]").innerHTML = table(["Volume do sistema", "Capacidade", "Disponível", "Leitura", "Gravação"], (snapshot.disks || []).map(disk => `<tr><td>${esc(disk.name || disk.mountPoint || "Volume")}${disk.isAppVolume ? ` ${badge("measured", "Volume do aplicativo")}` : ""}<small>${esc(disk.mountPoint || "")} · ${esc(disk.kind || "Tipo indisponível")}</small></td><td>${esc(bytes(disk.totalBytes))}</td><td>${esc(bytes(disk.availableBytes))}</td><td>${esc(rate(disk.readBytesPerSec))}</td><td>${esc(rate(disk.writtenBytesPerSec))}</td></tr>`), "Volumes indisponíveis nesta amostra.");
    const inventory = snapshot.inventory || {};
    dialog.querySelector("[data-resource-inventory-description]").textContent = `Heap conhecido: ${bytes(inventory.memoryKnownBytes)} · soma parcial das estruturas internas; não corresponde à RAM residente dos processos.`;
    dialog.querySelector("[data-resource-inventory]").innerHTML = table(["Componente interno", "Heap", "Mapa lógico", "Disco lógico", "Itens"], (inventory.components || []).map(component => {
      const [kind, label] = basis(component.basis);
      return `<tr><td>${esc(component.label || component.id)} ${badge(kind, label)}<small>${esc(component.note || "")}</small></td><td>${esc(bytes(component.memoryBytes))}</td><td>${esc(bytes(component.mappedBytes))}</td><td>${esc(bytes(component.storageBytes))}</td><td>${esc(decimal(component.items))}</td></tr>`;
    }), "Inventário interno indisponível nesta amostra.");
    renderStorage(snapshot.storage);
    renderActions(snapshot.actions || {});
    dialog.querySelector("[data-resource-notes]").replaceChildren(...(snapshot.notes || []).map(note => { const item = document.createElement("li"); item.textContent = note; return item; }));
    renderChart();
    dialog.querySelector("[data-resource-time]").textContent = `Amostra: ${time(snapshot.sampledAtMs)}${snapshot.warmingUp ? " · aguardando intervalo para taxas de CPU e I/O" : ""}`;
    dialog.querySelector("[data-resource-export]").disabled = false;
    setStatus(`${snapshot.preview ? "Prévia do navegador · métricas simuladas" : snapshot.warmingUp ? "Preparando a primeira medição de taxas…" : "Monitoramento ativo · atualização a cada segundo"}${paused ? " · visualização pausada" : ""}`);
  }
  function renderStorage(storage) {
    const description = dialog.querySelector("[data-resource-storage-description]");
    if (!storage) { description.textContent = "Inventário de arquivos do aplicativo indisponível."; dialog.querySelector("[data-resource-storage]").innerHTML = ""; return; }
    const volume = (latest?.disks || []).find(disk => disk.isAppVolume && number(disk.totalBytes) && disk.totalBytes > 0);
    const share = volume && !storage.partial && number(storage.bytes) ? ` Arquivos do aplicativo: ${(100 * storage.bytes / volume.totalBytes).toLocaleString("pt-BR", { maximumFractionDigits: 3 })}% da capacidade deste volume.` : "";
    const volumeInfo = volume ? ` Volume do aplicativo: ${bytes(volume.availableBytes)} disponíveis de ${bytes(volume.totalBytes)}.${share}` : "";
    description.textContent = `${storage.partial ? "≥ " : ""}${bytes(storage.bytes)} em ${decimal(storage.files)} arquivos · ${storage.partial ? "levantamento parcial; total completo indisponível" : "levantamento concluído"}${storage.stale ? " · amostra antiga" : ""} · ${time(storage.sampledAt)}.${volumeInfo} ${storage.root || ""}. ${storage.note || ""}`;
    dialog.querySelector("[data-resource-storage]").innerHTML = table(["Arquivos do aplicativo", "Tamanho lógico", "Arquivos"], (storage.paths || []).map(path => `<tr><td>${esc(path.path)}${path.partial ? ` ${badge("estimate", "Parcial")}` : ""}</td><td>${esc(bytes(path.bytes))}</td><td>${esc(decimal(path.files))}</td></tr>`), "Nenhum arquivo catalogado nesta amostra.");
  }
  function renderChart() {
    if (!latest || !dialog) return;
    const metric = dialog.querySelector("[data-resource-chart-metric]").value;
    const minutes = Number(dialog.querySelector("[data-resource-chart-range]").value);
    const end = latest.sampledAtMs, cutoff = end - minutes * 60000;
    const points = (latest.history || []).filter(point => number(point.timestampMs) && point.timestampMs >= cutoff && point.timestampMs <= end);
    const fields = metric === "cpu" ? ["appCpuPercent", "hostCpuPercent"] : metric === "memory" ? ["appMemoryBytes", "hostMemoryUsedBytes"] : ["readBytesPerSec", "writtenBytesPerSec"];
    const labels = metric === "io" ? ["Leitura do aplicativo", "Gravação do aplicativo"] : ["Aplicativo", "Sistema"];
    const valid = points.flatMap(point => fields.map(field => point[field]).filter(number));
    const max = metric === "cpu" ? 100 : Math.max(1, ...valid);
    const minTime = Math.min(cutoff, points[0]?.timestampMs ?? cutoff), span = Math.max(1000, end - minTime);
    const segments = field => {
      let paths = [], current = [];
      for (const point of points) {
        if (!number(point[field])) { if (current.length) paths.push(current); current = []; continue; }
        current.push(`${4 + (point.timestampMs - minTime) / span * 592},${126 - Math.max(0, Math.min(max, point[field])) / max * 120}`);
      }
      if (current.length) paths.push(current);
      return paths.map(path => `<polyline points="${path.join(" ")}" />`).join("");
    };
    const title = metric === "cpu" ? "Histórico de uso de CPU" : metric === "memory" ? "Histórico de RAM" : "Histórico de leitura e gravação do aplicativo";
    const scale = metric === "cpu" ? "0–100% da máquina" : `0–${metric === "io" ? rate(max) : bytes(max)}`;
    const svg = dialog.querySelector("[data-resource-chart]");
    svg.setAttribute("aria-label", `${title}. ${valid.length ? `${points.length} amostras; escala ${scale}.` : "Medição indisponível neste intervalo."}`);
    svg.innerHTML = `<line class="resource-chart-grid" x1="4" x2="596" y1="6" y2="6"/><line class="resource-chart-grid" x1="4" x2="596" y1="66" y2="66"/><line class="resource-chart-grid" x1="4" x2="596" y1="126" y2="126"/><g class="resource-chart-app">${segments(fields[0])}</g><g class="resource-chart-system">${segments(fields[1])}</g>`;
    dialog.querySelector("[data-resource-chart-legend]").innerHTML = labels.map(label => `<span>${esc(label)}</span>`).join("");
    dialog.querySelector("[data-resource-chart-scale]").textContent = valid.length ? `${points.length} amostras · ${scale}` : "Sem medições disponíveis neste intervalo";
    dialog.querySelector("[data-resource-chart-period]").textContent = `${time(minTime)} — ${time(end)}`;
  }
  function renderActions(actions) {
    // The task registry is shown here separately from measured process totals.
    const stateLabel = state => state === "active" ? "Em andamento" : state === "completed" ? "Finalizada" : "Estado indisponível";
    dialog.querySelector("[data-resource-operations-description]").textContent = `${decimal(actions.totalCompleted)} operações finalizadas${actions.untrackedActive ? ` · ${decimal(actions.untrackedActive)} tarefas ativas sem detalhamento` : ""}. ${actions.cpuBasis || "CPU da thread iniciadora; exclui workers paralelos e o renderizador."} Finalizada significa que terminou; não implica sucesso.`;
    dialog.querySelector("[data-resource-operations]").innerHTML = table(["Operação interna", "Estado", "Início", "Duração", "CPU da thread"], [...(actions.active || []), ...(actions.recent || [])].map(action => `<tr><td>${esc(action.label || "Operação")}${action.progress ? `<small>${esc(action.progress.phase)} · ${esc(decimal(action.progress.completed))}${number(action.progress.total) && action.progress.total > 0 ? ` / ${esc(decimal(action.progress.total))}` : ""} ${esc(action.progress.unit || "")}</small>` : ""}</td><td>${esc(stateLabel(action.state))}</td><td>${esc(time(action.startedAtMs))}</td><td>${number(action.elapsedMs) ? `${decimal(action.elapsedMs / 1000)} s` : "—"}</td><td>${number(action.threadCpuMs) ? `${decimal(action.threadCpuMs)} ms` : "Indisponível"}</td></tr>`), "Nenhuma operação interna registrada.");
  }
  async function refresh(force = false) {
    if (!dialog || paused && !force) return null;
    if (pending?.version === version) return pending.promise;
    const request = version;
    clearTimeout(timer);
    dialog.querySelector("[data-resource-refresh]").disabled = true;
    const promise = call("resource_snapshot").then(snapshot => {
      if (request !== version || !dialog || paused && !force) return null;
      if (!snapshot || !number(snapshot.sampledAtMs)) throw new Error("Amostra de recursos indisponível.");
      latest = snapshot; render(snapshot); return snapshot;
    }).catch(error => {
      if (request === version && dialog) {
        const warming = !latest && /Primeira coleta(?: de recursos)? em andamento/i.test(String(error));
        setStatus(warming ? "Aguardando a primeira coleta de recursos…" : `${latest ? `Amostra anterior de ${time(latest.sampledAtMs)} mantida. ` : ""}Não foi possível medir: ${error}`, !warming);
      }
      return null;
    }).finally(() => {
      if (pending?.promise === promise) pending = null;
      if (request === version && dialog) { dialog.querySelector("[data-resource-refresh]").disabled = false; if (!paused) timer = setTimeout(() => refresh(), 1000); }
    });
    pending = { version: request, promise };
    return promise;
  }
  function exportSnapshot() {
    if (!latest) return;
    const blob = new Blob([JSON.stringify(latest, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob), link = document.createElement("a");
    link.href = url; link.download = `loginsight-recursos-${new Date(latest.sampledAtMs).toISOString().replace(/[:.]/g, "-")}.json`;
    link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  function togglePause() {
    paused = !paused; clearTimeout(timer);
    const control = dialog.querySelector("[data-resource-pause]");
    control.setAttribute("aria-pressed", String(paused));
    control.innerHTML = `<i class="fas fa-${paused ? "play" : "pause"}" aria-hidden="true"></i> ${paused ? "Retomar" : "Pausar"}`;
    if (paused) setStatus(`${latest?.preview ? "Prévia do navegador · métricas simuladas · " : ""}Visualização pausada · a coleta nativa continua`);
    else refresh();
  }
  function close() {
    if (!dialog) return;
    version++; clearTimeout(timer); timer = null; dialog.remove(); dialog = null;
    for (const [node, inert] of background) node.inert = inert;
    background = []; button?.setAttribute("aria-expanded", "false");
    if (previousFocus?.isConnected) previousFocus.focus(); else button?.focus();
  }
  function open() {
    if (dialog) { dialog.querySelector("[data-resource-close]").focus(); return; }
    previousFocus = document.activeElement; version++; latest = null; paused = false;
    dialog = document.createElement("div"); dialog.className = "modal-overlay resources-overlay";
    dialog.id = "resources-overlay";
    dialog.innerHTML = `<section class="modal resources-modal" role="dialog" aria-modal="true" aria-labelledby="resources-title" aria-describedby="resources-intro"><div class="modal-head"><h3 id="resources-title"><i class="fas fa-gauge-high" aria-hidden="true"></i> Gerenciador de recursos</h3><button class="icon-btn" type="button" data-resource-close aria-label="Fechar gerenciador de recursos"><i class="fas fa-xmark" aria-hidden="true"></i></button></div><div class="modal-body">
      <div class="resources-toolbar"><p id="resources-intro">Aplicativo e sistema · medições independentes do Caso aberto</p><div class="resources-toolbar-actions"><button class="btn ghost small" type="button" data-resource-export disabled><i class="fas fa-download" aria-hidden="true"></i> Exportar JSON</button><button class="btn ghost small" type="button" data-resource-pause aria-pressed="false"><i class="fas fa-pause" aria-hidden="true"></i> Pausar</button><button class="btn ghost small" type="button" data-resource-refresh><i class="fas fa-rotate" aria-hidden="true"></i> Atualizar agora</button></div></div>
      <div><span class="resources-status" data-resource-status role="status">Coletando recursos…</span><p class="resources-notes" data-resource-time></p></div>
      <div class="resources-columns"><section class="resources-section"><h4>LogInsight e seus processos</h4><p class="resources-description">Consumo medido no sistema operacional. CPU e I/O usam a diferença entre amostras.</p><div class="resources-metrics" data-resource-app></div></section><section class="resources-section"><h4>Sistema operacional</h4><p class="resources-description" data-resource-host-description></p><div class="resources-metrics" data-resource-host></div></section></div>
      <section class="resources-history"><div class="resources-chart-head"><h4>Histórico de recursos</h4><div><label>Métrica <select data-resource-chart-metric><option value="cpu">CPU</option><option value="memory">RAM</option><option value="io">Leitura e gravação</option></select></label> <label>Intervalo <select data-resource-chart-range><option value="1">1 minuto</option><option value="5" selected>5 minutos</option><option value="15">15 minutos</option></select></label></div></div><svg class="resources-chart" data-resource-chart role="img" viewBox="0 0 600 132" preserveAspectRatio="none" aria-label="Aguardando medições"></svg><div class="resources-chart-caption"><div class="resources-chart-legend" data-resource-chart-legend></div><span data-resource-chart-scale></span><span data-resource-chart-period></span></div><p class="resources-notes">Coleta contínua no aplicativo; fechar este painel não apaga o histórico. Lacunas representam medições indisponíveis.</p></section>
      <section class="resources-section"><h4>Processos próprios</h4><p class="resources-description">O processo principal e seus descendentes. RAM residente compartilhada pode aparecer em mais de um processo; memória virtual não representa RAM física.</p><div class="resources-table-wrap" data-resource-processes></div></section>
      <section class="resources-section"><h4>Volumes do sistema</h4><p class="resources-description">Capacidade e espaço disponível do volume inteiro. Taxas por volume ficam indisponíveis quando o sistema não fornece um contador.</p><div class="resources-table-wrap" data-resource-disks></div></section>
      <section class="resources-section"><h4>Estruturas internas</h4><p class="resources-description" data-resource-inventory-description></p><div class="resources-table-wrap" data-resource-inventory></div></section>
      <section class="resources-section"><h4>Arquivos do aplicativo</h4><p class="resources-description" data-resource-storage-description></p><div class="resources-table-wrap" data-resource-storage></div></section>
      <section class="resources-section"><h4>Operações internas</h4><p class="resources-description" data-resource-operations-description>Registro de trabalho do motor. CPU e RAM são medidas por processo.</p><div class="resources-table-wrap" data-resource-operations></div></section>
      <section class="resources-section" data-resource-controls><h4>Trabalho em andamento</h4><button class="btn ghost small" type="button" data-resource-tasks>Ver tarefas em andamento</button><p class="resources-description">Abre o painel de tarefas do aplicativo, com os controles de cancelamento disponíveis para cada trabalho.</p></section>
      <ul class="resources-notes" data-resource-notes></ul>
    </div></section>`;
    background = [...document.body.children].map(node => [node, node.inert]);
    for (const [node] of background) node.inert = true;
    document.body.append(dialog); button?.setAttribute("aria-expanded", "true");
    for (const target of dialog.querySelectorAll("[data-resource-app],[data-resource-host]")) target.innerHTML = '<p class="resources-empty">Aguardando a primeira amostra…</p>';
    dialog.querySelector("[data-resource-close]").onclick = close;
    dialog.querySelector("[data-resource-refresh]").onclick = () => refresh(true);
    dialog.querySelector("[data-resource-pause]").onclick = togglePause;
    dialog.querySelector("[data-resource-export]").onclick = exportSnapshot;
    dialog.querySelector("[data-resource-tasks]").onclick = () => { close(); window.Tasks?.open(); };
    dialog.querySelector("[data-resource-chart-metric]").onchange = renderChart;
    dialog.querySelector("[data-resource-chart-range]").onchange = renderChart;
    dialog.onclick = event => { if (event.target === dialog) close(); };
    dialog.onkeydown = event => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
      if (event.key !== "Tab") return;
      const controls = [...dialog.querySelectorAll("button:not(:disabled),select,[tabindex='0']")].filter(node => node.offsetParent !== null);
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    dialog.querySelector("[data-resource-close]").focus(); refresh();
  }
  button?.addEventListener("click", open);
  return { open, close, refresh, snapshot: () => latest };
})();
