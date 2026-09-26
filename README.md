# Molt

Molt is an optimizing Python-to-native and WebAssembly compiler with a Rust-owned
runtime and explicit compatibility boundaries.

Molt is under active development, not a drop-in replacement for CPython. It targets
an expanding verified subset without a hidden host-Python fallback. Language,
stdlib, third-party package support, and native/WASM parity remain incomplete;
see [current status](docs/spec/STATUS.md) before choosing a workload.

The release priority is a stable `v1.0` contract, with a scoped, evidence-backed
`v0.0.1` as the initial milestone. Neither implies full
Python or ecosystem compatibility beyond its verified subset. See the
[release milestone](ROADMAP.md#first-release-milestone).
Release readiness also requires the declared workload and resource budgets in
the [performance authority](tools/PERF_AUTHORITY.md#v10-acceptance-scope).
Publication requires same-source semantic exit evidence; stable releases also
require a signed H0 phase exit. See the [release contract](packaging/PACKAGING.md).
These gates do not imply that the remaining release matrix has passed.
The [Pact acceptance lanes](docs/agent/PROOF_QUEUE.md#named-pact-witness-lanes)
bind native/WASM execution and oracle parity to portable, source-pinned receipts;
package seals or successful builds alone do not establish acceptance.
The [verified-subset contract](docs/spec/areas/compat/contracts/verified_subset_contract.md)
defines test selection, source-change checks, and the exact cross-target pass law.

## Why Molt

- **Standalone output**: compiled binaries do not rely on a host Python installation.
- **Rust-first runtime**: hot semantics and stdlib behavior are pushed down into
  runtime primitives and intrinsics instead of Python fallbacks.
- **Evidence-backed compatibility**: differential tests compare supported
  behavior with CPython; support is scoped to the tested configuration.
- **Co-equal targets**: native and WASM correctness, determinism, performance,
  and binary size are engineering goals, not a claim of completed parity.

## Project Contract

- CPython `>=3.12` parity target within the verified subset.
  Current target-version policies are `3.12`, `3.13`, and `3.14`; accepting a
  version policy does not certify every feature on that version.
- Compiled artifacts must work without a host Python installation.
- CPython module names retain their standard-library role. Molt-specific
  helpers live under `moltlib`, including `moltlib.io.stream` for bounded file
  iteration; they are not extensions to CPython's `io` namespace.
- Support is specific to Python version, OS, architecture, backend, runtime
  profile, and capabilities. Windows, macOS, Linux, and WASM are in scope;
  unverified cells are not implied by a pass on another configuration.
- Dynamic execution and reflection follow the
  [dynamic-semantics policy](docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md).
  Scoped mutation and introspection are not a promise of arbitrary runtime
  monkeypatching or unrestricted dynamism.

## What Molt Supports Today

- Native AOT compilation through Cranelift by default, with LLVM opt-in.
  The experimental Rust source emitter is a separate backend.
- Standalone binary workflows with no runtime dependency on local CPython.
- A growing Rust-first stdlib lowering program with generated audit surfaces.
- Differential testing against CPython as a core validation path.
- WASM build workflows, with cross-target parity still incomplete and actively
  tracked.
- Third-party integration through shared import/runtime primitives and
  source-recompiled extensions. C-API symbol coverage alone does not establish
  package compatibility. See the [extension ABI contract](docs/spec/areas/compat/contracts/libmolt_extension_abi_contract.md)
  for header and runtime-linkage requirements, and the [ecosystem matrix](docs/spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md)
  for package support.

## Source Checkout Quickstart

For the full setup and troubleshooting path, use
[docs/getting-started.md](docs/getting-started.md).

```bash
uv sync --group dev --python 3.12
uv run --python 3.12 molt doctor --json
uv run --python 3.12 molt run examples/hello.py
```

Run these commands from the repository root after installing the
[prerequisites](docs/getting-started.md#prerequisites). `uv sync` creates the
project environment but does not activate it; `uv run` selects it without
shell-specific activation. The first build may compile the backend and runtime.

```bash
uv run --python 3.12 molt run examples/hello.py --release
uv run --python 3.12 molt compare examples/hello.py
```

`molt run` defaults to `dev`; `molt build` defaults to `release`. Both accept
`--profile dev|release` and `--release`. For explicit output paths and Windows,
macOS, and Linux invocation, see
[build and run](docs/getting-started.md#build-and-run-hello-world).

These profiles select optimization of **your program**, not the compiler itself.
Release bundles ship a production-optimized compiler that is reused for both
profiles, alongside the matching runtime sources. Compiler developers can opt
into a development host build with `MOLT_BACKEND_PROFILE=dev` in a source checkout.
`--diagnostics` reports the compiler identity/profile separately from the program
and runtime profiles, together with build-phase and cache information.

Build/run progress adapts to stderr: an interactive indicator on terminals,
bounded phase lines in logs. Use `--headless` or `--progress plain` for plain
status, `--progress off` to disable status, or `--quiet` to suppress successful
compiler notices too. Errors and guest output remain visible; JSON mode emits
no progress chatter.

## Install

Release bundles keep one immutable CLI/compiler source and a separate private
dependency environment. `molt setup --install-cli-dependencies` explicitly
authorizes dependency installation; normal launches do not install or repair it.
Compilation and readiness checks do not install Rust targets either; missing
toolchains produce actionable setup diagnostics for you to review and run.
`molt doctor` identifies the active installation and competing PATH entries
without changing them. See [binary installation](packaging/INSTALL.md).

- Package and installer paths: see [docs/getting-started.md](docs/getting-started.md)
- Packaging details: [packaging/README.md](packaging/README.md)
- Toolchain diagnostics: `uv run --python 3.12 molt doctor --json` (not a
  compatibility or release certification).

## Status

Current detailed state lives in [docs/spec/STATUS.md](docs/spec/STATUS.md).
Forward priorities live in [ROADMAP.md](ROADMAP.md). The near-term execution
slice lives in [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md).

For compatibility and proof detail:

- Docs index: [docs/INDEX.md](docs/INDEX.md)
- Spec index: [docs/spec/README.md](docs/spec/README.md)
- Compatibility architecture: [docs/spec/areas/compat/README.md](docs/spec/areas/compat/README.md)
- Dated benchmark report: [docs/benchmarks/bench_summary.md](docs/benchmarks/bench_summary.md)
  (read its source revision, timing mode, and comparator coverage; it does not
  establish current performance or superiority to unmeasured compilers).
- Standalone proof workflow: [docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md](docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md)

## Development

- Contributor map: [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md)
- Operations and multi-agent workflow: [docs/OPERATIONS.md](docs/OPERATIONS.md)
- Build storage, output lifetimes, and proof custody: [docs/agent/PROOF_QUEUE.md](docs/agent/PROOF_QUEUE.md#cargo-output-placement)
- Shared frontend analysis, streaming AST/tooling identities, and live import resolution:
  [source scan authority](docs/internals/source_scan_authority.md)
- Benchmark workflows: [docs/BENCHMARKING.md](docs/BENCHMARKING.md)
