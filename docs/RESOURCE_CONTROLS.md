# Molt Resource Controls

This document describes the resource configuration and tracker API. Complete
runtime enforcement is a V1.0 release blocker; see
[V1-19 in the release findings](agent/V1_HANDOFF_FINDINGS.md). A configured limit
does not establish an enforced deployment budget.

## Overview

The resource control system is built on a pluggable `ResourceTracker` trait
installed per-thread at runtime initialization. Its interfaces cover:

- Heap memory usage
- Wall-clock execution time
- Heap allocation count
- Call stack recursion depth
- Pre-emptive operation size estimates

The default tracker (`UnlimitedTracker`) performs no accounting. `LimitedTracker`
implements configurable checks, but selected allocation hooks do not cover all
runtime-owned storage. Production callers of its time, recursion and operation-size
methods have not been found in the runtime source. These settings must not be
used as a claim of sandbox enforcement.

## ResourceTracker Trait

```rust
pub trait ResourceTracker {
    fn on_allocate(&mut self, size: usize) -> Result<(), ResourceError>;
    fn on_free(&mut self, size: usize);
    fn on_grow(&mut self, additional_bytes: usize) -> Result<(), ResourceError>;
    fn on_shrink(&mut self, released_bytes: usize);
    fn check_time(&mut self) -> Result<(), ResourceError>;
    fn check_recursion_depth(&mut self, depth: usize) -> Result<(), ResourceError>;
    fn check_operation_size(&mut self, op: &OperationEstimate) -> Result<(), ResourceError>;
}
```

Selected allocation owners call the memory hooks through `with_tracker`.
Implementations must avoid reentry while that mutable borrow is held:

```rust
resource::with_tracker(|t| t.on_allocate(4096))?;
```

### Thread ownership and non-reentrancy

`with_tracker` holds a mutable `RefCell` borrow of the thread-local tracker.
Tracker hooks must not call paths that re-enter `with_tracker`; a nested borrow
panics. `LimitedTracker::check_operation_size` currently formats operation labels
inside the hook. Such allocations must not be routed back through the same
borrowed tracker. Custom trackers should return structured errors and defer any
formatting that could re-enter tracking until the borrow ends.

Each thread has its own tracker instance. `set_global_tracker_factory` creates
independent per-thread trackers and does not provide one aggregate cap.
Cross-thread tracking requires an allocation-bound owner. `on_shrink` releases
bytes charged by growth or rolls back a growth reservation without decrementing
the live allocation count.

## LimitedTracker Configuration

`LimitedTracker` is created from a `ResourceLimits` struct. Unset memory,
duration, allocation-count and recursion limits are unlimited. Each unset
per-operation cap uses the combined operation cap, or 10 MiB when that cap is
also unset. These operation defaults apply when `LimitedTracker` is installed;
`UnlimitedTracker` does not check operation sizes.

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

`ResourceLimits` owns the Rust tracker configuration. The Python dataclass
(`src/molt/capability_manifest.py`) serializes manifest resource fields into
`MOLT_RESOURCE_MAX_*` environment variables, and the runtime parser has a matching
field for each, including per-operation caps. This field parity does not prove
that every deployment path transports the environment or enforces the policy.

The `check_time` implementation samples `Instant::elapsed()` every 10th call.
Its cost and actual enforcement require production consumer measurements.

## Operation and execution checks

`OperationEstimate` describes power, repetition, multiplication, left-shift and
string-replacement result sizes. `LimitedTracker::check_operation_size` checks
these estimates against its configured caps. Overflow is rejected by the
estimate/check implementation.

The tracker time, recursion and operation-size methods currently have no
production consumers in the inspected runtime. Standalone size helpers have a
separate limited caller set and do not prove that the manifest's per-operation
caps reach every builtin. Frame entry, host reentry and builtin admission must
be migrated through the canonical resource owner before these policies qualify.

Each real denial path must also prove its Python exception or host-trap behavior.
The intended recursion behavior must preserve CPython's catchable
`RecursionError`; configuration and trait tests alone do not establish runtime
exception semantics.

## Configurable Memory Protection

Memory settings are opt-in and resolve through `ResourceLimits`. The existing
tracker and platform backstop are separate mechanisms with different scopes.
Neither currently qualifies a whole-runtime memory cap across all supported cells.

### Setting the cap: `MOLT_RESOURCE_MAX_MEMORY`

`MOLT_RESOURCE_MAX_MEMORY` is the one memory cap. The capability manifest
emits it as a raw byte count; a person may write a human-readable size like
`512M`, `2G`, `64MB`, or `1.5GiB`. Both spellings resolve into the **same**
`ResourceLimits.max_memory` field. There is no second name and no parallel
enforcement path.

```bash
# Configure a 64 MiB tracker limit. Complete runtime enforcement remains open.
MOLT_RESOURCE_MAX_MEMORY=64M ./my_app
```

