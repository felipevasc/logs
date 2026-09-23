/* Preview-only remote service fixture. No network calls or credential persistence. */
(() => {
  const connections = new Map(), secrets = new Map();
  let serial = 0, cancelled = false;
  window.__remoteMock = { delay: 100, nextError: null, persistentSecrets: true, requests: [] };
  const flags = connection => ({ ...connection, hasPassword: secrets.has(connection.id), passwordSaved: !!connection._remember && secrets.has(connection.id) });
  const publicConnection = connection => { const result = flags(connection); delete result._remember; return result; };
  window.__mockRemoteCancel = () => { cancelled = true; };
  window.__mockRemote = async (command, args = {}) => {
    const mock = window.__remoteMock;
    mock.requests.push({ command, connection: args.connection ? structuredClone(args.connection) : undefined, passwordProvided: !!args.password, rememberPassword: args.rememberPassword, from: args.from, to: args.to });
    if (command === "remote_list") return { connections: [...connections.values()].map(publicConnection), persistentSecrets: mock.persistentSecrets };
    if (command === "remote_delete") { connections.delete(args.id); secrets.delete(args.id); return null; }
    if (command === "remote_save") {
      const connection = { ...args.connection, id: args.connection.id || `remote-${++serial}`, _remember: !!args.rememberPassword };
      if (args.rememberPassword && !mock.persistentSecrets) throw Error("Este sistema permite senha apenas durante a sessão.");
      if (args.password) secrets.set(connection.id, args.password);
      connections.set(connection.id, connection); return publicConnection(connection);
    }
    cancelled = false;
    await new Promise(resolve => setTimeout(resolve, mock.delay));
    if (cancelled) throw Error("Operação cancelada");
    if (mock.nextError) { const message = mock.nextError; mock.nextError = null; throw Error(message); }
    if (command === "remote_test") return { ok: true, message: "Acesso confirmado ao índice. A conexão está pronta para importar." };
    if (command === "remote_import") {
      const count = Math.min(6000, args.connection.maxRecords);
      return { path: `C:/preview/remote-${Date.now()}.jsonl`, count, total: 6000, totalRelation: "eq", limited: count < 6000, bytes: count * 256, metadataField: "_loginsight_remote" };
    }
    throw Error(`Fixture remota desconhecida: ${command}`);
  };
})();
