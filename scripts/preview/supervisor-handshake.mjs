// Windows PowerShell 5.1's .NET Framework StreamWriter can emit a UTF-8 BOM
// before the supervisor writes anything. Only the complete run frame releases
// the child; a preamble, partial command or EOF never authorizes its import.
const commands = ['run\n', 'run\r\n', '\uFEFFrun\n', '\uFEFFrun\r\n'].map(value => Buffer.from(value, 'utf8'));
const maxBytes = Math.max(...commands.map(command => command.length));

export function waitForSupervisorRun(input) {
  return new Promise((resolve, reject) => {
    let received = Buffer.alloc(0);
    const finish = error => {
      input.removeListener('data', onData);
      input.removeListener('end', onEnd);
      input.removeListener('close', onEnd);
      input.removeListener('error', finish);
      if (error) reject(error);
      else resolve();
    };
    const onEnd = () => finish(Error('Supervisor closed before assigning its job'));
    const onData = chunk => {
      if (received.length + chunk.length > maxBytes) return finish(Error('Unexpected supervisor command'));
      received = Buffer.concat([received, chunk]);
      const matches = commands.filter(command => received.length <= command.length && command.subarray(0, received.length).equals(received));
      if (!matches.length) return finish(Error('Unexpected supervisor command'));
      if (matches.some(command => command.length === received.length)) finish();
    };
    if (input.destroyed || input.readableEnded) return onEnd();
    input.on('data', onData);
    input.once('end', onEnd);
    input.once('close', onEnd);
    input.once('error', finish);
  });
}
