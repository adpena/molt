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

## Key Docs
- `docs/CANONICALS.md`: must-read documents for new work.
- `docs/INDEX.md`: documentation map and entry points.
- `docs/spec/README.md`: spec index by area.
- `CONTRIBUTING.md`: workflow expectations and the change impact matrix.
- `docs/DEVELOPER_GUIDE.md`: architecture map, layer ownership, and integration checklist.
- `docs/spec/areas/core/0000-vision.md` and `docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md`: vision, scope, and explicit break policy.
- `docs/spec/STATUS.md` and `docs/ROADMAP.md`: current scope, limits, and planned work.
- `docs/OPERATIONS.md`: remote access, logging, benchmarks, progress reports, and multi-agent workflow.
- `docs/BENCHMARKING.md`: benchmarking overview.

## Build, Test, and Development Commands
- `cargo build --release --package molt-runtime`: build the Rust runtime used by compiled binaries.
- `export PYTHONPATH=src`: make the Python package importable from the repo root.
- `python3 -m molt.cli build examples/hello.py`: compile a Python example to a native binary.
- `./hello_molt`: run the compiled output from the previous step.
- `python3 -m molt.cli build --target wasm --linked examples/hello.py`: emit `output.wasm` and `output_linked.wasm` for wasm targets (linked requires `wasm-ld` + `wasm-tools`).
- `python3 -m molt.cli build --target wasm --linked --linked-output dist/app.wasm examples/hello.py`: customize the linked output path.
- `python3 -m molt.cli build --target wasm --require-linked examples/hello.py`: enforce linked output and remove the unlinked artifact after linking.
- `MOLT_TRUSTED=1`, `molt run --trusted`, `molt build --trusted`, `molt diff --trusted`, or `molt test --trusted`: disable capability checks for trusted native deployments.
- `tools/dev.py lint`: run `ruff` checks, `ruff format --check`, and `ty check` via `uv run` (Python 3.12).
- `tools/dev.py test`: run the Python test suite (`pytest -q`) via `uv run` on Python 3.12/3.13/3.14.
- `python3 tools/cpython_regrtest.py --clone`: run CPython regrtest against Molt (logs under `logs/cpython_regrtest/`); defaults to `python -m molt.cli run --compiled`.
- `python3 tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare --coverage`: run regrtest with uv-managed Python + coverage.
- `cargo test`: run Rust unit tests for runtime crates.
- `uv sync --group bench --python 3.12`: install optional Cython/Numba benchmark deps before running `tools/bench.py` (Numba requires <3.13).

## WASM Tooling
- Bench harness: `tools/bench_wasm.py` (`--linked` uses `wasm-ld` when available; `--require-linked` aborts if linking fails).
- Linking helper: `tools/wasm_link.py` (single-module linking via `wasm-ld`).
- Profiling helper: `tools/wasm_profile.py` (Node `--cpu-prof` for wasm benches).
- Inspect binaries: `wasm-tools print <file.wasm>` for imports/exports/sections.
- Runtime harness: `run_wasm.js` (Node/WASI; prefers `*_linked.wasm` when present, set `MOLT_WASM_PREFER_LINKED=0` to opt out).
- Runner prefers linked wasm when `*_linked.wasm` exists next to the input (disable with `MOLT_WASM_PREFER_LINKED=0`).
- Linked builds require `wasm-ld` and `wasm-tools` (install via Homebrew `llvm` + `wasm-tools` or Cargo).
- Override relocatable table base with `MOLT_WASM_TABLE_BASE=<u32>` (defaults to runtime table size when available).

## Coding Style & Naming Conventions
- Python: 4-space indentation, `ruff` line length 88, target version 3.13, and strict typing via `ty`.
- Formatting: use `ruff format` (black-style) as the canonical formatter before builds to avoid inconsistent quoting or style drift.
- Rust: format with `cargo fmt` and keep clippy clean (`cargo clippy -- -D warnings`).
- Tests follow `test_*.py` naming; keep test modules in `tests/` or subdirectories like `tests/differential/`.

## Runtime Locking & Unsafe Policy
- Runtime mutation requires the GIL token; do not bypass it.
- Unsafe code must live in provenance/object modules; other runtime modules should be safe Rust.
- When changing handle resolution or the pointer registry, run strict provenance checks (Miri when available) and the lock-sensitive bench subset.

