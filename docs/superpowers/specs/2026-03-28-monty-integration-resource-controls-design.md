# Monty Integration: Resource Controls, Guardrails & Tiered Execution Design

## Goal

Enhance Molt with Monty-inspired resource controls, audit infrastructure, pre-emptive
DoS guards, uncatchable resource exceptions, snapshot/resume, shared capability manifests,
embeddable SDK, REPL, fuzz targets, and the architectural foundation for a tiered
Monty→Molt execution model at the edge.

## Motivation

Pydantic's Monty (a secure Python bytecode interpreter in Rust) solves the complementary
half of Python-at-the-edge: instant startup, sandbox execution, snapshot/resume. Molt
solves AOT compilation to native/WASM. Together they form a V8-style tiered execution
model: Monty interprets cold/one-shot code, Molt AOT-compiles hot paths.

The immediate wins are adopting Monty's battle-tested resource control patterns into
Molt's WASM host boundary and codegen — essential for multi-tenant Cloudflare Workers
where one module must not starve others.

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                     Capability Manifest (TOML)                   │
│  [capabilities]                                                  │
│  allow = ["net", "fs.read"]                                      │
│  [resources]                                                     │
│  max_memory = "64MB"                                             │
│  max_duration = "30s"                                            │
│  max_allocations = 1_000_000                                     │
│  max_recursion_depth = 500                                       │
│  [audit]                                                         │
│  enabled = true                                                  │
│  sink = "structured_log"                                         │
└──────────────────────┬───────────────────────────────────────────┘
                       │
          ┌────────────┴────────────┐
          │                         │
     ┌────▼────┐             ┌──────▼──────┐
     │  Monty  │             │    Molt     │
     │ (interp)│             │   (AOT)     │
     └────┬────┘             └──────┬──────┘
          │                         │
          │  Shared Python Subset   │
          │  Shared Type Stubs      │
          │  Shared Test Suite      │
          │                         │
     ┌────▼─────────────────────────▼────┐
     │        WASM Host Boundary          │
     │  ┌─────────────────────────────┐  │
     │  │     ResourceTracker         │  │
     │  │  - on_allocate / on_free    │  │
     │  │  - check_time (rate-limited)│  │
     │  │  - check_recursion_depth    │  │
     │  │  - check_operation_size     │  │
     │  └─────────────────────────────┘  │
     │  ┌─────────────────────────────┐  │
     │  │       AuditLogger           │  │
     │  │  - capability checks        │  │
     │  │  - I/O operations           │  │
     │  │  - resource limit hits      │  │
     │  └─────────────────────────────┘  │
     └───────────────────────────────────┘
```

---

## Wave A: Core Security Infrastructure

### A1. Resource Tracker for WASM Host Boundary

**File: `runtime/molt-runtime/src/resource.rs` (new)**

A pluggable resource tracking trait, inspired by Monty's `ResourceTracker`:

```rust
/// Pluggable resource control — injected at WASM host import boundary.
/// Every allocation, deallocation, growth, and time-check flows through this.
pub trait ResourceTracker: Send + Sync {
    /// Called before every heap allocation. Return Err to reject.
    fn on_allocate(&mut self, size: usize) -> Result<(), ResourceError>;

    /// Called when memory is freed.
    fn on_free(&mut self, size: usize);

    /// Called when a container grows in-place (list.append, dict insert).
    fn on_grow(&mut self, additional_bytes: usize) -> Result<(), ResourceError>;

    /// Called at statement boundaries — rate-limited internally.
    fn check_time(&mut self) -> Result<(), ResourceError>;

    /// Called before function call push.
    fn check_recursion_depth(&mut self, depth: usize) -> Result<(), ResourceError>;

    /// Pre-emptive check before expensive operations.
    fn check_operation_size(&mut self, op: &OperationEstimate) -> Result<(), ResourceError>;
}

#[derive(Debug)]
pub enum ResourceError {
    Memory { used: usize, limit: usize },
    Time { elapsed_ms: u64, limit_ms: u64 },
    Allocation { count: usize, limit: usize },
    Recursion { depth: usize, limit: usize },
    OperationTooLarge { op: String, estimated_bytes: usize },
}

/// Pre-emptive operation size estimate — checked BEFORE the operation runs.
#[derive(Debug)]
pub enum OperationEstimate {
    Pow { base_bits: u32, exponent: u64 },
    Repeat { item_bytes: usize, count: u64 },
    Multiply { a_bits: u32, b_bits: u32 },
    LeftShift { value_bits: u32, shift: u32 },
    StringReplace { input_len: usize, old_len: usize, new_len: usize, count: usize },
}

