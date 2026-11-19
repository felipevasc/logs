import { win32 } from 'node:path';
import { pathToFileURL } from 'node:url';

// Cargo prints the path it executed. Match only direct test outputs, including
// the newer layout in which the unit directory and executable share a hash.
export function isCargoTestBinary(root, executable, unit = false) {
  const relative = win32.relative(root, executable);
  const match = /^(?:deps\\([^\\/:]+\.exe)|build\\loginsight\\([a-f0-9]+)\\out\\([^\\/:]+\.exe))$/i.exec(relative);
  if (!match) return false;
  const name = match[1] || match[3];
  if (match[2] && !name.toLowerCase().endsWith('-' + match[2].toLowerCase() + '.exe')) return false;
  return !unit || /^loginsight_lib-[\w-]+\.exe$/i.test(name);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , root, executable, mode] = process.argv;
  process.exitCode = root && executable && isCargoTestBinary(root, executable, mode === 'unit') ? 0 : 1;
}
