// Small mandatory UI smoke plus affected subsystem checks. Full regression is
// explicit; unknown paths and missing comparison data fail closed to full scope.
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export const fullPreview = [
  'test-navigation.mjs', 'test-ui-scale.mjs', 'test-resource-settings.mjs', 'test-workspace-context.mjs',
  'test-responsiveness.mjs', 'test-journeys.mjs', 'test-threats.mjs', 'test-security-on-demand.mjs', 'test-remote-sources.mjs',
  'test-analysis-workbench.mjs', 'test-discovery.mjs', 'test-updates.mjs',
  'test-explorer-timeline.mjs', 'test-field-transform.mjs', 'test-exclusion-archive.mjs',
  'test-case-references.mjs', 'test-canonical-field-actions.mjs', 'test-java-trace.mjs',
  'test-native-case-startup.mjs', 'test-native-case-timeline.mjs',
  'test-context-menu.mjs', 'test-waiting-visuals.mjs', 'test-waiting-reactions.mjs', 'test-waiting-verification.mjs', 'test-waiting-manual.mjs', 'test-waiting-boundaries.mjs', 'test-waiting-playback.mjs', 'test-waiting-reduced-motion.mjs', 'test-waiting-restore-access.mjs', 'test-waiting-micro-reactions.mjs', 'test-toast-feedback.mjs', 'test-record-actions.mjs', 'test-export-scope.mjs', 'test-case-report-waiting.mjs', 'test-timeline-export-waiting.mjs', 'test-columns.mjs', 'test-source-actions.mjs', 'test-settings-codes.mjs', 'test-timestamp-editor.mjs',
];
export const essentialPreview = [
  'test-navigation.mjs', 'test-responsiveness.mjs', 'test-updates.mjs',
  'test-case-references.mjs', 'test-native-case-startup.mjs', 'test-native-case-timeline.mjs',
];
const affected = [
  [/^frontend\/(?:analysis-|discovery|event-insights)/, ['test-analysis-workbench.mjs', 'test-discovery.mjs']],
  [/^frontend\/(?:canonical-fields|detail-fields|entity-menu|query-|field-transform)/, ['test-field-transform.mjs', 'test-canonical-field-actions.mjs']],
  [/^frontend\/(?:exclusion-|case-removals)/, ['test-exclusion-archive.mjs']],
  [/^frontend\/(?:explorer-timeline|timeline|case-timeline)/, ['test-explorer-timeline.mjs']],
  [/^frontend\/timeline-export\./, ['test-timeline-export-waiting.mjs']],
  [/^frontend\/(?:java-trace)/, ['test-java-trace.mjs']],
  [/^frontend\/(?:journeys|case-trails|participants-ui)/, ['test-journeys.mjs']],
  [/^frontend\/(?:threats|security)/, ['test-journeys.mjs', 'test-threats.mjs', 'test-security-on-demand.mjs']],
  [/^frontend\/remote-sources/, ['test-remote-sources.mjs']],
  [/^frontend\/workspace/, ['test-waiting-reduced-motion.mjs', 'test-waiting-restore-access.mjs', 'test-waiting-micro-reactions.mjs', 'test-workspace-context.mjs', 'test-export-scope.mjs', 'test-source-actions.mjs', 'test-timestamp-editor.mjs']],
  [/^frontend\/case-report\./, ['test-case-report-waiting.mjs']],
  [/^frontend\/(?:ui-scale|styles|index\.html)/, ['test-ui-scale.mjs']],
  [/^frontend\/resource-settings\.(?:js|css)$/, ['test-resource-settings.mjs']],
  [/^frontend\/(?:context-menu|command-palette)/, ['test-navigation.mjs', 'test-context-menu.mjs', 'test-source-actions.mjs']],
  [/^frontend\/(?:waiting-|tasks)/, ['test-responsiveness.mjs', 'test-waiting-visuals.mjs', 'test-waiting-reactions.mjs', 'test-waiting-verification.mjs', 'test-waiting-manual.mjs', 'test-waiting-boundaries.mjs', 'test-waiting-playback.mjs', 'test-waiting-reduced-motion.mjs', 'test-waiting-restore-access.mjs', 'test-waiting-micro-reactions.mjs', 'test-case-report-waiting.mjs', 'test-timeline-export-waiting.mjs']],
];
const nativePath = /^(?:src-tauri\/|\.github\/|\.cargo\/|rust-toolchain(?:\.toml)?$|package(?:-lock)?\.json$|scripts\/(?:ci\/|release\/|tauri-build\.mjs$|prepare-frontend\.mjs$))/;
const documentationPath = /^(?:docs\/|README\.md$|LICENSE(?:\.[^/]*)?$|\.gitignore$|\.gitattributes$)/;
const knownFrontendPath = /^frontend\/(?:performance-core\.js|updates\.(?:js|css)|case-(?:evidence[^/]*|references\.[^/]*|content\.[^/]*|report\.js|intel\.js)|evidence-ui\.js|icon-picker\.js|assets\/.*)$/;

