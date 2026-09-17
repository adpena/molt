# Getting Started

Start from a source checkout and compile a small program. Molt is under active
development; consult [current status](spec/STATUS.md) for compatibility limits.

## Prerequisites

- Python 3.12+ (the examples select 3.12 through `uv`)
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

### Local repo workflow

Run from the cloned repository root. The commands below work in PowerShell and
POSIX shells without activating a virtual environment:

```bash
uv sync --group dev --python 3.12
```

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

## Compare Against CPython

```bash
uv run --python 3.12 molt compare examples/hello.py
```

This compares one program, not the whole verified subset. The compiler host
interpreter and target semantics are separate: `molt build --python-version`
selects a supported target policy (`3.12`, `3.13`, or `3.14`). See
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
- WASM linked builds require `wasm-ld` and `wasm-tools`; running WASM also
  requires the appropriate host, such as Wasmtime for `molt run --target wasm`.
  A native build does not verify the WASM cell.
- After changing `pyproject.toml` or dependency groups, rerun `uv sync` so the
  editable `molt` install in `.venv` stays current.

## Where To Go Next

- Current state: [spec/STATUS.md](spec/STATUS.md)
- Roadmap: [../ROADMAP.md](../ROADMAP.md)
- Benchmarking: [BENCHMARKING.md](BENCHMARKING.md)
- Developer guide: [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
