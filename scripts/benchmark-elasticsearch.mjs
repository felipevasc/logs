// Synthetic local benchmark matching src-tauri/src/big_data_tests.rs.
// Run only against the disposable, loopback Elasticsearch benchmark container:
// docker run --detach --rm --name loginsight-big-data-bench --memory 2g
//   -p 127.0.0.1:19200:9200 -e discovery.type=single-node
//   -e xpack.security.enabled=false -e "ES_JAVA_OPTS=-Xms1g -Xmx1g"
//   docker.elastic.co/elasticsearch/elasticsearch:9.5.4
// node scripts/benchmark-elasticsearch.mjs --events=100000,1000000
// Or ingest with --prepare-only, then run --queries-only after other work stops.
// Finish with: docker stop loginsight-big-data-bench
// This measures HTTP _count (no returned row IDs), not the explorer UI API.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';

const args = Object.fromEntries(process.argv.slice(2).map((arg) => {
  if (arg === '--prepare-only' || arg === '--queries-only') return [arg.slice(2), true];
  assert.ok(arg.startsWith('--') && arg.includes('='), `Expected --key=value: ${arg}`);
  const at = arg.indexOf('=');
  return [arg.slice(2, at), arg.slice(at + 1)];
}));
for (const key of Object.keys(args)) assert.ok(['events', 'url', 'output', 'trials', 'prepare-only', 'queries-only', 'ingest-label'].includes(key), `Unknown option: ${key}`);
const prepareOnly = args['prepare-only'] === true;
const queriesOnly = args['queries-only'] === true;
assert.ok(!(prepareOnly && queriesOnly), 'Choose one benchmark mode.');
const base = new URL(args.url ?? 'http://127.0.0.1:19200');
assert.equal(base.protocol, 'http:');
assert.equal(base.hostname, '127.0.0.1', 'Benchmark is restricted to loopback.');
assert.equal(base.port, '19200', 'Benchmark is restricted to the disposable container port.');
assert.equal(base.pathname, '/');
const counts = (args.events ?? '100000,1000000').split(',').map(Number);
for (const count of counts) assert.ok(Number.isSafeInteger(count) && count > 200 && count <= 100_000_000, `Invalid event count: ${count}`);
const trials = Number(args.trials ?? 5);
assert.ok(Number.isSafeInteger(trials) && trials >= 3 && trials <= 100);
const outputDirectory = path.resolve(args.output ?? 'output/big-data');
const batchSize = 5000;
const timestampBase = 1_700_000_000_000;

async function request(endpoint, options = {}) {
  const response = await fetch(new URL(endpoint, base), {
    ...options,
    headers: { 'content-type': 'application/json', ...options.headers },
    signal: AbortSignal.timeout(180_000),
  });
  const text = await response.text();
  let data;
  try { data = JSON.parse(text); } catch { throw new Error(`Invalid JSON from ${endpoint}: ${text.slice(0, 1000)}`); }
  if (!response.ok) throw new Error(`HTTP ${response.status} ${endpoint}: ${JSON.stringify(data)}`);
  return data;
}

function dockerMetadata(command) {
  try { return JSON.parse(execFileSync('docker', command, { encoding: 'utf8', timeout: 30_000 }).trim()); }
  catch (error) { return { unavailable: error.message }; }
}

function event(i) {
  // serde_json::Value uses lexicographically sorted object keys in this project.
  return {
    bytes: i % 10_000,
    code: i % 100 === 0 ? '500' : '200',
    level: i % 100 === 0 ? 'error' : 'info',
    message: i % 10_000 === 0 ? 'rare-marker-7429 authentication failed' : 'worker completed normal request',
    request_id: `req-${String(i).padStart(9, '0')}`,
    source: `service-${i % 1000}`,
    timestamp: timestampBase + i,
    user: `user-${i % 5000}`,
  };
}

function residueCount(count, residue, modulus) {
  return count <= residue ? 0 : Math.floor((count - 1 - residue) / modulus) + 1;
}

function queries(count) {
  const rare = residueCount(count, 0, 10_000);
  return [
    ['rare_substring', { wildcard: { message: { value: '*rare-marker-7429*', case_insensitive: true } } }, rare],
    ['absent_substring', { wildcard: { message: { value: '*absent-marker-963*', case_insensitive: true } } }, 0],
    ['exact_field', { term: { source: 'service-742' } }, residueCount(count, 742, 1000)],
    ['query_language', { bool: { filter: [
      { term: { user: 'user-742' } },
      { wildcard: { message: { value: '*worker*', case_insensitive: true } } },
    ] } }, residueCount(count, 742, 5000)],
    // Lucene regexp matches the whole field; this synthetic message starts and
    // ends with the same literals as the Rust substring regex, so counts agree.
    ['literal_regex', { regexp: { message: { value: 'rare-marker-7429.*failed' } } }, rare],
    ['time_window', { range: { timestamp: {
      gte: timestampBase + Math.floor(count / 2),
      lte: timestampBase + Math.floor(count / 2) + 100,
      format: 'epoch_millis',
    } } }, 101],
    ['broad_substring', { wildcard: { message: { value: '*worker*', case_insensitive: true } } }, count - rare],
  ];
}

