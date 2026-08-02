/* Servidor estático de preview: serve frontend/ injetando o mock do Tauri.
   Uso: node scripts/preview/serve.mjs [porta] */
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("../../frontend", import.meta.url)));
const mockFile = fileURLToPath(new URL("./mock-tauri.js", import.meta.url));
const port = Number(process.argv[2]) || 4173;

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
};

createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    let path = decodeURIComponent(url.pathname);
    if (path === "/__mock__.js") {
      res.writeHead(200, { "content-type": MIME[".js"] });
      res.end(await readFile(mockFile));
      return;
    }
    if (path === "/") path = "/index.html";
    const file = normalize(join(root, path));
    if (!file.startsWith(root)) throw new Error("forbidden");
    let body = await readFile(file);
    if (file.endsWith("index.html")) {
      body = Buffer.from(
        body.toString("utf8").replace(
          '<script src="app.js"></script>',
          '<script src="/__mock__.js"></script>\n  <script src="app.js"></script>',
        ),
      );
    }
    res.writeHead(200, { "content-type": MIME[extname(file)] || "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end("not found");
  }
}).listen(port, () => console.log(`preview em http://127.0.0.1:${port}`));
