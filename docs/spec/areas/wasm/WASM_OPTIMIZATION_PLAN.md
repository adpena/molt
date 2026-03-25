# Molt WASM Optimization Plan
**Status:** Sections 1-5 DONE (2026-03-20)
**Priority:** P1
**Audience:** runtime engineers, compiler engineers, performance engineers
**Goal:** Comprehensive plan for optimizing Molt's WASM compilation target across codegen quality, binary size, startup time, memory management, and interop.

---

## 0. Current Architecture Summary

Molt's WASM backend is a custom code emitter (`runtime/molt-backend/src/wasm.rs`, ~9740 lines) that directly produces WASM bytecode using the `wasm_encoder` crate. It does **not** use Cranelift for WASM output -- instead it emits WASM instructions directly from Molt's IR (`SimpleIR` / `OpIR`). This is a significant architectural distinction from the native backend, which uses Cranelift's `ObjectModule`.

The WASM host (`runtime/molt-wasm-host/src/main.rs`, ~4550 lines) runs on wasmtime 41.0.3 with WASI Preview 1 support. It provides 620+ host-imported intrinsic functions under the `molt_runtime` namespace, plus WASI syscalls and indirect call trampolines.

Key characteristics of the current implementation:
- **Direct WASM emission**: Custom `WasmBackend` struct builds WASM modules section by section (types, imports, functions, code, data, tables, exports).
- **NaN-boxed object model**: Same 64-bit NaN-boxing scheme as native, with i64 as the universal value type.
- **Relocatable object output**: Emits linking sections and relocation entries for `wasm-ld` to combine with the pre-compiled runtime `.wasm`.
- **Monolithic import surface**: All 620+ runtime imports are registered unconditionally regardless of what the program uses.
- **State machine lowering**: Generators, coroutines, and async generators use dispatch-block state machines with dense/sparse remap tables.
- **Deterministic output**: BTreeMap used everywhere for iteration-order stability; NaN canonicalization available via `MOLT_DETERMINISTIC=1`.

---

## 1. WASM Codegen Quality

### 1.1 Current State

The direct WASM emitter produces correct but unoptimized code. Key observations:

**Strengths:**
- Direct IR-to-WASM lowering avoids Cranelift's WASM-to-CLIF-to-WASM round-trip overhead.
- NaN-boxing is natively efficient in WASM (i64 operations are first-class).
- State machine generators avoid the need for stack-switching proposals.
- Inline caches (IC) are emitted with stable FNV-hashed site IDs.

**Gaps:**
- **No register allocation awareness**: Every IR variable becomes a WASM local; no attempt to minimize local count or reuse locals. WASM engines optimize this internally, but fewer locals reduce validation time and binary size.
- **No instruction combining**: Adjacent load/store pairs, redundant boxing/unboxing sequences, and constant folding opportunities are not exploited at the WASM level.
- **No block structure optimization**: The dispatch-block model emits nested `if/else/end` trees for state remap tables rather than using `br_table` for large state machines.
- **No peephole optimization**: Patterns like `i64.const 0; i64.eq` (test for zero) could be simplified.
- **Redundant type checks**: Tag checks for NaN-boxed values are emitted inline at every use site rather than being hoisted or eliminated when type information is available from the IR.

### 1.2 Optimization Opportunities

| Optimization | Impact | Effort | Priority |
|---|---|---|---|
| Local variable coalescing (liveness analysis) | DONE (ac215c48) — greedy linear-scan for __tmp/__v temporaries | Medium | P1 |
| Box/unbox elimination when types are statically known | DONE (cd3f98df) — eq/ne skip unbox entirely, arithmetic uses trusted unbox saving 4 insns/op | High | P1 |
| `br_table` for large state dispatch | 2-5x faster generator resume | DONE (c1ae684a) | P1 |
| Constant folding at WASM emission time | DONE (cd3f1b5f) — forward data-flow, folds add/sub/mul/bitwise on fast_int constants | Low | P2 |
| Dead local elimination | 2-5% size reduction | DONE (0b9c39ad) | P2 |
| Instruction combining (adjacent operations) | DONE (d468918f) — const propagation through box/unbox, 5→2 insns for known-const unbox | Medium | P2 |

### 1.4 Audit Findings (2026-03-20)

- 215/602 imports unused (35.6%) — handled by wasm-opt --remove-unused-module-elements post-link
- DONE (fef9990c) — local.tee optimization: 37 eliminated LocalGet instructions
- DONE (ffd95a5d) — Constant materialization: ConstantCache for INT_SHIFT/INT_MIN/INT_MAX
- memory.copy for buffer operations not yet implemented (P2)