const startedAt = new Date().toISOString();
const server = await request('/');
assert.equal(server.version.number, '9.5.4', 'Use the verified stable image documented for this benchmark.');
await request('/_cluster/health?wait_for_status=yellow&timeout=120s');
let report = {
  started_at: startedAt,
  elasticsearch_version: server.version.number,
  lucene_version: server.version.lucene_version,
  server,
  image: 'docker.elastic.co/elasticsearch/elasticsearch:9.5.4',
  official_version_source: 'https://www.elastic.co/downloads/elasticsearch',
  environment: {
    hostname: os.hostname(), platform: os.platform(), arch: os.arch(), node_version: process.version,
    cpu: os.cpus()[0]?.model, host_logical_cpus: os.cpus().length, host_total_memory_bytes: os.totalmem(),
    docker: dockerMetadata(['info', '--format', '{"server_version":{{json .ServerVersion}},"logical_cpus":{{json .NCPU}},"memory_bytes":{{json .MemTotal}},"os_type":{{json .OSType}},"kernel":{{json .KernelVersion}},"storage_driver":{{json .Driver}}}']),
    container: dockerMetadata(['inspect', 'loginsight-big-data-bench', '--format', '{"name":{{json .Name}},"image":{{json .Config.Image}},"memory_bytes":{{json .HostConfig.Memory}},"nano_cpus":{{json .HostConfig.NanoCpus}},"port_bindings":{{json .HostConfig.PortBindings}}}']),
  },
  methodology: {
    dataset: 'src-tauri/src/big_data_tests.rs::benchmark_big_data',
    generated_object_keys: 'lexicographic order matching serde_json default map serialization',
    endpoint: `${base.origin}/<index>/_count`,
    trials, percentile: 'nearest rank; with five samples P95 is the maximum',
    batch_size: batchSize,
    bulk_workers: 1,
    caches: 'Elasticsearch query and request caches disabled; OS and engine pages may warm between trials',
    semantics: 'Synthetic equivalents only; message contains all substring targets in this dataset. _all in LogInsight also searches parsed fields/raw.',
    comparison_limit: 'HTTP _count returns only a count; Rust indexed_matches returns every matching row ID and verifies original predicates. Neither includes UI rendering, sorted pages or facets.',
    regex_limit: 'Literal regex uses the common supported subset with equal counts on this dataset. Lucene and Rust regex dialects are not generally equivalent.',
    storage_limit: 'ES primary store bytes include _source, wildcard and mapped fields but exclude translog; schemas differ from embedded index.',
  },
  datasets: [],
};
await mkdir(outputDirectory, { recursive: true });
if (queriesOnly) {
  const prepared = JSON.parse(await readFile(path.join(outputDirectory, 'elasticsearch.json'), 'utf8'));
  assert.equal(prepared.elasticsearch_version, server.version.number);
  assert.equal(prepared.server.name, server.name, 'Prepared data must belong to this same disposable container.');
  prepared.query_started_at = startedAt;
  prepared.methodology.trials = trials;
  prepared.query_environment = report.environment;
  report = prepared;
}
const reportName = `elasticsearch-${report.started_at.replace(/[:.]/g, '-')}.json`;
async function saveReport() {
  const text = `${JSON.stringify(report, null, 2)}\n`;
  await writeFile(path.join(outputDirectory, reportName), text);
  await writeFile(path.join(outputDirectory, 'elasticsearch.json'), text);
}

