// Run with --write only when intentionally updating the native display policy.
// Default mode checks the committed oracle against the actual frontend sources.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const sandbox = { window: {}, document: { addEventListener() {} }, Intl, Date, console };
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(root, 'frontend/evidence-ui.js'), 'utf8'), sandbox);
const timeline = fs.readFileSync(path.join(root, 'frontend/case-timeline.js'), 'utf8');
const exportMarker = 'return { render, rows, image };';
assert.equal(timeline.split(exportMarker).length, 2, 'legacy collector export must remain identifiable');
vm.runInContext(timeline.replace(exportMarker, 'return { collect, noteMap };'), sandbox);
// Execute the exact checked-in predicate expressions, instead of an independent
// approximation which could conceal a later semantic change in the UI.
const normalizeSource = timeline.match(/query = (view\.query\.trim\(\)\.toLocaleLowerCase\("pt-BR"\))/)?.[1];
const matchSource = timeline.match(/texts\.some\(text => (String\(text \|\| ""\)\.toLocaleLowerCase\("pt-BR"\)\.includes\(query\))\)/)?.[1];
assert.ok(normalizeSource && matchSource, 'legacy display search expressions changed');
const normalize = new Function('view', `return ${normalizeSource};`);
const match = new Function('text', 'query', `return ${matchSource};`);
const ui = sandbox.window.EvidenceUI;

const redactionInputs = [
  ['ordinary', 'Ação concluída; fonte Σ; π'],
  ['all secret keys', 'password=a passwd=b access_token=c refresh-token=d secretAccessKey=e client_secret=f token=g secret=h authorization=i cookie=j api-key=k'],
  ['ascii case', 'PASSWORD=One pAsSwD=Two ToKeN=Three'],
  ['quoted and escaped', 'password="a\\\"b" cookie=\'c\\\'d\' token=""'],
  ['bearer and basic', 'Authorization: Bearer abc.def Cookie=Basic Zm9v=='],
  ['not a word boundary', 'prefixpassword=x and xclient-secret=y'],
  ['long s is not ascii s', 'ſecret=visible paſſword=visible'],
  ['kelvin sign is not ascii k', 'apiKey=visible'],
  ['js BOM whitespace', 'token\uFEFF=\uFEFFsecret\uFEFFtail'],
  ['NEL is not js whitespace', 'token=secret\u0085tail'],
  ['MVS is not js whitespace', 'token=secret\u180Etail'],
  ['line separator value boundary', 'token=secret\u2028tail'],
  ['escaped line separator is not dot', 'password="foo\\\u2028bar"'],
  ['escaped newline is not dot', 'password="foo\\\nbar"'],
  ['unescaped quoted newline', 'password="foo\nbar"'],
  ['hash twenty chars', '$6$abcdefghijklmnopqrst tail'],
  ['hash nineteen chars', '$6$abcdefghijklmnopqrs tail'],
  ['bcrypt and allowed punctuation', '$2b$abcABC123./$=,-abcABC123 trailing'],
  ['complete PEM', 'before -----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY----- after'],
  ['truncated PEM', 'before -----BEGIN PRIVATE KEY-----\nabc\n'],
  ['mismatched PEM type', '-----BEGIN EC PRIVATE KEY-----abc-----END OPENSSH PRIVATE KEY----- tail'],
  ['environment whole line', 'DB_PASSWORD=one two three\nDATABASE_PASSWORD=four five\r\nAWS_SECRET_ACCESS_KEY=six seven'],
  ['environment case sensitivity', 'db_password=one two\nDB_PASSWORD=three four'],
  ['environment crosses whitespace newline', 'DB_PASSWORD=\n secret next\nremaining'],
  ['multiple passes', 'token=$6$abcdefghijklmnopqrst end'],
  ['empty secret', 'password=; token=, cookie=""'],
];
const redaction = redactionInputs.map(([name, input]) => ({ name, input, expected: ui.redact(input) }));

