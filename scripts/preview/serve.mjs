/* Servidor estático de preview: serve frontend/ injetando o mock do Tauri.
   Uso: node scripts/preview/serve.mjs [porta] */
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { handlePreviewDownload } from "./downloads.mjs";

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
    if (await handlePreviewDownload(req, res, url)) return;
    let path = decodeURIComponent(url.pathname);
    if (path === "/__mock-journeys__.js") {
      res.writeHead(200, { "content-type": MIME[".js"] });
      res.end(await readFile(new URL("./mock-journeys.js", import.meta.url))); return;
    }
    if (path === "/__mock-threats__.js") {
      res.writeHead(200, { "content-type": MIME[".js"] });
      res.end(await readFile(new URL("./mock-threats.js", import.meta.url))); return;
    }
    if (path === "/__threat-catalog__.json") {
      const catalogFile=fileURLToPath(new URL("../../src-tauri/resources/threat-rules.json", import.meta.url));
      res.writeHead(200, { "content-type": MIME[".json"], "X-Catalog-Path": encodeURIComponent(catalogFile), "cache-control": "no-store" });
      res.end(await readFile(catalogFile)); return;
    }
    if (path === "/__mock-remote__.js") {
      res.writeHead(200, { "content-type": MIME[".js"] });
      res.end(await readFile(new URL("./mock-remote.js", import.meta.url)));
      return;
    }
    if (path === "/__mock-pivot__.js") {
      res.writeHead(200, { "content-type": MIME[".js"] });
      res.end(await readFile(new URL("./mock-pivot.js", import.meta.url)));
      return;
    }
    if (path === "/__mock__.js") {
      res.writeHead(200, { "content-type": MIME[".js"] });
      res.end(await readFile(mockFile));
      return;
    }
    if (path === "/") path = "/index.html";
    const file = normalize(join(root, path));
    if (!file.startsWith(root + sep)) throw new Error("forbidden");
    let body = await readFile(file);
    if (file.endsWith("index.html")) {
      body = Buffer.from(
        body.toString("utf8").replace(
          '<script src="app.js"></script>',
          '<script src="/__mock-pivot__.js"></script>\n  <script src="/__mock-remote__.js"></script>\n  <script src="/__mock-journeys__.js"></script>\n  <script src="/__mock__.js"></script>\n  <script src="app.js"></script>',
        ),
      );
    }
    res.writeHead(200, { "content-type": MIME[extname(file)] || "application/octet-stream", "cache-control": "no-store" });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end("not found");
  }
}).listen(port, "127.0.0.1", () => console.log(`preview em http://127.0.0.1:${port}`));
