# Native Backend Optimization Audit

Full pipeline audit: Python source to native binary. Each stage identifies
concrete bottlenecks, missing optimizations, and estimated impact.

---

## 1. Frontend — Python AST Parsing and Desugaring

**Entry point**: `src/molt/cli.py` line 7439 (`ast.parse`), feeds into
`src/molt/frontend/__init__.py` class `SimpleTIRGenerator` (line 886).

**How it works**: Python's built-in `ast.parse` produces a CPython AST.
`SimpleTIRGenerator` is an `ast.NodeVisitor` that walks the tree and emits
a flat list of `MoltOp` objects (the "TIR" — Typed IR). There is no
separate HIR stage; desugaring happens inline during the AST visit. The
generator is ~35,000 lines covering all supported Python constructs.

**Supported features**: Full control flow (if/elif/else, for/while/break/
continue, try/except/finally, with), classes with single inheritance,
closures, generators, async/await, comprehensions, f-strings, walrus
operator, match/case, decorators, `*args`/`**kwargs`, type annotations
(advisory).

**Bottlenecks and optimization opportunities**:

| Issue | Location | Impact | Effort |
|-------|----------|--------|--------|
| **Single-pass visitor generates redundant ops**. For example, every `ast.Name` load produces a fresh `MoltValue` even when the same variable was just loaded on the previous op. No CSE at the AST-walk level. | `SimpleTIRGenerator.visit_Name` | Medium — downstream midend CSE catches some, but frontend dedup would reduce op count 5-10% for expression-heavy code | Medium |
| **No AST-level constant folding**. `2 + 3` emits `const(2), const(3), add` and relies on SCCP to fold. Python's `ast` module provides `ast.literal_eval` infrastructure that could pre-fold pure-constant expressions before TIR generation. | Frontend visitor arithmetic handlers | Low-Medium — SCCP handles this well, but skipping the op emission entirely saves midend time | Low |
| **Module-level code treated identically to function code**. Module initialization (`molt_main`) emits the same op sequences as function bodies, but module-level code is executed exactly once. This means SCCP/DCE budget is spent on code that cannot benefit from loop optimizations. | `_run_ir_midend_passes` line 29451 — stdlib modules skip midend entirely | Low | Low |
| **No inter-module type propagation**. `TypeFacts` (line 976) provides cross-module type hints but is only loaded when `--type-facts` is passed. Default builds have no inter-module type information, preventing specialization of imported function calls. | `type_facts` parameter, line 893 | High — inter-module specialization could eliminate many dynamic dispatches in real programs | High |

---

## 2. TIR Generation (there is no separate HIR)

The architecture documentation references `HIR -> TIR -> LIR` but the actual
implementation has only one IR level: the flat `MoltOp` list produced by
`SimpleTIRGenerator`. There is no explicit HIR or LIR data structure.

**IR format**: Each `MoltOp` has `kind: str`, `args: list[Any]`,
`result: MoltValue`, `metadata: dict`. This is a simple linear IR with
structured control flow markers (`IF`/`ELSE`/`END_IF`, `LOOP_START`/
`LOOP_END`, `TRY_START`/`TRY_END`).

**Key design properties**:
- **NaN-boxed value model**: All values are i64 — ints, bools, None are inline-tagged;
  floats are IEEE 754 doubles; pointers use the TAG_PTR tag. Defined in
  `runtime/molt-backend/src/lib.rs` lines 18-28.
- **Type hints travel as string annotations** on `MoltValue.type_hint` (line 36).
  The backend uses `fast_int: Option<bool>` (line 240) to enable inline
  integer fast-paths.

**Bottlenecks**:

| Issue | Location | Impact | Effort |
|-------|----------|--------|--------|
| **String-typed IR**. Op kinds, variable names, and type hints are all strings. The backend deserializes JSON and matches on `op.kind.as_str()` in a massive match statement (~12,000 lines). An enum-based IR would eliminate string allocation/comparison overhead in the backend. | `OpIR` struct (lib.rs:230), `match op.kind.as_str()` (lib.rs:1949) | Medium — backend compile time, not runtime | High |
| **JSON serialization between frontend and backend**. The TIR is serialized to JSON via `json.dumps(ir)` (cli.py:9597), piped to the backend process via stdin, then deserialized with serde. For large programs (thousands of functions), this serialization overhead is significant. The daemon mode helps but still uses JSON. | `_compile_with_backend_daemon` (cli.py:5425) | Medium — 100-500ms for large programs | Medium |
| **No SSA form in the TIR**. Variables can be reassigned; the backend builds Cranelift SSA from the variable-based IR. A pre-SSA conversion in the frontend would enable more aggressive optimizations. | `def_var_named` / `builder.def_var` usage throughout lib.rs | Low — Cranelift handles SSA construction well via `FunctionBuilder` | High |

