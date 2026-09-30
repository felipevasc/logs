/* Pure, testable helpers for bounded background work and honest phase estimates. */
window.PerformanceTools = (() => {
  "use strict";
  function queue(concurrency = 1) {
    let active = 0;
    const waiting = [];
    function pump() {
      while (active < concurrency && waiting.length) {
        const job = waiting.shift();
        if (!job.current()) { job.reject(new Error("Operação substituída.")); continue; }
        active++;
        Promise.resolve().then(job.run).then(job.resolve, job.reject).finally(() => { active--; pump(); });
      }
    }
    return { add(run, current = () => true) { return new Promise((resolve, reject) => { waiting.push({ run, current, resolve, reject }); setTimeout(pump, 0); }); }, pending: () => waiting.length, active: () => active };
  }
  function estimate(previous, payload, now = performance.now()) {
    const phase = `${payload.operationId || payload.operation || "legacy"}:${payload.phaseId || payload.phase}:${payload.unit || ""}`;
    const completed = Math.max(0, Number(payload.completed) || 0), total = Math.max(0, Number(payload.total) || 0);
    const reset = !previous || previous.phase !== phase || completed < previous.completed || total !== previous.total;
    const samples = reset ? [] : previous.samples.slice();
    samples.push({ completed, now });
    while (samples.length > 20 || (samples.length > 2 && now - samples[0].now > 12000)) samples.shift();
    const seconds = (now - samples[0].now) / 1000;
    const rate = seconds >= 1 ? (completed - samples[0].completed) / seconds : null;
    // Estimate this phase only after several samples; finalization and unknown totals have no ETA.
    const eta = total > completed && rate > 0 && samples.length >= 3 && seconds >= 2 ? (total - completed) / rate : null;
    return { phase, completed, total, samples, rate, eta, percent: payload.state === "ready" ? 100 : total > completed ? completed / total * 100 : null, updated: now };
  }
  const duration = seconds => { const s = Math.max(0, Math.round(seconds)); return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}min ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor(s / 60) % 60}min`; };
  return { queue, estimate, duration };
})();