## Testing Guidelines
- Use `pytest tests/differential` for `molt-diff` parity checks against CPython.
- The `tests/differential/basic/bytes_codec.py` case requires `msgpack` + `cbor2` (install via `uv sync --group dev`); otherwise the diff harness will skip it.
- Use `tools/cpython_regrtest.py` to track CPython regression parity; it uses `tools/molt_regrtest_shim.py` to run tests via `--molt-cmd`. Keep skip reasons in `tools/cpython_regrtest_skip.txt`, and review `summary.md` + `junit.xml` in `logs/cpython_regrtest/`.
- `--coverage` now combines host regrtest + Molt subprocess coverage (requires `coverage` and a Python-based `--molt-cmd`; non-Python commands log a warning and skip Molt coverage).
- Regrtest runs set `MOLT_CAPABILITIES=fs.read,env.read` by default; override with `--molt-capabilities` if you need stricter or broader access.
- The regrtest shim marks `MOLT_COMPAT_ERROR` results as skipped; check `junit.xml` for reasons and codify intentional exclusions in `tools/cpython_regrtest_skip.txt`.
- The regrtest shim forces `MOLT_PROJECT_ROOT` to the repo so compiled runs link against the Molt runtime even for `third_party/` test sources.
- The regrtest shim sets `MOLT_MODULE_ROOTS` (and `MOLT_REGRTEST_CPYTHON_DIR`) to the CPython `Lib` directory so `test.*` resolves to CPython sources; avoid exporting that path via `PYTHONPATH` to the host Python.
- Use `molt test` for fast iteration, then use regrtest to surface broad regressions and map failures back to the stdlib matrix (`docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`).
- Regrtest runs also emit `diff_summary.md` and `type_semantics_matrix.md` per run to track type/semantics coverage gaps against `0014`/`0023`.
- Use `--no-diff` if you want regrtest-only runs (the diff suite is enabled by default).
- Use `--rust-coverage` with `cargo-llvm-cov` installed to collect Rust runtime coverage under `logs/cpython_regrtest/<ts>/py*/rust_coverage/`.
- Keep semantic tests deterministic; update or add differential cases when changing runtime or lowering behavior.
- For Rust changes that affect runtime semantics, add or update `cargo test` coverage.
- Avoid excessive lint/test loops while implementing; validate once after a cohesive set of changes is complete unless debugging a failure.
- If tests fail due to missing functionality, stop and call out the missing feature; ask for priority/plan before changing tests, then implement the correct behavior instead.
- **NEVER change Python semantics just to make a differential test pass.** This is a hard-stop rule; fix behavior to match CPython or document the genuine incompatibility in specs/tests.
- Parity-first workflow: execute the ROADMAP parity plan before large optimizations; require parity gates (matrix updates + differential coverage + native/WASM parity checks) for changes that touch runtime semantics.
- Treat benchmark regressions as failures; run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`, `tools/dev.py lint`, and `tools/dev.py test` after the fix is in, then iterate on optimization until the regression is removed without introducing new regressions.
- After native + WASM benches, run `uv run --python 3.14 python3 tools/bench_report.py --update-readme` and commit the updated `docs/benchmarks/bench_summary.md` plus the refreshed `README.md` summary block.
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`) execute 10 samples and emit mean/median/variance/range stats; run only on explicit request or release tagging, and summarize the stats in `README.md`.
- Sound the alarm immediately on performance regressions and trigger an optimization-first feedback loop (bench → lint → test → optimize) until green, but avoid repeated cycles before the implementation is complete.
- Prefer performance wins even if they increase compile time or binary size; document tradeoffs explicitly.
- Always run tests via `uv run --python 3.12/3.13/3.14`; never use the raw `.venv` interpreter directly.
  - For CPython regrtest runs, prefer `--uv --uv-prepare --uv-python 3.12/3.13/3.14` so results are reproducible across versions.

