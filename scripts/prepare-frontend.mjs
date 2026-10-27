import { cpSync, mkdirSync, rmSync, readFileSync, existsSync } from "node:fs";
import { dirname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = resolve(root, "frontend");
const output = resolve(source, "dist");

rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });

for (const item of ["index.html", "icon-picker.js", "case-timeline.js", "case-removals.js", "case-content.js", "case-content.css", "case-trails.js", "case-trails.css", "case-report.js", "context-menu.js", "app.js", "analysis-context.js", "canonical-fields.js", "analysis-fields.js", "explorer-timeline.js", "explorer-timeline.css", "field-transform.js", "field-transform.css", "exclusion-archive.js", "exclusion-visibility.js", "exclusion-archive.css", "case-references.js", "case-references.css", "performance-core.js", "detail-fields.js", "java-trace.js", "java-trace.css", "timeline.js", "timeline-export.js", "workspace.js", "workspace-context.js", "analysis-workbench.js", "discovery.js", "threats.js", "remote-sources.js", "journeys.js", "styles.css", "workspace.css", "workspace-context.css", "analysis-workbench.css", "timeline-refinements.css", "timeline-export.css", "discovery.css", "threats.css", "remote-sources.css", "journeys.css", "query-lang.js", "entity-menu.js", "query-bar.js", "evidence-ui.js", "security.js", "security.css", "investigation.js", "investigation.css", "event-insights.js", "case-intel.js", "command-palette.js", "updates.js", "updates.css", "ui-scale.js", "tasks.js", "vendor"]) {
  cpSync(resolve(source, item), resolve(output, item), { recursive: true });
}

// A successful build must include every local script referenced by the entrypoint.
for (const item of [
  "participants-ui.js", "assets", "date-values.js", "load-progress.js", "waiting-visuals.js", "waiting-visuals.css", "waiting-progress.js",
  "case-evidence.js", "case-evidence-session.js", "case-evidence-analysis.js",
  "case-evidence-preserved.js", "case-evidence-items.js", "case-evidence-actions.js",
  "case-evidence-detail.js", "case-evidence-timeline.js", "case-evidence-display.js",
  "case-evidence-transfer.js", "case-evidence-recovery.js", "resource-settings.js", "resource-settings.css",
  "resources.js", "resources.css", "value-inspector.js",
  "field-indexes.js",
  "compromise-progress.js",
  "log-split.js", "log-split.css",
  "local-native.js", "local-explorer.js", "local-explorer.css", "file-viewer.js",
  "regex-syntax.js", "regex-editor.js", "regex-editor.css", "regex-assist.js", "regex-workbench.js",
  "event-context.js", "event-pattern-search.js", "field-extraction.js", "extraction-model.js", "columns-engine.js", "columns.js", "columns.css", "extraction.js", "extraction.css",
]) {
  cpSync(resolve(source, item), resolve(output, item), { recursive: true });
}

for (const [, src] of readFileSync(resolve(output, "index.html"), "utf8").matchAll(/<script\b[^>]*\bsrc="([^"]+)"/g)) {
  const path = resolve(output, src.split(/[?#]/)[0]);
  if (!path.startsWith(output + sep) || !existsSync(path)) throw Error(`Missing or invalid bundled script: ${src}`);
}
