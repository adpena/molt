# Molt

Molt compiles Python into standalone native binaries and WASM with a Rust-owned
runtime, deterministic tooling, and explicit compatibility boundaries.

It is not trying to be a hidden CPython launcher. Molt targets a verified,
production-minded subset that can keep expanding without giving up control over
performance, packaging, or runtime semantics.

## Why Molt

- **Standalone output**: compiled binaries do not rely on a host Python installation.
- **Rust-first runtime**: hot semantics and stdlib behavior are pushed down into
  runtime primitives and intrinsics instead of Python fallbacks.
- **Deterministic engineering**: parity, performance, and security are treated
  as measurable gates, not vague goals.
- **Cross-target ambition**: native and WASM are both first-class targets.

## Project Contract

- CPython `>=3.12` parity target for supported Molt semantics.
- Full product target: full CPython `>=3.12` parity for the supported subset
  without hidden host fallback.
- Compiled artifacts must work without a host Python installation.
- By design, Molt does not support unrestricted `exec`/`eval`/`compile`,
  runtime monkeypatching, or unrestricted reflection in compiled binaries.

## What Molt Supports Today

- Native AOT compilation through the Rust backend.
- Standalone binary workflows with no runtime dependency on local CPython.
- A growing Rust-first stdlib lowering program with generated audit surfaces.
- Differential testing against CPython as a core validation path.
- WASM build workflows, with cross-target parity still incomplete and actively
  tracked.

## 5-Minute Quickstart

For the full setup and troubleshooting path, use
[docs/getting-started.md](docs/getting-started.md).

```bash
uv sync --group dev --python 3.12   # installs the `molt` command into .venv
molt run examples/hello.py          # build + run, like `python examples/hello.py`
```

`uv sync` puts the `molt` command on your path (in `.venv`). From there the
common commands are:

```bash
molt run app.py             # build and run (fast `dev` profile, like `cargo run`)
molt build app.py --release # produce an optimized standalone binary
./app                       # run the compiled binary directly
molt compare app.py         # diff Molt's output against CPython
```

> `molt run` defaults to the fast `dev` profile and `molt build` defaults to the
> optimized `release` profile — the same convention as `cargo run` / `cargo
> build --release`. Override either with `--profile dev|release` (or the
> `--release` shorthand); both verbs accept both profiles. See
> [docs/getting-started.md](docs/getting-started.md#build-and-run-profiles).

> **From a source checkout without activating the venv**, prefix any command
> with `uv run --python 3.12`, e.g.
> `uv run --python 3.12 molt run examples/hello.py`. The module form
> `python3 -m molt.cli ...` is equivalent and is what the contributor proof
> lanes use.

## Install

- Package and installer paths: see [docs/getting-started.md](docs/getting-started.md)
- Packaging details: [packaging/README.md](packaging/README.md)
- Verification command: `molt doctor --json`

## Status

Current detailed state lives in [docs/spec/STATUS.md](docs/spec/STATUS.md).
Forward priorities live in [ROADMAP.md](ROADMAP.md). The near-term execution
slice lives in [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md).

For compatibility and proof detail:

- Docs index: [docs/INDEX.md](docs/INDEX.md)
- Spec index: [docs/spec/README.md](docs/spec/README.md)
- Compatibility architecture: [docs/spec/areas/compat/README.md](docs/spec/areas/compat/README.md)
- Detailed benchmark report: [docs/benchmarks/bench_summary.md](docs/benchmarks/bench_summary.md)
- Standalone proof workflow: [docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)

## Development

- Contributor map: [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md)
- Operations and multi-agent workflow: [docs/OPERATIONS.md](docs/OPERATIONS.md)
- Benchmark workflows: [docs/BENCHMARKING.md](docs/BENCHMARKING.md)
