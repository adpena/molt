# Molt Resource Controls

This document describes the resource control system that protects Molt-compiled
programs from accidental or malicious resource exhaustion.

## Overview

The resource control system is built on a pluggable `ResourceTracker` trait
installed per-thread at runtime initialization. It guards:

- Heap memory usage
- Wall-clock execution time
- Heap allocation count
- Call stack recursion depth
- Pre-emptive operation size (prevents DoS via large intermediate results)

The default tracker (`UnlimitedTracker`) is a zero-overhead no-op suitable for
trusted local development. For sandboxed deployments, install a `LimitedTracker`
with configurable limits parsed from the capability manifest.

## ResourceTracker Trait

```rust
pub trait ResourceTracker {
    fn on_allocate(&mut self, size: usize) -> Result<(), ResourceError>;
    fn on_free(&mut self, size: usize);
    fn on_grow(&mut self, additional_bytes: usize) -> Result<(), ResourceError>;
    fn check_time(&mut self) -> Result<(), ResourceError>;
    fn check_recursion_depth(&mut self, depth: usize) -> Result<(), ResourceError>;
    fn check_operation_size(&mut self, op: &OperationEstimate) -> Result<(), ResourceError>;
}
```

All hooks are called on the hot path. Implementations must be fast. The
thread-local tracker is accessed via `with_tracker`:

```rust
resource::with_tracker(|t| t.on_allocate(4096))?;
```

### Thread-Safety and Non-Reentrancy

`with_tracker` borrows the thread-local `ResourceTracker` via `RefCell`. This means:
- Calls to `with_tracker` must not be nested — calling `with_tracker` while already
  inside a `with_tracker` closure will panic with a borrow error.
- Each thread has its own tracker instance. Cross-thread tracking requires
  `set_global_tracker_factory` to install a factory that creates per-thread trackers.

## Non-Reentrancy

`with_tracker` holds a mutable borrow on the thread-local tracker via `RefCell`.
**Do not call `with_tracker` from within a tracker hook.** This will panic with
"already mutably borrowed."

In practice, this means `ResourceTracker` implementations must not allocate
via paths that re-enter `with_tracker`. The built-in `LimitedTracker` avoids
this — its error messages use `format!()` which allocates, but this happens
after the borrow is released (in the error return path, not inside the hook).

Custom tracker implementations should follow the same pattern: compute results
inside the hook, allocate error details outside.

## LimitedTracker Configuration

`LimitedTracker` is created from a `ResourceLimits` struct. Omitted fields
default to unlimited.

```rust
let limits = ResourceLimits {
    max_memory: Some(64 * 1024 * 1024),       // 64 MB
    max_duration: Some(Duration::from_secs(30)), // 30 seconds
    max_allocations: Some(1_000_000),
    max_recursion_depth: Some(500),
    // Combined per-operation fallback cap (used when a per-op cap is unset):
    max_operation_result_bytes: Some(10 * 1024 * 1024), // 10 MB
    // Per-operation caps (each falls back to max_operation_result_bytes / 10 MB):
    max_pow_result_bytes: Some(1 * 1024 * 1024),
    max_repeat_result_bytes: Some(1 * 1024 * 1024),
    max_shift_result_bytes: Some(1 * 1024 * 1024),
    max_string_result_bytes: Some(1 * 1024 * 1024),
};
resource::set_tracker(Box::new(LimitedTracker::new(&limits)));
```

`ResourceLimits` is the **single source of truth** for resource configuration.
The Python `ResourceLimits` dataclass (`src/molt/capability_manifest.py`) and the
`molt.capabilities.toml` schema serialize into this struct one field-for-one via
`MOLT_RESOURCE_MAX_*` env vars — every per-op cap a manifest declares reaches the
Rust tracker (there is no field that is advertised in Python but dropped at the
env boundary).

Time checks are rate-limited: `Instant::elapsed()` is only sampled every 10th
call to `check_time`, keeping the per-operation overhead near zero.

## Pre-emptive DoS Guards

The runtime checks the *estimated* result size of expensive operations **before**
executing them. This prevents OOM by rejecting pathological inputs early.

