// Update manifest (latest.json) read by the app's updater, and the checks it must pass before publication.
// Signatures follow the minisign format written by `tauri build` (see src-tauri/updater/README.md).
import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

// One entry per installer type: each installation updates only with its own format.
export const PACKAGES = [
  { key: 'windows-x86_64-nsis', match: name => name.endsWith('-setup.exe') },
  { key: 'windows-x86_64-msi', match: name => name.endsWith('.msi') },
  { key: 'linux-x86_64-appimage', match: name => name.endsWith('.AppImage') },
  { key: 'linux-x86_64-deb', match: name => name.endsWith('.deb') },
  { key: 'linux-x86_64-rpm', match: name => name.endsWith('.rpm') },
];
const ALL = PACKAGES.map(item => item.key);
const ED25519_SPKI = Buffer.from('302a300506032b6570032100', 'hex');
const decoded = value => Buffer.from(String(value).trim(), 'base64').toString('utf8').split(/\r?\n/);

export function updaterPubkey() {
  return process.env.LOGINSIGHT_UPDATER_PUBKEY || JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8')).plugins.updater.pubkey;
}

export function publicKey(encoded) {
  const raw = Buffer.from(decoded(encoded)[1] || '', 'base64');
  if (raw.length !== 42 || raw.subarray(0, 2).toString() !== 'Ed') throw Error('Invalid updater public key.');
  return { id: raw.subarray(2, 10), key: createPublicKey({ key: Buffer.concat([ED25519_SPKI, raw.subarray(10)]), format: 'der', type: 'spki' }) };
}

// Checks the file signature and the signed trusted comment; returns its fields (timestamp, file, version).
export function verifySignature(data, signature, key) {
  const lines = decoded(signature);
  const raw = Buffer.from(lines[1] || '', 'base64'), global = Buffer.from(lines[3] || '', 'base64');
  const trusted = lines[2]?.startsWith('trusted comment: ') ? lines[2].slice('trusted comment: '.length) : null;
  if (raw.length !== 74 || global.length !== 64 || trusted == null) throw Error('Malformed signature.');
  if (!raw.subarray(2, 10).equals(key.id)) throw Error('Signed with a different key.');
  const algorithm = raw.subarray(0, 2).toString();
  const message = algorithm === 'ED' ? createHash('blake2b512').update(data).digest() : algorithm === 'Ed' ? data : null;
  const signed = raw.subarray(10);
  if (!message || !verify(null, message, key.key, signed)) throw Error('The signature does not match the file.');
  if (!verify(null, Buffer.concat([signed, Buffer.from(trusted, 'utf8')]), key.key, global)) throw Error('The trusted comment is not signed.');
  return Object.fromEntries(trusted.split('\t').map(field => [field.slice(0, field.indexOf(':')), field.slice(field.indexOf(':') + 1)]));
}

export function buildManifest({ version, notes, pubDate, baseUrl, files, keys = ALL }) {
  const platforms = {};
  for (const { key, match } of PACKAGES.filter(item => keys.includes(item.key))) {
    const found = files.filter(file => match(file.name));
    if (found.length !== 1) throw Error(`Expected exactly one package for ${key}, found ${found.length}.`);
    platforms[key] = { signature: found[0].signature.trim(), url: baseUrl + encodeURIComponent(found[0].name) };
  }
  return { version, notes, pub_date: pubDate, platforms };
}

// Everything the installed apps will check, checked here first: nothing is published that they would reject.
export function verifyManifest(manifest, directory, { version, baseUrl, pubkey, keys = ALL }) {
  const key = publicKey(pubkey);
  if (manifest.version !== version) throw Error(`Manifest announces ${manifest.version}, expected ${version}.`);
  if (Number.isNaN(Date.parse(manifest.pub_date))) throw Error('Manifest date is invalid.');
  const found = Object.keys(manifest.platforms || {}).sort();
  if (found.join() !== [...keys].sort().join()) throw Error(`Unexpected platform keys: ${found.join(', ')}.`);
  for (const [platform, { url, signature }] of Object.entries(manifest.platforms)) {
    if (!url.startsWith(baseUrl)) throw Error(`${platform}: URL outside ${baseUrl}.`);
    const name = decodeURIComponent(url.slice(baseUrl.length));
    if (/[\\/]/.test(name) || !PACKAGES.find(item => item.key === platform).match(name)) throw Error(`${platform}: unexpected package ${name}.`);
    const fields = verifySignature(readFileSync(join(directory, name)), signature, key);
    if (fields.version !== version) throw Error(`${name}: signed for version ${fields.version}, expected ${version}.`);
    if (fields.file !== name) throw Error(`${name}: the signature names ${fields.file}.`);
  }
  return manifest;
}