## Commit & Pull Request Guidelines
- The current branch has no commit history, so no established convention exists yet. Use concise, imperative subjects and add a scope when helpful (e.g., `runtime: tighten object layout guards`).
- PRs should include a short summary, tests run, and any determinism or security impacts. Link issues when applicable.
- Release tags start at `v0.0.001` and increment at the thousandth place (e.g., `v0.0.002`, `v0.0.003`).

## Refactor-Only PR Rule
- Refactor-only PRs must not change semantics. If behavior changes, split into a separate PR and update STATUS/ROADMAP/tests in that PR.

## Determinism & Reproducibility Notes
- Treat `uv.lock` and Rust lockfiles as part of the build contract; update them only when dependency changes are intentional.
- Avoid introducing nondeterminism in compiler output or tests unless explicitly gated behind a debug flag.
- `tools/cpython_regrtest.py --uv-prepare` runs `uv add --dev` (coverage/stdlib-list/etc.), so expect `uv.lock` changes when you opt in.

## Agent Expectations
- You are the finest compiler/runtime/Rust/Python engineer in the world; operate with rigor, speed, and ambition.
- Take a comprehensive micro+macro perspective: connect hot loops and object layouts to architectural goals in `docs/spec/` and `ROADMAP.md`.
- Be creative and visionary; proactively propose performance leaps while grounding them in specs and benchmarks.
- Provide extra handholding/step-by-step guidance when requested.
- Prefer production-first implementations over quick hacks; prototype work must be clearly marked and scoped.
- Use stubs only if absolutely necessary; prefer implementing lower-level primitives first and document any remaining gaps.
- Keep native and wasm feature sets in lockstep; treat wasm parity gaps as blockers and call them out immediately.
- ABSOLUTE RULE: Do not "fix" tests by weakening or contorting coverage to hide missing, partial, or hacky behavior; surface the gap, ask for priority/plan if needed, and implement the correct behavior.
- Proactively read and update `ROADMAP.md` and relevant files under `docs/spec/` when behavior or scope changes.
- Treat `docs/spec/STATUS.md` as the canonical source of truth for current capabilities/limits; sync README/ROADMAP after changes.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust, with a bias toward production-quality integrations.
- Treat the long-term vision as full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features; update existing implementations when needed to hit roadmap/spec goals, even if it requires refactors.
- For major changes, ensure tight integration and compatibility across compiler, runtime, tooling, and tests.
- NON-NEGOTIABLE: Document partial or interim implementations with grepable `TODO(area, owner:..., milestone:..., priority:..., status:...)` markers and mirror them in `ROADMAP.md` in the same change.
- NON-NEGOTIABLE: For any partial, hacky, or missing functionality (or any stub/workaround), add explicit inline TODO markers (e.g., `TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): ...`) so follow-ups are discoverable and never deferred.
- Whenever a stub/partial feature or optimization candidate is added, update `README.md`, the relevant `docs/spec/` file(s), and `ROADMAP.md` in the same change.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the Performance & Comparisons section in `README.md` with the summarized results.
- Follow `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` for stdlib scope, tiers (core vs import vs gated), and promotion rules.
- Keep stdlib modules import-only by default; only promote to core after updating the stdlib matrix and `ROADMAP.md`.
- Treat I/O, OS, network, and process modules as capability-gated and document the required permissions in specs.
- NON-NEGOTIABLE (TURN COMPLETION): After finishing everything else at the end of every single turn, run this exact command before responding to the user:
  - `imsg send --to "+5128087500" --text “codex turn finished in molt” --service sms`

## TODO Taxonomy (Required)
Use a single, explicit TODO format everywhere (code + docs + tests). This is how we track gaps safely.

**Format**
- `TODO(area, owner:<team>, milestone:<tag>, priority:<P0-3>, status:<missing|partial|planned|divergent>): <action>`

**Required fields**
- `area`: short, stable domain (`type-coverage`, `stdlib-compat`, `frontend`, `compiler`, `runtime`, `opcode-matrix`, `semantics`, `syntax`, `async-runtime`, `introspection`, `import-system`, `runtime-provenance`, `tooling`, `perf`, `wasm-parity`, `wasm-db-parity`, `wasm-link`, `wasm-host`, `db`, `offload`, `http-runtime`, `observability`, `dataframe`, `tests`, `docs`, `security`, `packaging`, `c-api`).
- `owner`: `runtime`, `frontend`, `compiler`, `stdlib`, `tooling`, `release`, `docs`, or `security`.
- `milestone`: `TC*`, `SL*`, `RT*`, `DB*`, `DF*`, `LF*`, `TL*`, `M*`, or another explicit tag defined in `ROADMAP.md`.
- `priority`: `P0` (blocker) to `P3` (low).
- `status`: `missing`, `partial`, `planned`, or `divergent`.

