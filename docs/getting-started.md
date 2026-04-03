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
export PYTHONPATH=src
uv run --python 3.12 python3 -m molt.cli doctor --json
```

## Verify The Toolchain

```bash
molt doctor --json
```

Expected: JSON output with exit code `0`.

## Build And Run Hello World

```bash
export PYTHONPATH=src
uv run --python 3.12 python3 -m molt.cli build examples/hello.py
./hello_molt
```

You can also use the run wrapper directly:

```bash
export PYTHONPATH=src
uv run --python 3.12 python3 -m molt.cli run examples/hello.py
```

## Compare Against CPython

```bash
export PYTHONPATH=src
uv run --python 3.12 python3 -m molt.cli compare examples/hello.py
```

## Benchmark A Script

```bash
export PYTHONPATH=src
uv run --python 3.12 python3 -m molt.cli bench --script examples/hello.py
```

## Common Pitfalls

- macOS arm64 + Python 3.14: uv-managed 3.14 can hang; use system `python3.14`
  or stay on 3.12/3.13.
- WASM linked builds require `wasm-ld` and `wasm-tools`.
- Keep `PYTHONPATH=src` when running from a local checkout.

## Where To Go Next

- Current state: [spec/STATUS.md](spec/STATUS.md)
- Roadmap: [../ROADMAP.md](../ROADMAP.md)
- Benchmarking: [BENCHMARKING.md](BENCHMARKING.md)
- Developer guide: [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)
