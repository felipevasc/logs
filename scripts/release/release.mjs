// npm run release -- X.Y.Z: aligns the version in every file, commits and creates the tag vX.Y.Z.
// Pushing the tag to GitHub builds, tests, signs and publishes the version; installed apps then offer it.
// Release notes come from docs/releases/vX.Y.Z.md (created on the first run if missing).
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';

const args = process.argv.slice(2), version = args.find(arg => !arg.startsWith('--'));
const trailers = args.flatMap((arg, i) => (arg === '--trailer' && args[i + 1] ? ['--trailer', args[i + 1]] : []));
const fail = message => { console.error(message); process.exit(1); };
const git = (...params) => execFileSync('git', params, { encoding: 'utf8' }).trim();
const parts = value => value.split('.').map(Number);
const newer = (a, b) => { const [x, y] = [parts(a), parts(b)]; for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i]; return false; };
// Keeps each file's line endings.
function rewrite(file, change) {
  const text = readFileSync(file, 'utf8'), eol = text.includes('\r\n') ? '\r\n' : '\n';
  const next = change(text.replace(/\r\n/g, '\n'));
  if (next === text.replace(/\r\n/g, '\n')) fail(`A versão não foi encontrada em ${file}.`);
  writeFileSync(file, next.replace(/\n/g, eol));
}
const json = (text, update) => { const data = JSON.parse(text); update(data); return `${JSON.stringify(data, null, 2)}\n`; };

if (!/^\d+\.\d+\.\d+$/.test(version || '')) fail('Uso: npm run release -- X.Y.Z');
const current = JSON.parse(readFileSync('package.json', 'utf8')).version;
if (!newer(version, current)) fail(`A versão ${version} precisa ser maior que a atual (${current}).`);
const notes = `docs/releases/v${version}.md`, placeholder = '<!-- escreva aqui as novidades -->';
if (!existsSync(notes)) {
  writeFileSync(notes, `# LogInsight ${version}\n\n${placeholder}\n`);
  fail(`Criei ${notes}. Escreva as novidades (elas aparecem no aviso de atualização) e rode o comando de novo.`);
}
if (readFileSync(notes, 'utf8').includes(placeholder)) fail(`Complete as notas em ${notes} antes de publicar.`);
const pending = git('status', '--porcelain').split('\n').filter(line => line && !line.endsWith(notes));
if (pending.length) fail(`Há alterações não commitadas:\n${pending.join('\n')}`);
git('fetch', '--quiet', 'origin', 'main', '--tags');
try { git('merge-base', '--is-ancestor', 'origin/main', 'HEAD'); } catch { fail('Atualize a branch com a main do GitHub antes (git pull).'); }
if (git('tag', '--list', `v${version}`)) fail(`A tag v${version} já existe.`);

rewrite('package.json', text => json(text, data => { data.version = version; }));
rewrite('package-lock.json', text => json(text, data => { data.version = version; data.packages[''].version = version; }));
rewrite('src-tauri/tauri.conf.json', text => json(text, data => { data.version = version; }));
rewrite('src-tauri/Cargo.toml', text => text.replace(/(\[package\][\s\S]*?\nversion\s*=\s*")[^"]+(")/, `$1${version}$2`));
rewrite('src-tauri/Cargo.lock', text => text.replace(/(name = "loginsight"\nversion = ")[^"]+(")/, `$1${version}$2`));
execFileSync(process.execPath, ['scripts/release/verify-version.mjs'], { stdio: 'inherit' });
git('add', 'package.json', 'package-lock.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', notes);
execFileSync('git', ['commit', '--quiet', '-m', `release: v${version}`, ...trailers], { stdio: 'inherit' });
git('tag', '-a', `v${version}`, '-m', `LogInsight v${version}`);
console.log(`\nVersão ${version} pronta. Para publicar, envie o commit e a tag:\n\n  git push --follow-tags origin HEAD:main\n\nO GitHub Actions testa, gera os instaladores, assina e publica; os aplicativos instalados passam a oferecer a atualização.`);
