const fs = require('fs');
const { performance } = require('perf_hooks');

const startedAt = performance.now();
const reads = [];
const instantiations = [];
const bufferPaths = new WeakMap();
const startupMarks = new Map();
let firstStdoutMs = null;
let emitted = false;

process.on('molt:wasm:startup', (name) => startupMarks.set(name, performance.now()));

const runnerPhases = {
  manifest_and_module: ['runMain:assets:enter', 'runMain:init'],
  linked_import_setup: ['runMain:init', 'linked:instantiate:enter'],
  linked_engine_compile: ['linked:instantiate:enter', 'linked:compile:completed'],
  linked_instantiate_and_abi: ['linked:compile:completed', 'linked:instantiate:completed'],
  linked_host_bootstrap: ['linked:bootstrap:enter', 'linked:bootstrap:completed'],
  // The exported entry initializes the Molt runtime and calls the Python body.
  linked_guest_entry: ['linked:guest:enter', 'linked:guest:completed'],
  host_dispose: ['runMain:dispose:enter', 'runMain:dispose:completed'],
};

const originalReadFileSync = fs.readFileSync;
fs.readFileSync = function startupReadFileSync(pathLike, ...args) {
  const start = performance.now();
  const value = originalReadFileSync.call(this, pathLike, ...args);
  const end = performance.now();
  const path = String(pathLike);
  reads.push({ path, duration_ms: end - start, end_ms: end - startedAt });
  if (Buffer.isBuffer(value)) bufferPaths.set(value, path);
  return value;
};

const originalInstantiate = WebAssembly.instantiate;
WebAssembly.instantiate = async function startupInstantiate(source, imports) {
  const start = performance.now();
  try {
    return await originalInstantiate.call(WebAssembly, source, imports);
  } finally {
    const end = performance.now();
    instantiations.push({
      source: bufferPaths.get(source) || (source instanceof WebAssembly.Module ? '<module>' : '<bytes>'),
      duration_ms: end - start,
      end_ms: end - startedAt,
    });
  }
};

const originalStdoutWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = function startupStdoutWrite(...args) {
  if (firstStdoutMs === null) firstStdoutMs = performance.now() - startedAt;
  return originalStdoutWrite(...args);
};

function emit() {
  if (emitted) return;
  emitted = true;
  const runnerPhasesMs = {};
  for (const [phase, [begin, end]] of Object.entries(runnerPhases)) {
    const start = startupMarks.get(begin);
    const finish = startupMarks.get(end);
    if (start !== undefined && finish !== undefined && finish >= start) {
      runnerPhasesMs[phase] = finish - start;
    }
  }
  process.stderr.write(`molt_startup_phases=${JSON.stringify({
    schema_version: 2,
    node_to_preload_ms: startedAt,
    preload_to_exit_ms: performance.now() - startedAt,
    first_stdout_ms: firstStdoutMs,
    reads,
    instantiations,
    runner_phases_ms: runnerPhasesMs,
  })}\n`);
}

process.once('beforeExit', emit);
process.once('exit', emit);
