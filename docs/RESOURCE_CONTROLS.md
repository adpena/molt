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

## LimitedTracker Configuration

`LimitedTracker` is created from a `ResourceLimits` struct. Omitted fields
default to unlimited.

```rust
let limits = ResourceLimits {
    max_memory: Some(64 * 1024 * 1024),       // 64 MB
    max_duration: Some(Duration::from_secs(30)), // 30 seconds
    max_allocations: Some(1_000_000),
    max_recursion_depth: Some(500),
    max_operation_result_bytes: Some(10 * 1024 * 1024), // 10 MB
};
resource::set_tracker(Box::new(LimitedTracker::new(&limits)));
```

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
`max_operation_result_bytes` in the resource limits or `max_pow_result` /
`max_repeat_result` / `max_shift_result` / `max_string_result` in the manifest
`[resources.operation_limits]` section.

### Estimation Logic

- **Pow**: `result_bits = base_bits * exponent * 4` (4x safety factor for intermediates)
- **Repeat**: `item_bytes * count`
- **Multiply**: `result_bits = a_bits + b_bits`
- **LeftShift**: `result_bits = value_bits + shift`
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

- Trait + implementations: `runtime/molt-runtime/src/resource.rs`
- Hot-path guard callsites: `runtime/molt-runtime/src/object/ops_sys.rs`
- Manifest parsing: `src/molt/cli.py` (TOML/JSON/YAML capability manifest loader)
