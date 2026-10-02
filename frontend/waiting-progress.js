/* Presentation adapter for received progress. It does not estimate work, issue IPC,
   identify success from a full counter, or reinterpret source admission. */
window.WaitingProgress = (() => {
  "use strict";
  const labels = Object.freeze({
    "metadata-json-boundaries": "Localizando registros",
    "metadata-scan": "Indexando registros",
    "metadata-columns": "Identificando campos",
    "metadata-validate": "Conferindo checkpoint",
    "metadata-map-validate": "Conferindo índice",
    "metadata-checkpoint-write": "Guardando para retomar",
    "metadata-checkpoint-sync": "Confirmando gravação",
    "metadata-checkpoint-publish": "Publicando checkpoint",
    "metadata-checkpoint-committed": "Checkpoint preservado",
    "metadata-restore": "Retomando metadados",
    "metadata-lock": "Aguardando acesso",
    "analytics-select": "Selecionando registros",
    "analytics-verify": "Conferindo seleção",
    "analytics-sql": "Calculando o recorte",
    "analytics-time-index": "Lendo resumo temporal",
    "canonical-verify": "Conferindo conversão",
  });
  const measured = value => typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
  function snapshot(payload = {}, { operationId, elapsedMs, estimateMs } = {}) {
    payload = payload && typeof payload === "object" ? payload : {};
    const phaseId = typeof payload.phaseId === "string" ? payload.phaseId : "";
    const unit = typeof payload.unit === "string" ? payload.unit : "";
    const hasCount = measured(payload.completed) !== undefined && (unit || payload.total > 0);
    const awaitingLoadSettlement = payload.operation === "carregamento" && ["ready", "Pronto", "Concluído"].includes(phaseId);
    return {
      operationId: payload.operationId || operationId || "",
      phaseId,
      // A native phase may be ready while activation or Case-save acknowledgement
      // is still pending. Only the owner's settlement closes the waiting view.
      state: payload.error ? "error" : "running",
      label: payload.error ? String(payload.error) : awaitingLoadSettlement ? "Confirmando abertura"
        : (Object.hasOwn(labels, phaseId) ? labels[phaseId] : payload.phase || "Processando"),
      ...(hasCount ? { completed: payload.completed, total: payload.total, unit } : {}),
      elapsedMs: measured(payload.elapsedMs) ?? measured(elapsedMs),
      estimateMs: measured(estimateMs),
    };
  }
  return Object.freeze({ snapshot });
})();
