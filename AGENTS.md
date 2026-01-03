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
- `uv sync --group bench --python 3.12`: install optional Cython/Numba benchmark deps before running `tools/bench.py` (Numba requires <3.13).

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

## Agent Expectations
- Prefer production-first implementations over quick hacks; prototype work must be clearly marked and scoped.
- Proactively read and update `ROADMAP.md` and relevant files under `docs/spec/` when behavior or scope changes.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust, with a bias toward production-quality integrations.
- Treat the long-term vision as full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features; update existing implementations when needed to hit roadmap/spec goals, even if it requires refactors.
- For major changes, ensure tight integration and compatibility across compiler, runtime, tooling, and tests.
- Document partial or interim implementations with grepable `TODO(type-coverage, ...)` or `TODO(stdlib-compat, ...)` markers and mirror them in `ROADMAP.md`.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the Performance & Comparisons section in `README.md` with the summarized results.
- Follow `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for stdlib scope, tiers (core vs import vs gated), and promotion rules.
- Keep stdlib modules import-only by default; only promote to core after updating the stdlib matrix and `ROADMAP.md`.
- Treat I/O, OS, network, and process modules as capability-gated and document the required permissions in specs.

## Multi-Agent Workflow
- Use `AGENT_LOCKS.md` to coordinate file ownership and avoid collisions.
- Agents may use `gh` (GitHub CLI) and git over SSH to open/merge PRs; commit frequently with clear messages.
- Run extensive linting and testing before merges (`tools/dev.py lint`, `tools/dev.py test`, plus relevant `cargo` checks).
- Prioritize clear, explicit communication: scope, files touched, and tests run.
