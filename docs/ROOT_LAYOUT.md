# Root Layout Contract

This document defines the intended repo-root surface for Molt.

## Goal

The repository root should look like a production OSS project:

- obvious entrypoints
- obvious source/workspace directories
- obvious canonical artifact roots
- no loose scratch files, generated outputs, or one-off engineering debris

## Keep At Root

Root is reserved for:

- top-level manifests and lockfiles
  - `Cargo.toml`, `Cargo.lock`
  - `pyproject.toml`, `uv.lock`
- top-level project docs
  - `README.md`
  - `AGENTS.md`
  - `LICENSE`
  - `ROADMAP.md`
  - `SUPPORTED.md` (thin pointer doc only)
  - `OPTIMIZATIONS_PLAN.md`
- repo config
  - `.gitignore`
  - `.github/`
  - `.cargo/`
  - other small root-scoped config files
- canonical source/workspace directories
  - `src/`, `runtime/`, `crates/`, `tests/`, `tools/`, `docs/`, `examples/`, `demo/`, `bench/`, `ops/`, `packaging/`, `vendor/`, `include/`, `formal/`, `fuzz/`, `wasm/`, `wit/`
- canonical artifact roots already documented by repo policy
  - `target/`, `tmp/`, `logs/`, `build/`, `dist/`

## Do Not Keep At Root

The following do not belong at root:

- generated outputs such as `output.o`, `output.luau`, `manifest.json`, `worker.js`, coverage `.profraw`, ad hoc JSON outputs
- personal tool blobs or editor-specific exports
- one-off patch files and patch-application scripts
- session notes and historical scratch documents
- local denial/test output files

## Canonical Homes

- WASM runner: `wasm/run_wasm.js`
- Rust helper crates and tree-shaking/lazy-loading support: `crates/`
- Shared utility scripts: `tools/scripts/`
- Luau benchmark suite: `bench/luau/`
- Benchmark results: `bench/results/`
- Logs and operational traces: `logs/`
- Ephemeral scratch outputs: `tmp/`
- Cargo/build artifacts: `target/`
- Build staging artifacts: `build/`
- Internal notes, agent memory, archived artifacts, and one-off patch bundles: outside the repo in a local archive location

## Review Rule

This layout separates local artifact locations from publishable source. `logs/`,
`tmp/`, process/guard traces, agent handoffs, and one-off investigation plans must
not be tracked or force-added. Preserve their useful evidence outside the source
checkout or in ignored local artifacts. A directory being a canonical artifact
home does not make its contents public repository material.

Public documentation should serve users or contributors: setup, supported
contracts, architecture and design rationale, testing, maintained benchmarks,
security, and releases. Fold durable conclusions into those owning documents or
regressions instead of adding session reports and duplicate status files.
Curated benchmark data and test fixtures may be tracked when a maintained
consumer requires them; raw per-run logs and machine-specific state may not.

The `public-source-boundary` pre-commit hook rejects tracked files in the local
log/scratch roots, raw benchmark logs and guard traces, and machine-local Claude
settings. Run the configured hooks before landing; ignored files must not be
force-added to bypass this boundary. This path check supplements review of a
document's purpose; it does not classify maintained specifications as scratch.

If a new file is about to land in root, ask:

1. Is it a stable project entrypoint, manifest, or top-level doc?
2. Is it a stable canonical directory?
3. Is root explicitly the documented home for this artifact class?

If the answer to all three is no, it should not be in root.
