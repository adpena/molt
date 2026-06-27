# Getting Started

This is the shortest path to a real Molt install and first successful run.

## Prerequisites

- Python 3.12+
- Rust toolchain
- `uv`
- A C toolchain (`clang` on macOS/Linux, MSVC or clang on Windows)

Platform details and pitfalls live in:

- [README.md](../README.md)
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
- [OPERATIONS.md](OPERATIONS.md)

## Install

### Package install

- Homebrew / installer / packaging paths: [../packaging/README.md](../packaging/README.md)

### Local repo workflow

```bash
uv sync --group dev --python 3.12
./.venv/bin/molt doctor --json
```

## Verify The Toolchain

```bash
molt doctor --json
```

Expected: JSON output with exit code `0`.

## Build And Run Hello World

`uv sync` installs the `molt` command into `.venv` and onto your path. The
fastest first run is the drop-in form — it builds and runs in one step, just
like `python examples/hello.py`:

```bash
molt run examples/hello.py
```

To produce a standalone optimized binary and run it directly:

```bash
molt build examples/hello.py --release
./hello_molt
```

## Build And Run Profiles

`molt run` defaults to the fast **`dev`** profile (quick iteration) and `molt
build` defaults to the optimized **`release`** profile (shipping artifact). This
is the same convention as Rust's `cargo run` (dev) and `cargo build --release`,
and it is intentional, not a hidden surprise:

- The default is documented at both `molt run --help` and `molt build --help`.
- The verb does **not** lock the profile. Both verbs accept either profile, so
  you can always override with one additive flag:

```bash
molt run app.py --release         # iterate against an optimized build
molt build app.py --profile dev   # fast unoptimized build artifact
```

`--release` is shorthand for `--profile release`.

## Compare Against CPython

```bash
molt compare examples/hello.py
```

## Benchmark A Script

```bash
molt bench --script examples/hello.py
```

## Running From A Source Checkout

If you are working in the repository and have not activated `.venv`, prefix any
command with `uv run --python 3.12` so it uses the project's pinned interpreter:

```bash
uv run --python 3.12 molt run examples/hello.py
```

The module form is equivalent and is what the contributor proof lanes use:

```bash
uv run --python 3.12 python3 -m molt.cli run examples/hello.py
```

## Common Pitfalls

- macOS arm64 + Python 3.14: uv-managed 3.14 can hang; use system `python3.14`
  or stay on 3.12/3.13.
- WASM linked builds require `wasm-ld` and `wasm-tools`.
- After changing `pyproject.toml` or dependency groups, rerun `uv sync` so the
  editable `molt` install in `.venv` stays current.

## Where To Go Next

- Current state: [spec/STATUS.md](spec/STATUS.md)
- Roadmap: [../ROADMAP.md](../ROADMAP.md)
- Benchmarking: [BENCHMARKING.md](BENCHMARKING.md)
- Developer guide: [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