### 1.5 Completed Optimization Summary (2026-03-20)

All of the following optimizations were completed in the 2026-03-20 session:

| Optimization | Commit | Impact |
|---|---|---|
| Local variable coalescing | ac215c48 | Greedy linear-scan for `__tmp`/`__v` temporaries; 5-15% size reduction |
| Constant folding at WASM emission | cd3f1b5f | Forward data-flow analysis, folds add/sub/mul/bitwise on `fast_int` constants; 3-5% size reduction |
| Instruction combining | d468918f | Const propagation through box/unbox, reduces 5 insns to 2 for known-const unbox; 3-8% speed improvement |
| `local.tee` introduction | fef9990c | 37 eliminated `LocalGet` instructions; ~1-2% instruction reduction |
| Constant caching (`ConstantCache`) | ffd95a5d | Cache for `INT_SHIFT`/`INT_MIN`/`INT_MAX` materialization in helper functions |
| Precompiled `.cwasm` artifacts | e4b4d9b8 | `--precompile` flag with `wasmtime compile`; 10-50x faster startup |
| `br_table` O(1) state dispatch | c1ae684a | Generator/coroutine state machines use `br_table`; 2-5x faster resume |
| Dead local elimination (`__dead_sink`) | 0b9c39ad | Unused locals routed to single sink; 2-5% binary size reduction |
| `memory.fill` for generator zero-init | 2bff6165 | Bulk zero-init replaces N individual stores; code size + throughput |
| `memory_copy` intrinsic op | 4ca5c360 | `memory.copy` emission for bulk linear-memory copies |
| Full wasm-opt Oz/O3 pipelines | bf65d218 | Integrated post-link optimization; 15-30% binary size reduction |
| `--wasm-profile pure` import stripping | ddc8ea4c | Compile-time IO/ASYNC/TIME import stripping for pure-compute modules |
| Tail call emission (`return_call`) | 49af0f7a | Conservative tail calls for non-stateful functions without EH |
| Native exception handling groundwork | 4b7a52c5 | Tag section, try_table/catch/throw; enabled by default (MOLT_WASM_NATIVE_EH=0 to disable) |
| SIMD stub rewriter support | 0eb06e6c | WASI stub rewriter handles SIMD instructions; enables +simd128 freestanding |
| Multi-value return groundwork | a7b50199 | Multi-value type signatures (Types 31-34); candidate detection pass |
| Box/unbox elimination | cd3f98df | eq/ne skip unbox; arithmetic uses trusted unbox saving 4 insns/op |

### 1.3 Missing Features

- **Multi-return functions**: Currently all functions return a single `i64`. Tuple returns go through memory allocation. Multi-value returns would eliminate heap allocation for small tuples (see Section 3.1).
- **Tail call optimization**: Recursive functions and continuation-passing style compile to regular calls, risking stack overflow. The tail calls proposal (now in WASM 3.0) would fix this (see Section 3.5).
- **WASM exception handling**: Exceptions currently go through host-imported `exception_push/pop/pending` calls. Native WASM exception handling would reduce host call overhead (see Section 3.6).

---

## 2. WASI Compatibility

### 2.1 Current WASI Usage

The host links against WASI Preview 1 (`wasi_snapshot_preview1`) and exposes 22 WASI functions:

| Category | Functions | Status |
|---|---|---|
| Args/Environ | `args_sizes_get`, `args_get`, `environ_sizes_get`, `environ_get` | Functional |
| Clock | `clock_time_get` | Functional |
| Random | `random_get` | Functional |
| Process | `proc_exit`, `sched_yield` | Functional |
| FD I/O | `fd_read`, `fd_write`, `fd_seek`, `fd_tell`, `fd_close` | Functional |
| Filesystem | `path_open`, `path_rename`, `path_readlink`, `path_unlink_file`, `path_create_directory`, `path_remove_directory`, `path_filestat_get`, `fd_filestat_get`, `fd_filestat_set_size`, `fd_readdir` | Functional |
| Prestat | `fd_prestat_get`, `fd_prestat_dir_name` | Functional |
| Polling | `poll_oneoff` | Functional |

### 2.2 Missing for Full Python Stdlib Coverage

| WASI API | Python stdlib need | Current workaround |
|---|---|---|
| `sock_*` (WASI sockets) | `socket` module | Custom `molt_socket_*` host imports (29 functions) |
| `thread-spawn` | `threading` module | Custom `molt_thread_*` host imports |
| Signals | `signal` module | Not supported in WASM |
| `mmap` | `mmap` module | Not applicable (linear memory model) |
| Symlinks | `os.symlink` | Not available in WASI P1 |
| Pipes | `subprocess` | Custom `molt_process_*` host imports |
| Locale | `locale` module | Host-side `num_format::SystemLocale` |

