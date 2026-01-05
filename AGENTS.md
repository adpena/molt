# Repository Guidelines

## Project Structure & Module Organization
- `src/molt/` contains the Python compiler frontend and CLI (`cli.py`).
- `runtime/` hosts Rust crates for the runtime and object model (`molt-runtime`, `molt-obj-model`, `molt-backend`).
- `tests/` holds Python tests, including differential suites in `tests/differential/` and smoke/compliance tests.
- `examples/` contains small programs used in docs and manual validation.
- `docs/spec/` is the architecture and runtime specification set; treat it as the source of truth for behavior.
- `tools/` includes developer scripts like `tools/dev.py`.
- Keep Rust crate entrypoints (`lib.rs`) thin; place substantive runtime/backend logic in focused modules under `src/` and re-export from `lib.rs`.
- Standardize naming: Python modules use `snake_case`, Rust crates use `kebab-case`, and paths reflect module names (avoid ad-hoc casing).

## Build, Test, and Development Commands
- `cargo build --release --package molt-runtime`: build the Rust runtime used by compiled binaries.
- `export PYTHONPATH=src`: make the Python package importable from the repo root.
- `python3 -m molt.cli build examples/hello.py`: compile a Python example to a native binary.
- `./hello_molt`: run the compiled output from the previous step.
- `tools/dev.py lint`: run `ruff` checks, `ruff format --check`, and `ty check` via `uv run` (Python 3.12).
- `tools/dev.py test`: run the Python test suite (`pytest -q`) via `uv run` on Python 3.12/3.13/3.14.
- `cargo test`: run Rust unit tests for runtime crates.
- `uv sync --group bench --python 3.12`: install optional Cython/Numba benchmark deps before running `tools/bench.py` (Numba requires <3.13).

## Coding Style & Naming Conventions
- Python: 4-space indentation, `ruff` line length 88, target version 3.13, and strict typing via `ty`.
- Formatting: use `ruff format` (black-style) as the canonical formatter before builds to avoid inconsistent quoting or style drift.
- Rust: format with `cargo fmt` and keep clippy clean (`cargo clippy -- -D warnings`).
- Tests follow `test_*.py` naming; keep test modules in `tests/` or subdirectories like `tests/differential/`.

## Testing Guidelines
- Use `pytest tests/differential` for `molt-diff` parity checks against CPython.
- Keep semantic tests deterministic; update or add differential cases when changing runtime or lowering behavior.
- For Rust changes that affect runtime semantics, add or update `cargo test` coverage.
- Avoid excessive lint/test loops while implementing; validate once after a cohesive set of changes is complete unless debugging a failure.
- If tests fail due to missing functionality, stop and call out the missing feature; ask for priority/plan before changing tests, then implement the correct behavior instead.
- Treat benchmark regressions as failures; run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `tools/dev.py lint`, and `tools/dev.py test` after the fix is in, then iterate on optimization until the regression is removed without introducing new regressions.
- Sound the alarm immediately on performance regressions and trigger an optimization-first feedback loop (bench → lint → test → optimize) until green, but avoid repeated cycles before the implementation is complete.
- Prefer performance wins even if they increase compile time or binary size; document tradeoffs explicitly.
- Always run tests via `uv run --python 3.12/3.13/3.14`; never use the raw `.venv` interpreter directly.

## Commit & Pull Request Guidelines
- The current branch has no commit history, so no established convention exists yet. Use concise, imperative subjects and add a scope when helpful (e.g., `runtime: tighten object layout guards`).
- PRs should include a short summary, tests run, and any determinism or security impacts. Link issues when applicable.

## Determinism & Reproducibility Notes
- Treat `uv.lock` and Rust lockfiles as part of the build contract; update them only when dependency changes are intentional.
- Avoid introducing nondeterminism in compiler output or tests unless explicitly gated behind a debug flag.

## Agent Expectations
- You are the finest compiler/runtime/Rust/Python engineer in the world; operate with rigor, speed, and ambition.
- Take a comprehensive micro+macro perspective: connect hot loops and object layouts to architectural goals in `docs/spec/` and `ROADMAP.md`.
- Be creative and visionary; proactively propose performance leaps while grounding them in specs and benchmarks.
- Provide extra handholding/step-by-step guidance when requested.
- Prefer production-first implementations over quick hacks; prototype work must be clearly marked and scoped.
- Use stubs only if absolutely necessary; prefer implementing lower-level primitives first and document any remaining gaps.
- Keep native and wasm feature sets in lockstep; treat wasm parity gaps as blockers and call them out immediately.
- Do not "fix" tests by weakening coverage when functionality is missing; surface the missing capability and implement it properly.
- Proactively read and update `ROADMAP.md` and relevant files under `docs/spec/` when behavior or scope changes.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust, with a bias toward production-quality integrations.
- Treat the long-term vision as full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features; update existing implementations when needed to hit roadmap/spec goals, even if it requires refactors.
- For major changes, ensure tight integration and compatibility across compiler, runtime, tooling, and tests.
- Document partial or interim implementations with grepable `TODO(type-coverage, ...)` or `TODO(stdlib-compat, ...)` markers and mirror them in `ROADMAP.md`.
- Whenever a stub/partial feature or optimization candidate is added, update `README.md`, the relevant `docs/spec/` file(s), and `ROADMAP.md` in the same change.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the Performance & Comparisons section in `README.md` with the summarized results.
- Follow `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` for stdlib scope, tiers (core vs import vs gated), and promotion rules.
- Keep stdlib modules import-only by default; only promote to core after updating the stdlib matrix and `ROADMAP.md`.
- Treat I/O, OS, network, and process modules as capability-gated and document the required permissions in specs.

## Optimization Planning
- When a potential optimization is discovered but is complex, risky, or time-intensive, add a fully specced entry to `OPTIMIZATIONS_PLAN.md`.
- The plan must include: problem statement, hypotheses, alternative implementations, algorithmic references/research (papers preferred), perf evaluation matrix (benchmarks + expected deltas), risk/rollback, and integration steps.
- Compare alternatives with explicit tradeoffs and include checklists for validation and regression prevention.

## Multi-Agent Workflow
- Use `AGENT_LOCKS.md` to coordinate file ownership and avoid collisions.
- Agents may use `gh` (GitHub CLI) and git over SSH to open/merge PRs; commit frequently with clear messages.
- Run linting/testing once after a cohesive change set is complete (`tools/dev.py lint`, `tools/dev.py test`, plus relevant `cargo` checks); avoid repetitive cycles mid-implementation.
- Prioritize clear, explicit communication: scope, files touched, and tests run.
- After any push, monitor CI logs until green; if failures appear, propose fixes, implement them, push again, and repeat until green.
