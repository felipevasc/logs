// The Windows supervisor assigns this process to its private Job Object before
// releasing stdin. No test code (and therefore no browser) runs before then.
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { waitForSupervisorRun } from './supervisor-handshake.mjs';

try { await waitForSupervisorRun(process.stdin); }
finally { process.stdin.destroy(); }
process.argv.splice(1, 1);
await import(pathToFileURL(resolve(process.argv[1])).href);
