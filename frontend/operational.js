/* The summary keeps original records, comparisons and actions inside their signal. */
window.Operational = (() => {
  "use strict";
  const node = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const icon = (name) => {
    const i = node("i", "fas " + name);
    i.setAttribute("aria-hidden", "true");
    return i;
  };
  const button = (label, action, glyph) => {
    const b = node("button", "op-control");
    b.type = "button";
    if (glyph) b.append(icon(glyph));
    b.append(label);
    b.onclick = action;
    return b;
  };
  const fmt = (n) => Number(n).toLocaleString("pt-BR", { maximumFractionDigits: 1 });
  const filter = (column, op, value = "", value2 = null) => ({ column, op, value, value2 });
  const kinds = {
    duration: ["Duração", "fa-stopwatch"],
    steps: ["Etapas", "fa-code-branch"],
    bridge: ["Vínculo", "fa-link"],
    rare: ["Raridade", "fa-fingerprint"],
    mix: ["Carga", "fa-chart-column"],
    spike: ["Pico", "fa-chart-line"],
    gap: ["Intervalo", "fa-clock"],
    pattern: ["Padrão", "fa-layer-group"],
    quality: ["Leitura", "fa-file-lines"],
    security: ["Segurança", "fa-shield-halved"],
  };
  const prefKey = "loginsight.json-lists.v1";
  function choices() {
    try {
      return JSON.parse(localStorage.getItem(prefKey)) || {};
    } catch {
      return {};
    }
  }
  function rememberChoice(path, choice) {
    try {
      const prefs = new Map(Object.entries(choices()));
      prefs.set(path, choice);
      localStorage.setItem(prefKey, JSON.stringify(Object.fromEntries([...prefs].slice(-256))));
    } catch {}
  }
  function withChoice(source, path, choice) {
    if (!source) return null;
    let found = false;
    const members = (source.kind === "bundle" ? source.members : [source]).flatMap((member) => {
      if (member.kind !== "file") return [member];
      return (member.paths?.length ? member.paths : [member.path]).map((p) => {
        if (p === path) found = true;
        return {
          ...member,
          path: p,
          paths: [p],
          format: p === path ? "json-list:" + choice : member.format || "auto",
        };
      });
    });
    return !found ? null : members.length === 1 ? members[0] : { kind: "bundle", members };
  }
  function applyChoices(source) {
    let result = source;
    const prefs = choices();
    for (const member of source.kind === "bundle" ? source.members : [source]) {
      if (member.kind !== "file" || !["auto", "jsonl"].includes(member.format || "auto")) continue;
      for (const path of member.paths?.length ? member.paths : [member.path]) {
        if (Object.hasOwn(prefs, path) && typeof prefs[path] === "string")
          result = withChoice(result, path, prefs[path]) || result;
      }
    }
    return result;
  }
  function fromOverview(data) {
    return data.findings.map((f) => {
      const p = f.kind === "pattern" ? data.patterns.find((p) => p.example?.id === f.event_id) : null;
      const bucket = f.kind === "spike" ? data.buckets.find((b) => b.timestamp === f.start) : null;
      let filters = [];
      if (p) filters = [filter("message", "pattern", p.pattern)];
      else if (f.kind === "quality") filters = [filter("timestamp", "empty")];
      else if (f.start != null && f.end != null)
        filters = [filter("timestamp", "between", String(f.start), String(f.end))];
      return {
        ...f,
        source: p?.example?.source || "",
        count: bucket?.errors ?? p?.errors ?? (f.kind === "quality" ? data.undated : null),
        population: bucket?.count ?? data.total,
        filters,
        examples: [],
        observed: [],
        reference: [],
        measures: [],
      };
    });
  }
  function bar(label, n, max) {
    const line = node("div", "op-meter");
    line.append(node("span", "", label));
    const track = node("span", "op-meter-track"),
      mark = node("b");
    mark.style.width = (100 * Math.abs(n)) / Math.max(1, max) + "%";
    track.append(mark);
    line.append(track, node("strong", "", fmt(n)));
    return line;
  }
  function sequence(label, steps, open) {
    const wrap = node("div", "op-seq"),
      items = node("div", "op-stages");
    wrap.append(node("small", "", label), items);
    steps.forEach((v, i) => {
      if (i) items.append(icon("fa-angle-right"));
      items.append(button(v, () => open(i), "fa-file-lines"));
    });
    return wrap;
  }
  // Align repeated stages by their longest common subsequence. Each column
  // represents the same stage across both rows, even after an omission.
  function aligned(reference, observed, open) {
    const a = reference.length,
      b = observed.length;
    const matches = Array.from({ length: a + 1 }, () => new Uint8Array(b + 1));
    for (let i = a - 1; i >= 0; i--)
      for (let j = b - 1; j >= 0; j--)
        matches[i][j] =
          reference[i] === observed[j]
            ? matches[i + 1][j + 1] + 1
            : Math.max(matches[i + 1][j], matches[i][j + 1]);
    const grid = node("div", "op-alignment");
    grid.setAttribute("aria-label", "Comparação das etapas");
    const labels = node("div", "op-stage-pair");
    labels.append(node("small", "", "Referência"), node("small", "", "Observada"));
    grid.append(labels);
    let i = 0,
      j = 0;
    function item(name, index, missing) {
      if (index == null) return node("span", "op-stage-missing", missing);
      const stage = button(name, () => open(index));
      stage.title = "Abrir registro · " + name;
      return stage;
    }
    while (i < a || j < b) {
      const pair = node("div", "op-stage-pair");
      if (i < a && j < b && reference[i] === observed[j]) {
        pair.append(item(reference[i], b + i), item(observed[j], j));
        i++;
        j++;
      } else if (i < a && (j === b || matches[i + 1][j] >= matches[i][j + 1])) {
        pair.append(item(reference[i], b + i), item("", null, "Ausente"));
        i++;
      } else {
        pair.append(item("", null, "—"), item(observed[j], j));
        j++;
      }
      grid.append(pair);
    }
    return grid;
  }
  function originals(host, rows, start = 0) {
    let i = 0;
    if (!rows.length) {
      host.append(node("p", "quiet-empty", "Nenhum registro identificado para esta seleção."));
      return;
    }
    const title = node("div", "op-original-head"),
      provenance = node("details", "op-provenance"),
      loc = node("summary"),
      identity = node("code"),
      prev = button("", () => show(i - 1), "fa-chevron-left"),
      next = button("", () => show(i + 1), "fa-chevron-right"),
      page = node("span", "op-page"),
      pre = node("pre", "op-raw");
    prev.setAttribute("aria-label", "Registro anterior");
    next.setAttribute("aria-label", "Próximo registro");
    provenance.append(loc, identity);
    title.append(provenance, prev, page, next);
    host.append(title, pre);
    function show(pos) {
      i = pos;
      const e = rows[i];
      loc.textContent = [
        e.role,
        e.file || e.fields?.arquivo || e.source,
        e.code,
        e.outcome === "success"
          ? "Sucesso"
          : e.outcome === "failure"
            ? "Falha observada"
            : e.outcome === "blocked"
              ? "Bloqueio observado"
              : "",
        e.timestamp != null ? fmtTsFull(e.timestamp) : "",
      ]
        .filter(Boolean)
        .join(" · ");
      identity.textContent = [e.fields?.caminho || e.file, e.path || e.fields?.["log.record.path"], e.event_ref]
        .filter(Boolean)
        .join(" · ");
      loc.title = identity.textContent;
      provenance.open = false;
      pre.textContent = (e.raw || e.message || "") + (e.clipped ? "\n[…]" : "");
      page.textContent = i + 1 + " / " + rows.length;
      prev.disabled = !i;
      next.disabled = i + 1 >= rows.length;
    }
    show(Math.min(rows.length - 1, Math.max(0, start)));
  }
  function diagnose(host, s, ctx) {
    for (const test of s.tests || []) {
      const block = node("div", "op-comparison");
      block.append(
        node("strong", "", test.field + "=" + test.value),
        bar("Operações · " + fmt(test.hit) + "/" + fmt(test.group), (100 * test.hit) / test.group, 100),
        bar(
          "Controles · " + fmt(test.other_hit) + "/" + fmt(test.other),
          (100 * test.other_hit) / test.other,
          100,
        ),
        node(
          "p",
          "small muted",
          "Percentual com a sequência observada. Campo desconhecido em " +
            fmt(test.unknown) +
            " operações comparáveis.",
        ),
      );
      if (test.contrary) {
        const slot = node("div");
        block.append(
          button(
            "Contraexemplo",
            () => {
              slot.replaceChildren();
              originals(slot, [test.contrary]);
            },
            "fa-file-lines",
          ),
          slot,
        );
      }
      host.append(block);
    }
    const form = node("form", "op-hyp"),
      fields = Array.from(
        new Set([
          "source",
          "level",
          "code",
          "@outcome",
          ...(state.columns || []).map((c) => (typeof c === "string" ? c : c?.name)).filter(Boolean),
        ]),
      );
    const free = node("input");
    free.placeholder = "Comparar version=v2 quando estado=falha";
    free.setAttribute("aria-label", "Hipótese em português");
    const list = node("datalist");
    list.id = "op-hyp-fields";
    fields.forEach((v) => {
      const o = node("option");
      o.value = v;
      list.append(o);
    });
    function input(label, value, suggest) {
      const l = node("label", "", label),
        i = node("input");
      i.value = value || "";
      if (suggest) i.setAttribute("list", list.id);
      l.append(i);
      form.append(l);
      return i;
    }
    const group = input("Campo do grupo", fields.includes("version") ? "version" : "source", true),
      value = input("Valor do grupo", s.source && group.value === "source" ? s.source : ""),
      outcome = input("Campo do resultado", "@outcome", true),
      target = input("Resultado", "failure");
    const run = button("Comparar", null, "fa-scale-balanced");
    run.type = "submit";
    const options = node("details", "op-question");
    options.append(node("summary", "", "Escrever a hipótese"), free);
    const windowDetails = node("details", "op-window");
    windowDetails.append(node("summary", "", "Período do teste"));
    const from = node("input"),
      to = node("input");
    for (const [el, label] of [
      [from, "De"],
      [to, "Até"],
    ]) {
      el.type = "datetime-local";
      const l = node("label", "", label);
      l.append(el);
      windowDetails.append(l);
    }
    const output = node("div", "op-hyp-result");
    output.setAttribute("aria-live", "polite");
    form.append(list, options, windowDetails, run, output);
    host.append(form);
    form.onsubmit = async (ev) => {
      ev.preventDefault();
      output.replaceChildren();
      if (free.value.trim()) {
        const m =
          /^\s*(?:comparar?|compare|testar?)\s+([\w.@-]+)\s*=\s*(.+?)\s+(?:quando|com resultado|para)\s+([\w.@-]+)\s*=\s*(.+?)\s*\??$/iu.exec(
            free.value,
          );
        if (!m) {
          output.textContent =
            "Informe campo=valor para o grupo e para o resultado, ou use os quatro campos acima.";
          return;
        }
        [group.value, value.value, outcome.value, target.value] = m
          .slice(1)
          .map((x) => x.trim().replace(/^["']|["']$/g, ""));
      }
      const [g, v, o, y] = [group, value, outcome, target].map((i) => i.value.trim());
      if (!g || !o || !v || !y || g === o) {
        output.textContent = "Escolha dois campos diferentes e os valores a comparar.";
        return;
      }
      const windowFilters = [...ctx.captured.filters];
      if (from.value) windowFilters.push(filter("timestamp", "gte", String(+new Date(from.value))));
      if (to.value) windowFilters.push(filter("timestamp", "lte", String(+new Date(to.value))));
      if (from.value && to.value && from.value > to.value) {
        output.textContent = "O fim deve vir depois do início.";
        return;
      }
      const known = [...windowFilters, filter(g, "not_empty"), filter(o, "not_empty")],
        a = [...known, filter(g, "equals_exact", v)],
        b = [...known, filter(g, "not_equals_exact", v)],
        hit = filter(o, "equals_exact", y),
        miss = filter(o, "not_equals_exact", y);
      run.disabled = true;
      output.textContent = "Contando registros…";
      try {
        // Count consecutively to share the scheduler with foreground exploration.
        const counts = [];
        for (const filters of [windowFilters, a, b, [...a, hit], [...b, hit]]) {
          if (!ctx.current() || !host.isConnected) return;
          counts.push(await ctx.request("count_filtered", { filters }));
        }
        if (!ctx.current() || !host.isConnected) return;
        const [total, na, nb, ya, yb] = counts;
        output.replaceChildren();
        for (const [label, num, den] of [
          [v, ya, na],
          ["Outros valores", yb, nb],
        ])
          output.append(bar(label + " · " + fmt(num) + "/" + fmt(den), den ? (100 * num) / den : 0, 100));
        output.append(
          node(
            "p",
            "small muted",
            "Taxas em %. " +
              fmt(total - na - nb) +
              " registros sem um dos campos. " +
              (Math.min(na, nb) < 5
                ? "Poucos controles para interpretar a diferença."
                : "Comparação observacional."),
          ),
        );
        const sample = node("div");
        for (const [label, criteria] of [
          ["Favoráveis", [...a, hit]],
          ["Contraexemplos", [...a, miss]],
        ]) {
          output.append(
            button(
              label,
              async () => {
                const token = (sample.dataset.request = String(Number(sample.dataset.request || 0) + 1));
                try {
                  const result = await ctx.rows(criteria);
                  if (ctx.current() && sample.isConnected && sample.dataset.request === token) {
                    sample.replaceChildren();
                    originals(sample, result.rows);
                  }
                } catch (e) {
                  if (sample.isConnected && sample.dataset.request === token) sample.textContent = String(e);
                }
              },
              "fa-file-lines",
            ),
          );
        }
        output.append(sample);
      } catch (e) {
        if (host.isConnected) output.textContent = String(e);
      } finally {
        run.disabled = false;
      }
    };
  }
  function mount(host, overview, ctx) {
    const initial = fromOverview(overview);
    const rows = (filters) =>
      ctx.request("query_events", { filters, sortColumn: "id", sortDir: "asc", offset: 0, limit: 12 });
    ctx = { ...ctx, rows };
    let active = null,
      all = false,
      report = null,
      security = [],
      securityVersion = 0;
    const notice = node("div", "op-review"),
      sheet = node("section", "op-sheet"),
      head = node("header", "op-head"),
      heading = node("h2", "", "Indícios do recorte"),
      status = node("span", "op-status"),
      list = node("div", "op-list"),
      foot = node("div", "op-foot");
    status.setAttribute("role", "status");
    head.append(icon("fa-wave-square"), heading, status);
    const axis = node("div", "op-axis");
    if (overview.start != null && overview.end > overview.start) {
      const ticks = node("div", "op-axis-ticks"),
        span = overview.end - overview.start;
      for (const fraction of [0, 0.5, 1]) {
        const t = overview.start + fraction * span,
          label = new Date(t).toLocaleTimeString("pt-BR", {
            hour: "2-digit",
            minute: "2-digit",
            ...(span < 60000 ? { second: "2-digit" } : {}),
          });
        const time = node(
          "time",
          "",
          span >= 86400000
            ? new Date(t).toLocaleDateString("pt-BR", { day: "2-digit", month: "2-digit" }) + " " + label
            : label,
        );
        time.title = fmtTsFull(t);
        ticks.append(time);
      }
      axis.append(ticks);
      axis.setAttribute("aria-label", "Período da linha do tempo");
    }
    sheet.append(head, axis, list, foot);
    host.replaceChildren(notice, sheet);
    const close = () => {
      if (!active) return;
      active.row.setAttribute("aria-expanded", "false");
      active.panel.hidden = true;
      active.panel.replaceChildren();
      active = null;
    };
    host.onkeydown = (e) => {
      if (e.key === "Escape" && active) {
        const r = active.row;
        close();
        r.focus({ preventScroll: true });
      }
    };
    function draw() {
      const signals = [...security, ...(report?.signals || []), ...initial].sort(
          (a, b) => (b.priority || 30) - (a.priority || 30),
        ),
        seen = new Map(),
        unique = [];
      for (const s of signals) {
        const refs = [
          ...new Set(
            [
              ...(s.refs || []),
              ...s.examples.map((e) => e.event_ref),
              ...(s.cases || []).flatMap((c) => c[1].map((e) => e.event_ref)),
            ].filter(Boolean),
          ),
        ].sort();
        const duplicate = refs.length && seen.get(refs.join("|"));
        if (duplicate) {
          if (s.detail !== duplicate.detail && !(duplicate.reasons || []).includes(s.detail))
            (duplicate.reasons ??= []).push(s.detail);
          continue;
        }
        if (refs.length) seen.set(refs.join("|"), s);
        unique.push(s);
      }
      const oldRow = active?.row,
        preserve = active?.signal;
      // Resolve an asynchronous report without replacing the investigation under the pointer.
      if (active && list.childElementCount) {
        foot.replaceChildren(
          button("Atualizar indícios", () => {
            close();
            draw();
          }),
        );
        return;
      }
      list.replaceChildren();
      foot.replaceChildren();
      status.textContent = report
        ? fmt(unique.length) + (unique.length === 1 ? " indício" : " indícios")
        : "Analisando…";
      if (report?.limited)
        foot.append(
          node("span", "small muted", "Exemplos limitados · refine o recorte para ampliar a cobertura."),
        );
      if (!unique.length)
        list.append(
          node(
            "p",
            "op-empty",
            report
              ? "Nenhum indício destacado em " + fmt(report.total) + " registros."
              : "Analisando operações e relações…",
          ),
        );
      const explained =
        unique.length > 3
          ? unique.filter(
              (s) =>
                s.kind === "mix" &&
                s.measures?.length === 3 &&
                Math.abs(s.measures[2][1]) <= Math.max(1, s.measures[0][1] * 0.1),
            )
          : [];
      const regular = unique.filter((s) => !explained.includes(s));
      const visible = [...(all ? regular : regular.slice(0, 3)), ...explained];
      const compatible = node("details", "op-explained");
      compatible.append(node("summary", "", "Variações compatíveis com a carga · " + explained.length));
      const critical = regular.slice(3).filter((s) => (s.priority || 0) >= 90).length;
      if (critical && !all)
        foot.append(
          button(
            fmt(critical) + " de alta prioridade fora dos destaques",
            () => {
              all = true;
              close();
              draw();
            },
            "fa-circle-exclamation",
          ),
        );
      for (const [i, s] of visible.entries()) {
        const article = node("article", "op-track");
        article.dataset.kind = s.kind;
        const row = node("button", "op-row");
        row.type = "button";
        row.id = "op-row-" + i;
        row.setAttribute("aria-expanded", "false");
        row.setAttribute("aria-controls", "op-detail-" + i);
        const caption = node("span", "op-caption"),
          name = kinds[s.kind] || ["Indício", "fa-circle-dot"];
        caption.append(node("small", "", s.source || name[0]), node("strong", "", s.title));
        row.append(icon(name[1]), caption);
        const graph = node("span", "op-mini");
        graph.setAttribute("aria-hidden", "true");
        const marks = s.examples.map((e) => e.timestamp).filter(Number.isFinite);
        if (!marks.length && Number.isFinite(s.start)) marks.push(s.start);
        if (s.bins?.some(([n]) => n > 0) && overview.end > overview.start) {
          graph.className = "op-bins";
          const max = Math.max(1, ...s.bins.map(([n]) => n));
          s.bins.forEach(([total, hits], j) => {
            const mark = node("span");
            mark.style.height = (total ? Math.max(9, (100 * total) / max) : 0) + "%";
            if (hits) {
              mark.dataset.hit = "";
              const tip = node("i");
              tip.style.height = Math.max(14, (100 * hits) / total) + "%";
              mark.append(tip);
            }
            mark.title =
              fmtTsFull(overview.start + ((overview.end - overview.start) * j) / 32) +
              " · " +
              hits +
              " de " +
              total;
            graph.append(mark);
          });
        } else if (marks.length && overview.end > overview.start) {
          for (const t of marks.slice(0, 64)) {
            const tick = node("b");
            tick.style.left =
              Math.min(99, Math.max(0, (100 * (t - overview.start)) / (overview.end - overview.start))) + "%";
            graph.append(tick);
          }
          graph.title = "Horários das evidências no recorte";
        } else graph.classList.add("no-time");
        row.append(
          graph,
          node(
            "span",
            "op-amount",
            s.kind === "bridge"
              ? fmt(s.examples.length) + " registros"
              : s.count == null
                ? ""
                : fmt(s.count) + " / " + fmt(s.population),
          ),
          icon("fa-chevron-down"),
        );
        const panel = node("section", "op-detail");
        panel.hidden = true;
        panel.id = "op-detail-" + i;
        panel.setAttribute("aria-labelledby", row.id);
        article.append(row, panel);
        (explained.includes(s) ? compatible : list).append(article);
        const openMenu = () => {
          if (active?.row !== row) row.click();
          const menu = panel.querySelector(".op-more");
          if (menu) {
            menu.open = true;
            menu.querySelector("button")?.focus();
          }
        };
        row.oncontextmenu = (e) => {
          e.preventDefault();
          openMenu();
        };
        row.onkeydown = (e) => {
          if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
            e.preventDefault();
            openMenu();
          }
        };
        row.onclick = () => {
          const same = active?.row === row;
          close();
          if (same) return;
          active = { row, panel, signal: s };
          panel.hidden = false;
          row.setAttribute("aria-expanded", "true");
          const diagrams = node("div", "op-diagrams");
          let caseIndex = 0,
            offset = 0,
            cases = s.cases || [],
            wantedRecord = 0,
            originalButton;
          const selectedProofs = () => (cases.length ? [...cases[caseIndex][1], ...s.examples] : s.examples);
          const gap = s.kind === "gap" && Number.isFinite(s.start) && Number.isFinite(s.end);
          let gapEvidence;
          async function evidence() {
            if (!gap) return selectedProofs();
            if (gapEvidence) return gapEvidence;
            const nearby = [];
            for (const [op, t, dir, role] of [
              ["lt", s.start, "desc", "Antes do intervalo"],
              ["gt", s.end, "asc", "Depois do intervalo"],
            ]) {
              if (!ctx.current()) return [];
              const result = await ctx.request("query_events", {
                filters: [...ctx.captured.filters, filter("timestamp", op, String(t))],
                sortColumn: "timestamp",
                sortDir: dir,
                offset: 0,
                limit: 1,
              });
              if (result.rows.length) nearby.push({ ...result.rows[0], role });
            }
            gapEvidence = nearby;
            return nearby;
          }
          function openRecord(index) {
            wantedRecord = index;
            if (originalButton.getAttribute("aria-expanded") === "true") originalButton.click();
            originalButton.click();
          }
          if (s.cases?.length) {
            const pager = node("div", "op-case-pager"),
              label = node("strong"),
              position = node("span");
            const pages = new Map([[0, cases]]),
              total = s.selector ? s.count : cases.length;
            function display() {
              label.title = cases[caseIndex][0];
              label.textContent = label.title.replace(/^(?:copy_id|copia_id)=/iu, "Cópia ");
              position.textContent = fmt(offset + caseIndex + 1) + " / " + fmt(total);
              prev.disabled = offset + caseIndex === 0;
              next.disabled = offset + caseIndex + 1 >= total;
            }
            async function step(delta) {
              const at = offset + caseIndex + delta,
                newOffset = Math.floor(at / 12) * 12;
              if (at < 0 || at >= total) return;
              prev.disabled = next.disabled = true;
              try {
                if (!pages.has(newOffset)) {
                  position.textContent = "Carregando…";
                  const result = await ctx.request("operational_report", {
                    selector: s.selector,
                    offset: newOffset,
                  });
                  if (!ctx.current() || !position.isConnected) return;
                  const page = result.signals.find((v) => v.selector === s.selector)?.cases;
                  if (!page?.length) throw Error("Operações indisponíveis para este recorte.");
                  if (pages.size >= 3) pages.delete(pages.keys().next().value);
                  pages.set(newOffset, page);
                }
                cases = pages.get(newOffset);
                offset = newOffset;
                caseIndex = at - offset;
                display();
                if (originalButton.getAttribute("aria-expanded") === "true") openRecord(0);
              } catch (error) {
                if (position.isConnected) {
                  display();
                  position.textContent = String(error);
                }
              }
            }
            const prev = button("", () => step(-1), "fa-chevron-left"),
              next = button("", () => step(1), "fa-chevron-right");
            prev.setAttribute("aria-label", "Operação anterior");
            next.setAttribute("aria-label", "Próxima operação");
            position.setAttribute("aria-live", "polite");
            pager.append(label, prev, position, next);
            diagrams.append(pager);
            display();
          }
          if (s.reference?.length && s.observed?.length)
            diagrams.append(aligned(s.reference, s.observed, openRecord));
          else if (s.observed?.length) diagrams.append(sequence("Caminho", s.observed, openRecord));
          for (const [label, n] of s.measures || [])
            diagrams.append(bar(label, n, Math.max(...s.measures.map((m) => Math.abs(m[1])))));
          const actions = node("div", "op-actions"),
            extra = node("div", "op-extra"),
            more = node("details", "op-more"),
            toggle = node("summary", "op-control");
          toggle.setAttribute("aria-label", "Mais ações");
          toggle.title = "Mais ações";
          toggle.append(icon("fa-ellipsis"));
          more.append(toggle);
          panel.append(diagrams, actions, extra);
          let detailVersion = 0;
          function action(label, glyph, handler, parent = actions) {
            const b = button(
              label,
              async () => {
                const version = ++detailVersion,
                  open = b.getAttribute("aria-expanded") === "true";
                for (const other of actions.querySelectorAll("[aria-expanded]"))
                  other.setAttribute("aria-expanded", "false");
                more.open = false;
                extra.replaceChildren();
                if (open) return;
                b.setAttribute("aria-expanded", "true");
                const local = node("div");
                extra.append(local);
                try {
                  await handler(local, () => version === detailVersion && ctx.current() && local.isConnected);
                } catch (e) {
                  if (local.isConnected) local.textContent = String(e);
                }
              },
              glyph,
            );
            b.setAttribute("aria-expanded", "false");
            parent.append(b);
            return b;
          }
          originalButton = action("Registros", "fa-file-lines", async (slot, current) => {
            const start = wantedRecord;
            wantedRecord = 0;
            if (selectedProofs().length) {
              const proofs = selectedProofs();
              if (proofs.some((p) => p.clipped)) {
                const refs = proofs.filter((p) => p.clipped).map((p) => p.event_ref);
                const result = await ctx.request("query_events", {
                  filters: [...ctx.captured.filters, filter("event_ref", "in_exact", refs.join("\n"))],
                  sortColumn: "id",
                  sortDir: "asc",
                  offset: 0,
                  limit: refs.length,
                });
                if (!current()) return;
                const full = new Map(result.rows.map((e) => [e.event_ref, e]));
                originals(
                  slot,
                  proofs.map((p) =>
                    full.has(p.event_ref) ? { ...p, raw: full.get(p.event_ref).raw, clipped: false } : p,
                  ),
                  start,
                );
              } else originals(slot, proofs, start);
              return;
            }
            if (gap) {
              const adjacent = await evidence();
              if (current()) originals(slot, adjacent);
              return;
            }
            if (!s.filters?.length) {
              slot.textContent = "O indício descreve a distribuição do recorte.";
              return;
            }
            const result = await rows([...ctx.captured.filters, ...s.filters]);
            if (current()) originals(slot, result.rows);
          });
          action("Critério", "fa-circle-info", (slot) => {
            for (const reason of new Set([s.detail, ...(s.reasons || [])])) slot.append(node("p", "", reason));
          });
          action("Hipótese", "fa-scale-balanced", (slot) => diagnose(slot, s, ctx));
          if (ctx.scope === "dataset")
            action(
              "Salvar",
              "fa-bookmark",
              async (slot, current) => {
                const proofs = await evidence();
                if (!current()) return;
                const refs = [...new Set(proofs.map((e) => e.event_ref).filter(Boolean))],
                  criteria = refs.length ? [filter("event_ref", "in_exact", refs.join("\n"))] : s.filters;
                if (!criteria?.length) {
                  slot.textContent = "Selecione um indício com registros de origem.";
                  return;
                }
                const result = await ctx.request("query_events", {
                  filters: [...ctx.captured.filters, ...criteria],
                  sortColumn: "id",
                  sortDir: "asc",
                  offset: 0,
                  limit: Math.min(64, refs.length || 12),
                });
                if (!current()) return;
                if (!result.rows.length) {
                  slot.textContent = "Nenhum registro disponível para preservar.";
                  return;
                }
                const saved = await ctx.save(
                  result.rows,
                  {
                    kind: "grupo",
                    label: s.title,
                    note: [s.count != null ? fmt(s.count) + " de " + fmt(s.population) : "", s.source, s.detail]
                      .filter(Boolean)
                      .join(" · "),
                    foundCount: result.total,
                    sourceFilters: [...ctx.captured.filters, ...criteria],
                    tags: ["investigação operacional v1"],
                  },
                  current,
                );
                if (current()) slot.textContent = saved ? "Evidência salva." : "A evidência não foi salva.";
              },
              more,
            );
          const exact = s.filters?.length ? s.filters : null;
          more.append(
            button(
              s.cases?.length ? "Explorar operação" : s.partial ? "Explorar exemplos" : "Explorar",
              async () => {
                let proofs;
                try {
                  proofs = await evidence();
                } catch (e) {
                  if (ctx.current()) toast(String(e), "err");
                  return;
                }
                if (!ctx.current() || !panel.isConnected) return;
                const refs = proofs.map((p) => p.event_ref).filter(Boolean);
                if (cases.length) refs.splice(cases[caseIndex][1].length);
                const selection =
                  (!gap && exact) ||
                  (refs.length ? [filter("event_ref", "in_exact", [...new Set(refs)].join("\n"))] : exact || []);
                Workspace.applyFilters([...ctx.captured.filters, ...selection], true);
              },
              "fa-magnifying-glass",
            ),
          );
          if (s.episodeId != null)
            more.append(
              button(
                "Episódio completo",
                () => {
                  const current = window.Security?.cached();
                  if (current?.analysis_id !== s.analysisId || !ctx.current()) return;
                  const index = current.episodes.findIndex((e) => e.id === s.episodeId);
                  if (index >= 0) void window.Security.openEpisode(index);
                },
                "fa-shield-halved",
              ),
            );
          actions.append(more);
        };
        if (preserve === s && oldRow) row.click();
      }
      if (explained.length) list.append(compatible);
      if (regular.length > 3)
        foot.append(
          button(all ? "Mostrar três" : "Ver mais " + fmt(regular.length - 3), () => {
            all = !all;
            close();
            draw();
          }),
        );
    }
    draw();
    host.updateSecurity = async (data) => {
      const version = ++securityVersion;
      const found = [];
      const episodes = [...(data?.episodes || [])].sort(
        (a, b) => (b.evidence_level || 0) - (a.evidence_level || 0),
      );
      for (const episode of episodes.slice(0, 24)) {
        const detections = (episode.detections || [])
          .map((i) => data.detections?.[i])
          .filter(Boolean)
          .sort((a, b) => b.evidence_level - a.evidence_level);
        const lead = detections[0];
        if (!lead) continue;
        const members = [
          ...new Set(
            episode.event_refs?.length ? episode.event_refs : detections.flatMap((d) => d.event_refs || []),
          ),
        ];
        const partial = members.length > 256 || episode.members_complete === false;
        if (!members.length) continue;
        const refs = members.slice(0, 256);
        const filters = [filter("event_ref", "in_exact", refs.join("\n"))];
        if (ctx.captured.filters.length || ctx.scope === "case") {
          if (partial) continue;
          let count;
          try {
            count = await ctx.request("count_filtered", { filters: [...ctx.captured.filters, ...filters] });
          } catch {
            continue;
          }
          if (count !== refs.length) continue;
        }
        if (!ctx.current() || version !== securityVersion) return;
        const level = episode.evidence_level || lead.evidence_level || 0;
        found.push({
          kind: "security",
          title: episode.title,
          source: "Segurança · E" + level,
          priority: level >= 4 ? 98 : level >= 3 ? 78 : 42,
          detail:
            "Evidência E" +
            level +
            " · Impacto potencial: " +
            String(episode.severity || "indefinido") +
            " · Resultado: " +
            (window.EvidenceUI?.outcome?.(lead.outcome) || lead.outcome || "desconhecido") +
            (partial ? ". Os registros desta faixa são exemplos do episódio." : ""),
          start: episode.start,
          count: episode.record_count ?? (partial ? null : refs.length),
          partial,
          episodeId: episode.id,
          analysisId: data.analysis_id,
          population: overview.total,
          examples: [],
          measures: [],
          filters,
          refs,
        });
      }
      if (ctx.current() && version === securityVersion) {
        security = found;
        draw();
      }
    };
    async function load() {
      try {
        const result = await ctx.request("operational_report", {});
        if (!ctx.current() || !host.isConnected) return;
        report = result;
        draw();
        for (const [path, choices] of Object.entries(result.reviews || {})) {
          const review = node("details", "op-review-choice");
          review.append(node("summary", "", "Revisar leitura · " + path.split(/[\\/]/).pop()));
          for (const [key, preview] of Object.entries(choices)) {
            const b = button("Ler " + key, async () => {
              try {
                await ctx.review(path, key);
              } catch (e) {
                if (review.isConnected) review.append(node("p", "", String(e)));
              }
            });
            b.disabled = ctx.scope !== "dataset";
            review.append(b, node("pre", "op-raw", preview));
          }
          notice.append(review);
        }
      } catch (e) {
        if (ctx.current() && host.isConnected) {
          status.textContent = "Diagnóstico pendente";
          foot.replaceChildren(
            node("span", "small muted", String(e)),
            button("Tentar novamente", load, "fa-rotate"),
          );
        }
      }
    }
    if (overview.total) void load();
    else {
      report = { total: 0, signals: [] };
      draw();
    }
  }
  return { mount, withChoice, rememberChoice, applyChoices };
})();
