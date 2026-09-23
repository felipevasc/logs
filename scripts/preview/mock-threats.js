/* Browser preview contract only. Production matching uses Rust byte regexes.
   JS inline flags are approximated here; this module is never shipped in-app. */
const LIMIT = 64 * 1024;
const encoder = new TextEncoder();
let catalogPromise;
let catalogPath = '';
function regex(pattern) {
  const flags = new Set();
  const body = pattern.replace(/^\(\?([ims]+)\)/, (_, values) => { for (const f of values) flags.add(f); return ''; });
  return new RegExp(body, [...flags].join(''));
}
async function bundledCatalog() {
  catalogPromise ||= fetch('/__threat-catalog__.json', {cache:'no-store'}).then(response => {
    if (!response.ok) throw new Error('Catálogo de preview indisponível.');
    catalogPath=decodeURIComponent(response.headers.get('X-Catalog-Path')||'');
    return response.json();
  });
  return catalogPromise;
}
async function catalog() { return window.__mockThreatCatalog || bundledCatalog(); }
export async function threatCatalog() {
  catalogPromise = null;
  const bundled=await bundledCatalog(),file=await catalog(),ids=new Set(file.rules.map(rule=>rule.id));
  return { ...file, preview:true, path: catalogPath, updates_available:bundled.rules.filter(rule=>!ids.has(rule.id)).length, enabled: file.rules.filter(r => r.enabled).length, categories: [...new Set(file.rules.map(r => r.category))].sort() };
}
export async function threatCatalogUpdate() {
  const current=await catalog(),bundled=await bundledCatalog();
  if(current.error)throw new Error(current.error);
  const ids=new Set(current.rules.map(rule=>rule.id));
  if(ids.size!==current.rules.length)throw new Error('IDs duplicados no catálogo.');
  for(const rule of current.rules)regex(rule.pattern);
  const additions=bundled.rules.filter(rule=>!ids.has(rule.id));
  const merged=structuredClone({...current,rules:[...current.rules,...additions]});
  if(merged.rules.length>1000||encoder.encode(JSON.stringify(merged)).length>4*1024*1024)throw new Error('Catálogo excede os limites.');
  window.__mockThreatCatalog=merged;
  return {added:additions.length,backup_path:null,catalog:await threatCatalog()};
}
function prefix(text, limit) {
  if (encoder.encode(text).length <= limit) return text;
  let result = '', bytes = 0;
  for (const char of text) { const width = encoder.encode(char).length; if (bytes + width > limit) break; result += char; bytes += width; }
  return result;
}
function corpus(event) {
  let text = '', clipped = false, nodes = 0;
  const append = value => {
    if (!value) return;
    const available = LIMIT - encoder.encode(text).length - Number(!!text);
    if (available <= 0) { clipped = true; return; }
    const part = prefix(String(value), available); clipped ||= part.length < String(value).length;
    text += (text ? '\n' : '') + part;
  };
  append(event.message || '');
  if (event.description !== event.message) append(event.description || '');
  if (event.raw !== event.message) append(event.raw || '');
  function visit(value, depth) {
    if (nodes >= 1000 || depth > 12 || encoder.encode(text).length >= LIMIT) { clipped = true; return; }
    nodes++;
    if (typeof value === 'string') append(value);
    else if (Array.isArray(value)) { for (const item of value) visit(item, depth + 1); }
    else if (value && typeof value === 'object') { for (const [key, item] of Object.entries(value)) { append(key); visit(item, depth + 1); } }
    else append(JSON.stringify(value));
  }
  for (const [key, value] of Object.entries(event.fields || {})) {
    if (typeof value === 'string' && [event.message, event.raw, event.description].includes(value)) continue;
    append(key); visit(value, 0);
  }
  const originalLength = text.length;
  let previous = text;
  for (let i = 0; i < 2; i++) {
    const decoded = previous.replace(/(?:%[0-9a-f]{2})+/gi, encoded => {
      const bytes = Uint8Array.from(encoded.match(/%([0-9a-f]{2})/gi), hex => parseInt(hex.slice(1), 16));
      return new TextDecoder().decode(bytes);
    }).replace(/\\u([0-9a-f]{4})/gi, (original, hex) => { const code = parseInt(hex, 16); return code >= 0xd800 && code <= 0xdfff ? original : String.fromCharCode(code); });
    if (decoded === previous) break;
    append(decoded); previous = decoded;
  }
  return { text, originalLength, clipped };
}
async function scanRows(rows, filters) {
  const file = await catalog(), active = file.rules.filter(r => r.enabled).map(rule => ({ rule, regex: regex(rule.pattern) }));
  const selectors = (filters || []).filter(f => f.op === 'threat_rule');
  for (const filter of selectors) if (filter.column !== '_all' || filter.value !== '*' && !active.some(r => r.rule.id === filter.value)) throw new Error('Regra de ameaça inválida ou desativada.');
  let clipped = 0;
  const selected = [];
  for (const event of rows) {
    const body = corpus(event); clipped += Number(body.clipped);
    const matches = active.map(entry => ({ ...entry, match: entry.regex.exec(body.text) })).filter(entry => entry.match);
    if (selectors.every(filter => filter.value === '*' ? matches.length : matches.some(entry => entry.rule.id === filter.value))) selected.push({ event, body, matches });
  }
  return { active, selected, clipped };
}
const ranking = map => [...map].map(([name, count]) => ({ name, count })).sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
export async function threatScan(rows, filters) {
  const { active, selected, clipped } = await scanRows(rows, filters), hits = selected.filter(row => row.matches.length);
  const rules = new Map(), categories = new Map(), sources = new Map(), dates = [];
  let occurrences = 0, undated = 0;
  for (const { event, body, matches } of hits) {
    occurrences += matches.length;
    sources.set(event.source, (sources.get(event.source) || 0) + 1);
    if (Number.isFinite(event.timestamp)) dates.push(event.timestamp); else undated++;
    for (const category of new Set(matches.map(entry => entry.rule.category))) categories.set(category, (categories.get(category) || 0) + 1);
    for (const { rule, match } of matches) {
      const value = rules.get(rule.id) || { ...rule, count: 0, examples: [], start: null, end: null }; value.count++;
      if (Number.isFinite(event.timestamp)) { value.start = value.start === null ? event.timestamp : Math.min(value.start, event.timestamp); value.end = value.end === null ? event.timestamp : Math.max(value.end, event.timestamp); }
      if (value.examples.length < 3) value.examples.push({ event_id: event.id, event_ref: event.event_ref || '', timestamp: event.timestamp, snippet: body.text.slice(Math.max(0, match.index - 90), match.index + match[0].length + 130).slice(0, 800), normalized: match.index + match[0].length > body.originalLength });
      rules.set(rule.id, value);
    }
  }
  let width = 1000, start = dates.length ? Math.min(...dates) : null, end = dates.length ? Math.max(...dates) : null;
  while (dates.length && Math.floor(end / width) - Math.floor(start / width) >= 120) width *= 2;
  const bins = new Map(); for (const date of dates) { const key = Math.floor(date / width); bins.set(key, (bins.get(key) || 0) + 1); }
  const time = dates.length ? Array.from({ length: Math.floor(end / width) - Math.floor(start / width) + 1 }, (_, i) => { const key = Math.floor(start / width) + i; return { timestamp: key * width, count: bins.get(key) || 0 }; }) : [];
  return { total: selected.length, matched: hits.length, occurrences, complete: !clipped, clipped_records: clipped, enabled_rules: active.length, catalog_path: catalogPath, categories: ranking(categories), rules: [...rules.values()].sort((a, b) => b.count - a.count || a.id.localeCompare(b.id)), time, time_bucket_ms: width, sources: ranking(sources).slice(0, 10), undated_matches: undated, corpus_limit: LIMIT, start, end };
}
export async function threatEvents(rows, args) {
  const filters = args.filters || [];
  const selectors = filters.some(f => f.op === 'threat_rule') ? filters : [...filters, { column: '_all', op: 'threat_rule', value: '*' }];
  const { selected, clipped } = await scanRows(rows, selectors);
  const page = selected.slice(args.offset || 0, (args.offset || 0) + Math.max(1, Math.min(200, args.limit || 100)));
  let rowsClipped = 0;
  const previews = page.map(({ event }) => {
    if (encoder.encode(JSON.stringify(event)).length <= LIMIT) return event;
    rowsClipped++;
    const result = { ...event, fields: {}, message: prefix(event.message || '', 12000), raw: prefix(event.raw || '', 24000), description: prefix(event.description || '', 2000) };
    while (encoder.encode(JSON.stringify(result)).length > LIMIT) { const key = Object.keys(result).filter(key => typeof result[key] === 'string').sort((a, b) => result[b].length - result[a].length)[0]; result[key] = result[key].slice(0, Math.floor(result[key].length / 2)); }
    return result;
  });
  return { total: selected.length, rows: previews, complete: !clipped, clipped_records: clipped, rows_clipped: rowsClipped };
}
export async function threatFilterRows(rows, filters) {
  return (await scanRows(rows, filters)).selected.map(({event})=>event);
}
