/* Árvore e formatação dos valores do inspetor de eventos. */
(function (root) {
  "use strict";

  const MAX_PARSE_LENGTH = 2_000_000;
  const MAX_NODES = 2500;
  const MAX_DEPTH = 12;
  const escapeHtml = (value) => String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[char]);

  function xmlObject(element, depth = 0) {
    if (depth >= MAX_DEPTH) return element.textContent || "";
    const result = {};
    for (const attr of element.attributes) result[`@${attr.name}`] = attr.value;
    for (const child of element.children) {
      const value = xmlObject(child, depth + 1);
      if (Object.hasOwn(result, child.tagName)) {
        if (!Array.isArray(result[child.tagName])) result[child.tagName] = [result[child.tagName]];
        result[child.tagName].push(value);
      } else result[child.tagName] = value;
    }
    const ownText = [...element.childNodes].filter((node) => node.nodeType === 3 || node.nodeType === 4)
      .map((node) => node.nodeValue).join("").trim();
    if (ownText) result["#text"] = ownText;
    return Object.keys(result).length ? result : "";
  }

  function detect(value) {
    if (value && typeof value === "object") return { type: "json", data: value, text: JSON.stringify(value, null, 2) };
    const text = String(value ?? "");
    const trimmed = text.trim();
    if (!trimmed || trimmed.length > MAX_PARSE_LENGTH) return { type: "text", text };

    if (/^[\[{]/.test(trimmed)) {
      try {
        const data = JSON.parse(trimmed);
        if (data && typeof data === "object") return { type: "json", data, text: JSON.stringify(data, null, 2) };
      } catch (_) { /* conteúdo livre */ }
    }
    if (/^<\?xml\b|^<[\w:-]+(?:\s|>|\/)/i.test(trimmed) && !/<!DOCTYPE|<!ENTITY/i.test(trimmed) && typeof DOMParser !== "undefined") {
      const doc = new DOMParser().parseFromString(trimmed, "application/xml");
      if (!doc.querySelector("parsererror") && doc.documentElement) {
        return { type: "xml", data: { [doc.documentElement.tagName]: xmlObject(doc.documentElement) }, text };
      }
    }
    if (/^(?:\?[^\s]+|[^\s?=&]+=[^\r\n]+)$/.test(trimmed) && trimmed.includes("=")) {
      const query = trimmed.startsWith("?") ? trimmed.slice(1) : trimmed;
      const entries = [...new URLSearchParams(query).entries()];
      if (entries.length && entries.every(([key]) => key)) {
        const data = {};
        for (const [key, item] of entries) {
          if (Object.hasOwn(data, key)) {
            if (!Array.isArray(data[key])) data[key] = [data[key]];
            data[key].push(item);
          } else data[key] = item;
        }
        return { type: "query", data, text: entries.map(([key, item]) => `${key} = ${item}`).join("\n"), entries };
      }
    }
    if (/\n/.test(trimmed) && /(?:^|\n)\s*(?:at\s+|Caused by:|File ".+", line \d+|\S+Error:|\S+Exception:)/m.test(trimmed)) {
      return { type: "stack", text, data: { frames: text.split(/\r?\n/).filter(Boolean) } };
    }
    if (/\n/.test(trimmed) && /^(?:---\s*\n)?\s*[\w"'-][^\n:]{0,100}:\s*/.test(trimmed) && root.jsyaml) {
      try {
        const data = root.jsyaml.load(trimmed);
        if (data && typeof data === "object" && !(data instanceof Date)) {
          return { type: "yaml", data, text: root.jsyaml.dump(data, { lineWidth: 100, noRefs: true }) };
        }
      } catch (_) { /* conteúdo livre */ }
    }
    return { type: "text", text };
  }

  function buildTree(entries) {
    const roots = [];
    const nodes = new Map();
    let count = 0;
    function ensure(path) {
      const parts = String(path).split(".").filter(Boolean);
      if (!parts.length) return null;
      let parent = null;
      let full = "";
      for (const part of parts) {
        full = full ? `${full}.${part}` : part;
        let node = nodes.get(full);
        if (!node) {
          if (++count > MAX_NODES) return null;
          node = { name: part, path: full, children: [], hasValue: false, original: false };
          nodes.set(full, node);
          (parent ? parent.children : roots).push(node);
        }
        parent = node;
      }
      return parent;
    }
    for (const entry of entries) {
      const node = ensure(entry.key);
      if (!node) continue;
      node.value = entry.value;
      node.hasValue = true;
      node.original = true;
      node.filterValue = entry.filterValue;
      node.mono = entry.mono;
    }
    function expand(node, depth) {
      if (depth >= MAX_DEPTH || count >= MAX_NODES) return;
      if (node.hasValue) {
        const parsed = detect(node.value);
        node.format = parsed;
        if (parsed.data && typeof parsed.data === "object") {
          for (const [key, value] of Object.entries(parsed.data)) {
            if (count >= MAX_NODES) break;
            const childPath = `${node.path}.${key}`;
            const child = nodes.get(childPath) || ensure(childPath);
            if (!child) break;
            if (!child.original) { child.value = value; child.hasValue = true; child.mono = true; }
          }
        }
      }
      for (const child of node.children) expand(child, depth + 1);
    }
    for (const node of roots) expand(node, 0);
    return roots;
  }

  function highlightJson(text) {
    return escapeHtml(text).replace(/(&quot;(?:\\.|(?!&quot;).)*?&quot;)(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g,
      (match, quoted, colon, keyword) => `<span class="${quoted ? (colon ? "j-key" : "j-str") : keyword ? "j-bool" : "j-num"}">${match}</span>`);
  }

  function format(value, parsed = detect(value)) {
    if (parsed.type === "json") return { type: "JSON", html: highlightJson(parsed.text), text: parsed.text };
    if (parsed.type === "xml") {
      const doc = new DOMParser().parseFromString(parsed.text, "application/xml");
      const lines = [];
      function walk(element, depth) {
        const indent = "  ".repeat(depth);
        const attrs = [...element.attributes].map((attr) => ` ${attr.name}="${attr.value}"`).join("");
        const open = `<${element.tagName}${attrs}>`;
        const children = [...element.children];
        const ownText = [...element.childNodes].filter((node) => node.nodeType === 3 || node.nodeType === 4)
          .map((node) => node.nodeValue).join("").trim();
        if (!children.length) lines.push(`${indent}${open}${ownText}</${element.tagName}>`);
        else {
          lines.push(`${indent}${open}`);
          if (ownText) lines.push(`${indent}  ${ownText}`);
          children.forEach((child) => walk(child, depth + 1));
          lines.push(`${indent}</${element.tagName}>`);
        }
      }
      walk(doc.documentElement, 0);
      const text = lines.join("\n");
      const html = escapeHtml(text).replace(/(&lt;\/?[\w:-]+)(.*?)(\/?&gt;)/g,
        (_, tag, attrs, end) => `<span class="j-key">${tag}</span>${attrs.replace(/(&quot;.*?&quot;)/g, '<span class="j-str">$1</span>')}<span class="j-key">${end}</span>`);
      return { type: "XML", html, text };
    }
    if (parsed.type === "query") {
      return { type: "Parâmetros", text: parsed.text,
        html: parsed.entries.map(([key, item]) => `<span class="j-key">${escapeHtml(key)}</span><span class="detail-equals"> = </span><span class="j-str">${escapeHtml(item)}</span>`).join("\n") };
    }
    if (parsed.type === "yaml") {
      return { type: "YAML", text: parsed.text, html: escapeHtml(parsed.text).replace(/^(\s*(?:-\s+)?[^\n:#]+:)(.*)$/gm,
        (_, key, item) => `<span class="j-key">${key}</span><span class="j-str">${item}</span>`) };
    }
    if (parsed.type === "stack") {
      return { type: "Stack trace", text: parsed.text, html: parsed.text.split(/\r?\n/).map((line) =>
        `<span class="${/\s+at\s+|File "/.test(line) ? "detail-stack-frame" : /Caused by:|Error:|Exception:/.test(line) ? "detail-stack-error" : ""}">${escapeHtml(line)}</span>`).join("\n") };
    }
    return { type: "Texto", html: escapeHtml(parsed.text), text: parsed.text };
  }

  const api = { buildTree, detect, format, escapeHtml };
  root.DetailFields = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;
})(typeof window === "undefined" ? globalThis : window);
