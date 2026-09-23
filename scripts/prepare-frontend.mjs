import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(root, "frontend");
const output = resolve(source, "dist");

rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });

for (const item of ["index.html", "icon-picker.js", "case-timeline.js", "app.js", "timeline.js", "timeline-export.js", "workspace.js", "workspace-context.js", "analysis-workbench.js", "discovery.js", "threats.js", "remote-sources.js", "journeys.js", "styles.css", "workspace.css", "workspace-context.css", "analysis-workbench.css", "timeline-refinements.css", "timeline-export.css", "discovery.css", "threats.css", "remote-sources.css", "journeys.css", "vendor"]) {
  cpSync(resolve(source, item), resolve(output, item), { recursive: true });
}