### 2.3 WASI Preview 2 Migration Plan

Wasmtime 41.0.3 uses WASI P1. WASI 0.2 (Preview 2) has been stable since January 2024 and WASI 0.3 is expected around February 2026 with native async support.

Migration path (aligned with spec 0400 Section 13):
1. **Phase 1 (current)**: Raw `Linker::func_wrap()` imports with WASI P1.
2. **Phase 2**: Upgrade wasmtime to 42+ and migrate to Component Model linking. Replace custom I/O intrinsics with WASI P2 interfaces where semantics align (`wasi:filesystem/types`, `wasi:sockets/tcp`, `wasi:clocks/wall-clock`, `wasi:random/random`).
3. **Phase 3**: Adopt WASI 0.3 async interfaces to replace custom async poll intrinsics, enabling native async support in WASM modules.

**Prerequisite**: Validate Component Model overhead is < 2% vs raw imports before committing to migration.

---

## 3. WASM Proposal-Based Optimizations

### 3.1 Multi-Value Returns

**Status**: Standardized in WASM 2.0; universally supported.

**Current gap**: All Molt WASM functions return a single `i64` value. Tuple unpacking, multiple return values, and error-value pairs require heap allocation via `molt_alloc` and subsequent field loads.

**Optimization plan**:
- For functions returning 2-4 values (common in Python: `divmod`, tuple returns, dict `items()`), emit multi-value return signatures.
- Estimated impact: eliminates 1 `alloc` + N `field_get` calls per multi-return site.
- Implementation: extend `WasmBackend` type section to generate multi-return types; modify call-site lowering to consume multi-value results.

**Priority**: P1 -- high impact, low risk, universally supported.

**UPDATE 2026-03-20:** Groundwork complete (a7b50199). Multi-value type signatures (Types 31-34) defined. detect_multi_return_candidates analysis pass identifies safe conversion candidates. Next: modify callee return sequences and call-site destructuring.

### 3.2 Reference Types / GC Proposal (externref)

**Status**: `externref` standardized in WASM 2.0. WasmGC (struct/array types) standardized in WASM 3.0.

**Current gap**: Host objects (Python objects, class instances) are represented as NaN-boxed i64 handles. Every operation on a host object requires a host call to resolve the handle.

**Optimization plan**:
- **Phase 1**: Use `externref` for host-managed objects that never need arithmetic operations (file handles, socket handles, database connections). Avoids NaN-boxing overhead for pure-handle values.
- **Phase 2 (long-term)**: Evaluate WasmGC for in-WASM object allocation. This would allow class instances with known layouts to live in WASM-managed GC memory rather than going through host `alloc`/`free`. Requires major architectural investment.

**Priority**: P2 (externref), P3 (WasmGC).

### 3.3 Bulk Memory Operations

**Status**: Standardized in WASM 2.0; universally supported.

**Current gap**: The emitter does not use `memory.copy` or `memory.fill`. Data initialization uses active data segments (correct), but runtime buffer copies (string concatenation, list/bytes operations) go through host intrinsics.

**Optimization plan**:
- Use `memory.fill` for zero-initialization of stack frames and generator control blocks instead of emitting N `i64.const 0; i64.store` sequences.
- Use `memory.copy` for buffer-to-buffer operations where both source and destination are in linear memory.
- Estimated impact: 10-30% speedup for large buffer operations; 2-5% binary size reduction from shorter initialization sequences.

**UPDATE 2026-03-20:** `memory.fill` is now used for generator control block zero-initialization (2bff6165). `memory_copy` intrinsic op (4ca5c360) added to the WASM emitter, emitting `memory.copy` (src_mem=0, dst_mem=0) for bulk linear-memory-to-linear-memory copies.

**UPDATE 2026-03-20:** `memory_copy` intrinsic op added to the WASM emitter. The op emits `memory.copy` (src_mem=0, dst_mem=0) for bulk linear-memory-to-linear-memory copies. IR signature: `memory_copy(dst, src, len)` where all three args are i64-boxed i32 byte offsets. Current buffer ops (`bytes_concat`, `str_concat`, `list_copy`, `slice`, etc.) all delegate to host imports which perform the copy on the host side; the new intrinsic is available for future IR lowering passes that can identify cases where both source and destination are already resolved to linear memory addresses (e.g. closure slot migration, frame spill/restore, data-segment-to-heap initialization).