| Operation | Guard | Example |
| --- | --- | --- |
| `a ** b` | `check_pow_size` | `2 ** (1 << 40)` rejected before computation |
| `a << n` | `check_lshift_size` | `1 << 1_000_000` rejected |
| `s * n` | `check_repeat_size` | `"x" * 10_000_000_000` rejected |
| `a * b` (BigInt) | `check_bigint_mul_size` | Large integer multiplication rejected |
| `str.replace` | `OperationEstimate::StringReplace` | Pathological replacement expansion rejected |

Default limit: 10 MB per single-operation result. Configurable via
`max_operation_result_bytes` (the combined fallback) in the resource limits, or
per-operation via `max_pow_result` / `max_repeat_result` / `max_shift_result` /
`max_string_result` in the manifest `[resources.operation_limits]` section. Each
per-op cap governs only its own operation and falls back to the combined cap (or
the 10 MB default) when unset; `max_shift_result` also governs BigInt
multiplication. These per-op caps are emitted to the runtime as
`MOLT_RESOURCE_MAX_{POW,REPEAT,SHIFT,STRING}_RESULT` and
`MOLT_RESOURCE_MAX_OPERATION_RESULT`, and are honored by the in-VM tracker's
`check_operation_size` — they are no longer dropped at the env boundary.

### Estimation Logic

- **Pow**: `result_bits = base_bits * exponent * 4` (4x safety factor for intermediates)
- **Repeat**: `item_bytes * count`
- **Multiply**: `result_bits = a_bits + b_bits`
- **LeftShift**: `result_bytes = ceil((value_bits + shift) / 8) * 2` (2x safety factor for intermediates)
- **StringReplace**: accounts for per-replacement growth, empty-string edge case

Overflow in estimation is treated as "too large" and rejected.

## Uncatchable Resource Exceptions

Resource limit violations (memory, time, allocation, operation size) raise
runtime exceptions that are **uncatchable** by Python `try/except`. This
prevents untrusted code from suppressing resource exhaustion:

```python
try:
    2 ** (1 << 40)   # Resource violation
except Exception:
    pass              # Does NOT catch resource errors
```

The sole exception is `RecursionError`, which remains catchable to match
CPython semantics.

## Configurable Memory Protection

A compiled molt binary can cap its own memory so a runaway program cannot OOM
the host. This is **opt-in** (off by default — no limit is installed unless one
is configured) and resolves through the single `ResourceLimits` path, so there
is exactly one enforcement model with two layered backstops.

### Front door: `MOLT_MEMORY_LIMIT`

`MOLT_MEMORY_LIMIT` is the ergonomic, human-readable alias for the memory cap.
It accepts sizes like `512M`, `2G`, `64MB`, `1.5GiB`, or a bare byte count, and
resolves into the **same** `ResourceLimits.max_memory` field as the canonical
`MOLT_RESOURCE_MAX_MEMORY` (which the capability manifest emits). It is **not** a
parallel enforcement path.

```bash
# Cap the compiled binary at 64 MiB. A program that allocates past it gets a
# (uncatchable) MemoryError from the in-VM tracker instead of OOM-killing the host.
MOLT_MEMORY_LIMIT=64M ./my_app
```

Resolution / precedence:

- If both `MOLT_MEMORY_LIMIT` and `MOLT_RESOURCE_MAX_MEMORY` are set, the
  user-facing alias wins and a one-line override notice is printed to stderr.
- A malformed value (e.g. `MOLT_MEMORY_LIMIT=not-a-size`, `0M`, `-5M`) is a
  configuration error: the runtime reports it and aborts at init rather than
  silently ignoring the limit.
- With neither set, behavior is unchanged: no tracker and no OS backstop are
  installed (the zero-overhead `UnlimitedTracker` default remains).

The limit installs through `install_global_limited_tracker`, which uses the
global tracker factory — so spawned worker/compilation threads inherit the same
cap (a per-thread `set_tracker` alone would leave them unlimited).

### Two-layer enforcement (defense in depth)