const searchInputs = [
  ['Portuguese accents', '  AÇÃO  ', ['Uma ação registrada']],
  ['accents remain distinct', 'acao', ['ação']],
  ['final sigma', 'ΟΣ', ['ΟΣ']],
  ['sigma stays contextual', 'οσ', ['ΟΣ']],
  ['isolated sigma', 'Σ', ['σ']],
  ['sigma before cased letter', 'ΟΣΑ', ['οσα']],
  ['sigma combining case ignorable', 'AΣ\u0301', ['aς\u0301']],
  ['sigma combining followed by cased', 'AΣ\u0345B', ['aσ\u0345b']],
  ['sigma apostrophe', 'AΣ’', ['aς’']],
  ['sigma underscore stops context', 'AΣ_B', ['aς_b']],
  ['sigma joiner case ignorable', 'AΣ\u200D', ['aς\u200D']],
  ['Latin capital dotted I', 'İ', ['i\u0307']],
  ['BOM is trimmed', '\uFEFFquery\uFEFF', ['query']],
  ['NEL is preserved', '\u0085query\u0085', ['query']],
  ['MVS is preserved', '\u180Equery\u180E', ['query']],
  ['no cross field match', 'abc', ['ab', 'c']],
  ['fields are not trimmed', 'a b', ['a  b']],
  ['replacement can match', '[OCULTO]', [ui.redact('token=secret')]],
  ['hidden secret cannot match', 'secret', [ui.redact('token=secret')]],
  ['empty query', ' \uFEFF\u2028', ['']],
  ['astral case pair', '\u{10400}', ['\u{10428}']],
];
for (const point of [0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0xa0, 0x1680, ...Array.from({length:11},(_,i)=>0x2000+i), 0x2028, 0x2029, 0x202f, 0x205f, 0x3000, 0xfeff]) {
  const space = String.fromCodePoint(point);
  searchInputs.push([`trim U+${point.toString(16).toUpperCase()}`, `${space}x${space}`, ['x']]);
  redaction.push({name:`regexp whitespace U+${point.toString(16).toUpperCase()}`,input:`token${space}=${space}value${space}tail`,expected:ui.redact(`token${space}=${space}value${space}tail`)});
}
const search = searchInputs.map(([name, query, fields]) => {
  const normalized = normalize({query});
  return { name, query, fields, normalized, expected: !normalized || fields.some(field => match(field, normalized)) };
});
const contextInputs = [
  { name:'ordered levels and duplicate matches', eventRef:'e', message:'body', description:'fallback', detections:[
    {event_refs:['e'],evidence_level:5,name:'primeiro'},
    {event_refs:['other'],evidence_level:3,name:'ignorado'},
    ...[0,1,2,3,4,5,6,1.5,'5',null].map(evidence_level=>({event_refs:['e','e'],evidence_level,name:'salvo'})),
  ]},
  { name:'description fallback and secret suffix', eventRef:'e', message:'', description:'description', detections:[{event_refs:['e'],evidence_level:2,name:'token=secret'}]},
  { name:'redact after joining', eventRef:'e', message:'token=', description:'ignored', detections:[{event_refs:['e'],evidence_level:3,name:'context'}]},
  { name:'missing references and name', eventRef:'e', message:'', description:'', detections:[{}, {event_refs:null}, {event_refs:['e']}, {event_refs:['e'],name:null}]},
  { name:'empty reference stays present', eventRef:'', message:'', description:'', detections:[{event_refs:[''],name:'empty'}]},
  { name:'missing reference is undefined', message:'', description:'', detections:[{event_refs:['',null,'undefined'],name:'array does not match'}, {event_refs:'contains undefined',name:'string includes matches'}]},
  { name:'string includes receiver', eventRef:'e', message:'', description:'', detections:[{event_refs:'prefix-e-suffix',name:'string reference'}]},
];
const context = contextInputs.map(value => {
  const expected = ui.eventContext({detection:{detections:value.detections}}, {event_ref:value.eventRef});
  return {...value, expected, detail:ui.redact([value.message || value.description || '', expected].filter(Boolean).join('\n'))};
});

// Exercise the actual collector, preserving its type-dependent text policy.
const authored = {
  items:[{id:'i',label:'token=source-secret',rows:[
    {id:1,event_ref:'1',timestamp:1,name:'token=event-secret',message:'password=event-detail'},
    {id:2,event_ref:'2',timestamp:2,name:'second',message:'other'},
    {id:3,event_ref:'3',timestamp:3,name:'single',message:'single body'},
  ]}],
  timeline:{compact:false,groups:[{id:'g',name:'token=group-secret',ids:['e:i:1','e:i:2']}],annotations:[{id:'n',anchor:'g',text:'token=note-secret'}]},
  manual:[{id:'m',start:4,name:'token=manual-secret',description:'password=manual-detail'}],
};
const collected = sandbox.window.CaseTimeline.collect(authored);
const notes = sandbox.window.CaseTimeline.noteMap(collected.config, collected.entries);
const presentation = JSON.parse(JSON.stringify(collected.entries.map(entry=>({id:entry.id,type:entry.type,title:entry.title,detail:entry.detail,source:entry.source,notes:(notes.get(entry.id)||[]).map(note=>note.text)}))));
assert.equal(presentation[0].title, 'token=group-secret');
assert.equal(presentation[0].source, 'token="[oculto]"');
assert.deepEqual(presentation[0].notes, ['token=note-secret']);
assert.equal(presentation.at(-1).title, 'token=manual-secret');
assert.equal(presentation.at(-1).detail, 'password=manual-detail');
const output = { schemaVersion:1, redaction, search, context, presentation };
const destination = path.join(root, 'scripts/tests/fixtures/native-display-policy.json');
if (process.argv.includes('--write')) fs.writeFileSync(destination, `${JSON.stringify(output,null,2)}\n`);
else assert.deepEqual(output, JSON.parse(fs.readFileSync(destination,'utf8')));
console.log(`Native display policy oracle: ${redaction.length} redaction, ${search.length} search, ${context.length} context, ${presentation.length} presentation fixtures agree with frontend (Node ${process.version}, Unicode ${process.versions.unicode})`);