try {
  for (const count of counts) {
    const index = `loginsight-bench-${count}`;
    let dataset;
    if (queriesOnly) {
      dataset = report.datasets.find((item) => item.events === count && item.index === index);
      assert.ok(dataset, `Missing prepared dataset: ${count}`);
      assert.equal((await request(`/${index}/_count`)).count, count);
      dataset.stats_at_ingest_complete ??= dataset.stats_at_query_start;
      dataset.queries = [];
    } else {
    const existing = await fetch(new URL(`/${index}`, base), { method: 'HEAD', signal: AbortSignal.timeout(30_000) });
    if (existing.ok) await request(`/${index}`, { method: 'DELETE' });
    else assert.equal(existing.status, 404);
    await request(`/${index}`, { method: 'PUT', body: JSON.stringify({
      settings: {
        number_of_shards: 1, number_of_replicas: 0, refresh_interval: '-1',
        'queries.cache.enabled': false, 'requests.cache.enable': false,
      },
      mappings: {
        dynamic: false,
        properties: {
          message: { type: 'wildcard' }, source: { type: 'keyword' },
          user: { type: 'keyword' }, code: { type: 'keyword' }, level: { type: 'keyword' },
          timestamp: { type: 'date', format: 'epoch_millis' },
          request_id: { type: 'keyword' }, bytes: { type: 'long' },
        },
      },
    }) });
    const ingestStarted = performance.now();
    let sourceBytes = 0;
    let generatedMs = 0;
    let bulkHttpMs = 0;
    let lastLog = 0;
    for (let offset = 0; offset < count; offset += batchSize) {
      const generationStart = performance.now();
      const lines = [];
      const end = Math.min(count, offset + batchSize);
      for (let i = offset; i < end; i++) {
        const serialized = JSON.stringify(event(i));
        sourceBytes += Buffer.byteLength(serialized) + 1;
        lines.push('{"index":{}}', serialized);
      }
      const body = `${lines.join('\n')}\n`;
      generatedMs += performance.now() - generationStart;
      const bulkStarted = performance.now();
      const bulk = await request(`/${index}/_bulk?filter_path=errors,items.*.error`, {
        method: 'POST', headers: { 'content-type': 'application/x-ndjson' }, body,
      });
      bulkHttpMs += performance.now() - bulkStarted;
      if (bulk.errors) throw new Error(`Bulk failed: ${JSON.stringify(bulk).slice(0, 5000)}`);
      if (end === count || performance.now() - lastLog >= 15_000) {
        console.log(`ELASTICSEARCH_INGEST ${JSON.stringify({ events: count, indexed: end, elapsed_ms: performance.now() - ingestStarted })}`);
        lastLog = performance.now();
      }
    }
    const refreshStarted = performance.now();
    await request(`/${index}/_refresh`, { method: 'POST' });
    const refreshMs = performance.now() - refreshStarted;
    const ingestMs = performance.now() - ingestStarted;
    const flushStarted = performance.now();
    await request(`/${index}/_flush?wait_if_ongoing=true`, { method: 'POST' });
    const flushMs = performance.now() - flushStarted;
    const stats = await request(`/${index}/_stats/store,docs,segments,merge`);
    const primary = stats.indices[index].primaries;
    assert.equal(primary.docs.count, count);
    dataset = {
      events: count, source_bytes: sourceBytes, index, index_bytes: primary.store.size_in_bytes,
      ingest_ms: ingestMs, generated_ms: generatedMs, bulk_http_ms: bulkHttpMs,
      refresh_ms: refreshMs, flush_ms: flushMs,
      indexing_docs_per_second: count * 1000 / ingestMs,
      stats_at_ingest_complete: primary,
      ingest_label: args['ingest-label'] ?? 'No concurrent workload declared',
      queries: [],
    };
    report.datasets.push(dataset);
    console.log(`ELASTICSEARCH_BUILD ${JSON.stringify({ events: count, source_bytes: sourceBytes, ingest_ms: ingestMs, index_bytes: dataset.index_bytes })}`);
    await saveReport();
    }
    if (prepareOnly) continue;
    dataset.stats_at_query_start = (await request(`/${index}/_stats/store,docs,segments,merge`)).indices[index].primaries;
    dataset.index_bytes_at_query_start = dataset.stats_at_query_start.store.size_in_bytes;
    for (const [name, query, expected] of queries(count)) {
      const times = [];
      for (let trial = 0; trial < trials; trial++) {
        const started = performance.now();
        const result = await request(`/${index}/_count`, { method: 'POST', body: JSON.stringify({ query }) });
        const elapsed = performance.now() - started;
        assert.equal(result._shards.failed, 0);
        assert.equal(result.count, expected, `${count} ${name} count parity`);
        times.push(elapsed);
      }
      const ordered = [...times].sort((a, b) => a - b);
      const result = {
        events: count, query: name, matches: expected, median_ms: ordered[Math.floor(ordered.length / 2)],
        p95_ms: ordered[Math.ceil(ordered.length * 0.95) - 1], samples_ms: times,
        elasticsearch_query: query, parity: true,
      };
      dataset.queries.push(result);
      console.log(`ELASTICSEARCH_QUERY ${JSON.stringify(result)}`);
      await saveReport();
    }
    dataset.stats_after_queries = (await request(`/${index}/_stats/store,docs,segments,merge,query_cache,request_cache`)).indices[index].primaries;
    await saveReport();
    // Retain the measured stats in the report; free storage before the next size.
    await request(`/${index}`, { method: 'DELETE' });
  }
  report[prepareOnly ? 'prepared_at' : 'finished_at'] = new Date().toISOString();
  await saveReport();
  console.log(`ELASTICSEARCH_REPORT ${path.join(outputDirectory, reportName)}`);
} catch (error) {
  report.failed_at = new Date().toISOString();
  report.error = error.stack;
  await saveReport();
  throw error;
}