/// Production implementation with configurable limits.
pub struct LimitedTracker {
    pub max_allocations: Option<usize>,
    pub max_duration: Option<std::time::Duration>,
    pub max_memory: Option<usize>,
    pub max_recursion_depth: Option<usize>,
    // Internal state
    allocation_count: usize,
    memory_used: usize,
    start_time: std::time::Instant,
    time_check_counter: u32, // rate-limit check_time to every Nth call
}

/// No-op tracker for unconstrained execution (native dev builds).
pub struct UnlimitedTracker;
```

**Integration points in WASM host imports (`wasm_imports.rs`):**
- `heap_alloc` → `tracker.on_allocate(size)`
- `heap_free` → `tracker.on_free(size)`
- `list_append` / `dict_set` → `tracker.on_grow(delta)`
- Every `call_indirect` → `tracker.check_recursion_depth(depth)`
- Statement-level import (new) → `tracker.check_time()`

**Key design decision:** The time check is rate-limited to every 10th call (counter
in the tracker, no `Instant::elapsed()` on every statement). This matches Monty's
approach and keeps overhead <1% on hot loops.

### A2. Pre-emptive Operation Size Guards

**File: `src/molt/frontend/__init__.py` (modify SimpleTIRGenerator)**
**File: `runtime/molt-backend/src/passes.rs` (new pass)**

Two-layer defense:

1. **Compile-time (frontend):** When both operands are compile-time constants,
   reject at IR generation with a clear error.

2. **Runtime (TIR pass):** For dynamic operands, inject a `CheckOperationSize`
   TIR instruction before the operation. The backend compiles this to a host
   import call that checks via the ResourceTracker.

```
# TIR instruction (new)
CheckOperationSize(op_kind: OperationKind, operands: Vec<Value>)

# Maps to WASM host import:
(import "molt" "check_operation_size" (func $check_op_size (param i32 i64 i64) (result i32)))
```

**Operations guarded:**
| Operation | Guard | Threshold |
|-----------|-------|-----------|
| `a ** b` | `check_pow_size(bits(a), b)` | Result > 10MB estimated |
| `s * n` | `check_repeat_size(len(s), n)` | Result > 10MB |
| `a * b` (bigint) | `check_mult_size(bits(a), bits(b))` | Result > 10MB |
| `a << n` | `check_lshift_size(bits(a), n)` | Result > 10MB |
| `s.replace(old, new)` | `check_replace_size(...)` | Result > 10MB |

**Compiler elision:** When the TIR optimization pass can prove both operands are
bounded (via type facts or constant propagation), the guard is eliminated entirely.
This is *better* than Monty — Monty always checks at runtime; Molt can prove safety
at compile time.

### A3. Uncatchable Resource Exceptions

**File: `runtime/molt-backend/src/wasm.rs` (modify exception handling)**

Resource limit violations bypass the normal Python exception dispatch:

```rust
// In WASM codegen, when emitting try_table:
// - Normal Python exceptions: caught by try_table catch clauses
// - Resource exceptions: use a distinct tag that is NOT caught by any user try_table

// New exception tags:
const RESOURCE_EXCEPTION_TAG: u32 = 0xFFFF_FFFE; // uncatchable
const PYTHON_EXCEPTION_TAG: u32 = 0;              // normal Python exceptions

// RecursionError is the ONE exception that remains catchable (CPython compat)
```

**Implementation:**
1. Resource tracker returns `ResourceError` variant
2. WASM host import converts to a trap with resource error info
3. The trap propagates through all `try_table` blocks (they don't catch it)
4. Only the top-level entry point catches it and returns error info to host

### A4. Structured Audit Logging

**File: `runtime/molt-runtime/src/audit.rs` (new)**

```rust
/// Structured audit event emitted for every capability-gated operation.
#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub timestamp_ns: u64,
    pub operation: &'static str,      // "fs.read", "net.connect", etc.
    pub capability: &'static str,     // capability that was checked
    pub args: AuditArgs,              // operation-specific arguments
    pub decision: AuditDecision,      // allowed / denied / resource_exceeded
    pub module: String,               // Python module that triggered this
    pub function: Option<String>,     // function name if available
}

#[derive(Debug, Serialize)]
pub enum AuditDecision {
    Allowed,
    Denied { reason: String },
    ResourceExceeded { error: ResourceError },
}

