# Repository Guidelines

## Project Structure & Module Organization
- `src/molt/` contains the Python compiler frontend and CLI (`cli.py`).
- `runtime/` hosts Rust crates for the runtime and object model (`molt-runtime`, `molt-obj-model`, `molt-backend`).
- `tests/` holds Python tests, including differential suites in `tests/differential/` and smoke/compliance tests.
- `examples/` contains small programs used in docs and manual validation.
- `docs/spec/` is the architecture and runtime specification set; treat it as the source of truth for behavior.
- `tools/` includes developer scripts like `tools/dev.py`.

## Build, Test, and Development Commands
- `cargo build --release --package molt-runtime`: build the Rust runtime used by compiled binaries.
- `export PYTHONPATH=src`: make the Python package importable from the repo root.
- `python3 -m molt.cli build examples/hello.py`: compile a Python example to a native binary.
- `./hello_molt`: run the compiled output from the previous step.
- `tools/dev.py lint`: run `ruff` checks, `ruff format --check`, and `mypy` on `src`.
- `tools/dev.py test`: run the Python test suite (`pytest -q`).
- `cargo test`: run Rust unit tests for runtime crates.

## Coding Style & Naming Conventions
- Python: 4-space indentation, `ruff` line length 88, target version 3.13, and strict typing via `mypy`.
- Rust: format with `cargo fmt` and keep clippy clean (`cargo clippy -- -D warnings`).
- Tests follow `test_*.py` naming; keep test modules in `tests/` or subdirectories like `tests/differential/`.

## Testing Guidelines
- Use `pytest tests/differential` for `molt-diff` parity checks against CPython.
- Keep semantic tests deterministic; update or add differential cases when changing runtime or lowering behavior.
- For Rust changes that affect runtime semantics, add or update `cargo test` coverage.

## Commit & Pull Request Guidelines
- The current branch has no commit history, so no established convention exists yet. Use concise, imperative subjects and add a scope when helpful (e.g., `runtime: tighten object layout guards`).
- PRs should include a short summary, tests run, and any determinism or security impacts. Link issues when applicable.

## Determinism & Reproducibility Notes
- Treat `uv.lock` and Rust lockfiles as part of the build contract; update them only when dependency changes are intentional.
- Avoid introducing nondeterminism in compiler output or tests unless explicitly gated behind a debug flag.
