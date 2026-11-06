# Molt v0.1: what it claims

Status: the claim for the first public release, v0.1. It is not released. The release is held while any
Tier 1 cell fails or any v0.1-blocking row is open; `tools/known_issues.py` lists them.

## The claim

Molt is an ahead-of-time compiler for Python. Molt v0.1 claims exactly two things. Each is proven by one
flagship witness, end to end:

1. **Django offload on native.** A Django app keeps running on CPython and offloads one endpoint to a
   `molt-worker` whose entrypoints Molt compiles to native code (Cranelift, release, Linux x86_64). The
   worker is a single binary; `molt_accel` is the client (`pip install molt[accel]`). Specs:
   [0600](areas/demos/0600_KILLER_DEMO_DJANGO_OFFLOAD.md),
   [0912](areas/demos/0912_MOLT_ACCEL_DJANGO_DECORATOR_SPEC.md),
   [0911](areas/runtime/0911_MOLT_WORKER_V0_SPEC.md).
2. **Falcon OCR on Cloudflare Workers.** Molt compiles the Falcon OCR model driver to one linked
   `wasm32-wasip1` module (release, built on macOS arm64) that runs on Cloudflare Workers. Build
   procedure: [`deploy/cloudflare/README.md`](../../deploy/cloudflare/README.md).

Both claims hold for Python 3.12 and Python 3.15, with the GIL runtime. Molt's WASM output needs an
engine with WebAssembly SIMD128.

**No performance claim.** v0.1 makes no performance claim, including the throughput target in spec
0600. A later performance claim will name the Tier 1 cell it was measured on.

## The support matrix

[`config/release_targets.toml`](../../config/release_targets.toml) is the one authority. Its four
`[[tier1_cell]]` rows are the Tier 1 cells; `molt.release_matrix.TIER1_CELLS` is their typed projection
and the [public contract](PUBLIC_CONTRACT_V1.md) snapshots them.

- **Tier 1** cells block the release. v0.1 publishes release artifacts only for their two build hosts,
  macOS arm64 and Linux x86_64. It publishes no sdist and no platform-independent wheel. On any other
  target, building from the git source is possible but unsupported in v0.1.
- **Tier 2** is every other cell: Python 3.13 and 3.14, the LLVM backend, the JIT tier, the browser and
  split-runtime WASM hosts, WebGPU, C extensions, and free-threaded builds. They are tested, but they do
  not block v0.1.
- The other four release targets are Tier 2 and **not published in v0.1**: macOS x86_64, Linux aarch64,
  Windows x86_64 and Windows arm64.
- **Not run:** the Luau and Rust source emitters and MLIR emission are experimental. No presubmit or
  postsubmit cell runs them.

Tier 1 widens only by owner decision.

## Known issues

Never written by hand. Generate the list from the findings ledger:

    uv run --python 3.12 python -B tools/known_issues.py

It lists the open rows that block v0.1 (including any row not yet classified), the Tier 2 rows and the
rows deferred past v0.1.

## Versions

The version rule is stated once, in the
[public contract](PUBLIC_CONTRACT_V1.md#versioning-and-release-gating).