/// Pluggable audit sink.
pub trait AuditSink: Send + Sync {
    fn emit(&self, event: &AuditEvent);
}

/// Implementations:
/// - `StructuredLogSink` — JSON lines to stderr or file
/// - `NullSink` — no-op (production default when audit disabled)
/// - `BufferedSink` — collects events for batch export
/// - `WasmHostSink` — forwards to WASM host callback
```

**Integration:** Every `capabilities.require()` call flows through the audit system.
The VFS layer (`caps.rs`) emits events before/after capability checks.

---

## Wave B: Strategic Infrastructure

### B1. Enhanced Capability Manifest Format

**File: `molt.capabilities.toml` (new standard format)**

Extends the current JSON format with resource limits, audit config, and
Monty-compatible sections:

```toml
[molt.capabilities]
version = "2.0"

# Capability grants — same semantics as current system
[capabilities]
allow = ["net", "fs.read", "env.read", "time.wall"]
deny = ["fs.write"]

# Per-package capability scoping
[capabilities.packages.my_module]
allow = ["net"]

# NEW: Resource limits — enforced by ResourceTracker
[resources]
max_memory = "64MB"
max_duration = "30s"
max_allocations = 1_000_000
max_recursion_depth = 500
gc_interval = 10_000

# NEW: Pre-emptive operation guards
[resources.operation_limits]
max_pow_result_bytes = "10MB"
max_repeat_result_bytes = "10MB"
max_string_result_bytes = "10MB"

# NEW: Audit configuration
[audit]
enabled = true
sink = "structured_log"    # structured_log | null | buffered | wasm_host
output = "stderr"          # stderr | stdout | file path
format = "jsonl"           # jsonl | compact

# NEW: Monty compatibility section — for tiered execution
[monty]
compatible = true
shared_stubs = "stubs/"    # type stub directory for both Monty and Molt
execution_tier = "auto"    # auto | interpret | compile
tier_up_threshold = 100    # call count before AOT compilation
```

**Migration:** The existing JSON format continues to work. The new TOML format
is preferred and adds resource + audit + Monty sections. A `molt migrate-manifest`
command converts JSON → TOML.

### B2. Snapshot/Resume via WASM Asyncify

**Design for WASM execution pause/resume at external call boundaries.**

This is the most architecturally complex feature. Implementation approach:

1. **Asyncify transform** — Apply Binaryen's Asyncify pass to the linked WASM
   module. This instruments all function call sites with stack save/restore.

2. **External call yield** — When WASM code calls a host function that requires
   async resolution (database query, HTTP request, AI model call):
   - Host returns a "pending" sentinel value
   - Asyncify unwinds the WASM stack, saving state to linear memory
   - Host serializes linear memory + globals + table to bytes

3. **Resume** — Host deserializes state back into WASM memory, calls Asyncify
   rewind, execution continues from the exact yield point.

4. **Storage** — Serialized state goes to Cloudflare Durable Objects or R2.

```
Python code:    result = await fetch_data(url)
                         │
Molt WASM:      call $host_fetch_data
                         │
Host boundary:  → Save WASM state (Asyncify unwind)
                → Serialize to Durable Object
                → ... time passes ...
                → Deserialize from Durable Object
                → Restore WASM state (Asyncify rewind)
                         │
WASM resumes:   result is now available
```

**File changes:**
- `tools/wasm_link.py` — add Asyncify pass to post-link pipeline
- `runtime/molt-backend/src/wasm.rs` — mark external call sites for Asyncify
- `runtime/molt-wasm-host/` — implement save/restore/serialize
- New: `runtime/molt-snapshot/` crate for state serialization

### B3. Embeddable SDK (molt-embed)

**New crate: `runtime/molt-embed/`**

Minimal API for embedding Molt compilation in other Rust applications:

```rust
use molt_embed::{Molt, Limits, Capabilities};

