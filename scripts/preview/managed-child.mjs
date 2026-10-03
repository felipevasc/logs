// The Windows supervisor assigns this process to its private Job Object before
// releasing stdin. No test code (and therefore no browser) runs before then.
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const input = createInterface({ input: process.stdin });
const command = await new Promise((resolve, reject) => {
  input.once('line', resolve);
  input.once('close', () => reject(new Error('Supervisor closed before assigning its job')));
});
input.close();
process.stdin.destroy();
if (command !== 'run') throw Error('Unexpected supervisor command');
process.argv.splice(1, 1);
await import(pathToFileURL(resolve(process.argv[1])).href);