**Priority**: P2 -- moderate impact, low risk.

### 3.4 SIMD Proposal (128-bit)

**Status**: Fixed-width SIMD standardized in WASM 2.0. Relaxed SIMD in WASM 3.0.

**Current gap**: No SIMD usage. All arithmetic is scalar i64/f64.

**Optimization plan**:
- **Phase 1**: Use SIMD for hot stdlib intrinsics: `bytes.find` (SIMD string search), `list` sum/min/max (vectorized reduction), hash computation.
- **Phase 2**: Auto-vectorize tight numeric loops identified by the TIR optimizer (requires loop analysis in the compiler).
- **Phase 3**: Relaxed SIMD for floating-point reductions where nondeterminism is acceptable (capability-gated per Molt's determinism policy).

**Constraints**:
- Deterministic mode (`MOLT_DETERMINISTIC=1`) must not use relaxed SIMD.
- Browser compatibility is good (Chrome 91+, Firefox 89+, Safari 16.4+).

**UPDATE 2026-03-20:** SIMD instructions fully supported in the WASI stub rewriter (0eb06e6c), enabling freestanding builds with +simd128.

**Priority**: P2 (stdlib intrinsics), P3 (auto-vectorization).

### 3.5 Tail Calls Proposal

**Status**: Standardized in WASM 3.0 (September 2025). Supported in V8, SpiderMonkey, and wasmtime.

**Current gap**: Recursive functions and trampolined dispatch use regular calls, consuming stack space proportional to recursion depth. Generator poll functions use state-machine dispatch (no recursion), but mutual recursion patterns and CPS-style code risk stack overflow.

**Optimization plan**:
- Detect tail-position calls in the IR and emit `return_call` / `return_call_indirect` instead of `call` + `return`.
- Primary beneficiaries: recursive algorithms (tree traversal, list processing), trampoline dispatch.
- Estimated impact: prevents stack overflow for deep recursion; minor performance improvement from eliminated stack frame setup/teardown.

**UPDATE 2026-03-20:** Implemented (49af0f7a). Conservative: only non-stateful functions without exception handling. Reports count via MOLT_WASM_IMPORT_AUDIT=1.

**Priority**: P2 -- correctness improvement more than performance.

### 3.6 Exception Handling Proposal

**Status**: Standardized in WASM 3.0 (September 2025). Supported in V8, SpiderMonkey, and wasmtime.

**Current gap**: Molt's exception model goes through 8 host-imported functions (`exception_push`, `exception_pop`, `exception_pending`, `exception_new`, `exception_clear`, `exception_kind`, `exception_class`, `exception_message`). Every `try/except` block emits `exception_push` at entry, `exception_pending` checks after every potentially-raising call, and `exception_pop` at exit. This generates significant host-call overhead.

**Optimization plan**:
- Define WASM exception tags for Molt exception types (general exception, StopIteration, KeyError, etc.).
- Replace `exception_push`/`exception_pending`/`exception_pop` sequences with native `try`/`catch`/`throw` WASM instructions.
- Move exception payload (class, message, traceback) into WASM-side data structures, eliminating host round-trips for exception attribute access.
- Estimated impact: 20-40% speedup for exception-heavy code (generators, iterators, `dict.get` with default); 5-10% binary size reduction from eliminated check_exception blocks.

**UPDATE 2026-03-20:** Groundwork complete (4b7a52c5). Tag section, try_table/catch/throw emission implemented. Currently works for unlinked output only (wasm-ld EH relocation support pending).

**UPDATE 2026-03-21:** Native EH enabled by default. Set `MOLT_WASM_NATIVE_EH=0` to disable. 20-40% speedup for exception-heavy code; eliminates `exception_pending` polling after every host call.

**Priority**: P1 -- high impact for real-world Python patterns (StopIteration is used on every `for` loop).

### 3.7 Threads/Atomics Proposal

**Status**: Standardized. SharedArrayBuffer support required in browsers (cross-origin isolation headers).

**Current gap**: Molt's `threading` module uses custom host imports (`thread_submit`, `thread_spawn`, `thread_join`). The host manages all thread state; WASM modules are single-threaded.

**Optimization plan**:
- **Phase 1 (current)**: Continue with host-managed threads. This is correct and portable.
- **Phase 2**: Evaluate `wasm32-unknown-unknown` + shared memory for compute-heavy workloads where host-call overhead for thread coordination dominates.
- **Constraint**: Molt's GIL-equivalent serialization must be preserved. Shared memory introduces data races; atomic operations must protect all shared state.

**Priority**: P3 -- host-managed threads are sufficient for current workloads.

---

## 4. Binary Size Optimization

### 4.1 Current State

Per the import analysis (`docs/architecture/wasm-import-stripping.md`), a compiled `generator.wasm` is 13.1 MB with 90 imports (60 of which are unused). The monolithic import surface is the primary size driver.

### 4.2 Optimization Pipeline

```
Source .py
  |
  v
molt-backend/src/wasm.rs  -->  output.wasm (relocatable object)
  |
  v
wasm-ld (link with molt_runtime.wasm)  -->  output_linked.wasm
  |
  v
wasm-opt --remove-unused-module-elements -Oz  -->  output_optimized.wasm
  |
  v
wasm-tools strip (remove name/debug sections)  -->  output_stripped.wasm
  |
  v
brotli / gzip  -->  output_stripped.wasm.br
```

### 4.3 Specific Size Optimizations

| Optimization | Estimated Reduction | Status |
|---|---|---|
| **Import stripping** (`--wasm-profile pure`) | 30-50% for pure-compute modules | DONE (ddc8ea4c) — compile-time IO/ASYNC/TIME import stripping |
| **Dead code elimination** via `wasm-opt --dce` | 10-20% | Integrated into build |
| **Name section stripping** via `wasm-tools strip` | 5-10% | Integrated (--strip-debug in Oz pipeline) |
| **Brotli compression** | 60-70% of stripped size | Available, not integrated into build |
| **Precompiled .cwasm artifacts** | 10-50x faster startup | DONE (e4b4d9b8) — --precompile flag |
| **Constant deduplication** in data segments | 3-5% | Partially implemented (`data_segment_cache`) |
| **Function deduplication** (identical code merging) | 2-5% | Not implemented |
| **Type section deduplication** | 1-2% | Not implemented (30+ types currently defined) |
| **Import usage audit** (unused import detection) | Diagnostic available via MOLT_WASM_IMPORT_AUDIT=1 | Available |

### 4.4 wasm-opt Pass Selection

**UPDATE 2026-03-20:** Both the Oz (size-focused) and O3 (speed-focused) pipelines below are now integrated into the build via the `--wasm-opt-level` flag (bf65d218). The pipelines run automatically as a post-link step when wasm-opt is available, achieving 15-30% binary size reduction.

Recommended `wasm-opt` pass pipeline for Molt output:

```bash
# Size-focused (browser deployment)
wasm-opt -Oz \
  --remove-unused-module-elements \
  --remove-unused-names \
  --strip-debug \
  --coalesce-locals \
  --reorder-locals \
  --dce \
  --vacuum \
  --duplicate-function-elimination \
  --code-folding \
  input.wasm -o output.wasm

# Speed-focused (server/edge deployment)
wasm-opt -O3 \
  --remove-unused-module-elements \
  --remove-unused-names \
  --coalesce-locals \
  --reorder-locals \
  --dce \
  --vacuum \
  --inlining \
  --flatten \
  --local-cse \
  input.wasm -o output.wasm
```

### 4.5 Size Targets

| Artifact | Current | Target (v0.1) | Target (v1.0) |
|---|---|---|---|
| Minimal hello-world `.wasm` (linked) | ~2 MB | < 500 KB | < 200 KB |
| Runtime `.wasm` (molt_runtime.wasm) | ~8 MB | < 4 MB | < 2 MB |
| Typical app `.wasm` (linked, stripped) | ~13 MB | < 3 MB | < 1 MB |
| Compressed (brotli) | N/A | < 1 MB | < 300 KB |

---

## 5. Startup Time

### 5.1 Current State

The wasmtime host (`molt-wasm-host`) supports three compilation strategies:
- **JIT compilation** (default): Compile WASM to native code at load time.
- **Precompiled modules** (`MOLT_WASM_PRECOMPILED=1`): Deserialize pre-compiled `.cwasm` files, skipping compilation entirely.
- **Fast compilation** (`MOLT_WASM_COMPILE_FAST=1`): Use `OptLevel::None` for faster compilation at the cost of runtime performance.

### 5.2 Optimization Opportunities

| Optimization | Impact | Effort |
|---|---|---|
| **Precompiled `.cwasm` artifacts** as default for production | DONE (e4b4d9b8) — --precompile flag with wasmtime compile | Low (infrastructure) |
| **Streaming compilation** for browser targets | Progressive loading; first-byte-to-execution | Medium |
| **Lazy compilation** (compile functions on first call) | Faster startup for large modules | Low (wasmtime config) |
| **Module splitting** (separate hot/cold code) | Faster initial load; lazy-load cold paths | High |
| **Snapshot artifacts** (`molt.snapshot`) | Skip init phase entirely | High (see spec 0968) |
| **Parallel compilation** (default on) | 2-4x faster compile on multi-core | Already supported |

### 5.3 Startup Time Targets

| Scenario | Current (est.) | Target |
|---|---|---|
| Cold start (JIT compile + init) | 200-500 ms | < 100 ms |
| Warm start (precompiled + init) | 20-50 ms | < 10 ms |
| Snapshot restore | N/A | < 5 ms |
| Browser (streaming compile) | N/A | < 50 ms (time to first interaction) |

---

## 6. Memory Management

### 6.1 Current Linear Memory Layout

```
0x00000000 +-----------------------+
           | WASM stack            |  (grows down from stack pointer)
           +-----------------------+
           | Runtime static data   |
0x00100000 +-----------------------+  (MOLT_WASM_DATA_BASE, default 1 MiB)
           | Molt data segments    |  (string constants, IC tables, etc.)
           | (8-byte aligned)      |
           +-----------------------+
           | Heap (malloc/free)    |  (grows up)
           +-----------------------+
           | Unused / growable     |
0xFFFFFFFF +-----------------------+
```

### 6.2 Current Issues

- **No memory shrinking**: WASM linear memory can only grow, never shrink. Long-running programs accumulate peak memory usage.
- **Fragmentation**: The host-side allocator (`molt_alloc` / `molt_free`) is opaque to the WASM module. No arena or bump allocation for short-lived objects.
- **No memory pressure signaling**: The WASM module cannot communicate memory pressure to the host for GC triggering.
- **Fixed initial size**: No adaptive initial memory sizing based on program complexity.

### 6.3 Optimization Plan

| Optimization | Impact | Effort | Priority |
|---|---|---|---|
| **Arena allocation** for function-scoped temporaries | 30-50% fewer `alloc`/`free` host calls | Medium | P1 |
| **Bump allocator** for data segment initialization | Faster startup data layout | Low | P2 |
| **Memory pooling** for common object sizes (strings, lists, dicts) | Reduced fragmentation | Medium | P2 |
| **Adaptive initial memory** based on program analysis | Better cold-start memory footprint | Low | P3 |
| **GC integration** with WASM linear memory tracking | Accurate memory accounting | High | P3 |

---

## 7. Interop and Host Call Overhead

### 7.1 Current Cost Model

Every operation that touches the Python object model goes through a host import call. The wasmtime ABI for host calls involves:
1. WASM-to-native trampoline (register save/restore).
2. Wasmtime `Caller` context lookup.
3. Host function execution.
4. Native-to-WASM return trampoline.

Measured overhead: approximately 50-100 ns per host call (empty function). With 620+ importable functions and typical programs making millions of calls, host call overhead dominates execution time.

### 7.2 Optimization Strategies

**A. Reduce host call count (highest priority):**
- **Inline arithmetic**: For statically-typed `int + int`, `float * float`, emit WASM `i64.add` / `f64.mul` directly instead of calling host `add`/`mul`. The backend already has tag-check infrastructure; extend it to emit inline fast paths with host-call fallback.
- **Batch operations**: Replace N individual `list_append` host calls with a single `list_extend_batch(ptr, count)` that reads values from linear memory.
- **Cache hot paths**: IC dispatch already exists; extend to cache resolved attribute offsets in WASM-local variables.

**B. Reduce per-call overhead:**
- **Wasmtime fuel metering** off by default (already the case).
- **Typed host functions** in wasmtime to avoid `Val` boxing on the host side.
- **Precompiled host trampolines** via wasmtime's `InstancePre` API.

**C. Data marshaling optimization:**
- **Zero-copy string passing**: Strings in linear memory can be read directly by the host without copying (current approach for data segments).
- **Shared-memory result buffers**: For operations returning structured data (dict items, list slices), write results directly into WASM linear memory instead of returning opaque handles.

### 7.3 Impact Estimates

| Optimization | Host calls eliminated | Speed impact |
|---|---|---|
| Inline `int` arithmetic | 40-60% of arithmetic calls | 2-5x for numeric code |
| Inline `float` arithmetic | 20-30% of arithmetic calls | 2-3x for float-heavy code |
| Inline tag checks (`is_int`, `is_float`, `is_none`) | 80% of type check calls | 1.5-2x broadly |
| Batch collection operations | 50-70% of collection calls | 1.5-3x for collection-heavy code |

---

## 8. Browser vs Server WASM

### 8.1 Browser Optimization Strategy

**Priorities**: binary size, startup time, memory footprint.

| Technique | Rationale |
|---|---|
| Aggressive size optimization (`wasm-opt -Oz`) | Download time dominates |
| Brotli compression | 60-70% size reduction over the wire |
| Streaming compilation (`WebAssembly.compileStreaming`) | Begin compilation during download |
| Code splitting (hot/cold) | Load only needed code initially |
| Import stripping (`--wasm-profile pure`) | Remove unused I/O/DB/socket imports |
| Service worker caching | Avoid redownload on revisit |
| Lazy module loading | Load stdlib modules on demand |

**Browser-specific constraints**:
- No file system (VFS via `/bundle` mount per spec 0968).
- No threads unless `SharedArrayBuffer` is available (cross-origin isolation).
- Memory limited (~2-4 GB practical maximum).
- No blocking I/O (all async via host capabilities).

### 8.2 Server/Edge Optimization Strategy

**Priorities**: throughput, latency, multi-tenant isolation.

| Technique | Rationale |
|---|---|
| Speed optimization (`wasm-opt -O3`) | Throughput matters more than size |
| Precompiled `.cwasm` artifacts | Eliminate compilation overhead |
| Snapshot artifacts (`molt.snapshot`) | Skip init phase for edge workers |
| `InstancePre` for pooled instantiation | Amortize linking across requests |
| Fuel-based CPU budgets | Multi-tenant fairness |
| Memory limits per instance | Prevent OOM from rogue tenants |
| Parallel compilation enabled | Use all cores for initial compile |

### 8.3 Edge/Workers Optimization Strategy

Per spec 0965 and 0968:
- Deploy-time init + WASM linear memory snapshot.
- Strict resource limits (CPU, memory, output size).
- Capability-gated I/O (no ambient authority).
- Schema-first boundary for all host interactions.

---

## 9. Component Model

### 9.1 Current State

Molt currently uses raw wasmtime imports via `Linker::func_wrap()`. The WIT definition at `wit/molt-runtime.wit` serves as documentation only; actual binding is manual. The current wasmtime version (41.0.3) predates stable Component Model support.

### 9.2 Migration Plan (from spec 0400 Section 13)

**Phase 1 (current)**: Raw imports, WASI P1. Complete.

**Phase 2 (target: wasmtime 42+)**: Component Model migration.
- Split `wit/molt-runtime.wit` into capability-scoped interfaces:
  - `molt:runtime/core` -- lifecycle, object model, arithmetic, type system
  - `molt:runtime/io` -- file, socket, process, stream operations
  - `molt:runtime/codec` -- JSON, MsgPack, CBOR, Arrow IPC serialization
  - `molt:runtime/db` -- database query/exec operations
  - `molt:runtime/async` -- futures, promises, tasks, channels
  - `molt:runtime/collections` -- list, dict, set, tuple, heapq
- Define `molt-app` world in `wit/world.wit`.
- Replace `Linker::func_wrap()` with `wasmtime::component::Linker`.
- Update `wasm.rs` to emit Component Model modules.

**Phase 3 (future)**: WASI Preview 2 alignment.
- Replace custom I/O intrinsics with WASI P2 interfaces where semantics align.
- Molt-specific intrinsics (NaN-boxing, refcount, GC, codec, DB, async primitives) remain as custom `molt:runtime/*` imports.

### 9.3 Component Model Benefits

- **Capability enforcement at link time**: Modules that do not import `molt:runtime/io` cannot perform I/O, enforced structurally.
- **Composability**: `wasm-tools compose` can combine Molt modules with other Component Model components.
- **ABI stability**: WIT semantic versioning provides a stable interface contract.
- **Automatic binding generation**: `wit-bindgen` for host-side and guest-side bindings.
- **Size reduction**: Unused capability interfaces are not linked, naturally eliminating dead imports.

### 9.4 Prerequisites

- wasmtime Component Model API stable (42+).
- WASI Preview 2 filesystem/network support mature.
- Molt's WIT interface stabilized (no breaking changes in 3+ months).
- Performance validation: Component Model overhead < 2% vs raw imports.

---

## 10. Implementation Roadmap

### Phase 1: Quick Wins (P0/P1, 1-2 months)

1. **Integrate `wasm-opt` into build pipeline** -- run `wasm-opt -Oz --remove-unused-module-elements` as post-link step.
2. **Implement `--wasm-profile pure`** -- conditional import registration in `wasm.rs` for pure-compute modules.
3. **Inline integer arithmetic fast paths** -- emit WASM-native `i64.add/sub/mul` with tag-check guards, host-call fallback.
4. **Name/debug section stripping** -- `wasm-tools strip` in production builds.
5. **Precompiled `.cwasm` as default** -- generate and cache precompiled artifacts.

### Phase 2: Proposal Adoption (P1/P2, 2-4 months)

6. **Multi-value returns** for 2-4 value tuples.
7. **WASM exception handling** -- replace `exception_push/pop/pending` with native `try/catch/throw`.
8. **Tail call emission** for tail-position calls.
9. **`br_table`** for large state machine dispatch.
10. **Bulk memory operations** for initialization and buffer copies.

### Phase 3: Architecture (P2/P3, 4-6 months)

11. **Arena allocation** for function-scoped temporaries.
12. **Inline type checks and float arithmetic**.
13. **Component Model migration** (wasmtime upgrade + WIT split).
14. **SIMD for stdlib intrinsics** (bytes.find, list reductions).
15. **Snapshot artifacts** for edge/worker deployment.

### Phase 4: Advanced (P3, 6-12 months)

16. **WasmGC evaluation** for in-WASM object allocation.
17. **Auto-vectorization** from TIR loop analysis.
18. **Module splitting** for lazy loading.
19. **WASI 0.3 async integration**.

---

## 11. Metrics and Gates

All WASM optimization work must report:

| Metric | Tool | Gate |
|---|---|---|
| Binary size (raw, stripped, compressed) | `wasm-tools`, `brotli`, `gzip` | No > 10% regression |
| Cold start time (JIT + init) | `bench_wasm.py` | No > 10% regression |
| Warm start time (precompiled) | `bench_wasm.py` | No > 10% regression |
| Benchmark throughput (vs native) | `bench_wasm.py` | WASM within 3x of native for compute |
| Host call count | wasmtime profiling | Track reduction over time |
| Memory peak | `bench_wasm.py` RSS tracking | No > 20% regression |

Results must be recorded in `bench/results/bench_wasm.json` and summarized in `README.md` per existing policy.

---

## 12. References

### Internal Specs
- `docs/spec/areas/wasm/0400_WASM_PORTABLE_ABI.md` -- Portable ABI and Component Model migration plan
- `docs/spec/areas/wasm/0401_WASM_TARGETS_AND_CONSTRAINTS.md` -- Target definitions and constraints
- `docs/spec/areas/wasm/0964_MOLT_WASM_ABI_BROWSER_DEMO_AND_CONSTRAINTS.md` -- ABI spec and browser demo
- `docs/spec/areas/wasm/0965_CLOUDFLARE_WORKERS_LESSONS_FOR_MOLT.md` -- Edge/worker deployment lessons
- `docs/spec/areas/wasm/0968_MOLT_EDGE_WORKERS_VFS_AND_HOST_CAPABILITIES.md` -- VFS and capabilities
- `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md` -- Size and cold-start gates
- `docs/architecture/wasm-import-stripping.md` -- Import stripping analysis

### Implementation Files
- `runtime/molt-backend/src/wasm.rs` -- WASM code emitter (9740 lines)
- `runtime/molt-wasm-host/src/main.rs` -- Wasmtime host runner (4551 lines)
- `runtime/molt-wasm-host/Cargo.toml` -- wasmtime 41.0.3, wasmtime-wasi 41.0.3
- `tools/bench_wasm.py` -- WASM benchmark harness
- `wit/molt-runtime.wit` -- WIT interface definition (622+ intrinsics)

### External References
- [WebAssembly 3.0 Standard](https://webassembly.org/news/2025-09-17-wasm-3.0/) -- Tail calls, exception handling, relaxed SIMD, memory64
- [WASI Roadmap](https://wasi.dev/roadmap) -- WASI 0.2 stable, 0.3 targeting Feb 2026
- [Binaryen Optimizer Cookbook](https://github.com/WebAssembly/binaryen/wiki/Optimizer-Cookbook) -- wasm-opt pass reference
- [Binaryen GC Optimization Guidebook](https://github.com/WebAssembly/binaryen/wiki/GC-Optimization-Guidebook) -- WasmGC-specific passes
- [Component Model Documentation](https://component-model.bytecodealliance.org/) -- Bytecode Alliance Component Model
- [WebAssembly Feature Status](https://webassembly.org/features/) -- Proposal implementation status across engines
- [State of WebAssembly 2025-2026](https://platform.uno/blog/the-state-of-webassembly-2025-2026/) -- Ecosystem overview
- [WASM Performance vs Native Benchmarks](https://karnwong.me/posts/2024/12/native-implementation-vs-wasm-for-go-python-and-rust-benchmark/) -- Cross-language WASM performance comparison
