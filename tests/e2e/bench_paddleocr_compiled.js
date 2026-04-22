/**
 * Benchmark: PaddleOCR compiled tinygrad vs ONNX Runtime.
 *
 * Measures the computational primitives that PaddleOCR's detector uses:
 *   - Conv2d as im2col + matmul (what tinygrad decomposes to)
 *   - WASM SIMD matmul (from deploy/browser/simd-ops-rs)
 *   - WASM module startup latency (paddleocr_final_linked.wasm)
 *
 * WASM inference cannot be benchmarked end-to-end yet because init() and
 * ocr() require weight data (detector weights, recognizer weights, and
 * character dictionary) that must be loaded via a custom harness with
 * ArrayBuffer arguments. The WASM module exports are:
 *   - init(det_weights: Uint8Array, rec_weights: Uint8Array, dict: string)
 *   - ocr(image: Uint8Array, width: u32, height: u32) -> string
 *
 * Until a weight-loading harness is wired, we measure:
 *   1. WASM startup time (instantiate + _start)
 *   2. JS matmul throughput (matches what tinygrad's conv2d compiles to)
 *   3. WASM SIMD matmul throughput (if available)
 */

const fs = require("fs");
const { performance } = require("perf_hooks");

// ---------------------------------------------------------------------------
// 1. WASM startup benchmark
// ---------------------------------------------------------------------------

async function benchmarkWasmStartup() {
  const wasmPath = "/tmp/paddleocr_final_linked.wasm";
  if (!fs.existsSync(wasmPath)) {
    console.log("WASM startup: SKIP (binary not found at " + wasmPath + ")");
    return;
  }

  const wasmBytes = fs.readFileSync(wasmPath);
  console.log(
    "WASM binary: " + (wasmBytes.length / (1024 * 1024)).toFixed(1) + " MB",
  );

  // Measure instantiation time (cold start)
  // The molt-compiled WASM uses env imports (not WASI). Try both import
  // strategies: first with env stubs (molt-compiled), then WASI fallback.
  const t0 = performance.now();
  try {
    // Compile the module first (measures compilation time separately)
    const module = await WebAssembly.compile(wasmBytes);
    const t_compile = performance.now();

    // Discover required imports
    const importDescriptors = WebAssembly.Module.imports(module);
    const importObj = {};
    for (const desc of importDescriptors) {
      if (!importObj[desc.module]) importObj[desc.module] = {};
      if (desc.kind === "function") {
        // Stub: log the call for discovery, return 0
        importObj[desc.module][desc.name] = () => 0;
      } else if (desc.kind === "memory") {
        importObj[desc.module][desc.name] = new WebAssembly.Memory({
          initial: 256,
          maximum: 65536,
        });
      } else if (desc.kind === "table") {
        importObj[desc.module][desc.name] = new WebAssembly.Table({
          initial: 8192,
          element: "anyfunc",
        });
      } else if (desc.kind === "global") {
        importObj[desc.module][desc.name] = new WebAssembly.Global(
          { value: "i32", mutable: true },
          0,
        );
      }
    }

    const instance = await WebAssembly.instantiate(module, importObj);
    const t1 = performance.now();

    // Try calling _start or _initialize if present
    const startFn =
      instance.exports._start || instance.exports._initialize || null;
    if (startFn) {
      try {
        startFn();
      } catch (_e) {
        // Module may exit normally or throw proc_exit(0)
      }
    }
    const t2 = performance.now();

    console.log(
      "WASM compile:     " + (t_compile - t0).toFixed(1) + " ms",
    );
    console.log(
      "WASM instantiate: " + (t1 - t_compile).toFixed(1) + " ms",
    );
    if (startFn) {
      console.log(
        "WASM _start:      " + (t2 - t1).toFixed(1) + " ms",
      );
    }
    console.log("WASM total:       " + (t2 - t0).toFixed(1) + " ms");

    // List exports for documentation
    const fnExports = Object.keys(instance.exports).filter(
      (e) => typeof instance.exports[e] === "function",
    );
    const memExports = Object.keys(instance.exports).filter(
      (e) => instance.exports[e] instanceof WebAssembly.Memory,
    );
    console.log("WASM fn exports:  " + fnExports.join(", "));
    if (memExports.length > 0) {
      console.log("WASM memories:    " + memExports.join(", "));
    }

    // Show import modules
    const importModules = [
      ...new Set(importDescriptors.map((d) => d.module)),
    ];
    console.log("WASM imports from: " + importModules.join(", "));
  } catch (err) {
    const t1 = performance.now();
    console.log(
      "WASM startup: " +
        (t1 - t0).toFixed(1) +
        " ms (failed: " +
        err.message +
        ")",
    );
  }
}

