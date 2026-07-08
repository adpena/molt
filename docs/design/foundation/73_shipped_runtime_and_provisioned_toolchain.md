# 73 - Shipped Runtime Artifacts and Provisioned Toolchain

Status: design contract, 2026-07-06

Molt must behave like a real compiler toolchain: a user program should compile
because the compiler owns its runtime, sysroot, dependency provisioning, and
resource budget. "The host cannot build it" is not an architecture. Manual
environment hunting, per-program runtime rebuilds, and unbounded Cargo fanout are
toolchain defects.

This design is the foundation for R73. It binds two currently active repair
lanes into one end state.

Current apparatus evidence:

- `20260706T170954-r4a-spectral-wasm-build-active-uv-clean-v5-646ac77b5f734554`
  passed an APDataStore-side WASM rebuild through queue-owned RunContext roots
  after the exFAT hardlink classifier was synced into the proof worktree.
- The row used `UV_LINK_MODE=copy`, `MOLT_CACHE=D:\Molt\.molt_cache`, and
  `CARGO_TARGET_DIR=D:\Molt\target\sessions\r4a-wasm-numeric`; the fix is to
  materialize cache outputs through the canonical link-or-copy path, not to
  reroute to legacy roots or disable caching.

## R73.1 Shipped Runtime Artifact

`molt_runtime.wasm` is Molt's equivalent of `rustc`'s target `std`, not a user
program dependency to rebuild on every compile.

End state:

- Runtime artifacts are content-addressed by target triple, runtime profile,
  feature manifest, Rust toolchain identity, WASI/sysroot identity, and runtime
  source digest.
- Release builds publish the runtime artifact and integrity metadata as a
  shipped/cacheable toolchain component.
- User builds resolve the runtime artifact from the managed target root, package
  payload, or remote artifact cache before any rebuild is attempted.
- A cold machine hydrates the artifact; an 8 GB machine reuses it. Neither pays a
  full runtime rebuild for an ordinary user program.
- Rebuilding the runtime is a validator, release, or cache-miss event with a
  bounded job plan, not the default program compile path.

Acceptance:

- A clean user `molt build --target wasm` reuses or hydrates
  `molt_runtime.wasm`; it does not invoke a full runtime Cargo rebuild when the
  content-addressed artifact exists and validates.
- The artifact carries sha256, target feature manifest, import/export closure,
  runtime profile, toolchain identity, and provenance.
- Runtime cache publication works on APDataStore/exFAT through the canonical
  copy/rename fallback; no lane may reroute to legacy roots or disable cache to
  hide filesystem behavior.
- Proof queue has deterministic diagnostics for runtime artifact cache misses,
  corrupt artifacts, unsupported filesystem publish operations, and unexpected
  runtime rebuilds.

## R73.2 Provisioning Layer

Molt owns its build inputs the way a compiler owns a sysroot.

End state:

- Toolchain requirements derive from checked-in contracts and package build
  metadata, not operator memory.
- WASI sysroot, C/C++ compiler, wasm linker, Cython, Meson, Ninja, Python build
  frontend, and package source-extension requirements resolve through one
  provisioning authority.
- Missing dependencies are installed, hydrated, or fail closed with exact
  package/toolchain requirements and a reproducible command.
- Source-recompiled extensions build standalone from their metadata; witness
  bypasses are only viability evidence, never the product path.

Acceptance:

- A package extension build can regenerate from source on a clean machine using
  only the managed target root and package metadata.
- The provisioning layer writes a manifest naming every derived tool, version,
  sysroot, include path, library path, and build-backend input.
- Missing C-API/ABI behavior closes as a reusable primitive or fails closed with
  a precise diagnostic; no package-specific fallback or host-CPython escape.

## Resource Budget

Correct compilers build large projects on ordinary machines.

End state:

- Runtime builds, source-extension builds, and proof lanes have memory-bounded
  job planning.
- Cargo, C/C++, Cython, Meson, and linker fanout obey the same budget authority.
- The default proof profile is safe on an 8 GB machine; higher parallelism is an
  explicit local override.

Acceptance:

- A clean R73 proof lane completes with an 8 GB memory ceiling or fails with a
  deterministic "budget too small for this requested profile" diagnostic before
  thrashing.
- Memory guard custody records the job plan, peak RSS, and any bounded
  quarantine or cleanup action.

## Non-Negotiables

- No per-user-program runtime rebuild when a matching shipped runtime artifact
  exists.
- No manual `MOLT_WASI_SYSROOT` hunting as the normal path.
- No host-CPython fallback for package semantics or extension behavior.
- No rerouting from APDataStore to legacy drives to hide cache or filesystem
  defects.
- No unclassified recurring proof failures: every repeated failure class becomes
  a named diagnostic with a positive fixture and a negative control.
