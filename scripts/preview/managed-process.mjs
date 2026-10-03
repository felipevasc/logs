import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';

const local = name => fileURLToPath(new URL(name, import.meta.url));
const cleanupError = message => Object.assign(Error(message), { fatalCleanup: true });
export const previewEnvironment = (env = process.env) => ({ ...env, PLAYWRIGHT_CHANNEL: env.PLAYWRIGHT_CHANNEL || 'chromium' });

// A supervisor owns each process tree, including browsers that detach from the
// Node process group. Its successful exit means that every owned child exited.
export function spawnManaged(args, { cwd, env = process.env, stdout = 'inherit' } = {}) {
  let executable, parameters;
  if (process.platform === 'win32') {
    executable = 'powershell.exe';
    const quote = value => `'${value.replaceAll("'", "''")}'`;
    const encodedArgs = Buffer.from(JSON.stringify([local('managed-child.mjs'), ...args])).toString('base64');
    const script = `& {\n${readFileSync(local('windows-job.ps1'), 'utf8')}\n} -NodePath ${quote(process.execPath)} -ArgumentsBase64 ${quote(encodedArgs)}`;
    // Invoke the checked-in command explicitly, as the native harness does;
    // leave machine execution policy and antivirus settings unchanged.
    parameters = ['-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')];
  } else if (process.platform === 'linux') {
    executable = 'python3';
    parameters = [local('linux-process-tree.py'), process.execPath, ...args];
  } else throw Error('The preview suite supports Windows and Linux process supervision.');
  const child = spawn(executable, parameters, { cwd, env, windowsHide: true, stdio: ['pipe', stdout, 'inherit'] });
  // The supervisor watches this pipe for EOF. Closing it requests cleanup;
  // killing the Node test directly would lose ownership of browser descendants.
  child.stdin.on('error', () => {});
  let finished = false, stopping;
  const closed = new Promise((resolve, reject) => {
    child.once('error', error => {
      finished = true;
      reject(process.platform === 'linux' && error.code === 'ENOENT'
        ? Error('Preview supervision requires Python 3.9+ with Linux pidfd support; install python3 before running the suite.') : error);
    });
    child.once('close', (code, signal) => { finished = true; resolve({ code, signal }); });
  });
  // Startup can fail before a caller starts waiting for readiness.
  closed.catch(() => {});
  const stop = () => stopping ??= (async () => {
    if (!finished) child.stdin.end();
    let timer;
    try {
      const result = await Promise.race([closed, new Promise((_, reject) => {
        timer = setTimeout(() => reject(cleanupError('Process tree cleanup timed out; remaining tests must not run.')), 20_000);
      })]);
      if (result.code === 125) throw cleanupError('Process tree supervisor failed; remaining tests must not run.');
      return result;
    } finally { clearTimeout(timer); }
  })();
  return { child, stdout: child.stdout, closed, stop };
}

export async function waitManaged(managed, timeoutMs) {
  let timer;
  try {
    const result = await Promise.race([managed.closed, new Promise((_, reject) => {
      timer = setTimeout(() => reject(Error(`Timed out after ${timeoutMs} ms`)), timeoutMs);
    })]);
    if (result.code !== 0) throw Error(`Exited with ${result.code ?? result.signal}`);
  } finally {
    clearTimeout(timer);
    // Do not return/reject (or allow the next scenario to start) until cleanup
    // has completed, including on timeout and on a nonzero test exit.
    await managed.stop();
  }
}