---

## 3. Type Inference and Specialization

**Type system**: `src/molt/type_facts.py` defines `TypeFacts` — a map from
module/function/variable to type strings with trust levels (`advisory`,
`guarded`, `trusted`). The type system is entirely optional and advisory by
default.

**Specialization mechanism**: The frontend sets `fast_int: true` on arithmetic
ops when it can prove both operands are integers (via type hints, literal
analysis, or loop induction variables). This is the **only** specialization
path.

**How `fast_int` works** (backend lib.rs lines 2102-2190 for `add`):
- `fast_int=true`: Skip the tag check; directly unbox both operands as ints,
  compute inline `iadd`, check overflow, fall back to `molt_add` runtime call
  on overflow. Saves ~3-5ns per operation.
- `fast_int=false` (default): Emit tag checks (`is_int_tag`) for both operands,
  branch to fast inline path if both are ints, else call `molt_add`.

**Bottlenecks and opportunities**:

| Issue | Location | Impact | Effort |
|-------|----------|--------|--------|
| **No float specialization**. There is no `fast_float` path. Float arithmetic always dispatches through `molt_add`/`molt_mul`/etc even when both operands are known floats. The NaN-boxing makes this especially wasteful since float values are the "naked" representation — no tag extraction needed. | All arithmetic ops in lib.rs | **High** — scientific/numerical code takes a ~2-3x penalty vs C for pure float loops | Medium |
| **No container element type specialization**. `container_elem_hints` and `dict_key_hints` are tracked (frontend line 963-965) but not used for specialization. A list known to contain only ints could use a packed representation. | Frontend type tracking, backend container ops | Medium-High for numeric workloads | High |
| **No return type propagation across calls**. If `def foo() -> int` is annotated, callers of `foo()` don't get `fast_int` on the result. The type facts system supports this but it's not wired to the call site specialization. | `FunctionFacts.returns` (type_facts.py:24), call lowering in frontend | Medium | Medium |
| **Loop induction variable analysis is limited**. `_analyze_loop_bound_facts` and `_analyze_affine_loop_compare_truth` (frontend line 31852-31853) analyze simple `range()` loops, but don't propagate int types through loop body operations. | SCCP, `LoopBoundFact` dataclass (line 61) | Medium — loop-heavy code misses specialization | Medium |
| **No speculative devirtualization**. Method calls on known classes could be statically dispatched, but the frontend always emits dynamic `get_attr` + `call` sequences. | Frontend method call lowering | High — class-heavy code pays full dynamic dispatch cost | High |

---

## 4. Midend Optimizations (SCCP, DCE, CSE, LICM, Edge Threading)

**Entry point**: `_run_ir_midend_passes` (frontend line 29436) invokes
`_canonicalize_control_aware_ops` (line 35116), which runs the fixed-point
optimization loop in `_canonicalize_control_aware_ops_impl` (line 34342).

**Optimization pipeline** (per round, up to `max_rounds`):
1. **Structural simplification** (`_canonicalize_structured_regions_pre_sccp`) — removes trivially dead structured regions
2. **SCCP** (`_compute_sccp`, line 31832) — sparse conditional constant propagation with lattice merge
3. **Phi trimming** (`_trim_phi_args_by_executable_edges`) — removes dead phi inputs
4. **Branch pruning** (`_rewrite_structured_if_regions`) — eliminates statically resolved if/else
5. **Edge threading** (`_rewrite_loop_try_edge_threading`) — threads predictable loop/try edges
6. **GVN/CSE** — global value numbering and common subexpression elimination
7. **LICM** — loop-invariant code motion
8. **Guard hoisting** — moves type guards out of loops
9. **DCE** — dead code elimination

