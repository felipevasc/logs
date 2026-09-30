import { fileURLToPath } from "node:url";
import { mkdir, writeFile } from 'node:fs/promises';

export async function captureFailure(page, name, error, details = {}) {
  const directory = new URL('../../output/playwright/', import.meta.url);
  await mkdir(directory, { recursive: true });
  const diagnostics = { ...details, error: String(error.stack || error) };
  try {
    diagnostics.page = await page.evaluate(() => {
      const current = typeof state === 'undefined' ? null : state;
      return {
        url: location.href, page: document.body.dataset.page,
        workspaceReady: window.WorkspaceContext?.ready, changing: window.WorkspaceContext?.changing,
        scope: window.WorkspaceContext?.scope(), loaded: current?.loaded, loadOverlay: current?.loadOverlay,
        total: current?.total, rows: current?.rows?.length, columns: current?.columns, visibleCols: current?.visibleCols,
        queryError: current?.queryError, activeCase: current?.cases?.active, artifact: current?.currentArtifact?.id,
        cases: current?.cases?.cases?.map(item => ({ id: item.id, items: item.items?.length, activeScope: item.workspace?.activeScope })),
        treeScope: document.querySelector('#explore-tree')?.dataset.treeScope,
        tableRows: [...document.querySelectorAll('#events-table tbody tr')].slice(0, 5).map(row => ({ id: row.dataset.eventId, cells: [...row.children].map(cell => cell.dataset.column) })),
        loadStatus: document.querySelector('#load-status')?.textContent,
        toasts: [...document.querySelectorAll('.toast')].map(node => node.textContent),
        requests: window.__mockRequests,
      };
    });
  } catch (captureError) { diagnostics.captureError = String(captureError); }
  try { await page.screenshot({ path: fileURLToPath(new URL(`${name}-failure.png`, directory)), fullPage: true }); }
  catch (captureError) { diagnostics.screenshotError = String(captureError); }
  await writeFile(new URL(`${name}-failure.json`, directory), JSON.stringify(diagnostics, null, 2));
  console.error(JSON.stringify(diagnostics, null, 2));
}