export function planValidation(files, { full = false, deleted = [] } = {}) {
  if (!Array.isArray(files) || files.some(p => typeof p !== 'string' || !p || /[\0\r\n]/.test(p))) {
    throw Error('Changed paths must be a valid array of Git paths.');
  }
  if (!Array.isArray(deleted) || deleted.some(path => !files.includes(path))) throw Error('Invalid deleted paths.');
  const removed = new Set(deleted);
  if (full || !files.length) return { native: true, preview: [...fullPreview], scope: 'full', reason: full ? 'Explicit full regression' : 'No reliable changed-path comparison' };
  let native = false, broad = false, active = false;
  const selected = new Set();
  for (const path of files) {
    if (documentationPath.test(path)) continue;
    active = true;
    if (path === 'scripts/ci/validation-plan.mjs') { broad = true; continue; }
    if (nativePath.test(path)) { native = true; broad = true; continue; }
    // The shared monolith owns many subsystems; file-only selection cannot
    // safely narrow its browser coverage, but it does not require Rust rebuilds.
    if (/^frontend\/(?:app\.js|index\.html|styles\.css|analysis-context\.js|performance-core\.js)$/.test(path)) { broad = true; continue; }
    if (/^scripts\/tests\//.test(path)) continue; // all Node tests always run
    const previewTest = path.match(/^scripts\/preview\/(test-[a-z0-9-]+\.mjs)$/);
    if (previewTest) {
      if (removed.has(path)) broad = true;
      else selected.add(previewTest[1]);
      continue;
    }
    if (path === 'scripts/preview/test-detail-fields.cjs') continue; // npm test
    if (/^scripts\/preview\//.test(path)) { broad = true; continue; }
    const matches = affected.filter(([pattern]) => pattern.test(path));
    for (const [, tests] of matches) for (const test of tests) selected.add(test);
    if (matches.length || knownFrontendPath.test(path)) continue;
    // New dependencies, tooling, vendored code or uncategorized files require
    // the complete matrix until their scope is reviewed and mapped here.
    native = true; broad = true;
  }
  if (!active) return { native: false, preview: [], scope: 'documentation', reason: 'Only documentation or repository metadata changed' };
  for (const test of broad ? fullPreview : essentialPreview) selected.add(test);
  return { native, preview: [...selected].sort(), scope: broad ? 'full' : 'essential-and-affected', reason: broad ? 'Shared UI, native, tooling or unknown runtime inputs changed' : 'Essential smoke plus changed UI subsystems; not full regression' };
}

export function parseChangedPaths(output) {
  const parts = output.split('\0');
  if (parts.pop() !== '' || parts.length % 2) throw Error('Invalid Git changed-path response.');
  const files = [], deleted = [];
  for (let i = 0; i < parts.length; i += 2) {
    const [status, path] = parts.slice(i, i + 2);
    if (!/^[AMDT]$/.test(status) || !path) throw Error('Unsupported Git changed-path status.');
    files.push(path);
    if (status === 'D') deleted.push(path);
  }
  return { files, deleted };
}

function validatePlan(plan) {
  if (!plan || typeof plan.native !== 'boolean' || !Array.isArray(plan.preview) || !['full', 'documentation', 'essential-and-affected'].includes(plan.scope)) throw Error('Invalid validation plan.');
  if (plan.preview.some(name => !/^test-[a-z0-9-]+\.mjs$/.test(name))) throw Error('Invalid preview test in validation plan.');
  if (!plan.preview.length && plan.scope !== 'documentation') throw Error('Non-documentation validation must select browser checks.');
  return plan;
}

function main() {
  const [mode = 'plan', path = 'output/validation-plan.json'] = process.argv.slice(2);
  if (mode === 'preview') {
    const plan = validatePlan(JSON.parse(readFileSync(path, 'utf8')));
    if (!plan.preview.length) { console.log('Browser checks not applicable: documentation-only scope.'); return; }
    console.log(`Browser scope: ${plan.scope}; ${plan.preview.length} scripts (not a native WebView test).`);
    const result = spawnSync(process.execPath, ['scripts/preview/run-smoke.mjs', ...plan.preview], { stdio: 'inherit' });
    if (result.error) throw result.error;
    process.exitCode = result.status ?? 1;
    return;
  }
  if (mode !== 'plan') throw Error(`Unknown validation mode: ${mode}`);
  const full = process.env.FULL_REGRESSION === 'true';
  const base = process.env.PR_BASE_SHA, head = process.env.PR_HEAD_SHA;
  let files = [], deleted = [];
  if (!full && base && head) {
    if (![base, head].every(sha => /^[a-f0-9]{40}$/.test(sha))) throw Error('Invalid comparison commit.');
    // --no-renames keeps both sides of a move/deletion in the scope decision.
    ({ files, deleted } = parseChangedPaths(execFileSync('git', ['diff', '--name-status', '--no-renames', '-z', `${base}...${head}`], { encoding: 'utf8' })));
  }
  const plan = { ...planValidation(files, { full, deleted }), files, deleted };
  writeFileSync(path, JSON.stringify(plan, null, 2) + '\n');
  console.log(JSON.stringify(plan, null, 2));
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `native=${plan.native}\nbrowser=${!!plan.preview.length}\n`);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY,
    `## Validation scope\n\n${plan.reason}\n\n- Native Windows/Linux: ${plan.native ? 'required' : 'not applicable to changed paths'}\n- Browser: ${plan.preview.length} selected checks\n- Node and release safeguards: always required\n\n${plan.preview.map(test => `- ${test}`).join('\n')}\n`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