**Tiered execution** (line 29585): Functions are classified into tiers A/B/C
based on module, function name, op count, and PGO hot function lists. Tier A
gets full optimization (deep edge threading, CSE, LICM, guard hoisting).
Tier C gets minimal optimization. Budget-based degradation (line 34486)
progressively disables expensive passes when time budget is exceeded.

**Quality assessment**:

| Pass | Status | Quality | Issues |
|------|--------|---------|--------|
| **SCCP** | Implemented | Good | Handles constants, booleans, comparisons, type tags, and loop bounds. Does NOT handle heap values (container contents, attribute loads) — these go to `_SCCP_OVERDEFINED` immediately. Missing-value (`_SCCP_MISSING`) handling is correct and well-tested. |
| **Branch pruning** | Implemented | Good | Correctly prunes statically resolved if/else and elif chains. Respects structured control flow. |
| **Edge threading** | Implemented | Good | Threads loop breaks and try/except edges when SCCP proves them dead. Gated behind Tier A for safety. |
| **CSE** | Implemented | Fair | Tracks stats (`cse_attempted`/`cse_accepted`) but limited to pure operations. Does not CSE across heap-reading operations (loads, attribute accesses). `cse_readheap_attempted`/`cse_readheap_rejected` stats (line 29495-29497) suggest readheap CSE is attempted but mostly rejected. |
| **LICM** | Implemented | Fair | Hoists loop-invariant pure operations. Does not hoist loads because it lacks alias analysis at the TIR level. |
| **Guard hoisting** | Implemented | Fair | Hoists type checks (`isinstance`, tag checks) out of loops. Stats suggest moderate acceptance rate. |
| **DCE** | Implemented | Good | Removes unused value-producing ops. Pure-op DCE attempts separate accounting. |
| **GVN** | Implemented | Fair | Tracks `gvn_attempted`/`gvn_accepted` but appears lightweight — no hash-consing or full value numbering. |

**Bottlenecks**:

| Issue | Location | Impact | Effort |
|-------|----------|--------|--------|
| **No heap-aware SCCP**. Any operation that reads from or writes to the heap (attribute access, subscript, container mutation) immediately makes the result overdefined. This prevents constant-folding through `self.x` patterns, `dict[key]` lookups with known keys, etc. | `eval_lattice_value` in SCCP (line 31940+) | High — class-heavy code gets almost no constant propagation | High |
| **No escape analysis**. Objects that don't escape the current function could be stack-allocated. The backend has `elide_dead_struct_allocs` (lib.rs:480) but it only catches completely unused allocations, not escape-bounded ones. | Backend `elide_dead_struct_allocs` | Medium-High — eliminates heap allocation + refcount overhead for temporaries | High |
| **Stdlib modules skip midend entirely**. Line 29451: `if self._source_is_stdlib_module: return ops`. Stdlib wrappers get zero optimization. This is intentional for correctness but means stdlib call-heavy code sees unoptimized call sequences. | `_run_ir_midend_passes` line 29451 | Low-Medium — stdlib is mostly thin wrappers over intrinsics | Low |
| **Fixed-point convergence can degrade**. The budget system (line 34486) progressively disables passes (CSE, edge threading, guard hoisting, LICM) when time budget is exceeded. For Tier B/C functions, this means most optimizations never run. | `maybe_apply_budget_degrade` | Medium | Low |
| **No strength reduction**. Multiplications by powers of 2 are not converted to shifts. Modular arithmetic patterns are not recognized. Division by constants is not converted to multiply-high. | Not implemented | Low-Medium — Cranelift handles some of this internally | Medium |

---

## 5. Backend IR Lowering (TIR to Cranelift CLIF)

**Entry point**: `NativeCompiler::compile` (lib.rs:1171) processes the `SimpleIR`
(deserialized JSON) through three pre-passes then calls `compile_func` per function.

**Pre-passes** (lib.rs:1171-1177):
1. `apply_profile_order` — reorders functions based on PGO profile for better code locality
2. `elide_dead_struct_allocs` — removes unused class allocations and their stores
3. `inline_functions` — inlines small leaf functions (<=30 ops by default)

