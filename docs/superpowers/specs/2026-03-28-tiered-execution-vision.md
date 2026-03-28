# Monty-Molt Tiered Execution: Architectural Vision

**Status:** North Star Design Document
**Date:** 2026-03-28
**Authors:** Molt Core Team

---

## 1. Executive Summary

The Python edge ecosystem has a missing piece: a runtime that handles both cold-start latency and sustained throughput without compromise. Today, interpreters pay for startup speed with execution overhead; AOT compilers pay for throughput with compile latency. Neither alone is sufficient for serverless edge deployments where a request handler may run once (cold import of a rarely-hit endpoint) or millions of times (hot JSON serialization loop).

This document describes a **V8-style tiered execution architecture** that unifies two complementary Python runtimes:

- **Monty** (Pydantic's bytecode interpreter in Rust): sub-microsecond startup, snapshot/resume, resource tracking built into the eval loop. Interprets cold and one-shot code with zero compilation overhead.
- **Molt** (AOT compiler, Cranelift + wasm-encoder): compiles hot paths through a TIR optimization pipeline (SCCP, escape analysis, type guard hoisting, unboxing, BCE, refcount elision, strength reduction, DCE) to native code or WASM at 10-100x interpreter speed.

The two runtimes share a **contract surface** -- a capability manifest (`molt.capabilities.toml`), type stubs, exception semantics, resource error behavior, and a conformance test suite -- so that tier-up from interpreted to compiled is invisible to user code. A function starts life in Monty's eval loop; when call-count thresholds are crossed, Molt compiles it in the background; an atomic pointer swap replaces the interpreted entry point with the compiled one. The user program never observes the transition.

This is not theoretical. The shared capability manifest already exists (`molt.capabilities.toml` v2.0 with a `[monty]` section), the `ResourceTracker` trait and `AuditSink` are implemented in `molt-runtime`, Molt's TIR pipeline has 16 optimization passes with deoptimization infrastructure, and the compilation cache (`tir/cache.rs`) provides content-addressed artifact storage. What remains is the plumbing between the two runtimes: call counters, background compilation, and the atomic swap.

---

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                    Tiered Python Runtime                             │
│                                                                      │
│  Tier 0: Monty Interpreter (cold code)                              │
│    - <1us startup, register-based bytecode VM                       │
│    - Full snapshot/resume: freeze mid-execution, restore later      │
│    - ResourceTracker woven into eval loop (every N instructions)    │
│    - Yields at external call boundaries (cooperative scheduling)    │
│    - Type profiling: records argument types for each call site      │
│                                                                      │
│  Tier 1: Molt AOT Compiler (hot paths)                              │
│    - Frontend: Python AST -> SimpleIR -> SSA-form TIR               │
│    - Optimizer: 16-pass TIR pipeline (see Section 2.1)              │
│    - Backend: Cranelift -> native | wasm-encoder -> WASM            │
│    - Pre-emptive DoS guards: compile-time elision of pow/repeat/    │
│      shift overflow checks when operands are provably bounded       │
│    - Deoptimization: speculative type assumptions with fallback     │
│      to generic code on guard failure (tir/deopt.rs)                │
│                                                                      │
│  Tier-Up Controller                                                  │
│    - Per-function call counters (atomic u32, cache-line aligned)    │
│    - Background compilation on dedicated thread/worker              │
│    - Content-addressed compilation cache (tir/cache.rs)             │
│    - Atomic swap: interpreted entry -> compiled entry               │
│    - Shared ResourceTracker + AuditSink across both tiers           │
│                                                                      │
│  Shared Contracts                                                    │
│    - molt.capabilities.toml (v2.0, [monty] section)                │
│    - Type stubs (stubs/ directory, shared type vocabulary)           │
│    - Python subset: no exec/eval/compile, no unrestricted reflect   │
│    - Conformance test suite (parity/ directory)                     │
│    - Serialized TIR function signatures (tir/serialize.rs)          │
└──────────────────────────────────────────────────────────────────────┘
```

### 2.1 The TIR Optimization Pipeline

Molt's Typed Intermediate Representation is the compilation backbone. Functions are lowered from SimpleIR through SSA construction (`tir/ssa.rs`), then refined through a configurable pass pipeline (`tir/passes/mod.rs`):

```
                    SimpleIR (stack-machine ops)
                           │
                    ┌──────▼──────┐
                    │ SSA Construction │
                    │ (lower_from_simple) │
                    └──────┬──────┘
                           │
               ┌───────────▼───────────┐
               │   Type Refinement     │  DynBox -> I64/F64/Bool/Str
               │   (type_refine.rs)    │  via forward dataflow
               └───────────┬───────────┘
                           │
            ┌──────────────▼──────────────┐
            │      TIR Pass Pipeline      │
            │                              │
            │  1. Unboxing                │  NaN-box elimination
            │  2. Escape Analysis         │  Stack-allocate non-escaping objects
            │  3. Refcount Elimination    │  Elide provably-dead inc/dec pairs
            │  4. Type Guard Hoisting     │  Hoist loop-invariant type checks
            │  5. SCCP                    │  Sparse conditional constant prop
            │  6. Strength Reduction      │  Algebraic simplification
            │  7. Bounds Check Elim       │  Prove index-in-range
            │  8. Dead Code Elimination   │  Remove unreachable ops
            │                              │
            │  (+ CHA, closure spec,      │
            │   monomorphize, vectorize,  │
            │   deforestation, fast-math, │
            │   interprocedural, polyhedral│
            │   when applicable)          │
            └──────────────┬──────────────┘
                           │
              ┌────────────▼────────────┐
              │ Backend Code Generation │
              ├────────────┬────────────┤
              │ Cranelift   │ wasm-encoder│
              │ (native)    │ (WASM)     │
              └────────────┴────────────┘
```

The key insight for tiered execution: **Monty's type profiling data feeds directly into Molt's type refinement.** When Monty observes that `def process(items)` has been called 100 times and `items` was always `list[dict[str, int]]`, that type profile becomes a speculative type assumption in TIR. The unboxing pass can then eliminate NaN-boxing for the entire function body. If the assumption is ever violated, the deoptimization framework (`tir/deopt.rs`) transfers control back to Monty with materialized live values.

### 2.2 The Type System Bridge

Molt's TIR type system (`tir/types.rs`) is designed for exactly this kind of progressive refinement:

```
TirType lattice:

    DynBox            (top: unknown type, NaN-boxed)
       │
   ┌───┼───┬───┬───┐
   │   │   │   │   │
  I64 F64 Bool Str None    (unboxed scalars)
   │   │   │   │   │
  List Dict Set Tuple Func  (reference types, parameterized)
   │
  Union(a, b, c)           (up to 3 members; beyond -> DynBox)
   │
  Never                    (bottom: unreachable)
```

Values start as `DynBox` and get refined through type inference and profiling data. The `meet` operation at SSA join points computes the most specific common supertype. When Monty supplies profile-guided type information, TIR can start functions at a more refined lattice position, enabling deeper optimization.

---

## 3. Tier-Up Decision Model

The tier-up controller decides when to promote a function from Monty interpretation to Molt compilation. The model balances compilation cost against execution savings.

### 3.1 Call Counting

Every function in Monty's dispatch table carries a 32-bit atomic call counter, incremented on entry:

```
counter < T_compile  ->  interpret (Tier 0)
counter = T_compile  ->  enqueue for background compilation
counter > T_compile  ->  interpret until compiled version ready
compiled ready       ->  atomic swap to Tier 1
```

**Default threshold:** `tier_up_threshold = 100` (configurable in `molt.capabilities.toml` under `[monty]`). This is deliberately lower than V8's Sparkplug threshold (~200 calls) because Molt's compilation is ahead-of-time quality -- there is no Maglev-equivalent middle tier to amortize. One compilation, full optimization.

### 3.2 Size Heuristics

Not all functions benefit from compilation:

| Category | Heuristic | Action |
|---|---|---|
| **Tiny** (< 5 bytecode ops) | Cost of compilation exceeds savings | Never compile; inline at call site in compiled callers |
| **Small** (5-50 ops) | Moderate benefit | Compile after 2x normal threshold |
| **Medium** (50-500 ops) | Standard benefit | Compile at normal threshold |
| **Large** (> 500 ops) | High benefit, high compile cost | Compile at 0.5x threshold, use parallel compilation |
| **Megamorphic** (> 3 observed types per arg) | Type speculation unlikely to pay off | Compile with generic (DynBox) types only |

### 3.3 Type Stability Score

Each call site tracks a type stability metric:

```
stability = unique_type_profiles / total_calls
```

- `stability = 1.0`: perfectly monomorphic. Full speculative optimization with deopt guards.
- `stability < 0.1`: near-monomorphic. Speculate on dominant type, deopt on rare path.
- `stability > 0.3`: polymorphic. Compile with Union types or DynBox. No speculation.

This maps directly to TIR's Union type: `Union(I64, F64)` is a 2-member union that can be dispatched with a single tag check, while `Union(I64, F64, Str)` triggers the 3-member limit, and 4+ collapses to `DynBox`.

### 3.4 Priority Queue

When multiple functions cross the threshold simultaneously, the tier-up controller ranks them:

```
priority = call_count * estimated_speedup * (1 / estimated_compile_time)
```

Where `estimated_speedup` is derived from the function's type stability (monomorphic functions benefit more) and `estimated_compile_time` is proportional to TIR op count. The highest-priority function compiles first. The compilation cache (`tir/cache.rs`) means previously-compiled functions skip the queue entirely -- only the content hash lookup is needed.

---

## 4. Shared Contracts

The contract surface between Monty and Molt is the single most important design element. If the contracts diverge, tier-up becomes observable to user code, which violates the fundamental invariant.

### 4.1 Capability Manifest (`molt.capabilities.toml`)

The manifest (already at v2.0) is the single source of truth for what a Python program is allowed to do:

```toml
[capabilities]
allow = ["net", "fs.read", "env.read", "time.wall"]
deny = ["fs.write"]

[capabilities.packages.my_module]
allow = ["net"]

[resources]
max_memory = "64MB"
max_duration = "30s"
max_allocations = 1_000_000
max_recursion_depth = 500

[resources.operation_limits]
max_pow_result = "10MB"
max_repeat_result = "10MB"

[io]
mode = "virtual"   # "real" | "virtual" | "callback"

[monty]
compatible = true
shared_stubs = "stubs/"
execution_tier = "auto"   # "auto" | "interpret" | "compile"
tier_up_threshold = 100
```

Both runtimes parse this manifest at initialization. Monty checks capabilities in its eval loop; Molt checks them at WASM host import boundaries (`wasm_imports.rs`). The `ResourceTracker` trait (`runtime/molt-runtime/src/resource.rs`) is the enforcement mechanism on both sides.

### 4.2 Type Stub Format

Type stubs in `stubs/` serve dual duty:

1. **Monty** uses them for its `ty` type checker (compile-time validation of user code).
2. **Molt** uses them as initial type facts for TIR type refinement (pre-seeding the lattice above `DynBox`).

The stub format is standard `.pyi` with Molt-specific extensions expressed as `# molt:` pragmas:

```python
# stubs/json.pyi

# molt: always_compile=true (JSON parsing is always hot)
def loads(s: str) -> dict[str, Any]: ...

# molt: unbox_return=true (return type is always dict)
def dumps(obj: Any, *, indent: int | None = None) -> str: ...
```

### 4.3 Exception Semantics

Both runtimes implement identical exception behavior:

- **Catchable exceptions** (ValueError, TypeError, etc.): standard Python semantics. `try/except` works identically in both tiers.
- **Uncatchable resource exceptions** (MemoryError from ResourceTracker, TimeoutError from duration limit): modeled after Monty's pattern. These **bypass all except handlers** and propagate directly to the host boundary. Neither `except Exception` nor `except BaseException` in user code can suppress them.

This is critical for multi-tenant safety. A malicious program cannot catch a memory limit error and continue allocating. The `ResourceError` enum in `molt-runtime/src/resource.rs` distinguishes five variants (Memory, Time, Allocation, Recursion, OperationTooLarge), all of which are uncatchable.

Molt enforces this at compile time: the codegen for `try/except` blocks includes a check against the exception type tag, and resource exceptions carry a tag that never matches any Python exception class.

### 4.4 IO Mode Semantics

Three IO modes, identical behavior in both tiers:

| Mode | Monty Behavior | Molt Behavior |
|---|---|---|
| `real` | Direct syscalls via Rust std | WASM imports to host-provided real I/O |
| `virtual` | In-memory VFS, capped by `max_size` per mount | Same VFS, mounted at WASM host boundary |
| `callback` | Yields to host callback at I/O points | Same callback, invoked through WASM import |

The `callback` mode is the most interesting: Monty yields at every I/O boundary (file read, network connect, env lookup), returning control to the host with a description of the requested operation. The host decides whether to allow, deny, or virtualize the operation. Molt implements identical yield points as WASM import calls that trap back to the host with the same operation descriptors.

---

## 5. Edge Deployment Architecture

### 5.1 Cloudflare Workers: The Target Platform

```
┌─────────────────────────────────────────────────────────────────┐
│                    Cloudflare Edge Network                       │
│                                                                  │
│   ┌──────────────────────────────────────────────┐              │
│   │             Worker Isolate                    │              │
│   │                                               │              │
│   │  ┌─────────────┐    ┌──────────────────────┐ │              │
│   │  │   Monty VM   │    │  Molt WASM Module    │ │              │
│   │  │ (always      │◄──►│ (loaded on demand    │ │              │
│   │  │  loaded,     │    │  from R2 cache)      │ │              │
│   │  │  ~200KB)     │    │                      │ │              │
│   │  └──────┬───────┘    └──────────┬───────────┘ │              │
│   │         │                       │              │              │
│   │         └───────────┬───────────┘              │              │
│   │                     │                          │              │
│   │  ┌──────────────────▼────────────────────┐   │              │
│   │  │        Shared Host Boundary            │   │              │
│   │  │  ResourceTracker | AuditSink           │   │              │
│   │  │  CapabilityChecker | IODispatcher      │   │              │
│   │  └───────────────────────────────────────┘   │              │
│   └──────────────────────────────────────────────┘              │
│                                                                  │
│   ┌──────────────┐  ┌────────────────┐  ┌───────────────────┐  │
│   │ R2 (module   │  │ Durable Objects│  │ KV (config/       │  │
│   │  cache)      │  │ (snapshots)    │  │  manifests)       │  │
│   └──────────────┘  └────────────────┘  └───────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 Request Lifecycle

**First request (cold start):**

1. Worker isolate spins up. Monty VM loads from the worker bundle (~200KB, <1ms).
2. Monty interprets the request handler immediately. No compilation delay.
3. `ResourceTracker` limits enforced from instruction #1.
4. Response returned. Total cold-start overhead: **<5ms** (dominated by isolate creation, not Python startup).
5. Type profile data recorded for each function call.

**Subsequent requests (warming):**

6. Call counters increment. Functions crossing `tier_up_threshold` are enqueued.
7. Background compilation kicks off (on the same isolate, between request handling).
8. Molt compiles Python -> SimpleIR -> TIR -> optimized WASM.
9. Compiled module stored in R2 (content-addressed by source hash + type profile).
10. Atomic swap: function dispatch table entry updated from Monty bytecode offset to WASM function index.

**Hot path (compiled):**

11. Request arrives. Dispatch hits the compiled WASM function directly.
12. Execution at near-native speed (10-100x faster than interpretation).
13. Same `ResourceTracker` checks at host import boundaries.
14. If deoptimization triggers (rare type seen for first time), control transfers back to Monty for that invocation, and the function may be recompiled with updated type profile.

### 5.3 Snapshot/Resume with Durable Objects

Monty's snapshot capability enables a unique edge pattern: **suspend a Python computation mid-execution and resume it on a different request.**

```
Request 1:  Monty executes → hits async I/O → snapshot state → return early
            Durable Object stores: (bytecode offset, locals, stack, call frames)

Request 2:  Load snapshot from Durable Object → resume from exact point
            If function has been tier-up'd since snapshot, resume in Monty
            (compiled code cannot resume mid-function from interpreter state)
```

This enables long-running workflows (multi-step form processing, paginated API crawls, saga orchestration) on a platform with 30-second execution limits.

### 5.4 Module Caching at Edge

Molt's compilation cache (`tir/cache.rs`) uses content-addressed storage. The cache key is a hash of:

- Source code content (via `serialize::content_hash`)
- Type profile (argument types observed by Monty)
- Capability manifest hash (different permissions may enable different optimizations)
- Compiler version tag

Compiled WASM modules are stored in R2 with this key. When a new isolate starts on the same edge node, it checks R2 before compiling. Cache hit rate at steady state should exceed 99% -- only new deployments trigger fresh compilation.

The WASM split plan (`tir/wasm_split.rs`) separates modules into core, stdlib, and user code. The core module (~300KB) is bundled with the worker. Stdlib modules are loaded on demand from R2. User code modules are compiled per-deployment and cached.

---

## 6. Implementation Phases

### Phase 0: Shared Capability Manifest -- COMPLETE

- `molt.capabilities.toml` v2.0 with `[monty]` section
- `ResourceTracker` trait in `molt-runtime/src/resource.rs` (5 error variants, thread-local dispatch)
- `AuditSink` trait in `molt-runtime/src/audit.rs` (4 sink implementations, zero-overhead NullSink default)
- Pre-emptive operation guards in `[resources.operation_limits]`
- IO mode semantics (`real`/`virtual`/`callback`)

### Phase 1: Monty as Optional Dependency in WASM Host

**Goal:** Ship a worker that can run Monty-interpreted Python alongside Molt-compiled WASM.

- Add Monty as an optional Rust dependency in `molt-runtime` (feature-gated: `monty-interp`)
- Implement `MontyDispatcher` that wraps Monty's eval loop behind Molt's function dispatch interface
- Both share the same `ResourceTracker` instance (thread-local, set once at host init)
- Both share the same `AuditSink` instance
- Monty's capability checks delegate to the same `CapabilityChecker` that Molt's WASM imports use
- **Deliverable:** Worker that interprets all Python via Monty, with Molt's resource controls

### Phase 2: Call Counter Instrumentation

**Goal:** Molt-compiled functions track their own invocation count for tier-down/recompilation decisions. Monty-interpreted functions track counts for tier-up.

- Add `call_counter: AtomicU32` to Monty's function table entries
- Add `call_counter` WASM global per function in Molt's codegen (`wasm.rs`)
- Tier-up controller reads counters via shared memory (WASM) or direct access (Monty)
- Type profiling: Monty records `(func_name, arg_types)` tuples in a fixed-size ring buffer
- **Deliverable:** Observability into function hotness, type stability metrics

### Phase 3: Background Compilation Pipeline

**Goal:** Compile hot functions without blocking request handling.

- Compilation queue: priority queue ordered by `call_count * stability * (1/estimated_cost)`
- Compilation worker: runs Molt's full pipeline (SimpleIR -> TIR -> optimize -> wasm-encoder)
- Uses Molt's existing TIR pipeline (`passes::run_pipeline`) with type profile data seeding the lattice
- Content-addressed cache check before compilation (avoid redundant work)
- Compilation artifacts stored in-memory and persisted to R2 for cross-isolate reuse
- **Deliverable:** Functions compiled in background, artifacts ready for swap

### Phase 4: Atomic Function Swap

**Goal:** Replace interpreted function with compiled version without stopping execution.

- Function dispatch table: `enum FuncEntry { Interpreted(MontyOffset), Compiled(WasmFuncIdx) }`
- Swap is a single atomic write to the dispatch table entry
- In-flight calls to the old version complete normally (no interruption)
- New calls go to the compiled version
- Deoptimization path: if a compiled function hits a `DeoptPoint` (`tir/deopt.rs`), it materializes live values into a `DeoptState` struct and calls back into Monty's eval loop at the corresponding bytecode offset
- **Deliverable:** Seamless tier-up visible only in latency metrics

### Phase 5: Serialized Type Information Exchange

**Goal:** Monty's type profiles and Molt's type facts converge into a shared format.

- Define `TypeProfile` protobuf/flatbuffer schema: `{func_name, call_count, arg_types[], return_type, stability_score}`
- Monty serializes profiles after each request batch
- Molt deserializes profiles to seed `TirType` lattice positions in `type_refine.rs`
- Molt serializes compiled function signatures back (via `tir/serialize.rs`) so Monty can validate call-site compatibility before tier-up
- Profile-guided recompilation: when type stability drops below threshold, recompile with broader types
- **Deliverable:** Closed-loop type information flow between interpreter and compiler

### Phase 6: Production Hardening

- Tier-up telemetry: emit structured events for every compilation, swap, and deopt
- Compilation timeout: kill compilations that exceed 500ms (fall back to interpretation)
- Memory budget: limit TIR optimization memory to 32MB per function (abort and compile unoptimized)
- A/B testing: configurable percentage of requests routed through interpreter vs. compiled
- Graceful degradation: if Molt compilation fails, function stays in Monty permanently

---

## 7. Performance Analysis

### 7.1 Comparison with V8's Tier-Up Model

| Property | V8 | Monty-Molt |
|---|---|---|
| **Tier 0** | Ignition (bytecode interpreter) | Monty (bytecode interpreter) |
| **Tier 1** | Sparkplug (baseline compiler, no optimization) | -- (skipped) |
| **Tier 2** | Maglev (mid-tier, partial optimization) | -- (skipped) |
| **Tier 3** | TurboFan (full optimizing compiler) | Molt (full optimizing AOT compiler) |
| **Compilation trigger** | Call count + loop iterations | Call count + type stability |
| **Deoptimization** | Lazy deopt, on-stack replacement | Off-stack deopt, return to interpreter |
| **Type feedback** | Inline caches (ICs) | Ring buffer type profiles |
| **Speculation** | Monomorphic/polymorphic ICs | TirType lattice + Union types |

The critical difference: **Molt skips the middle tiers.** V8 needs Sparkplug and Maglev because TurboFan compilation is expensive (10-100ms for large functions). Molt's Cranelift backend compiles in 1-10ms for typical Python functions because:

1. Python functions are small (median ~30 bytecode ops vs. thousands of JS ops in hot V8 functions).
2. Cranelift is designed for fast compilation (single-pass register allocation).
3. Molt's TIR is already partially optimized by the pass pipeline before hitting Cranelift.

This means a single tier-up step suffices. The interpreter-to-optimized gap is bridged in one jump.

### 7.2 Expected Latencies

| Operation | Expected Latency | Notes |
|---|---|---|
| Monty cold start | <1us | Bytecode already loaded in worker bundle |
| Monty per-call overhead | ~100ns | Dispatch + counter increment |
| Monty type profiling | ~50ns/call | Ring buffer append, no allocation |
| Molt compilation (small func, <50 ops) | 1-3ms | SimpleIR -> TIR -> WASM |
| Molt compilation (medium func, 50-500 ops) | 3-10ms | Full optimization pipeline |
| Molt compilation (large func, >500 ops) | 10-50ms | Interprocedural + polyhedral |
| Tier-up swap | ~10ns | Single atomic pointer write |
| Deoptimization (Molt -> Monty) | ~5us | Materialize live values + transfer |
| Compiled function execution | 10-100x faster than Monty | Depends on type stability |
| Cache lookup (in-memory) | ~100ns | HashMap lookup by content hash |
| Cache lookup (R2) | ~5ms | Edge-local storage, not cross-region |

### 7.3 Memory Overhead

Running both runtimes simultaneously has a cost:

| Component | Memory | Justification |
|---|---|---|
| Monty VM (bytecode + dispatch tables) | ~200KB | Always loaded, handles cold code |
| Monty type profile buffer | ~64KB | Fixed-size ring buffer, 1024 entries |
| Molt compiled modules (cached) | 50KB-2MB | Proportional to compiled function count |
| TIR during compilation | 1-32MB transient | Freed after compilation completes |
| Dispatch table (dual entries) | ~8 bytes/function | One pointer per function, negligible |

Total steady-state overhead of the tiered approach vs. Molt-only: **~300KB**. This is the cost of instant cold starts -- a trade-off that pays for itself on the first request.

### 7.4 Cache Effectiveness

At the edge, compiled modules are shared across isolates on the same node via R2:

- **Deployment-level cache hit rate:** ~99.9%. A new deployment compiles once; all subsequent isolates reuse the artifact.
- **Cross-deployment reuse:** Content-addressed keys mean identical functions across different deployments share compiled artifacts.
- **Cache invalidation:** Automatic. Changed source code produces a different content hash. No manual invalidation needed.
- **Cache size per deployment:** 500KB-5MB typical (only hot functions are compiled; cold code stays in Monty).

---

## 8. Security Implications

### 8.1 Single Sandbox Boundary

Both runtimes execute inside the same WASM isolate (or native process). The sandbox boundary is **not** between Monty and Molt -- it is between the combined runtime and the host environment. This means:

- A vulnerability in Monty's eval loop and a vulnerability in Molt's compiled code have identical blast radius.
- There is no privilege escalation path from one tier to the other.
- The `ResourceTracker` is the single enforcement point for both tiers.

### 8.2 ResourceTracker: Universal Enforcement

The `ResourceTracker` trait (`molt-runtime/src/resource.rs`) is thread-local and set once at initialization. Both runtimes call into it:

- **Monty:** Checks `on_allocate` on every object creation, `check_time` every N instructions (rate-limited), `check_recursion_depth` on every call.
- **Molt:** Checks `on_allocate` via WASM host imports (`wasm_imports.rs`), `check_time` at loop back-edges (compiled in by codegen), `check_operation_size` for pow/repeat/shift operations (compiled as pre-checks).

The `LimitedTracker` implementation tracks five dimensions simultaneously: heap bytes, wall-clock duration, allocation count, recursion depth, and per-operation result size. All limits are read from `molt.capabilities.toml` at startup.

Resource errors (`ResourceError` enum) produce **uncatchable exceptions** in both tiers. This is the single most important security property: user code cannot suppress resource limit violations.

### 8.3 Audit Logging Across Tiers

The `AuditSink` (`molt-runtime/src/audit.rs`) records every capability check with:

- Nanosecond timestamp
- Operation identifier (`fs.read`, `net.connect`, `env.lookup`)
- Capability tested
- Operation-specific arguments (`AuditArgs::Path`, `AuditArgs::Network`, etc.)
- Decision (Allowed / Denied / ResourceExceeded)
- Originating Python module

Audit events are identical regardless of which tier executed the code. A `json.loads()` call produces the same audit record whether Monty interpreted it or Molt compiled it. This is enforced by routing all capability checks through the same `CapabilityChecker`, which both runtimes call at their respective I/O boundaries.

### 8.4 Capability Violations

When a capability is denied:

1. Both tiers raise an identical `PermissionError` with the same message format.
2. The audit sink records the denial.
3. The error is catchable (unlike resource errors) -- user code can handle denied capabilities gracefully.
4. Per-package scoping (`[capabilities.packages.my_module]`) applies identically in both tiers.

### 8.5 Compilation as an Attack Surface

Background compilation introduces a new attack vector: a malicious program could craft code that is expensive to compile (deeply nested control flow, exponential type unions). Mitigations:

- **Compilation timeout:** 500ms hard limit. Functions that exceed this stay interpreted.
- **TIR memory budget:** 32MB per function. Exceeded -> abort and compile unoptimized (skip the pass pipeline).
- **Pass-level guards:** Each TIR pass checks iteration count. The SCCP pass, for example, uses a forward scan (not iterative fixpoint) to bound analysis time.
- **Queue depth limit:** At most 16 functions queued for compilation. Beyond that, lowest-priority entries are evicted.

---

## 9. Open Questions

1. **On-stack replacement (OSR):** Should Molt support tier-up mid-function (inside a long-running loop)? V8 does this with TurboFan. The current design only swaps at function entry boundaries, which means a function stuck in a hot loop cannot tier up until the loop exits. OSR is complex but may be necessary for workloads with few function calls but many loop iterations.

2. **Tier-down:** If a compiled function's type profile shifts (new argument types observed), should Molt recompile with broader types, or deoptimize back to Monty and re-profile? The current design favors recompilation, but deopt-and-reprofile may produce better code for type-unstable functions.

3. **Shared heap:** Monty and Molt currently assume different object layouts (Monty uses its own tagged pointers; Molt uses NaN-boxing). Cross-tier function calls require marshaling. Should we converge on a single object representation? The trade-off is interpreter speed (Monty's layout is optimized for eval-loop dispatch) vs. interop cost.

4. **Profile serialization format:** Protobuf, FlatBuffers, or a custom binary format? FlatBuffers has zero-copy deserialization, which matters when loading profiles from R2. Protobuf has better tooling. A custom format is smallest but least maintainable.

5. **Multi-isolate coordination:** On Cloudflare, multiple isolates may handle requests for the same worker. Should compilation be coordinated (one isolate compiles, others wait for the artifact in R2) or independent (each isolate compiles its own hot functions)? Coordination reduces total compute but adds latency for the waiting isolates.

---

## 10. Success Criteria

The tiered execution architecture succeeds when:

- **Cold start:** First request to a new endpoint completes in <10ms (Monty interpretation, no compilation).
- **Warm throughput:** Hot-path requests execute within 2x of hand-optimized Rust for equivalent logic.
- **Tier-up transparency:** No user-observable behavioral difference between interpreted and compiled execution for any program in the shared Python subset.
- **Resource safety:** No program, regardless of tier, can exceed its declared resource limits by more than one check interval (configurable, default 1ms for time, 4KB for memory).
- **Cache reuse:** >99% cache hit rate for compiled modules across isolates on the same edge node after warmup.
- **Deopt rate:** <0.1% of compiled function calls trigger deoptimization at steady state (type profiles are stable).

This is the architectural north star. Every commit to Monty or Molt should be evaluated against whether it moves toward or away from this vision. The shared contracts are the load-bearing interface -- if they hold, the rest is engineering. If they drift, the tiers become two separate runtimes instead of one.
