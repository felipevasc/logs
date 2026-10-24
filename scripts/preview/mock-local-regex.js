/* Synthetic transport for the expression workbench in the browser preview.
 * It answers regex_preview / sample_text with the JavaScript RegExp engine so the
 * interface (editing, colours, menus, versioning) can be exercised without Tauri.
 * It is NOT PCRE2: native semantics are verified by the Rust tests and by
 * scripts/preview/test-local-native.mjs against the real application. */
(() => {
  const calls = [];
  const flagsOf = (p) => `dgu${p.caseSensitive ? "" : "i"}${p.multiline ? "m" : ""}${p.dotAll ? "s" : ""}`;
  const escape = (t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  function groupInfos(pattern) {
    const a = window.RegexSyntax.analyze(pattern);
    const out = [{ index: 0, names: [] }];
    for (const g of a.captures) out.push({ index: g.index, names: g.name ? [g.name] : [] });
    return out;
  }
  function preview(r) {
    const p = r.pattern;
    let re;
    try { re = new RegExp(p.engine === "literal" ? escape(p.text) : p.text, flagsOf(p)); }
    catch (error) {
      const m = /position (\d+)/.exec(String(error.message));
      return { ok: false, stage: "compile", error: { message: `PCRE2 (simulado): ${error.message}`, byteOffset: m ? +m[1] : null, offset: m ? +m[1] : null }, engine: p.engine, pcre2Version: "10.mock" };
    }
    const infos = groupInfos(p.text), spans = [];
    const lines = r.scope === "line" ? r.text.split(/(?<=\n)/) : [r.text];
    let base = 0;
    for (const part of lines) {
      const text = r.scope === "line" ? part.replace(/\r?\n$/, "") : part;
      re.lastIndex = 0;
      for (let m; spans.length < 512 && (m = re.exec(text));) {
        const groups = [];
        for (let i = 1; i < infos.length; i++) {
          const range = m.indices[i];
          groups.push(range ? { index: i, names: infos[i].names, participated: true, start: base + range[0], end: base + range[1], byteStart: 0, byteEnd: 0, value: m[i] } : { index: i, names: infos[i].names, participated: false, start: 0, end: 0, byteStart: 0, byteEnd: 0 });
        }
        spans.push({ start: base + m.index, end: base + m.index + m[0].length, byteStart: 0, byteEnd: 0, captures: {}, groups });
        if (m[0] === "") re.lastIndex++;
      }
      base += part.length;
    }
    return { ok: true, engine: p.engine, pcre2Version: "10.mock", groups: infos, spans, complete: spans.length < 512, limit: spans.length < 512 ? null : "occurrences", bytes: r.text.length, length: r.text.length };
  }
  window.__mockLocalCalls = calls;
  window.__mockLocalAnalysis = async ({ request: r, filters }) => {
    calls.push({ action: r.action, filters });
    if (typeof window.__mockDelay === "function") await new Promise((resolve) => setTimeout(resolve, window.__mockDelay(r) || 0));
    if (r.action === "regex_preview") return preview(r);
    if (r.action === "sample_text") {
      const text = (window.__mockSamples || {})[`${r.id}:${r.field}`] ?? "";
      return { field: r.field, text, bytes: text.length, truncated: false };
    }
    if (r.action === "highlights") return r.texts.map((t) => (typeof window.__mockMarks === "function" ? window.__mockMarks(t) : []));
    if (r.action === "context") {
      const rows = (window.__mockContextRows || []).map((row) => ({ spans: [], truncatedBefore: false, truncatedAfter: false, matchesComplete: true, ...row }));
      return { center: r.id, mode: r.mode, subjectScope: r.subjectScope, rows, more: false, presentationFiltersIgnored: true, visibilityRespected: true };
    }
    if (window.__mockLocalExtra) return window.__mockLocalExtra(r);
    throw Error(`Unmocked presentation request ${r.action}`);
  };
})();