**Inlining** (lib.rs:587): Limited to `call_internal` sites where the callee
has <=`INLINE_OP_LIMIT` (30) ops, no control flow, no nested internal calls.
Variable renaming uses a prefix scheme. Does NOT inline across module
boundaries or inline functions with loops.

**Codegen structure** (lib.rs:1631+): `compile_func` builds Cranelift IR via
`FunctionBuilder`. It processes each `OpIR` in sequence with a massive
`match op.kind.as_str()` dispatch (~12,000 lines covering ~200 op kinds).
For each op:
- Constants become `iconst` with NaN-boxed values
- Arithmetic ops emit inline fast-paths (tag check, unbox, compute, rebox,
  overflow check, fallback to runtime call)
- All function calls go through `declare_function` + `call` — runtime functions
  are imported symbols
- Structured control flow (if/loop/try) maps to Cranelift blocks with
  explicit `brif`/`jump`/`switch_to_block`

**Cranelift settings** (lib.rs:1008-1131):
- `opt_level = "speed"` (always)
- `regalloc_algorithm`: `backtracking` (release), `single_pass` (dev)
- `enable_alias_analysis = true`
- `use_colocated_libcalls = true` (direct PC-relative calls)
- `enable_heap_access_spectre_mitigation = false`
- `enable_table_access_spectre_mitigation = false`
- `probestack_strategy = "inline"`
- `preserve_frame_pointers`: true (debug), false (release)
- Host CPU feature detection enabled by default (AVX2, BMI2, POPCNT, etc.)

**Bottlenecks**:

| Issue | Location | Impact | Effort |
|-------|----------|--------|--------|
| **Redundant function signature declarations**. Every op that calls a runtime function re-declares its signature via `module.make_signature()` + `declare_function`. For the same runtime function called 100 times in one function, this creates 100 identical signature objects. Should cache `FuncRef` per runtime function name per function. | Throughout `compile_func` match arms | Medium — compile-time overhead, ~15-25% of backend time for call-heavy programs | Medium |
| **No peephole optimization between adjacent ops**. Each op is lowered independently. Patterns like `unbox_int(box_int_value(x))` (identity) across adjacent ops are not recognized. The NaN-boxing creates many box/unbox pairs that cancel. | Sequential op processing in compile_func | Medium — unnecessary instructions in hot loops | High |
| **Inline int overflow check is expensive**. The `fast_int` path for `add` (lib.rs:2106-2142) emits: unbox, iadd, box, unbox-roundtrip comparison, brif (3 blocks). This is ~8 instructions for what could be a single `iadd` + `jo` on x86. | `fast_int` arithmetic lowering, all ops | Medium — hot arithmetic loops carry ~4 unnecessary instructions per op | Medium |
| **No inline caching in the backend**. The frontend emits IC site IDs (`stable_ic_site_id`, lib.rs:123) for attribute access and method dispatch, but the backend emits them as opaque constants. Inline cache stubs are handled entirely at the runtime level via function calls. A JIT-style IC stub could avoid the call overhead. | IC-related ops | Medium-High for attribute-heavy code | Very High |
| **`elide_dead_struct_allocs` is conservative**. Only removes allocations where ALL uses are `store`/`store_init`/`guarded_field_set`/`guarded_field_init`/`object_set_class` at position 0. Any other use (including reading) prevents elision. | lib.rs:480-543 | Low — catches only fully dead allocations, not partially used ones | Medium |
| **Inlining limit of 30 ops is very conservative**. Many useful small functions (property getters, simple math wrappers, one-liner methods) are 30-80 ops due to error handling and type checking overhead. | `INLINE_OP_LIMIT` (lib.rs:555) | Medium — larger inline limit with cost-benefit analysis would help | Low |

---

## 6. Cranelift Codegen Quality

**CLIF IR quality**: The generated CLIF is correct but verbose due to the
NaN-boxing scheme. Every value operation requires tag/unbox/compute/rebox
sequences. Cranelift's own optimization passes (GVN, alias analysis,
dead code elimination) clean up some redundancy.

**Cranelift vs LLVM gaps**:

