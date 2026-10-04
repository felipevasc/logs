/* Date-like values shared by the table, record detail, value inspector and filters.
   Mirrors sources::epoch_to_ms / parse_timestamp / typed_instant in Rust, so a
   date typed in a filter means the same instant in every engine. */
window.DateValues = (() => {
  "use strict";
  const pad = (n, w = 2) => String(n).padStart(w, "0");

  // Epoch seconds/ms/µs/ns by magnitude, exactly like the Rust reader.
  function epochToMs(number) {
    if (!Number.isFinite(number)) return null;
    const magnitude = Math.abs(number);
    const ms = magnitude >= 1e17 ? number / 1e6 : magnitude >= 1e14 ? number / 1e3
      : magnitude >= 1e11 ? number : magnitude >= 1e8 ? number * 1000 : null;
    if (ms == null) return null;
    const truncated = Math.trunc(ms);
    return Math.abs(truncated) <= 8.64e15 ? truncated : null;
  }

  // Local (naive) or zoned date/time text; formats match parse_timestamp.
  const LOCAL = [
    [/^(\d{4})[-/](\d{1,2})[-/](\d{1,2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2})(?:[.,](\d{1,9}))?)?)?$/, "ymd"],
    [/^(\d{1,2})[/-](\d{1,2})[/-](\d{4})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2})(?:[.,](\d{1,9}))?)?)?$/, "dmy"],
  ];
  function parseDate(text) {
    const value = String(text ?? "").trim().replace(/^\[|\]$/g, "");
    if (value.length < 8) return null;
    if (/^-?\d+(\.\d+)?$/.test(value)) return epochToMs(Number(value));
    if (/(?:Z|[+-]\d{2}:?\d{2})$/i.test(value) && /\d{4}-\d{2}-\d{2}T/i.test(value)) {
      const zoned = Date.parse(value);
      return Number.isNaN(zoned) ? null : zoned;
    }
    for (const [pattern, order] of LOCAL) {
      const m = value.match(pattern);
      if (!m) continue;
      const [year, month, day] = order === "ymd" ? [m[1], m[2], m[3]] : [m[3], m[2], m[1]];
      const fraction = m[7] ? Number(`0.${m[7]}`) * 1000 : 0;
      const date = new Date(Number(year), Number(month) - 1, Number(day), Number(m[4] || 0), Number(m[5] || 0), Number(m[6] || 0), Math.trunc(fraction));
      if (date.getMonth() !== Number(month) - 1 || date.getDate() !== Number(day)) return null;
      return date.getTime();
    }
    return null;
  }

  // A field value read as an instant (`sources::text_to_ms`).
  function textToMs(value) {
    if (typeof value === "number") return epochToMs(value);
    const text = String(value ?? "").trim();
    return /^-?\d+(\.\d+)?(e[+-]?\d+)?$/i.test(text) ? epochToMs(Number(text)) : parseDate(text);
  }

  // Date/time typed by the user, never a plain number (`sources::typed_instant`).
  function typedInstant(text) {
    const value = String(text ?? "").trim();
    if (/^-?\d+(\.\d+)?$/.test(value) || !/[-/T]/.test(value)) return null;
    return parseDate(value);
  }

  function format(ms, { millis = false } = {}) {
    if (ms == null || !Number.isFinite(ms)) return "";
    const d = new Date(ms);
    const base = `${pad(d.getDate())}/${pad(d.getMonth() + 1)}/${d.getFullYear()} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
    return millis && d.getMilliseconds() ? `${base}.${pad(d.getMilliseconds(), 3)}` : base;
  }

  // Names that conventionally hold instants: JWT claims and *_at/*Time/*date.
  const NAMES = new Set(["iat", "exp", "nbf", "auth_time", "authtime", "updated", "created", "modified", "expires", "expiry",
    "expiration", "issued", "ts", "time", "timestamp", "datetime", "date", "start", "end", "lastlogin", "last_login"]);
  function isDateField(name) {
    const segment = String(name ?? "").split(".").pop();
    const lower = segment.toLowerCase();
    return NAMES.has(lower) || /(?:_at|_on|_time|_ts|_date|_epoch|timestamp|datetime)$/.test(lower) || /[a-z](?:At|On|Time|Ts|Date|Epoch|Timestamp)$/.test(segment);
  }

  // Formatted date shown beside the original value, or null when the value is
  // not clearly an instant (plausible years only; plain local text is already readable).
  function describe(name, value) {
    if (name === "timestamp" || value == null || typeof value === "boolean" || typeof value === "object" || !isDateField(name)) return null;
    const text = String(value).trim();
    const numeric = typeof value === "number" || /^-?\d+(\.\d+)?$/.test(text);
    const zoned = !numeric && /\d{4}-\d{2}-\d{2}T.*(?:Z|[+-]\d{2}:?\d{2})$/i.test(text);
    if (!numeric && !zoned) return null;
    const ms = textToMs(text);
    if (ms == null) return null;
    const year = new Date(ms).getFullYear();
    if (year < 1990 || year > 2100) return null;
    return format(ms, { millis: numeric && Math.abs(Number(text)) >= 1e11 });
  }

  // Filter values: what the user sees and edits versus what is stored.
  function isDateColumn(column) { return column === "timestamp" || isDateField(column); }
  function toInput(column, value) {
    if (value == null || value === "" || !isDateColumn(column)) return value;
    const ms = column === "timestamp" && /^-?\d+(\.\d+)?$/.test(String(value).trim()) ? Number(value) : textToMs(value);
    return ms == null ? value : format(ms, { millis: column === "timestamp" && new Date(ms).getMilliseconds() !== 0 });
  }
  function fromInput(column, text) {
    if (column !== "timestamp") return text;
    // The record timestamp keeps its stored epoch-ms form for every consumer.
    const ms = typedInstant(text);
    return ms == null ? text : String(ms);
  }

  return Object.freeze({ epochToMs, parseDate, textToMs, typedInstant, format, isDateField, isDateColumn, describe, toInput, fromInput });
})();
