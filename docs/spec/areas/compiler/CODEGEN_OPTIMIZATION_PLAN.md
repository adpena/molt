# Codegen Optimization Plan

Status: **Updated 2026-04-01 — see per-section status below** | Author: codegen-team | Date: 2026-03-12

This document surveys the current state of Molt's Cranelift-based code generation backend (`runtime/molt-backend/src/lib.rs`, `wasm.rs`) and proposes concrete optimization work across eleven areas. Each section states the current state (with code references), proposed improvements, expected impact, and implementation effort.

### Status snapshot (updated 2026-04-01)

Sections 1-5 were originally marked "DONE (2026-03-20)" but this was based on Cranelift defaults being configured, not custom optimization code. Meanwhile, the TIR pipeline (added after this plan) implements escape analysis and monomorphization as TIR passes, which this plan listed as not-started.

| # | Area | Status | Notes |
|---|------|--------|-------|
| 1 | Cranelift Optimization Flags | **Done** | Cranelift 0.130 configured with full flag set |
| 2 | Calling Convention Optimization | **Done (defaults)** | Standard C ABI via Cranelift; no custom calling convention |
| 3 | Register Allocation | **Done (defaults)** | Cranelift's regalloc2; no custom tuning |
| 4 | Instruction Selection | **Done (defaults)** | Cranelift's ISLE rules; no custom patterns |
| 5 | Branch Optimization | **Done (defaults)** | Cranelift's egraph optimizer; no custom branch opts |
| 6 | Inline Caching | **Not started** | No `InlineCache` code in backend |
| 7 | Escape Analysis | **Done (TIR pass)** | `tir/escape_analysis.rs` (510 lines) — NoEscape/ArgEscape/GlobalEscape lattice, stack promotion |
| 8 | Specialization | **Done (TIR pass)** | `tir/monomorphize.rs` (653 lines) — type-specialized function copies |
| 9 | Memory Access Patterns | **Partial (TIR pass)** | `tir/deforestation.rs` (27.8KB) — iterator fusion eliminates intermediates |
| 10 | WASM-Specific Optimizations | **Done** | Tail call optimization, WASM feature flags |
| 11 | Profile-Guided Optimization | **Stub** | `llvm_backend/pgo.rs` (185 lines) in LLVM backend only |

---

## Table of Contents