| Optimization | Cranelift Status | LLVM Status | Impact |
|-------------|-----------------|-------------|--------|
| **Loop unrolling** | Not implemented | Full support | Medium — Cranelift doesn't unroll, relies on branch prediction |
| **Auto-vectorization** | Not implemented | SLP + loop vectorization | Low for Molt (NaN-boxed values prevent vectorization) |
| **Instruction scheduling** | Basic | Full | Low-Medium — matters for deeply pipelined CPUs |
| **Register coalescing** | Backtracking allocator handles well | Full coalescing | Low — Cranelift's allocator is competitive |
| **Tail call optimization** | Supported via `return_call` | Full TCO | Available but not used — Molt doesn't emit `return_call` |
| **Function outlining** | Not implemented | MachineOutliner | Low — reduces code size but Molt binaries are already compact |
| **Profile-guided layout** | `set_cold_block` used (lib.rs:2118) | Full PGO | Medium — cold blocks are marked but no hot-path straightening |
| **Interprocedural optimization** | None (AOT object model) | Full LTO + IPA | High — the biggest gap; runtime calls can't be inlined |

**Specific Cranelift tuning opportunities**:

| Opportunity | Details | Impact |
|-------------|---------|--------|
| **Enable `egraph_simplify`** | E-graph-based algebraic simplification exists (lib.rs line 16, `egraph_simplify.rs`) but is gated behind `#[cfg(feature = "egraphs")]` and NOT wired into the pipeline (comment: "Prototype. Not wired into the compilation pipeline"). | Low-Medium |
| **Use `cranelift_frontend::Switch` for method dispatch** | Currently imported (lib.rs:4) but underutilized. Large if/elif chains for type dispatch could use jump tables. | Low |
| **Stack slot coalescing** | Each `const_str` and `const_bytes` allocates a separate `ExplicitSlot` (lib.rs:2048-2052). Reusing slots for non-overlapping lifetimes would reduce stack frame size. | Low |
| **Tail call for self-recursive functions** | Cranelift supports `return_call` but Molt never emits it. Simple self-recursion (common in recursive algorithms) could be converted. | Low-Medium |

---

## 7. Linking

**Linker invocation** (cli.py lines 9843-10055):

The final binary is produced by linking three components:
1. `main_stub.c` — a generated C file with `main()` that calls `molt_runtime_init()`,
   `molt_main()`, and handles exceptions (cli.py:9871-9952)
2. `output.o` — the Cranelift-generated object file containing all compiled functions
3. `libmolt_runtime.a` — the static Rust runtime library

Linking uses the system C compiler (`CC` env var, default `clang`). For
cross-compilation, Zig CC or `MOLT_CROSS_CC` is used.

**Dev-profile linker acceleration**: `_resolve_dev_linker` (referenced at cli.py:10015)
selects a fast linker (`mold`, `lld`, `sold`) for `--profile dev` builds.
Falls back to default linker if the fast linker fails (line 10057-10073).

**Bottlenecks**:

| Issue | Location | Impact | Effort |
|-------|----------|--------|--------|
| **No `-dead_strip` / `--gc-sections` at link time**. The link command (cli.py:10029-10031) does not pass `-Wl,-dead_strip` (macOS) or `-Wl,--gc-sections` (Linux). Unused runtime functions remain in the final binary. | Link command construction, cli.py:10029 | **Medium** — can reduce binary size 10-30% for small programs that use few runtime features | **Low** |
| **No LTO across Rust runtime and compiled code**. The runtime is precompiled as `libmolt_runtime.a` and the compiled code is a separate `.o` file. Link-time optimization cannot inline runtime helper functions into compiled code. | Architecture: separate Cranelift object + Rust static lib | **High** — the biggest single optimization gap. Hot runtime functions like `molt_add`, `molt_get_attr`, `molt_dec_ref` are called millions of times but can never be inlined. | **Very High** |
| **No section ordering / code layout optimization**. Functions are placed in the order they appear in the IR. No profile-guided function reordering at the object level (though `apply_profile_order` in lib.rs:754 reorders functions in the IR based on PGO hot lists). | Object emission via `ObjectModule::finish` (lib.rs:1290) | Low-Medium | Medium |
| **Static linking only**. The runtime is always statically linked. For applications with multiple Molt-compiled modules, each binary includes a full copy of the runtime. Shared library support would reduce aggregate disk usage. | Architecture decision | Low | High |
| **No CFI / control flow integrity**. The link command doesn't enable `-fsanitize=cfi` or similar. Not a performance issue but a security hardening gap. | Link command construction | N/A (security) | Medium |

