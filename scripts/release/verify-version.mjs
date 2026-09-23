import { readFileSync, existsSync } from 'node:fs';

const json = file => JSON.parse(readFileSync(file, 'utf8'));
const version = json('package.json').version;
if (!/^\d+\.\d+\.\d+$/.test(version)) throw Error('Release requires a stable semantic version.');
const lock = json('package-lock.json');
const cargo = readFileSync('src-tauri/Cargo.toml', 'utf8').match(/\[package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/)[1];
const cargoLock = readFileSync('src-tauri/Cargo.lock', 'utf8').match(/name = "loginsight"\r?\nversion = "([^"]+)"/)[1];
const versions = [lock.version, lock.packages[''].version, json('src-tauri/tauri.conf.json').version, cargo, cargoLock];
if (versions.some(value => value !== version)) throw Error(`Version mismatch: ${[version, ...versions].join(', ')}`);
if (!existsSync(`docs/releases/v${version}.md`)) throw Error(`Missing release notes for v${version}.`);
console.log(`Version v${version} agrees across package, Tauri, Cargo and lockfiles.`);