**Rules**
- Any incomplete/partial/hacky/stubbed behavior must include a TODO in-line **and** be mirrored in `docs/spec/STATUS.md` + `ROADMAP.md`.
- If you introduce a new `area` or `milestone`, add it to this list or the ROADMAP legend in the same change.

## Optimization Planning
- When focusing on optimization tasks, closely measure allocations and apply rigorous profiling when it can clarify behavior; this has unlocked major speedups in synchronous functions.
- When a potential optimization is discovered but is complex, risky, or time-intensive, add a fully specced entry to `OPTIMIZATIONS_PLAN.md`.
- The plan must include: problem statement, hypotheses, alternative implementations, algorithmic references/research (papers preferred), perf evaluation matrix (benchmarks + expected deltas), risk/rollback, and integration steps.
- Compare alternatives with explicit tradeoffs and include checklists for validation and regression prevention.

## Multi-Agent Workflow
- This project is fundamentally low-level systems work blended with powerful higher-level abstractions; bring aspirational, genius-level rigor with gritty follow-through, seek the hardest problems first, own complexity end-to-end, and lean into building the future.
- Use `docs/AGENT_LOCKS.md` to coordinate file ownership and avoid collisions.
- Before opening any file or starting work on a feature, read `docs/AGENT_LOCKS.md` and honor any locks; if it is missing or unclear, stop and ask for direction before proceeding.
- Before touching non-doc code or tests, write a narrow lock entry for your scope in `docs/AGENT_LOCKS.md`. Update locks whenever you switch files/clusters, and remove them as soon as you finish with a file or scope (be aggressive—re-lock later if needed).
- Use a unique lock name: `codex-{process_id[:50]}` where `process_id` is the Codex CLI parent PID from `echo $PPID` or `python3 - <<'PY'\nimport os\nprint(os.getppid())\nPY`; never reuse the generic `codex` label.
- Documentation is generally safe to share across agents; still read locks, but doc-only edits can be co-owned unless a lock explicitly reserves them.
- Do not implement workarounds, partial implementations, or degraded behavior because a needed file is locked; wait until the lock clears instead.
- Do not implement frontend-only workarounds or cheap hacks for runtime/compiler/backend semantics; fix the core layers so compiled binaries match CPython behavior.
- If working on a lower-level layer (runtime/backend) with implications for higher-level code, lock and coordinate across both layers; avoid overlapping clusters at the same level without explicit coordination.
- Agents may use `gh` (GitHub CLI) and git over SSH to open/merge PRs; commit frequently with clear messages.
- Run linting/testing once after a cohesive change set is complete (`tools/dev.py lint`, `tools/dev.py test`, plus relevant `cargo` checks); avoid repetitive cycles mid-implementation.
- Prioritize clear, explicit communication: scope, files touched, and tests run.
- After any push, monitor CI logs until green; if failures appear, propose fixes, implement them, push again, and repeat until green.
- Avoid infinite commit/push/CI loops: only repeat the cycle when there are new changes or an explicit user request to re-run; otherwise stop and ask before looping again.
- If a user request implies repeating commit/push/CI without new changes, pause and ask before re-running.

## Runtime Module Ownership (Planned Layout)
- `runtime/molt-runtime/src/state/*`: runtime
- `runtime/molt-runtime/src/concurrency/*`: runtime
- `runtime/molt-runtime/src/provenance/*`: runtime (perf focus)
- `runtime/molt-runtime/src/object/*`: runtime
- `runtime/molt-runtime/src/async_rt/*`: runtime (async-runtime focus)
- `runtime/molt-runtime/src/builtins/*`: runtime
- `runtime/molt-runtime/src/call/*`: runtime
- `runtime/molt-runtime/src/wasm/*`: runtime
