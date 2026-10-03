// `tauri build` signing the updater artifacts with the key committed in src-tauri/updater.
// The key is intentionally not secret (see src-tauri/updater/README.md), so any clone can build a release.
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const env = { ...process.env };
env.TAURI_SIGNING_PRIVATE_KEY ||= readFileSync(resolve(root, 'src-tauri/updater/signing.key'), 'utf8').trim();
env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= '';
const cli = resolve(root, 'node_modules/@tauri-apps/cli/tauri.js');
const { status, error } = spawnSync(process.execPath, [cli, 'build', ...process.argv.slice(2)], { cwd: root, env, stdio: 'inherit' });
if (error) throw error;
process.exit(status ?? 1);
