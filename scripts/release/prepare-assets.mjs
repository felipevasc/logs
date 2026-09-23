import { readdirSync, readFileSync, mkdirSync, copyFileSync, writeFileSync, statSync } from 'node:fs';
import { join, basename, resolve } from 'node:path';
import { createHash } from 'node:crypto';

const [input, output] = process.argv.slice(2);
if (!input || !output || resolve(input) === resolve(output)) throw Error('Provide separate input and output directories.');
const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
const extensions = ['.exe', '.msi', '.deb', '.rpm', '.AppImage'];
const files = [];
function visit(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) throw Error(`Unexpected symlink: ${entry.name}`);
    if (entry.isDirectory()) visit(path);
    else if (extensions.some(extension => entry.name.endsWith(extension))) files.push(path);
  }
}
visit(input);
if (files.length !== extensions.length || extensions.some(extension => files.filter(path => path.endsWith(extension)).length !== 1)) throw Error('Expected exactly one EXE, MSI, DEB, RPM and AppImage.');
const magic = { '.exe': '4d5a', '.msi': 'd0cf11e0a1b11ae1', '.deb': '213c617263683e0a', '.rpm': 'edabeedb', '.AppImage': '7f454c46' };
const assets = files.map(path => {
  const name = basename(path), bytes = readFileSync(path), extension = extensions.find(value => name.endsWith(value));
  if (!name.includes(version) || statSync(path).size < 100_000 || !bytes.subarray(0, 8).toString('hex').startsWith(magic[extension])) throw Error(`Invalid or mismatched installer: ${name}`);
  return { path, name, size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
}).sort((a, b) => a.name.localeCompare(b.name, 'en'));
mkdirSync(output, { recursive: true });
if (readdirSync(output).length) throw Error('Output directory must be empty.');
for (const asset of assets) copyFileSync(asset.path, join(output, asset.name));
writeFileSync(join(output, 'SHA256SUMS.txt'), assets.map(asset => `${asset.sha256}  ${asset.name}\n`).join(''));
console.log(JSON.stringify({ version, assets: assets.map(({ path, ...asset }) => asset) }, null, 2));