1. **Layer 1 — in-VM tracker (the contract).** The `LimitedTracker`
   `on_allocate` / `on_grow` hooks account the logical Python heap. This layer is
   precise, deterministic, and identical across native / WASM / LLVM / Luau, and
   produces the uncatchable `ResourceError::Memory`.
2. **Layer 2 — OS backstop (`RLIMIT_AS`, native only).** When a memory cap is
   configured, runtime init also calls `setrlimit(RLIMIT_AS, …)` (and
   `RLIMIT_DATA`) set *above* the Layer-1 limit (headroom = max(64 MiB, 25%)).
   This bounds allocations the tracker cannot see — Rust-internal metadata, FFI,
   runtime structures — converting a runaway into a clean failure instead of an
   OOM-kill of the host. It is a **backstop only**, never the limit user-visible
   behavior depends on; the soft limit is tightened raise-only (never lowered
   below a host-imposed bound).
   - **Linux:** `RLIMIT_AS` genuinely caps the address space.
   - **macOS:** `setrlimit(RLIMIT_AS, …)` rejects small finite caps (EINVAL), so
     the backstop degrades to best-effort and the in-VM tracker (Layer 1) is the
     sole enforcement. `install_address_space_backstop` honestly reports this.
   - **WASM:** not applicable — the host-controlled linear-memory `max` page
     count is the backstop already.

> Capability-tier (deployment-profile) defaults — automatically applying a tight
> cap for untrusted edge deployments — are intentionally **not** implemented yet:
> the word "tier" is overloaded across three axes in the spec corpus, and
> default-on policy is deferred until that vocabulary is disambiguated. Today the
> protection is strictly opt-in via the env above.

## Integration with WASM Host Boundary

For WASM deployments, the host installs a `LimitedTracker` during module
instantiation, before any guest code runs. The tracker is wired into:

- The linear memory allocator (`on_allocate` / `on_free` / `on_grow`)
- Every host-to-guest function re-entry (`check_time`)
- Every call frame push (`check_recursion_depth`)
- Arithmetic and string builtins (`check_operation_size`)

When a resource limit is hit, the WASM host receives a trap with a structured
`ResourceError` that distinguishes memory, time, allocation, and operation
violations.

## Example: Configuring Limits for Cloudflare Workers

Create a `molt.capabilities.toml` manifest:

```toml
[manifest]
version = "2.0"
description = "Cloudflare Workers edge deployment"

[capabilities]
allow = ["net", "env.read"]
deny = ["fs.write", "fs.read"]

[resources]
max_memory = "128MB"
max_duration = "30s"
max_allocations = 5_000_000
max_recursion_depth = 200

[resources.operation_limits]
max_pow_result = "1MB"
max_repeat_result = "1MB"
max_shift_result = "1MB"
max_string_result = "1MB"

[io]
mode = "virtual"

[audit]
enabled = true
sink = "jsonl"
output = "stderr"
```

Build the module with the manifest:

```bash
molt build --target wasm --require-linked \
    --capability-manifest molt.capabilities.toml \
    worker.py
```

The compiled WASM module will enforce all declared limits at runtime. The
Cloudflare Workers host can also override limits via its own initialization
layer.

## Source Files

- Trait, `ResourceLimits` (single source of truth), `LimitedTracker`,
  `parse_human_size` (the `MOLT_MEMORY_LIMIT` front door), and
  `install_address_space_backstop` (RLIMIT_AS): `runtime/molt-runtime/src/resource.rs`
- Env parsing + `molt_runtime_init_resources` (resolves `MOLT_MEMORY_LIMIT` /
  `MOLT_RESOURCE_MAX_*` and installs both layers):
  `runtime/molt-runtime/src/object/ops_sys.rs`
- Child-process limit inheritance (per-op caps + memory): `runtime/molt-runtime/src/async_rt/process.rs`
- Python `ResourceLimits` dataclass, manifest parsing, and `to_env_vars`
  serialization (one env var per field, no silent drops): `src/molt/capability_manifest.py`
- Tests: `runtime/molt-runtime/tests/resource_enforcement.rs` (end-to-end env →
  tracker enforcement + RLIMIT_AS backstop), `tests/test_manifest_env.py`
  (Python↔env parity, no per-op field drop)