---

## 8. Binary Size

**Cargo release profile** (Cargo.toml lines 12-17):
```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

This is well-configured: full LTO, single codegen unit (maximum optimization),
panic=abort (no unwinding tables), strip=true (no debug symbols). The Rust
runtime itself is optimized and stripped.

**Binary composition** (approximate, for a "hello world"):
- Runtime (`libmolt_runtime.a`): ~2-4 MB (contains all intrinsics, object model,
  GC, async runtime, scheduler, all builtin type implementations)
- Compiled code (`output.o`): ~10-100 KB depending on program size
- C runtime overhead: ~1-2 KB (main stub)
- Total: ~2-4 MB minimum

**Size optimization opportunities**:

| Opportunity | Details | Impact |
|-------------|---------|--------|
| **Link-time dead code elimination** | Adding `-Wl,-dead_strip` (macOS) or `-Wl,--gc-sections` + compiling with `-ffunction-sections -fdata-sections` could strip unused runtime functions. A "hello world" likely uses <10% of the runtime. | **High** — could reduce binary from ~3 MB to ~500 KB for simple programs |
| **Compile runtime with `#[cfg]` feature gates** | The runtime includes database connectors, HTTP, WebSocket, GUI, CSV, argparse, and many other subsystems. Feature-gating unused subsystems at compile time would be more effective than link-time stripping. | High — requires Cargo feature refactoring |
| **Data segment deduplication** | `intern_data_segment` (lib.rs:1150) already deduplicates identical byte strings. This is well-implemented. | Already done |
| **Compressed sections** | Object files could use compressed debug sections (zstd). Not relevant for stripped release builds. | N/A for release |
| **`panic = "abort"` in the compiled code** | Already set for the runtime via Cargo.toml. The compiled `.o` file doesn't generate Rust panic paths. | Already done |

---

## Summary: Top 10 Optimization Opportunities (by estimated impact)

| # | Opportunity | Stage | Est. Runtime Impact | Est. Effort |
|---|------------|-------|--------------------:|-------------|
| 1 | **Cross-boundary LTO / runtime inlining** | Linking | 20-40% for call-heavy code | Very High |
| 2 | **Float specialization (`fast_float` path)** | Type inference + Backend | 2-3x for float-heavy code | Medium |
| 3 | **Link-time dead code elimination** | Linking + Binary size | 50-70% binary size reduction | Low |
| 4 | **Inter-module type propagation** | Type inference | 10-20% fewer dynamic dispatches | High |
| 5 | **Heap-aware SCCP** | Midend | 10-15% for class-heavy code | High |
| 6 | **Speculative devirtualization** | Frontend + Backend | 10-15% for class-heavy code | High |
| 7 | **Cache runtime function refs in backend** | Backend compile time | 15-25% faster compilation | Medium |
| 8 | **Escape analysis + stack allocation** | Midend + Backend | 5-10% for allocation-heavy code | High |
| 9 | **Return type propagation to callers** | Type inference | 5-10% fewer runtime dispatches | Medium |
| 10 | **Inlining threshold increase to ~80 ops** | Backend | 3-5% fewer call overhead | Low |

---

## Appendix: Key File Reference

| File | Lines | Role |
|------|------:|------|
| `src/molt/frontend/__init__.py` | 35,233 | AST visitor, TIR generation, midend optimizations (SCCP, CSE, LICM, DCE, edge threading) |
| `src/molt/frontend/cfg_analysis.py` | 349 | CFG construction, dominator computation |
| `src/molt/type_facts.py` | ~200 | Type facts schema and filtering |
| `src/molt/cli.py` | 16,684 | Build orchestration, linking, caching |
| `runtime/molt-backend/src/lib.rs` | 14,106 | Cranelift codegen, NaN-boxing, function inlining, struct elision |
| `runtime/molt-backend/src/ir_schema.rs` | 72 | Op field validation (minimal) |
| `runtime/molt-backend/src/egraph_simplify.rs` | ~120 | E-graph simplification prototype (not wired in) |
| `runtime/molt-backend/src/wasm.rs` | (varies) | WASM backend codegen |
| `Cargo.toml` | 42 | Build profiles (release, dev-fast, release-fast, wasm-release) |
