/* Browser preview only: bounded, short-lived downloads with real attachment filenames. */
import { randomUUID } from "node:crypto";
const files = new Map(), MAX_FILE = 32 * 1024 * 1024, MAX_BODY = Math.ceil(MAX_FILE * 4 / 3) + 8192, MAX_CACHE = MAX_FILE * 2;
const prefix = "/__timeline-downloads__/";
const json = (res, status, value) => { res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" }); res.end(JSON.stringify(value)); };
function prune() { for (const [key, file] of files) if (file.expires < Date.now()) files.delete(key); }
export async function handlePreviewDownload(req, res, url) {
  if (!url.pathname.startsWith(prefix)) return false;
  prune();
  if (req.method === "POST" && url.pathname === prefix) {
    if (req.headers.origin && req.headers.origin !== `http://${req.headers.host}` || !String(req.headers["content-type"]).startsWith("application/json")) { json(res, 403, { error: "Origem inválida." }); return true; }
    if (Number(req.headers["content-length"]) > MAX_BODY) { json(res, 413, { error: "Arquivo muito grande." }); req.resume(); return true; }
    let size = 0; const chunks = [];
    for await (const chunk of req) { size += chunk.length; if (size > MAX_BODY) { json(res, 413, { error: "Arquivo muito grande." }); req.resume(); return true; } chunks.push(chunk); }
    let value;
    try { value = JSON.parse(Buffer.concat(chunks).toString("utf8")); } catch { json(res, 400, { error: "Dados inválidos." }); return true; }
    const format = value.format, filename = String(value.filename || "");
    if (!["png", "pdf"].includes(format) || !filename.endsWith(`.${format}`) || filename.length > 190 || /[\\/\x00-\x1f\x7f]/.test(filename) || typeof value.base64 !== "string" || !/^[A-Za-z0-9+/]+={0,2}$/.test(value.base64)) { json(res, 400, { error: "Nome ou formato inválido." }); return true; }
    const bytes = Buffer.from(value.base64, "base64");
    const valid = format === "png" ? bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])) : bytes.subarray(0, 5).toString() === "%PDF-";
    if (!valid || bytes.length > MAX_FILE) { json(res, 400, { error: "Arquivo inválido." }); return true; }
    let cached = [...files.values()].reduce((total, file) => total + file.bytes.length, 0);
    while (cached + bytes.length > MAX_CACHE && files.size) { const key = files.keys().next().value; cached -= files.get(key).bytes.length; files.delete(key); }
    const token = randomUUID(); files.set(token, { filename, format, bytes, expires: Date.now() + 120000 });
    json(res, 200, { url: `${prefix}${token}/${encodeURIComponent(filename)}`, filename }); return true;
  }
  if (req.method === "GET" || req.method === "HEAD") {
    const [, token, encodedName] = url.pathname.slice(prefix.length - 1).split("/");
    const file = files.get(token);
    if (!file || decodeURIComponent(encodedName || "") !== file.filename) { json(res, 404, { error: "O download expirou. Exporte novamente." }); return true; }
    const fallback = file.filename.normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/[^\x20-\x7E]|["\\]/g, "-");
    res.writeHead(200, { "content-type": file.format === "pdf" ? "application/pdf" : "image/png", "content-length": file.bytes.length,
      "content-disposition": `attachment; filename="${fallback}"; filename*=UTF-8''${encodeURIComponent(file.filename)}`, "cache-control": "no-store", "x-content-type-options": "nosniff" });
    res.end(req.method === "HEAD" ? undefined : file.bytes); return true;
  }
  json(res, 405, { error: "Método inválido." }); return true;
}
