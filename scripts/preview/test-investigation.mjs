import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { launchBrowser } from './browser.mjs';
import { captureFailure } from './diagnostics.mjs';
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: 'reduce' });
const errors = []; page.on('pageerror', error => errors.push(error.message));
let phase = 'startup';
try {
  await mkdir('output/playwright', { recursive: true });
  await page.goto(process.argv[2] || 'http://127.0.0.1:4173');
  await page.waitForFunction(() => WorkspaceContext?.ready && !WorkspaceContext.changing && state.loaded && state.rows.length && !state.loadOverlay);
  await page.evaluate(() => {
    const value = '<img src=x onerror="window.__unsafe=true">';
    window.__mockInvestigationFixture = { analysis_id: 'preview-analysis', investigation: { enabled: true, complete: true, population: 6000, signals: 2, edges: 1 } };
    const signal = { id: 'signal-a', kind: 'new_relationship', namespace: 'tenant-a', value, priority: 25, explanation: 'Relação nova; confira cobertura.', measurements: { events: 1 }, benign_alternatives: ['Deploy autorizado'], missing_evidence: ['Autorização'] };
    window.__mockInvestigationPages = {
      queue: { items: [{ namespace: 'tenant-a', column: '@user', value, priority: 25, ranking: { score: 25, independent_groups: 1, meaning: 'Ordem de investigação, sem promoção de evidência.' } }] },
      signals: { items: [signal] }, members: { items: [{ event_ref: state.rows[0].event_ref || `preview:${state.rows[0].id}`, event_id: state.rows[0].id, timestamp: state.rows[0].timestamp }] },
      graph: { items: [{ namespace: 'tenant-a', left_kind: 'user', left_value: value, right_kind: 'host', right_value: 'host-a', relation: 'observed_with', quality: 'contextual' }] },
      story: { items: [{ event_ref: 'ref', event_id: 0, timestamp: null, facts: { metadata: { command: 'password=do-not-show' } } }], story: { text: 'Há eventos relacionados.', limitations: ['O primeiro evento não identifica a origem.'] } },
      hunts: { items: [{ id: 'identity', title: 'Credenciais e sessão', required_fields: ['ator', 'timestamp'] }] },
      'hunt:identity': { items: [signal], playbook: { title: 'Credenciais e sessão', required_fields: ['ator'], competing_hypotheses: ['Credencial expirada'], next_queries: ['Comparar sucessos e falhas'] } },
      proposal: {items:[{signal_id:'signal-a',rule_hash:'hash-a',rule:{enabled:false,evidence:{maturity:'unassessed'}},limitation:'Revisão com controles e sem promoção de evidência.'}]},
    };
  });
  await page.evaluate(() => Workspace.showPage('compromises'));
  await page.locator('[data-calculate-compromises]').click();
  const host = page.locator('[data-investigation]'); await host.waitFor();
  phase = 'queue and literal text';
  await host.getByText('Ordem de investigação, sem promoção de evidência.', { exact: true }).waitFor({ state: 'attached' });
  assert.equal(await host.locator('img').count(), 0); assert.equal(await page.evaluate(() => window.__unsafe), undefined);
  await page.screenshot({ path: 'output/playwright/investigation-queue.png', fullPage: true });
  await host.getByRole('button', { name: 'Sinais da entidade', exact: true }).click();
  await host.getByText('Relação nova; confira cobertura.').waitFor();
  await host.getByRole('button', { name: 'Ver eventos de apoio', exact: true }).click();
  await host.getByRole('button', { name: 'Abrir evento original', exact: true }).click();
  await host.getByText('Evento original', { exact: true }).waitFor();
  phase = 'guided hunts and competing hypotheses';
  await host.getByRole('button', { name: 'Caças guiadas', exact: true }).click();
  await host.getByRole('button', { name: 'Abrir caça', exact: true }).click();
  await host.getByText('Hipóteses concorrentes: Credencial expirada').waitFor();
  assert.match(await host.innerText(), /Comparar sucessos e falhas/);
  await host.getByRole('button', { name: 'Cronologia', exact: true }).click();
  await host.getByText('Há eventos relacionados.').waitFor();
  assert.match(await host.innerText(), /password=do-not-show/, 'original values remain literal, as in the event inspector');
  phase = 'optional synthesis citation review stays separate from classification';
  await host.getByRole('button', {name:'Preparar síntese assistida',exact:true}).click();
  const draft=host.getByRole('textbox',{name:'Síntese assistida com citações (JSON)',exact:true});await draft.fill(JSON.stringify({claims:[{kind:'observation',text:'password=do-not-show',event_refs:['ref-a'],signal_ids:[]}]}));
  await host.getByRole('button',{name:'Conferir citações da síntese',exact:true}).click();await host.getByText(/human_review_required/).waitFor();assert.match(await host.innerText(),/do-not-show/);
  phase = 'proposals with failed controls and stale identity';
  await host.getByRole('button',{name:'Sinais',exact:true}).click();
  await host.getByRole('button',{name:'Propor regra',exact:true}).click();
  const activate=host.getByRole('button',{name:'Ativar regra revisada neste Caso',exact:true});assert.equal(await activate.isDisabled(),true);
  await host.getByRole('textbox',{name:'Controles positivos e negativos da proposta (JSON)',exact:true}).fill(JSON.stringify({positive:['ref-a'],negative:['ref-b'],holdout_declaration:'Origens e períodos distintos'}));
  await host.getByRole('button',{name:'Avaliar com controles',exact:true}).click();await host.getByText(/"false_positives": 1/).waitFor();assert.equal(await activate.isDisabled(),true);
  await page.evaluate(()=>{window.__mockProposalEvaluation={activation_gate:true,analysis_id:'old-analysis'};});
  await host.getByRole('button',{name:'Avaliar com controles',exact:true}).click();await host.getByText('Error: Avaliação incompleta ou desatualizada.').waitFor();assert.equal(await activate.isDisabled(),true);
  await page.evaluate(()=>{window.__mockProposalEvaluation={activation_gate:true};});
  await host.getByRole('button',{name:'Avaliar com controles',exact:true}).click();await host.getByText(/"activation_gate": true/).waitFor();assert.equal(await activate.isDisabled(),false);
  await host.getByRole('textbox',{name:'Controles positivos e negativos da proposta (JSON)',exact:true}).fill('{}');assert.equal(await activate.isDisabled(),true);
  assert.equal(await page.evaluate(()=>window.__mockCommandCalls.investigation_proposal_accept||0),0);
  phase = 'failure, retry and outdated analysis';
  await page.evaluate(() => { window.__mockFailures = { investigation_page: 'Falha controlada' }; });
  await host.getByRole('button', { name: 'Sinais', exact: true }).click();
  await host.getByText(/Falha controlada/).waitFor();
  await page.evaluate(() => { window.__mockFailures = {}; window.__mockInvestigationPages.signals.analysis_id = 'wrong-analysis'; });
  await host.getByRole('button', { name: 'Tentar novamente', exact: true }).click();
  await host.getByText(/Resultado incompleto ou de outra análise/).waitFor();
  assert.equal(await page.evaluate(() => window.__mockRequests.filter(row => row.cmd === "triage" && !row.cacheOnly).length), 1, 'paging and hunts never scan');
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ queue: true, exactOriginalEvent: true, hunts: true, escapedText: true, literalOriginalValues: true, staleAnalysisRejected: true, errors }, null, 2));
} catch (error) { await captureFailure(page, 'test-investigation', error, { phase }); throw error; }
finally { await browser.close(); }