- A malformed value (e.g. `MOLT_RESOURCE_MAX_MEMORY=not-a-size`, `0M`, `-5M`)
  is a configuration error: the runtime reports it and aborts at init rather
  than silently ignoring the limit.
- With all `MOLT_RESOURCE_MAX_*` settings absent, `UnlimitedTracker` remains
  the default. Any configured resource field installs `LimitedTracker`.
  The optional OS memory backstop is attempted only when a memory cap is set.

The limit installs through `install_global_limited_tracker`, which uses the
global tracker factory. Newly attached threads receive independent trackers
with the same configuration; they do not share live usage. Replacement and
cross-thread retirement currently lose allocation-owner custody. V1-19 requires
a generation-owned aggregate ledger before this is a global cap.

### Two-layer enforcement (defense in depth)

1. **Layer 1 — selected owned layouts.** `LimitedTracker` receives
   `on_allocate` / `on_grow` calls from selected object and backing owners. Raw
   C projections, allocator APIs and other guest-dependent metadata are not
   completely covered. Thread-local replacement and retirement are also open
   correctness defects. A whole-heap or cross-backend limit is unqualified.
2. **Layer 2 — optional OS committed-memory backstop (Linux).** When a memory
   cap is configured, runtime init attempts to tighten the `RLIMIT_DATA` soft
   limit to the process's startup footprint (`VmData`) plus the tracker limit
   and headroom (max(64 MiB, 25%)). The requested limit covers committed data
   beyond selected tracker hooks. It is only tightened, subject to an existing
   host bound. Footprint lookup, `getrlimit` or `setrlimit` failure returns
   `None`, which runtime init currently ignores. Installation and allocation
   failure behavior require actual deployment qualification; configuration
   cannot guarantee a clean failure or protection from a host OOM kill.
   - **Address-space limits:** `RLIMIT_AS` also counts sparse reservations such
     as executable mappings, allocator arenas and guard regions. A small cap
     can reject mappings or stack growth before the intended heap budget is
     consumed.
   - **Child processes:** Linux child startup applies a hard `RLIMIT_DATA` cap
     when a positive child budget resolves from a numeric parent memory setting
     or `MOLT_CHILD_RLIMIT_GB`. An explicit positive child cap selects the
     tighter budget; zero disables this child installation. Existing inherited
     OS limits remain separate. A human-readable parent memory value does not
     pass this numeric child-budget parser. Failure to install a selected child
     cap fails child startup.
   - **macOS / Windows:** `install_memory_backstop` returns `None`; the runtime
     does not install this committed-memory backstop. The selected tracker
     hooks still have the accounting gaps described above.
   - **WASM:** a linear-memory page maximum bounds an instance only when its
     actual host configuration has an admitted finite maximum. This does not
     establish the other tracker policies or their denial semantics.

> Capability-tier (deployment-profile) defaults — automatically applying a tight
> cap for untrusted edge deployments — are intentionally **not** implemented yet:
> the word "tier" is overloaded across three axes in the spec corpus, and
> default-on policy is deferred until that vocabulary is disambiguated. Today the
> protection is strictly opt-in via the env above.

## WASM host boundary

WASM linear-memory maxima belong to the actual host instance configuration.
Native host tracker callbacks currently select host thread-local state. The
inspected runtime does not wire the tracker time, frame-recursion or
operation-size methods into guest execution. These callbacks and policies need
instance-owned custody and actual native/WASM denial controls before enforcement
or structured-trap behavior can be claimed.

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

This manifest expresses deployment intent. Its declared limits are not yet
qualified runtime enforcement. The owning host must admit and enforce its actual
instance limits; the SDK and linked-output policy transport also remain release
work. See the canonical release findings for acceptance status.

## Source Files

- Trait, `ResourceLimits` (single source of truth), `LimitedTracker`,
  `parse_human_size` (the `MOLT_RESOURCE_MAX_MEMORY` size grammar), and
  `install_memory_backstop` (Linux `RLIMIT_DATA`):
  `runtime/molt-runtime-resource/src/lib.rs` (re-exported as `molt_runtime::resource`)
- Env parsing + `molt_runtime_init_resources` (resolves `MOLT_RESOURCE_MAX_*`
  and installs the configured tracker while attempting the optional memory backstop):
  `runtime/molt-runtime/src/object/ops_sys.rs`
- Child-process limit inheritance (per-op caps + memory):
  `runtime/molt-runtime/src/async_rt/process/child_resources.rs`
- Python `ResourceLimits` dataclass, manifest parsing, and `to_env_vars`
  serialization (one env var per field, no silent drops): `src/molt/capability_manifest.py`
- Tests: `runtime/molt-runtime/tests/resource_enforcement.rs` (end-to-end env →
  tracker enforcement + `RLIMIT_DATA` backstop), `tests/test_manifest_env.py`
  (Python↔env parity, no per-op field drop)