let molt = Molt::new(python_source, Capabilities::from_toml("manifest.toml")?)?;
let wasm_bytes = molt.compile_wasm()?;
let result = molt.run_native(inputs, Limits::default())?;
```

Also exposed via PyO3 (`pydantic-molt` package) and napi-rs (`@pydantic/molt`).

---

## Wave C: Quality & Developer Experience

### C1. Interactive REPL (`molt repl`)

JIT-compiled REPL using Cranelift for instant feedback:

```bash
$ molt repl --capabilities env.read
molt> x = 42
molt> f"hello {x}"
'hello 42'
molt> import os; os.getenv("HOME")  # requires env.read capability
'/Users/adpena'
```

Persistent state across snippets via incremental compilation.

### C2. Fuzz Targets

**New: `runtime/molt-backend/fuzz/`**

Three fuzz targets:
1. `fuzz_tir_generator` — random Python AST → TIR (check no panics)
2. `fuzz_wasm_encoder` — random TIR → WASM (check valid output)
3. `fuzz_nan_boxing` — random values → NaN-box encode/decode roundtrip

### C3. Compile-Fail Tests

**New: `runtime/molt-runtime/tests/compile_fail/`**

Rust compile-time tests using `trybuild` that verify unsafe patterns are rejected:
- Dangling pointer to freed heap allocation
- Double mutable borrow of runtime state
- Capability escalation via internal API

### C4. Type-Checking Security Gate

**New CLI flag: `--type-gate`**

```bash
molt build --type-gate --capabilities net main.py
```

Rejects compilation if any code path touching `net` or `fs.write` capabilities
contains untyped variables or dynamic attribute access. Forces type discipline
in security-critical code.

---

## Wave D: Tiered Execution Vision

### D1. Monty→Molt Tiered Execution

Long-term architecture for edge Python execution:

```
Cold code → Monty interprets (<1μs startup)
     │
     │  execution counter reaches threshold
     │
Hot path → Molt AOT-compiles to WASM
     │
     │  cached compiled module
     │
Subsequent calls → execute compiled WASM (10-100x faster)
```

**Shared contracts:**
- Capability manifest format (`molt.capabilities.toml`)
- Type stubs directory (`stubs/`)
- Test expectation files
- Python subset definition (no exec/eval/compile, no runtime monkeypatching)

**Implementation:** Requires Monty as a dependency in the WASM host, with a
"tier-up" coordinator that monitors call counts and dispatches compilation.

---

## Testing Strategy

Each wave includes tests:

| Wave | Test Type | Location |
|------|-----------|----------|
| A1 | Unit tests for ResourceTracker + LimitedTracker | `runtime/molt-runtime/tests/resource.rs` |
| A1 | Integration: WASM module hitting limits | `runtime/molt-backend/tests/wasm_resource_limits.rs` |
| A2 | Frontend: constant-fold guard elimination | `tests/test_wasm_size_guards.py` |
| A2 | Runtime: dynamic guard triggers | `tests/test_runtime_guards.py` |
| A3 | Uncatchable exception propagation | `tests/test_uncatchable_exceptions.py` |
| A4 | Audit event emission | `runtime/molt-runtime/tests/audit.rs` |
| B1 | Manifest parsing + migration | `tests/test_capability_manifest.py` |
| B2 | Snapshot serialize/deserialize roundtrip | `runtime/molt-snapshot/tests/` |
| C2 | Fuzz: no panics on random input | `runtime/molt-backend/fuzz/` |
| C3 | Compile-fail: unsafe patterns rejected | `runtime/molt-runtime/tests/compile_fail/` |

## File Changes Summary

**New files:**
- `runtime/molt-runtime/src/resource.rs` — ResourceTracker trait + LimitedTracker
- `runtime/molt-runtime/src/audit.rs` — AuditSink trait + structured logging
- `runtime/molt-snapshot/` — new crate for WASM state serialization
- `runtime/molt-embed/` — new crate for embeddable SDK
- `runtime/molt-backend/fuzz/` — fuzz targets
- `molt.capabilities.toml` — example manifest in new format

**Modified files:**
- `runtime/molt-backend/src/wasm_imports.rs` — resource tracker hooks at host boundary
- `runtime/molt-backend/src/wasm.rs` — uncatchable exception tags, Asyncify markers
- `runtime/molt-backend/src/passes.rs` — CheckOperationSize TIR pass
- `runtime/molt-backend/src/lib.rs` — resource tracker integration
- `runtime/molt-runtime/src/vfs/caps.rs` — audit event emission
- `runtime/molt-runtime/src/object/ops.rs` — pre-emptive size checks
- `runtime/molt-runtime/src/lib.rs` — export resource + audit modules
- `src/molt/frontend/__init__.py` — compile-time size guard insertion
- `src/molt/cli.py` — new flags (--resource-limits, --audit-log, --type-gate)
- `src/molt/capabilities.py` — TOML manifest support
- `tools/wasm_link.py` — Asyncify post-link pass
