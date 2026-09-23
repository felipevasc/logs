/* Preview-only implementations of the native aggregate and pivot contracts. */
(() => {
  const raw = (row, field) => Object.hasOwn(row, field) ? row[field] : row.fields?.[field];
  const text = (row, field) => { const value = raw(row, field); return value === undefined || value === null ? null : typeof value === "object" ? JSON.stringify(value) : String(value); };
  const groupValue = (row, field) => { const value = text(row, field); return value?.trim() ? value : null; };
  const label = value => value == null ? "(vazio)" : value;
  const numeric = value => {
    const match = String(value).trim().toLowerCase().match(/^(-?\d+(?:[.,]\d+)?(?:e[+-]?\d+)?)\s*(bytes?|b|kb|mb|gb|tb|bps|kbps|mbps|gbps|ms|s|min|h)?$/);
    if (!match) return NaN;
    const scales = {b:1,byte:1,bytes:1,kb:1024,mb:1024**2,gb:1024**3,tb:1024**4,bps:1,kbps:1e3,mbps:1e6,gbps:1e9,ms:1,s:1000,min:60000,h:3600000};
    return Number(match[1].replace(',', '.')) * (scales[match[2]] || 1);
  };
  const measure = (rows, spec, stringLimit = 100) => {
    if (spec.func === "count") return rows.length;
    const strings = rows.map(row => text(row, spec.column)).filter(value => value !== null);
    if (spec.func === "count_distinct") return new Set(strings).size;
    if (spec.func === "string_agg") return strings.filter(Boolean).slice(0, stringLimit).join(", ");
    const values = strings.map(numeric).filter(Number.isFinite);
    const sum = values.reduce((acc, value) => acc + value, 0);
    if (spec.func === "sum") return sum;
    if (!values.length) return null;
    if (spec.func === "avg") return Math.round(sum / values.length * 100) / 100;
    if (spec.func === "min") return values.reduce((a, b) => Math.min(a, b), Infinity);
    if (spec.func === "max") return values.reduce((a, b) => Math.max(a, b), -Infinity);
    return null;
  };
  const alias = spec => spec.alias || `${spec.func}(${spec.column})`;
  window.__mockAggregate = (events, field, specs) => {
    const groups = new Map();
    for (const row of events) { const key = groupValue(row, field); if (!groups.has(key)) groups.set(key, []); groups.get(key).push(row); }
    return { columns: [field, ...specs.map(alias)], group_values: [...groups.keys()], rows: [...groups].map(([key, rows]) => Object.fromEntries([[field, label(key)], ...specs.map(spec => [alias(spec), measure(rows, spec)])])) };
  };
  window.__mockPivot = (events, spec) => {
    const rows = spec.rows || [], cols = spec.cols || [], values = spec.values || [];
    const columnKeys = [], paths = new Map(), cells = new Map(), totals = new Map();
    for (const row of events) {
      const columnKey = JSON.stringify(cols.map(field => groupValue(row, field)));
      if (!totals.has(columnKey)) { columnKeys.push(columnKey); totals.set(columnKey, []); }
      totals.get(columnKey).push(row);
      const full = rows.map(field => groupValue(row, field));
      const prefixes = rows.length ? rows.map((_, index) => full.slice(0, index + 1)) : [[]];
      for (const prefix of prefixes) {
        const key = JSON.stringify(prefix); paths.set(key, prefix);
        if (!cells.has(key)) cells.set(key, new Map());
        const columns = cells.get(key); if (!columns.has(columnKey)) columns.set(columnKey, []); columns.get(columnKey).push(row);
      }
    }
    const ordered = [...paths.values()].sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
    const visible = ordered.slice(0, spec.limit_rows || 2000);
    return {
      value_names: values.map(alias), col_keys: columnKeys.map(key => cols.length ? JSON.parse(key).map(label).join(" → ") : "(total)"), col_values: columnKeys.map(key => JSON.parse(key)), row_paths: visible.map(path => path.map(label)), row_values: visible,
      cells: visible.map(path => columnKeys.map(columnKey => {
        const group = cells.get(JSON.stringify(path))?.get(columnKey); return values.map(value => group ? measure(group, value, 50) : null);
      })),
      totals: columnKeys.map(columnKey => values.map(value => measure(totals.get(columnKey), value, 50))),
      truncated: ordered.length > visible.length, complete: true, processed_events: events.length,
    };
  };
})();
