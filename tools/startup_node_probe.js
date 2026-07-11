const fs = require('fs');
const { performance } = require('perf_hooks');

const startedAt = performance.now();
const reads = [];
const instantiations = [];
const bufferPaths = new WeakMap();
let firstStdoutMs = null;
let emitted = false;

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
  process.stderr.write(`MOLT_STARTUP_PHASES=${JSON.stringify({
    schema_version: 1,
    preload_to_exit_ms: performance.now() - startedAt,
    first_stdout_ms: firstStdoutMs,
    reads,
    instantiations,
  })}\n`);
}

process.once('beforeExit', emit);
process.once('exit', emit);