1. [Cranelift Optimization Flags](#1-cranelift-optimization-flags)
2. [Calling Convention Optimization](#2-calling-convention-optimization)
3. [Register Allocation](#3-register-allocation)
4. [Instruction Selection](#4-instruction-selection)
5. [Branch Optimization](#5-branch-optimization)
6. [Inline Caching](#6-inline-caching)
7. [Escape Analysis](#7-escape-analysis) — implemented as TIR pass
8. [Specialization](#8-specialization) — implemented as TIR pass
9. [Memory Access Patterns](#9-memory-access-patterns) — partially via deforestation
10. [WASM-Specific Optimizations](#10-wasm-specific-optimizations)
11. [Profile-Guided Optimization (PGO)](#11-profile-guided-optimization-pgo)

---

## 1. Cranelift Optimization Flags

### Current State

`SimpleBackend::new_with_target()` (lib.rs:1008-1131) configures Cranelift 0.128 with the following settings:

| Flag | Value | Notes |
|------|-------|-------|
| `opt_level` | `"speed"` | Maximum optimization level; Cranelift applies egraph-based rewrites, GVN, LICM, DCE, and constant folding |
| `is_pic` | `"true"` | Position-independent code for linking with system runtime |
| `enable_alias_analysis` | `"true"` | Redundant-load elimination within basic blocks |
| `machine_code_cfg_info` | `"true"` | CFG metadata for profilers |
| `use_colocated_libcalls` | `"true"` | Direct PC-relative calls, skipping GOT/PLT |
| `preserve_frame_pointers` | debug=`"true"`, release=`"false"` | Frees rbp/x29 in release |
| `enable_heap_access_spectre_mitigation` | `"false"` | Molt compiles trusted code |
| `enable_table_access_spectre_mitigation` | `"false"` | Same rationale |
| `probestack_strategy` | `"inline"` | Inline touch instructions for stack probes |
| `enable_verifier` | debug=`"false"`, release=`"true"` | IR verification |
| `log2_min_function_alignment` | debug=`"0"`, release=`"4"` | 16-byte alignment in release |

Host CPU feature detection is enabled by default via `cranelift_native::builder_with_options(true)` (lib.rs:1125), allowing AVX2, BMI2, POPCNT on x86_64 and NEON/AES/CRC on aarch64. `MOLT_PORTABLE=1` disables this for reproducible builds.

### Proposed Improvements

**P1: `opt_level` = `"speed_and_size"` evaluation.** Cranelift 0.128 supports `"speed_and_size"` which enables additional code-size-conscious optimizations. For Molt's many small runtime-call-heavy functions, reducing code size may actually improve i-cache hit rates. Benchmark both modes on the differential suite to measure net effect.

- **Expected impact**: 2-5% improvement on cache-heavy workloads (many small functions); possible 1-2% regression on compute-heavy tight loops.
- **Effort**: Low (1 day). Change one line, benchmark.

**P2: Per-function optimization levels.** Cranelift supports setting optimization level per-function context. Hot functions (from PGO profile) should use `opt_level=speed`; cold functions (exception handlers, rarely-called stdlib wrappers) could use `opt_level=speed_and_size` to reduce total code size.

- **Expected impact**: 3-8% improvement in i-cache pressure for large programs with many cold functions.
- **Effort**: Medium (3 days). Requires threading hotness info from `PgoProfileIR` into `compile_func`.

**P3: Enable `enable_nan_canonicalization` selectively.** For float-heavy code, NaN canonicalization ensures deterministic behavior. Currently not set (defaults to false). Given Molt's determinism requirement, this should be explicitly enabled for correctness.

- **Expected impact**: Correctness improvement; 1-2% slowdown on float-heavy benchmarks.
- **Effort**: Low (1 day).

---

## 2. Calling Convention Optimization

### Current State

All runtime intrinsics are called via the standard C ABI through Cranelift's `Import` linkage. Every `molt_*` function call (e.g., `molt_add`, `molt_call_bind_ic`, `molt_inc_ref_obj`) follows this pattern (lib.rs:2092-2097):

```
sig = module.make_signature();
sig.params.push(AbiParam::new(types::I64));  // NaN-boxed args
sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed return
callee = module.declare_function("molt_foo", Linkage::Import, &sig);
local_callee = module.declare_func_in_func(callee, builder.func);
builder.ins().call(local_callee, &[...]);
```

Trampoline functions (`ensure_trampoline`, lib.rs:1294-1400) wrap variable-arity calls into a fixed 3-parameter ABI (closure_bits, args_ptr, args_len). There are four trampoline kinds: `Plain`, `Generator`, `Coroutine`, `AsyncGen`.

Function inlining exists at the IR level (`inline_functions`, lib.rs:587-751) for small leaf functions (<=30 ops, no control flow, no nested internal calls). It renames variables with a prefix and splices callee ops into the caller.

### Proposed Improvements

**P1: Signature deduplication.** Every call site constructs a new `Signature` via `module.make_signature()`. The same (I64, I64) -> I64 signature is constructed hundreds of times per function. Cache signatures by arity in a `Vec<Signature>` indexed by param count.

- **Expected impact**: 5-10% compile-time reduction (signature construction dominates small functions).
- **Effort**: Low (2 days). Build a `SignatureCache` struct, thread through `compile_func`.

**P2: Direct intrinsic call specialization.** For the most common intrinsics (`molt_add`, `molt_sub`, `molt_mul`, `molt_inc_ref_obj`, `molt_dec_ref`, `molt_dec_ref_obj`), declare them once at module level rather than per-call-site. Currently `declare_function` is called at every use site; Cranelift deduplicates internally, but skipping the string lookup saves time.

- **Expected impact**: 3-5% compile-time reduction.
- **Effort**: Low (2 days). Build a `HashMap<&str, FuncId>` for common intrinsics at module init.

**P3: Tail call optimization for self-recursive functions.** Cranelift 0.128 supports `return_call` for tail calls. Molt should detect tail-position `call_internal` ops where the result flows directly to `ret` and lower them to `return_call`. This eliminates stack frame accumulation for recursive algorithms (tree traversal, linked list processing).

- **Expected impact**: Enables O(1) stack for tail-recursive programs; prevents stack overflow on deep recursion.
- **Effort**: Medium (5 days). Detect tail position in IR, emit `return_call` instead of `call`+`ret`, handle refcount cleanup before the tail call.

**P4: Inline ref-count operations.** `emit_maybe_ref_adjust` (lib.rs:200-205) always emits a function call to the runtime. For the common case (non-pointer NaN-boxed values), the call is a no-op. Inline the tag check: if the value's tag bits indicate an inline int/bool/none/float, skip the call entirely. This eliminates ~40% of ref-adjust calls.

- **Expected impact**: 10-15% improvement on allocation-heavy benchmarks.
- **Effort**: Medium (3 days). Emit `brif` on tag check, skip call for non-ptr values.

---

## 3. Register Allocation

### Current State

Register allocation is configured via `MOLT_BACKEND_REGALLOC_ALGORITHM` env var (lib.rs:1012-1024):
- Debug builds: `single_pass` (fast, poor code quality)
- Release builds: `backtracking` (Cranelift's regalloc2 with backtracking solver, better spill decisions)

Cranelift 0.128 ships regalloc2 with two algorithms:
- `single_pass`: Linear scan, O(n), no live-range splitting
- `backtracking`: Iterative backtracking solver with live-range splitting, spill weight heuristics, move coalescing

Frame pointers are omitted in release builds (lib.rs:1082-1091), freeing one register (rbp on x86_64, x29 on aarch64).

### Proposed Improvements

**P1: Reduce variable pressure in hot paths.** The `fast_int` code path for arithmetic (e.g., lib.rs:2106-2142) creates 3 blocks (fast, slow, merge) with block parameters. This forces the register allocator to handle phi nodes. For the common case where `fast_int=true` and the value is known to be an int, the slow block is never taken. Consider emitting the fast path inline without a merge block when type information is available.

- **Expected impact**: 5-8% improvement on tight arithmetic loops (fewer spills, no merge-block overhead).
- **Effort**: Medium (3 days). Requires plumbing type certainty from frontend.

**P2: Stack slot coalescing for temporary values.** Many operations allocate stack slots for output parameters (e.g., `out_slot` at lib.rs:2088 for `molt_bytes_from_bytes`). These slots are only live for one instruction. Reuse stack slots across operations that don't overlap.

- **Expected impact**: Reduced stack frame size, better cache behavior for deep call chains.
- **Effort**: Medium (4 days). Track slot liveness, implement a simple allocator.

**P3: Evaluate `checkedmul` for regalloc tuning.** regalloc2 has a cost model for spill weights. Currently Molt doesn't adjust spill costs for hot loops. Using Cranelift's `FuncLayout` block frequency annotations from PGO data would let regalloc2 make better decisions about which values to spill inside loops.

- **Expected impact**: 3-5% improvement on loop-heavy programs.
- **Effort**: High (1 week). Requires understanding regalloc2 cost model and Cranelift's block frequency API.

---

## 4. Instruction Selection

### Current State

Cranelift's instruction selection is architecture-aware. With host feature detection enabled (`cranelift_native::builder_with_options(true)`), the backend can emit:
- **x86_64**: LEA for address computation, BMI2 for bit manipulation, POPCNT/TZCNT, SSE/AVX for float ops
- **aarch64**: NEON for vector ops, fused multiply-add (FMA), conditional select (CSEL)

However, Molt's IR-to-CLIF lowering doesn't exploit architecture-specific patterns:

1. **Multiply-add**: `a * b + c` is lowered as separate `imul` + `iadd`. Cranelift can fuse these into LEA on x86 or MADD on ARM64, but only if the pattern is visible in one basic block.

2. **Tag checks**: `is_int_tag` (lib.rs:151-156) emits `band` + `iconst` + `icmp`. On x86_64, this could be a single `test` + `jz` if the tag constants are structured as power-of-two masks.

3. **Unbox/rebox sequences**: `unbox_int` + arithmetic + `box_int_value` (lib.rs:143-163) emits 6 instructions. The mask-shift-shift pattern is suboptimal; on x86_64 a single `movsx` with appropriate bit width could suffice.

### Proposed Improvements

**P1: Fused tag-check-and-unbox.** Combine `is_int_tag` + `unbox_int` into a single sequence that tests the tag and produces the unboxed value in one pass. The current code checks the tag then re-masks the value; the tag check already produces the masked value as an intermediate.

- **Expected impact**: 10-15% improvement on arithmetic-heavy code (2 fewer instructions per operation in the fast path).
- **Effort**: Medium (3 days). Refactor `is_int_tag` and `unbox_int` to share intermediate values.

**P2: Use `select` instead of branch for simple conditional boxing.** `box_bool_value` (lib.rs:177-183) uses `select` correctly, but many comparison results are branched-then-boxed. Replace branch+merge patterns with `select` where both paths produce a simple constant.

- **Expected impact**: 5-8% improvement on comparison-heavy code (eliminates branch misprediction).
- **Effort**: Low (2 days).

**P3: Strength-reduce NaN-box tag operations.** The tag constants (QNAN | TAG_INT = 0x7ff9_0000_0000_0000) have specific bit patterns. On x86_64, loading 64-bit constants requires `movabs` (10 bytes). Consider using relative constants or preloading tag constants into dedicated registers for frequently-used tags.

- **Expected impact**: 3-5% code size reduction for arithmetic-heavy functions.
- **Effort**: Medium (4 days). Requires Cranelift's constant-pool or global-value support.

---

## 5. Branch Optimization

### Current State

The backend uses `set_cold_block` (37 occurrences in lib.rs) to mark slow-path blocks as cold. This instructs Cranelift's block layout algorithm to move cold blocks out of the linear instruction stream, improving i-cache behavior for the hot path. Cold blocks are used for:
- Slow paths in `fast_int` arithmetic (runtime fallback calls)
- Exception handler labels (lib.rs:13901-13904)
- Overflow paths for inline integer operations

Block layout is otherwise determined by Cranelift's default heuristics based on branch structure.

`apply_profile_order` (lib.rs:754-781) reorders functions based on PGO hot-function list, placing hot functions first in the object file for better code locality.

### Proposed Improvements

**P1: Mark exception-handling blocks as cold systematically.** Currently only `is_function_exception_label` blocks get `set_cold_block`. Extend this to all `try_start`/`except` blocks and error-path branches (e.g., type-check failures, bounds-check failures).

- **Expected impact**: 2-5% improvement on programs with try/except blocks that rarely fire.
- **Effort**: Low (2 days). Add `set_cold_block` for all error-path blocks.

**P2: Loop rotation for do-while patterns.** Python `while` loops test the condition at the top. Rotating the loop to test at the bottom (do-while form) eliminates one branch per iteration. Cranelift can do this with appropriate block ordering.

- **Expected impact**: 3-5% improvement on tight loops with simple conditions.
- **Effort**: Medium (4 days). Detect `loop_start` + condition-check-at-top pattern, rearrange blocks.

**P3: Collapse consecutive branch chains.** When an `if` chain produces nested brif->brif sequences (common in elif chains), collapse them into a Cranelift `Switch` (jump table). The backend already uses `Switch` for state dispatch (wasm.rs state_switch), but not for general elif chains.

- **Expected impact**: 5-10% improvement on code with long if/elif chains (replaces O(n) branches with O(1) table lookup).
- **Effort**: Medium (5 days). Detect elif chains in IR, emit `Switch` when conditions are integer comparisons.

---

## 6. Inline Caching

### Current State

Molt implements monomorphic inline caches (ICs) for method dispatch. The IC system works as follows:

1. **Site ID generation**: `stable_ic_site_id` (lib.rs:123-141) generates a deterministic FNV hash from function name + op index + lane. This ID is NaN-boxed as an inline integer.

2. **Call path**: Method calls go through `molt_call_bind_ic` (runtime: `call/bind.rs:1484`), which takes (site_id, callable_bits, callargs_builder). The runtime maintains a per-site IC cache: if the callable matches the cached entry, it dispatches directly without full argument binding.

3. **Fast path**: `try_call_bind_ic_fast` (runtime: `call/bind.rs:1426`) handles the common case of positional-only calls with a cached IC entry, avoiding full callargs construction.

4. **Guarded direct calls**: For known function targets, the backend emits a guard sequence (lib.rs:9460-9539): check if the callable's function pointer matches the expected address, then dispatch directly. This avoids IC lookup entirely for monomorphic sites.

### Proposed Improvements

**P1: Polymorphic inline caches (PICs).** The current IC is monomorphic: one cached entry per site. For sites that dispatch to 2-3 different types (e.g., `len()` called on both lists and strings), a PIC with 2-4 entries would avoid cache thrashing.

- **Expected impact**: 10-20% improvement on polymorphic dispatch sites.
- **Effort**: High (1 week). Extend `CallBindIcEntry` to hold multiple entries, add state machine for promotion (mono -> poly -> megamorphic fallback).

**P2: IC stub generation in Cranelift.** Currently the IC lookup happens in Rust runtime code. Generate IC stubs directly in Cranelift: emit a compare-and-branch against the cached callable pointer, with a fallback to the full `molt_call_bind_ic`. This eliminates the function-call overhead for the IC check itself.

- **Expected impact**: 5-10% improvement on dispatch-heavy code (eliminates one function call per IC hit).
- **Effort**: High (2 weeks). Requires mutable code sections or indirect jump through a patchable slot.

**P3: IC statistics collection.** Add a `MOLT_IC_STATS=1` mode that counts IC hits, misses, and megamorphic fallbacks per site. This data feeds into PGO and helps identify optimization targets.

- **Expected impact**: Enables data-driven IC tuning. No direct perf impact.
- **Effort**: Low (2 days). Add atomic counters to IC entries, dump on exit.

---

## 7. Escape Analysis

### Current State

The backend has a rudimentary form of escape analysis: `elide_dead_struct_allocs` (lib.rs:480-543). This pass removes `alloc_class`/`alloc_class_trusted`/`alloc_class_static` operations where the allocated object is only used for `store`/`store_init`/`guarded_field_set`/`guarded_field_init`/`object_set_class` — i.e., it is initialized but never read or passed to another function.

This is a dead-allocation elimination, not true escape analysis. It only removes allocations whose fields are written but never read. Objects that are read but don't escape the function are still heap-allocated.

### Proposed Improvements

**P1: Stack allocation for non-escaping objects.** When an object is allocated, initialized, read, and then goes dead within the same function (no store to heap, no pass to callee, no return), allocate it on the stack instead of the heap. This eliminates `molt_alloc_class` call overhead and avoids reference counting.

Implementation sketch:
1. In a pre-pass, compute the set of variables that receive `alloc_class` results.
2. Track all uses: `store`/`load`/`guarded_field_get`/`guarded_field_set` are local uses. `call`, `ret`, `store` to another object's field are escapes.
3. For non-escaping objects, emit `stack_slot` instead of `molt_alloc_class`, and lower field access to direct stack loads/stores.

- **Expected impact**: 20-40% improvement on object-heavy code (list comprehensions creating temporary tuples, named-tuple-style dataclasses). Eliminates heap allocation, reference counting, and GC pressure.
- **Effort**: High (2 weeks). Requires tracking object size at compile time, handling field offsets consistently with the runtime object layout.

**P2: Scalar replacement of aggregates (SRA).** After stack allocation, if an object's fields are independently accessed, replace the object with individual SSA variables for each field. This exposes fields to register allocation and enables further optimizations (CSE, constant propagation through fields).

- **Expected impact**: Additional 10-20% on top of stack allocation for objects with few fields.
- **Effort**: High (2 weeks). Requires field-level alias analysis.

**P3: Extend `elide_dead_struct_allocs` to handle reads.** Currently, if any use is not in `allowed_use_kinds`, the allocation is preserved. Extend the pass to allow reads (`load`, `guarded_field_get`) when the read result is also dead.

- **Expected impact**: 2-5% improvement (catches more dead allocations).
- **Effort**: Low (2 days). Extend the use-kind whitelist and add transitive dead-use tracking.

---

## 8. Specialization

### Current State

The frontend emits `fast_int: true` on arithmetic ops (`add`, `sub`, `mul`, `inplace_add`, etc.) when type hints indicate integer operands (lib.rs:2106, 2197, 2738, 2793, 2844, 2895, 2946). The `fast_int` path:

1. Unboxes both operands (`unbox_int`)
2. Performs native i64 arithmetic (`iadd`, `isub`, `imul`)
3. Checks if the result fits in the 47-bit inline integer range (`int_value_fits_inline`)
4. If yes, re-boxes inline. If no, falls back to runtime call.

When `fast_int` is false, the default path checks both operands' tags at runtime, branches to the fast path if both are int-tagged, otherwise calls the runtime.

There is no specialization for:
- Float operations (always call runtime)
- String operations (always call runtime)
- Container operations (list append, dict lookup — always call runtime)

### Proposed Improvements

**P1: Float fast path.** Mirror the `fast_int` pattern for float operations. When both operands are known floats (no NaN-box tag bits set in the QNAN region), emit native `fadd`/`fsub`/`fmul`/`fdiv` directly. The NaN-box encoding stores floats as raw IEEE 754 bits — no unboxing needed, just `bitcast` to f64 and compute.

- **Expected impact**: 20-30% improvement on float-heavy code (scientific computing, ML preprocessing).
- **Effort**: Medium (5 days). Add `fast_float` flag in frontend, emit `fadd`/`fsub`/`fmul`/`fdiv` with tag check.

**P2: String concatenation specialization.** String `+` is common in Python. When both operands are known strings (ptr-tagged), emit a direct call to `molt_string_concat` instead of going through the generic `molt_add` dispatcher.

- **Expected impact**: 5-10% improvement on string-heavy code.
- **Effort**: Low (3 days). Add `fast_str` flag, emit direct call.

**P3: Monomorphic container operations.** When a container's element type is uniform (e.g., `list[int]`), specialize `list_append`, `list_getitem`, `dict_getitem` to skip type dispatch. For `list[int]`, store unboxed i64 values directly.

- **Expected impact**: 30-50% improvement on typed container operations (eliminates boxing/unboxing per element).
- **Effort**: Very high (1 month). Requires container type specialization in the type system and runtime.

---

## 9. Memory Access Patterns

### Current State

The NaN-boxed object model (`MoltObject`, defined in `molt-obj-model/src/lib.rs`) uses a 64-bit representation where inline values (int, bool, none) are encoded in the NaN payload and heap objects are encoded as tagged pointers.

Heap objects follow the runtime's object layout with a 40-byte header (`HEADER_SIZE_BYTES = 40`, lib.rs:36) containing reference count, type tag, and metadata. Field access goes through `molt_guarded_field_get`/`molt_guarded_field_set` runtime calls.

The data pool (`intern_data_segment`, lib.rs:1150-1169) deduplicates constant byte sequences (strings, bytes literals) via a `BTreeMap<Vec<u8>, DataId>`.

### Proposed Improvements

**P1: Inline field access for known-layout objects.** When the object type and field offset are known at compile time (e.g., class instances with `__slots__`), emit direct memory loads/stores instead of calling `molt_guarded_field_get`. The field offset can be computed as `header_size + field_index * 8`.

- **Expected impact**: 15-25% improvement on attribute-access-heavy code (eliminates function call + bounds check per field access).
- **Effort**: Medium (5 days). Requires plumbing layout info from frontend to backend.

**P2: Prefetch hints for sequential access patterns.** When iterating over a list or array, insert `prefetch` instructions to bring the next element into cache. Cranelift supports `heap_load` with prefetch hints.

- **Expected impact**: 5-10% improvement on large sequential iterations.
- **Effort**: Medium (4 days). Detect loop+index patterns, insert prefetch.

**P3: Object header compaction.** The 40-byte header is large relative to small objects (e.g., a 2-field object is 40 + 16 = 56 bytes). Consider a compact header (16 bytes: refcount + type tag) for objects that don't need the full metadata.

- **Expected impact**: 20-30% memory reduction for small objects, better cache density.
- **Effort**: High (2 weeks). Requires runtime changes to support dual header formats.

---

## 10. WASM-Specific Optimizations

### Current State

The WASM backend (`wasm.rs`, 9740 lines) generates wasm32 code using `wasm-encoder` 0.245.1. It uses:
- Linear memory (single memory, configurable size via `MOLT_WASM_MEMORY_PAGES`)
- Indirect function calls via `call_indirect` for dynamic dispatch
- A state-machine dispatch pattern for generators/coroutines using `br_table`
- Data segments for string constants and static data
- Relocation tables for linking (`RELOC_TABLE_BASE_DEFAULT = 4096`)

The backend does **not** currently use:
- Multi-value returns
- Reference types (`externref`, `funcref` in tables)
- Bulk memory operations (`memory.copy`, `memory.fill`)
- Tail calls (`return_call`)
- SIMD instructions

### Proposed Improvements

**P1: Bulk memory operations.** Replace manual byte-copy loops with `memory.copy` and `memory.fill`. These are widely supported (Chrome 75+, Firefox 79+, Node 15+) and execute as optimized memcpy/memset in the engine.

- **Expected impact**: 10-20% improvement on string/bytes operations in WASM.
- **Effort**: Low (2 days). Use `Instruction::MemoryCopy` and `Instruction::MemoryFill`.

**P2: Multi-value returns for tuple unpacking.** Python functions returning tuples currently box the tuple on the heap. With multi-value returns, small tuples (2-4 elements) can be returned directly on the WASM stack, avoiding heap allocation.

- **Expected impact**: 15-25% improvement on tuple-returning functions.
- **Effort**: Medium (5 days). Requires frontend to detect tuple returns, backend to emit multi-value return types.

**P3: Tail calls for recursive dispatch.** The WASM tail-call proposal (`return_call`, `return_call_indirect`) is now supported in Chrome 112+ and Firefox 121+. Use for self-recursive and mutually-recursive functions to prevent stack overflow in WASM.

- **Expected impact**: Enables deep recursion in WASM; eliminates stack overflow for tail-recursive programs.
- **Effort**: Medium (4 days). Detect tail position, emit `Instruction::ReturnCall`.

**P4: Reference types for function tables.** Currently function pointers are stored as integer indices. Using `funcref` tables with reference types enables the WASM engine to optimize indirect calls (type-checking at compile time instead of runtime).

- **Expected impact**: 5-10% improvement on indirect-call-heavy WASM code.
- **Effort**: Medium (5 days). Migrate function table to `funcref`, update call_indirect sites.

**P5: State-machine remap table compression.** `STATE_REMAP_TABLE_MAX_ENTRIES = 4096` and sparsity limit of 8 (wasm.rs:35-36) control generator/coroutine state dispatch tables. For generators with few states, emit a direct `br_table` instead of the remap indirection.

- **Expected impact**: 2-5% improvement on generator-heavy WASM code.
- **Effort**: Low (2 days).

---

## 11. Profile-Guided Optimization (PGO)

### Current State

The backend has basic PGO infrastructure:

1. **`PgoProfileIR`** (lib.rs:208-213): Contains `hot_functions: Vec<String>` — a list of function names ordered by hotness.
2. **`apply_profile_order`** (lib.rs:754-781): Reorders functions in the compilation unit so hot functions appear first in the object file. This improves code locality.
3. **Profile field in `SimpleIR`** (lib.rs:219): `profile: Option<PgoProfileIR>`.

There is no runtime profiling infrastructure to collect profiles. The `hot_functions` list must be manually provided.

### Proposed Improvements

**P1: Differential test profile collection.** The differential test suite (`tests/molt_diff.py`) already measures RSS and execution time. Extend it to collect function-call frequency data:

1. Add a `MOLT_PROFILE_COLLECT=1` mode that instruments compiled binaries with lightweight entry counters (atomic increment at function entry).
2. After execution, dump `{function_name: call_count}` to a JSON file.
3. Aggregate profiles across the full differential suite into a `pgo_profile.json`.
4. Feed the aggregated profile back via `SimpleIR.profile`.

Implementation sketch:
```
# In the build pipeline:
MOLT_PROFILE_COLLECT=1 python -m molt.cli build --profile dev tests/differential/basic/core_types
./output                     # writes /tmp/molt_profile_<pid>.json
python tools/pgo_aggregate.py /tmp/molt_profile_*.json > pgo_profile.json

# In subsequent builds:
MOLT_PGO_PROFILE=pgo_profile.json python -m molt.cli build --profile release app.py
```

- **Expected impact**: 5-15% improvement on real workloads through better function ordering, block layout, and IC warming.
- **Effort**: Medium (1 week). Entry counter instrumentation in backend, aggregation tool, CLI integration.

**P2: Branch frequency profiling.** Extend profile collection to record branch taken/not-taken counts. Feed this into:
- `set_cold_block` decisions (instead of static heuristics)
- Cranelift's block frequency annotations (when available)
- IC site hotness (identify megamorphic sites that need PIC upgrade)

- **Expected impact**: Additional 3-8% on top of function-level PGO.
- **Effort**: High (2 weeks). Requires per-branch instrumentation, which increases code size during profiling.

**P3: PGO-guided inlining decisions.** Currently `inline_functions` uses a static op-count threshold (30 ops). With call-frequency data, inline hot callee/caller pairs regardless of size (up to a higher limit), and skip inlining cold call sites even if the callee is small.

- **Expected impact**: 5-10% improvement (better inlining decisions reduce call overhead where it matters).
- **Effort**: Medium (4 days). Thread call-count data into `is_inlineable` and `inline_functions`.

**P4: PGO-guided specialization.** Combine IC statistics with PGO profiles to identify sites where type specialization would help most. For example, if a call site dispatches to `int.__add__` 99% of the time, generate a type-specialized fast path.

- **Expected impact**: 10-20% improvement on hot dispatch sites.
- **Effort**: High (2 weeks). Requires IC stats collection, profile-aware specialization pass.

---

## Priority Matrix

| # | Area | Improvement | Impact | Effort | Priority |
|---|------|-------------|--------|--------|----------|
| 2.1 | Calling | Signature deduplication | 5-10% compile | Low | **P0** |
| 2.2 | Calling | Intrinsic FuncId cache | 3-5% compile | Low | **P0** |
| 4.1 | Isel | Fused tag-check-and-unbox | 10-15% runtime | Medium | **P0** |
| 2.4 | Calling | Inline ref-count ops | 10-15% runtime | Medium | **P0** |
| 8.1 | Specialization | Float fast path | 20-30% runtime | Medium | **P1** |
| 7.1 | Escape | Stack allocation for non-escaping | 20-40% runtime | High | **P1** |
| 9.1 | Memory | Inline field access | 15-25% runtime | Medium | **P1** |
| 6.1 | IC | Polymorphic inline caches | 10-20% runtime | High | **P1** |
| 11.1 | PGO | Diff test profile collection | 5-15% runtime | Medium | **P1** |
| 10.1 | WASM | Bulk memory operations | 10-20% WASM | Low | **P1** |
| 1.2 | Flags | Per-function opt levels | 3-8% runtime | Medium | **P2** |
| 5.1 | Branch | Systematic cold-block marking | 2-5% runtime | Low | **P2** |
| 5.3 | Branch | Elif chain to Switch | 5-10% runtime | Medium | **P2** |
| 3.1 | Regalloc | Reduce fast_int variable pressure | 5-8% runtime | Medium | **P2** |
| 10.2 | WASM | Multi-value returns | 15-25% WASM | Medium | **P2** |
| 10.3 | WASM | Tail calls | correctness | Medium | **P2** |
| 2.3 | Calling | Tail call optimization | correctness | Medium | **P2** |
| 11.2 | PGO | Branch frequency profiling | 3-8% runtime | High | **P3** |
| 7.2 | Escape | Scalar replacement | 10-20% runtime | High | **P3** |
| 8.3 | Specialization | Monomorphic containers | 30-50% runtime | Very High | **P3** |
| 6.2 | IC | IC stub codegen | 5-10% runtime | High | **P3** |

---

## Implementation Phases

### Phase 1: Low-Hanging Fruit (2 weeks)
- Signature deduplication (2.1)
- Intrinsic FuncId cache (2.2)
- Fused tag-check-and-unbox (4.1)
- Inline ref-count tag check (2.4)
- Systematic cold-block marking (5.1)
- Bulk memory ops for WASM (10.1)

### Phase 2: Type Specialization (3 weeks)
- Float fast path (8.1)
- String concatenation specialization (8.2)
- Inline field access for known layouts (9.1)
- PGO profile collection infrastructure (11.1)

### Phase 3: Escape Analysis & ICs (4 weeks)
- Stack allocation for non-escaping objects (7.1)
- Polymorphic inline caches (6.1)
- PGO-guided inlining decisions (11.3)
- Multi-value WASM returns (10.2)

### Phase 4: Advanced (ongoing)
- Scalar replacement of aggregates (7.2)
- Monomorphic containers (8.3)
- IC stub codegen (6.2)
- Branch frequency profiling (11.2)

---

## Measurement Protocol

All optimizations must be validated against:

1. **Correctness**: Full differential test sweep (`tests/molt_diff.py --jobs 8 tests/differential/`) — zero regressions.
2. **Compile time**: Measure with `time` on a fixed set of 10 representative programs. Target: no more than 5% compile-time regression per optimization.
3. **Runtime performance**: Benchmark suite (`tools/bench.py --json-out`). Report absolute ns and relative change.
4. **Code size**: Measure `.text` section size of compiled objects. Report bytes and relative change.
5. **RSS**: Differential tests with `MOLT_DIFF_MEASURE_RSS=1`. Target: no increase.

Results must be recorded in `bench/results/` with the optimization flag and date.