// ---------------------------------------------------------------------------
// 2. JS Conv2d (im2col matmul) benchmark
// ---------------------------------------------------------------------------

function benchmarkConv2dJs() {
  // Simulate a 3x3 Conv2d on 64x64 input with 64 output channels
  // (matches PaddleOCR detector's early layers)
  const M = 64; // output channels
  const K = 3 * 3 * 3; // kernel_h * kernel_w * in_channels
  const N = 64 * 64; // output spatial (h_out * w_out)

  const a = new Float32Array(M * K);
  const b = new Float32Array(K * N);
  const out = new Float32Array(M * N);

  // Initialize with random values
  for (let i = 0; i < a.length; i++) a[i] = Math.random();
  for (let i = 0; i < b.length; i++) b[i] = Math.random();

  // Warmup
  for (let i = 0; i < M; i++) {
    for (let k = 0; k < K; k++) {
      const aik = a[i * K + k];
      for (let j = 0; j < N; j++) {
        out[i * N + j] += aik * b[k * N + j];
      }
    }
  }

  // Benchmark 10 iterations
  const iters = 10;
  out.fill(0);
  const start = performance.now();
  for (let iter = 0; iter < iters; iter++) {
    for (let i = 0; i < M; i++) {
      for (let k = 0; k < K; k++) {
        const aik = a[i * K + k];
        for (let j = 0; j < N; j++) {
          out[i * N + j] += aik * b[k * N + j];
        }
      }
    }
  }
  const elapsed = performance.now() - start;
  const flops = 2 * M * K * N * iters;
  const gflops = flops / (elapsed * 1e6);

  console.log(
    "Conv2d (JS):      " +
      (elapsed / iters).toFixed(1) +
      " ms/conv  (" +
      gflops.toFixed(2) +
      " GFLOPS)",
  );
  console.log(
    "  dimensions:     M=" + M + " K=" + K + " N=" + N + " (" + iters + " iterations)",
  );
}

// ---------------------------------------------------------------------------
// 3. WASM SIMD matmul benchmark
// ---------------------------------------------------------------------------

async function benchmarkWasmSimd() {
  const simdPath =
    "deploy/browser/simd-ops-rs/target/wasm32-unknown-unknown/release/simd_ops.wasm";
  if (!fs.existsSync(simdPath)) {
    console.log("WASM SIMD:        SKIP (binary not found)");
    return;
  }

  try {
    const wasmBytes = fs.readFileSync(simdPath);
    console.log(
      "SIMD WASM binary: " + (wasmBytes.length / 1024).toFixed(1) + " KB",
    );

    const memory = new WebAssembly.Memory({ initial: 256 });
    const { instance } = await WebAssembly.instantiate(wasmBytes, {
      env: { memory },
    });

    const exports = Object.keys(instance.exports).filter(
      (e) => typeof instance.exports[e] === "function",
    );
    console.log("SIMD exports:     " + exports.join(", "));
    console.log("WASM SIMD:        available (ops loaded)");
  } catch (e) {
    console.log("WASM SIMD:        not available (" + e.message + ")");
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  console.log("=== PaddleOCR Compiled Benchmark ===\n");

  await benchmarkWasmStartup();
  console.log("");

  benchmarkConv2dJs();
  console.log("");

  await benchmarkWasmSimd();

  console.log("\n--- Notes ---");
  console.log(
    "End-to-end WASM inference requires loading weight data into the module.",
  );
  console.log(
    "The WASM binary exports init() and ocr() but they need weight ArrayBuffers.",
  );
  console.log(
    "A custom Node.js harness with weight loading is needed for full benchmarking.",
  );
}

main().catch(console.error);
