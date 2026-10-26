# Getting Started

Start from a source checkout and compile a small program. Molt is under active
development; consult [current status](spec/STATUS.md) for compatibility limits.

## Prerequisites

- A [supported CPython compiler host](../packaging/INSTALL.md#requirements)
  (the repository pins its development host to CPython 3.12.15 through `uv`)
- Rust via `rustup`, using the version pinned in `rust-toolchain.toml`
- `uv`
- A native C compiler and linker: a platform toolchain on macOS/Linux, or MSVC
  Build Tools with the Windows SDK on Windows. Run Windows builds from a shell
  with that toolchain available.

Platform details and pitfalls live in:

- [README.md](../README.md)
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- [OPERATIONS.md](OPERATIONS.md)

## Install

### Release packages

Installer and package-manager definitions live in
[packaging](../packaging/README.md). Templates in this repository do not by
themselves establish that a release is available or accepted for your target.
Follow the [binary installation guide](../packaging/INSTALL.md) for explicit
private-dependency setup. `molt doctor --json` reports the active source,
interpreter and competing PATH installations; it does not remove or repair them.

### Local repo workflow

Run from the cloned repository root. The commands below work in PowerShell and
POSIX shells without activating a virtual environment:

```bash
uv sync --group dev --python 3.12.15
```

This creates the source bootstrap environment. Run it without a
`UV_PROJECT_ENVIRONMENT` that selects a managed image. Managed development and
proof commands select immutable generations, with the project's default `dev`
and `bench` groups; see [checkout provisioning](ops/INTEGRATION.md#checkout-provisioning)
for selection, explicit external environments and updates.

## Verify The Toolchain

```bash
uv run --python 3.12 molt doctor --json
```

Resolve any reported toolchain errors before building. Exit code `0` means the
doctor checks passed; it does not prove program semantics or target parity.

## Build And Run Hello World

`uv sync` installs `molt` into the project environment; it does not add it to
your current shell's PATH. `uv run` selects that environment. Build and run in
one step:

```bash
uv run --python 3.12 molt run examples/hello.py
```

The first build may compile the compiler backend and runtime. To produce an
optimized binary at an explicit path and run it directly on macOS/Linux:

```bash
uv run --python 3.12 molt build examples/hello.py --release --output hello
./hello
```

On Windows (PowerShell):

```powershell
uv run --python 3.12 molt build examples/hello.py --release --output hello.exe
.\hello.exe
```

Explicit `--output` paths above are relative to the project root. Without it,
use the output path reported by the build; do not assume a binary beside the
source file. Standalone means no host Python interpreter is required, not that
every binary is statically linked or independent of platform libraries.

## Build And Run Profiles

`molt run` defaults to **`dev`** for iteration; `molt build` defaults to the
optimized **`release`** profile. A release-profile build is not release
acceptance or certification.

- The default is documented at both `molt run --help` and `molt build --help`.
- The verb does **not** lock the profile. Both verbs accept either profile, so
  you can always override with one additive flag:

```bash
uv run --python 3.12 molt run examples/hello.py --release
uv run --python 3.12 molt build examples/hello.py --profile dev
```

`--release` is shorthand for `--profile release`.

Source-checkout WASM runtime builds default to nonincremental compilation for
both `dev` and `release`. Explicit iteration controls can select stable family
state; actual incrementality follows the captured Cargo plan. Explicit Cargo
targets and pinned sessions retain precedence, while generated session labels
do not isolate build state. See [build-time controls](OPERATIONS.md#build-time)
for opt-ins and proof/reproducibility constraints.

## Run From Memory (JIT Tier)

In a source checkout on macOS arm64, `molt run --tier jit` skips object
emission, the link and the native executable. The build publishes a JIT image
(`<name>_molt.jit`), and the `molt-jit` runner compiles the program into its
own memory with the same code generator and runs it against the same runtime
bytes as an AOT build:

```bash
uv run --python 3.12 molt run examples/hello.py --tier jit
```

The first run builds the runner and links the runtime image once. Each run
compiles the whole program, the stdlib modules it imports included, and so does
each process that starts the program again (a spawn-mode `multiprocessing`
worker). The JIT tier is therefore slower than `aot` for any program that
imports `os`: `import os, sys` compiles for 7-10 s at about 0.7 GB per start,
where the AOT executable starts in 0.3 s
([costs](cli-reference.md#molt-run)). `aot` stays the default. The JIT
tier never falls back to an AOT build: a construct it cannot run, such as a
cross target, the LLVM backend, an installed (non-checkout) Molt or a host
other than aarch64 macOS, fails before the build starts with a message that
names it. Linux is unverified and Windows is unsupported; see
[design/dev_jit_tier.md](design/dev_jit_tier.md).

## Compare Against CPython

```bash
uv run --python 3.12 molt compare examples/hello.py
```

This compares one program, not the whole verified subset. The compiler frontend
baseline is CPython 3.12+, including 3.15. Its interpreter and target semantics
are separate: `molt build --python-version` selects an admitted target policy
(currently `3.12`, `3.13`, or `3.14`; 3.15 completion is tracked in
[V1-35](agent/V1_HANDOFF_FINDINGS.md)). Select the source
frontend with uv in the same command; it must be at least as new as the target:

```bash
uv run --python 3.14 molt build examples/hello.py --python-version 3.14
```

The target comes from the explicit option, project configuration or supported
project floor; changing the host interpreter alone does not change that policy.
`molt compare` resolves a matching CPython oracle for that target and refuses an
explicit interpreter whose observed minor differs, before either workload runs.
The compiler build continues under the invoking frontend. Numeric `--python`
selections retain their explicit target shorthand; interpreter paths do not
silently select a different guest target.
The oracle uses a fresh empty bytecode read namespace with writes disabled;
ordinary site and environment behavior is retained. Its reported startup timing
is diagnostic and does not compare matched warm image caches or qualify release
performance. See the [compare contract](cli-reference.md#molt-compare).
Differential tooling also uses its launching interpreter as the frontend, so
`uv run --python 3.14 python tests/molt_diff.py --python-version 3.14 <test.py>`
selects the frontend and matching CPython oracle without an interpreter override.
Compiled programs require no host or fallback Python. See
[compatibility](spec/areas/compat/README.md) for version- and target-scoped proof.

## Benchmark A Script

```bash
uv run --python 3.12 molt bench --script examples/hello.py
```

## Alternate Entry Point

The module entry point is equivalent to the installed `molt` command:

```bash
uv run --python 3.12 python -m molt.cli run examples/hello.py
```

## Common Pitfalls

- For a Python/toolchain failure, retain the exact version, platform, command,
  and diagnostic; do not assume a failure on one version applies to all builds.
- WASM linked builds need the [WASM toolchain](spec/areas/tooling/0001-toolchains.md).
  `molt run --target wasm` uses the bundled linked-module runner on Node.js,
  not Wasmtime. A native build does not verify the WASM cell.
- After changing `pyproject.toml` or dependency groups, update a source bootstrap
  with `uv sync`. For a managed development image, use
  [checkout provisioning](ops/INTEGRATION.md#checkout-provisioning) to select a
  new generation; never sync or repair the published image in place.

## Where To Go Next

- Current state: [spec/STATUS.md](spec/STATUS.md)
- Roadmap: [../ROADMAP.md](../ROADMAP.md)
- Benchmarking: [BENCHMARKING.md](BENCHMARKING.md)
- Developer guide: [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
