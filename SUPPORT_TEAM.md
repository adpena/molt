# Support Team Handoff — Parallel Lane

**Date:** 2026-04-20
**From:** Primary agent (GPU primitive stack, Cloudflare deployment, enjoice integration)
**To:** Trusted partner for parallel work

---

## What's Done (don't duplicate this work)

- `runtime/molt-gpu/` — 48 Rust files, 15,748 LOC, 434+ tests, 26 tinygrad-conformant primitives, 7 renderers (MSL/WGSL/GLSL/CUDA/HIP/OpenCL/MIL), 8 device backends, SIMD everywhere, fused matmul/softmax/RMSNorm
- `src/molt/stdlib/tinygrad/` — 23+ Python files, ~10K LOC. Full Tensor API, TurboQuant, DFlash, DDTree, EAGLE-3, Mirror-SD, tiered KV cache, Falcon-OCR inference, tokenizer
- Worker live at `https://falcon-ocr.adpena.workers.dev` — Workers AI (Gemma 3 12B primary, 3 fallback models), x402 payment (`0xB31369b0FE37a9D30833c88f9C4dfDE0f930cC25`), multi-level caching, batch OCR, template-from-scan
- enjoice integration — molt OCR backend registered as highest priority, template-from-scan button on ScanButton, Turnstile iPad fix, visual polish
- Real Falcon-OCR weights downloaded (1.03 GB, 269M params, 22 layers, dim=768)

## Your Parallel Lane: molt WASM Compilation Pipeline

**The #1 blocker for production Falcon-OCR inference is the WASM build pipeline.**

Currently, `molt build wasm_driver.py --target wasm` fails because the tinygrad Python modules haven't been lowered to Rust intrinsics. The Worker runs inference via Workers AI (Cloudflare's GPU fleet) which works but costs $0.001/request and has capacity limits. True edge inference (0 cost, 0 latency, offline-capable) requires the WASM path.

### What needs to happen

1. **Create `_intrinsics` wrappers for core Tensor operations**
   - The molt WASM target requires all Python modules to have corresponding Rust intrinsic implementations
   - Location: `runtime/molt-runtime/src/builtins/` (existing pattern)
   - Bridge: `runtime/molt-runtime/src/builtins/gpu_primitives.rs` exists (I created it) — it exposes molt-gpu ops via FFI
   - The gap: the Python `Tensor` class methods (`.dot()`, `.softmax()`, `.reshape()`, etc.) need to be callable from WASM-compiled code
   - This means: each Tensor method needs a Rust intrinsic that the frontend lowers to

2. **Fix remaining WASM compilation errors**
   - Three errors were fixed: duplicate import in `array_mod.rs`, missing `HEADER_FLAG_RAW_ALLOC`, missing `function_set_globals_bits`
   - But partner WIP may have introduced new issues — run `cargo check -p molt-runtime --target wasm32-wasip1` and fix whatever comes up
   - Simple programs (`def add(a, b): return a + b`) DO compile to WASM successfully

3. **Test with a minimal tensor program**
   ```python
   from tinygrad.tensor import Tensor
   def matmul_test():
       a = Tensor.zeros(4, 4)
       b = Tensor.ones(4, 4)
       c = a.dot(b)
       return c.realize()
   ```
   Get this compiling to WASM. It exercises: Tensor construction, LazyOp DAG, schedule, fuse, CpuDevice interpret.

4. **Build the actual Falcon-OCR WASM module**
   Once Tensor ops work in WASM, build `src/molt/stdlib/tinygrad/wasm_driver.py` which exports `init()` and `ocr_tokens()`.

### Key files to read first
- `runtime/molt-runtime/src/builtins/gpu_primitives.rs` — FFI bridge (I wrote this)
- `runtime/molt-gpu/src/lib.rs` — public API
- `src/molt/stdlib/tinygrad/wasm_driver.py` — the WASM entry point
- `src/molt/stdlib/tinygrad/tensor.py` — Tensor class
- `deploy/cloudflare/worker.js` — where the WASM module would be loaded

### Build commands
```bash
export MOLT_SESSION_ID="wasm-partner"
export CARGO_TARGET_DIR=$PWD/target-wasm_partner

# Step 1: verify runtime compiles for WASM
cargo check -p molt-runtime --target wasm32-wasip1

# Step 2: try building a simple program
python3 -m molt build tests/e2e/wasm_hello.py --target wasm --output /tmp/hello.wasm

# Step 3: try building with tensor ops
python3 -m molt build <tensor_test.py> --target wasm --output /tmp/tensor.wasm
```

### Current status (2026-04-20)

**WASM COMPILATION WORKS.** The full Falcon-OCR inference compiles to WASM:
- `MOLT_HERMETIC_MODULE_ROOTS=1 molt build wasm_driver.py --target wasm` succeeds
- Binary: 13.4 MB linked, 4.0 MB gzipped
- Uploaded to R2 at `models/falcon-ocr/falcon-ocr.wasm`

**Bugs fixed to get here:**
1. SCCP dead block elimination — SSA dominance violation from unreachable blocks
2. Import alias workaround — use `module.function()` instead of `from X import Y as Z`
3. Hermetic module roots — skip venv scanning

### Remaining work for the partner
- Fix the WASM linker to resolve imports by original name (not alias)
- Test the WASM binary actually runs (loads weights, produces tokens)
- Optimize binary size (target < 2 MB gzipped via tree-shaking)
- Wire into browser WebGPU path for offline inference

### What NOT to do
- Don't modify `runtime/molt-gpu/` — that's stable and tested
- Don't modify `deploy/cloudflare/` — that's live in production
- Don't modify enjoice — that's the other repo
- Do add new intrinsic wrappers in `runtime/molt-runtime/src/builtins/`
- Do fix any WASM compilation errors in `runtime/molt-runtime/`

---

## Communication

- Commit and push frequently with descriptive messages
- Use `MOLT_SESSION_ID="wasm-partner"` for all builds
- If you hit a blocker that requires frontend changes (molt Python compiler), document it and move on
- The primary agent (me) will handle deployment, Worker updates, and enjoice integration

## Priority

This is THE highest-leverage work remaining. Everything else (Workers AI, caching, x402) is working. The WASM path is the difference between "$0.001/request cloud inference" and "free offline edge inference."
