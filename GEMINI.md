# Molt: AI-Assisted Systems Engineering

Molt leverages Artificial Intelligence as a **development-time accelerator** and **optimization strategist**. Crucially, the final compiled binaries are 100% deterministic machine code with zero runtime AI dependency.

## 🤖 AI Components (Dev-Time Only)

### 1. Invariant Mining & Trace Analysis
Instead of relying solely on static analysis, Molt uses AI to analyze application execution traces.
- **Goal:** Identify "Stable Class Layouts" and "Monomorphic Call Sites" that are likely to remain constant.
- **Outcome:** Suggests Tier 0 optimizations to the compiler core that might be missed by conservative static solvers.

### 2. Guard Synthesis
For Tier 1 (Guarded Python), AI helps synthesize the optimal runtime checks.
- **Goal:** Predict which types are most likely to appear at a dynamic call site.
- **Outcome:** Generates a tiered set of guards (`if type == A: ... elif type == B: ... else: slow_path`) based on observed frequency in training/benchmarking data.

### 3. Automated Test Generation
Molt uses LLMs integrated with `Hypothesis` to explore the edges of Python semantics.
- **Goal:** Find complex, nested Python snippets that cause Molt's output to diverge from CPython.
- **Outcome:** Increases the coverage and reliability of the `molt-diff` testing suite.

### 4. Code Generation & Refactoring
The Molt compiler itself is designed to be "AI-friendly". Its modular IR and clean Rust runtime are optimized for collaborative engineering between human researchers and AI agents.

## 🛡 Security & Determinism Invariants

1. **Deterministic Binaries:** The AI's role ends before the final binary is linked. Every instruction in a Molt executable can be traced back to a specific IR lowering pass and verified against the Technical Specification.
2. **No Hallucinations at Runtime:** There is no "probabilistic execution". All AI-suggested optimizations are validated by the compiler's **Soundness Model** before being committed to native code.
3. **Reproducibility:** Given the same source and the same AI-generated "Optimization Manifest" (JSON), the compiler produces bit-identical binaries.

## ✅ Engineering Expectations
- You are the finest compiler/runtime/Rust/Python engineer in the world; operate with rigor, speed, and ambition.
- Take a comprehensive micro+macro perspective: connect hot paths to architecture, specs, and roadmap goals.
- Be creative and visionary; propose bold optimizations, but validate with measurements and specs.
- When focusing on optimization tasks, closely measure allocations and use rigorous profiling when it would clarify behavior; this has delivered major speedups in synchronous functions.
- Provide extra handholding/step-by-step guidance when requested.
- Default to production-first implementations; avoid short-term hacks unless explicitly approved.
- Use stubs only if absolutely necessary; build lower-level primitives first and document any remaining gaps.
- Maintain full parity between native and wasm targets; close gaps immediately and treat wasm regressions as blockers.
- Keep Rust crate entrypoints (`lib.rs`) thin; factor substantive runtime/backend code into focused modules and re-export from `lib.rs`.
- Standardize naming: Python modules use `snake_case`, Rust crates use `kebab-case`, and paths reflect module names.
- ABSOLUTE RULE: Do not weaken or contort tests to mask missing, partial, or hacky functionality; surface the gap, ask for priority/plan, and implement the correct behavior.
- **NEVER change Python semantics just to make a differential test pass.** This is a hard-stop rule; fix behavior to match CPython or document the genuine incompatibility in specs/tests.
- Aggressively and proactively update `ROADMAP.md` and the specs in `docs/spec/` when scope or behavior changes.
- Treat `docs/spec/STATUS.md` as the canonical source of truth for current capabilities/limits; sync README/ROADMAP after changes.
- Update docs/spec and tests each turn as appropriate to reflect new behavior; if no updates are needed, note that in `CHECKPOINT.md`.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust.
- The project vision is full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features and refactor current implementations when required to meet roadmap/spec goals.
- For major changes, ensure tight integration and compatibility across the project.
- NON-NEGOTIABLE: Document partial or interim implementations with grepable `TODO(area, owner:..., milestone:..., priority:..., status:...)` tags and record follow-ups in `ROADMAP.md` in the same change.
- NON-NEGOTIABLE: For any partial, hacky, or missing functionality (or any stub/workaround), add explicit inline TODO markers (e.g., `TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): ...`) so follow-ups remain discoverable and never deferred.
- Whenever a stub/partial feature or optimization candidate is added, update `README.md`, the relevant `docs/spec/` files, and `ROADMAP.md` in the same change.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the Performance & Comparisons section in `README.md` with summarized results.
- Install optional benchmark deps with `uv sync --group bench --python 3.12` before recording Cython/Numba baselines (Numba requires <3.13).
- Treat benchmark regressions as build breakers; iterate on optimization + `tools/dev.py lint` + `tools/dev.py test` + benchmarks (`uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`) until the regression is gone and no new regressions appear, but avoid repeated cycles before the implementation is complete.
- Run `uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json` for every commit and commit the updated `bench/results/bench.json` (document blockers in `CHECKPOINT.md`). On Apple Silicon, prefer an arm64 interpreter (e.g., `--python /opt/homebrew/bin/python3.14`) so Codon baselines link.
- Run `uv run --python 3.14 python3 tools/bench_wasm.py --json-out bench/results/bench_wasm.json` for every commit and commit the updated `bench/results/bench_wasm.json` (document blockers in `CHECKPOINT.md`); also run the native bench and summarize WASM vs CPython ratios in `README.md`.
- After native + WASM benches, run `uv run --python 3.14 python3 tools/bench_report.py --update-readme` and commit the updated `docs/benchmarks/bench_summary.md` plus the refreshed `README.md` summary block.
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`) execute 10 samples and emit mean/median/variance/range stats; run only on explicit request or release tagging, and summarize the stats in `README.md`.
- Sound the alarm immediately on performance regressions; prioritize optimization feedback loops before shipping other work without overfitting to tests mid-implementation.
- Favor runtime performance over compile-time speed or binary size unless explicitly directed otherwise.
- Treat `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` as the source of truth for stdlib scope, tiering, and promotion rules.
- Keep stdlib modules import-only by default; promote to core only with spec + roadmap updates.
- Capability-gate any OS, I/O, network, or process modules and record the policy in specs.
- Use `ruff format` (black-style) as the canonical Python formatter before builds to avoid inconsistent quoting or formatting drift.
- When a potential optimization is complex or needs extended focus, add a fully specced entry to `OPTIMIZATIONS_PLAN.md` and propose a detailed evaluation plan (alternatives, checklists, perf matrix, regression gates, and research references; prefer papers and modern algorithms).
- This project is fundamentally low-level systems work blended with powerful higher-level abstractions; bring aspirational, genius-level rigor with gritty follow-through, seek the hardest problems first, own complexity end-to-end, and lean into building the future.
- Use `docs/AGENT_LOCKS.md` for multi-agent coordination and keep communication explicit about scope, touched files, and tests.
- Before opening any file or starting work on a feature, read `docs/AGENT_LOCKS.md` and honor any locks; if it is missing or unclear, stop and ask for direction before proceeding.
- Before touching non-doc code or tests, write a narrow lock entry for your scope in `docs/AGENT_LOCKS.md`. Update locks whenever you switch files/clusters, and remove them as soon as you finish with a file or scope (be aggressive—re-lock later if needed).
- Use a unique lock name: `codex-{process_id[:50]}` where `process_id` is the Codex CLI parent PID from `echo $PPID` or `python3 - <<'PY'\nimport os\nprint(os.getppid())\nPY`; never reuse the generic `codex` label.
- Documentation is generally safe to share across agents; still read locks, but doc-only edits can be co-owned unless a lock explicitly reserves them.
- Do not implement workarounds, partial implementations, or degraded behavior because a needed file is locked; wait until the lock clears instead.
- Do not implement frontend-only workarounds or cheap hacks for runtime/compiler/backend semantics; fix the core layers so compiled binaries match CPython behavior.
- If working on a lower-level layer (runtime/backend) with implications for higher-level code, lock and coordinate across both layers; avoid overlapping clusters at the same level without explicit coordination.
- Use `docs/AGENT_MEMORY.md` as an append-only coordination log during parallel work: record intended scope before starting and summarize changes/tests/benchmarks after finishing.
- When multiple agents are active, read both `docs/AGENT_LOCKS.md` and `docs/AGENT_MEMORY.md` first to avoid overlapping scopes, then update the memory log as you progress.
- Agents may use `gh` and git over SSH; commit after cohesive changes and run lint/test once at the end rather than in repeated cycles.
- After any push, monitor CI logs until green; if failures appear, propose fixes, implement them, push again, and repeat until green.
- Avoid infinite commit/push/CI loops: only repeat when there are new changes or an explicit request to re-run; otherwise stop and ask before looping again.
- If a user request implies repeating commit/push/CI without new changes, pause and ask before re-running.
- Release tags start at `v0.0.001` and increment at the thousandth place (e.g., `v0.0.002`, `v0.0.003`).
- Always run tests via `uv run --python 3.12/3.13/3.14`; never use the raw `.venv` interpreter directly.
- Always update `CHECKPOINT.md` after each assistant turn and when nearing context compaction; include an ISO-8601 timestamp and `git rev-parse HEAD` (note if dirty) for freshness checks.

## TODO Taxonomy (Required)
Use a single, explicit TODO format everywhere (code + docs + tests).

**Format**
- `TODO(area, owner:<team>, milestone:<tag>, priority:<P0-3>, status:<missing|partial|planned|divergent>): <action>`

**Required fields**
- `area`: `type-coverage`, `stdlib-compat`, `frontend`, `compiler`, `runtime`, `opcode-matrix`, `semantics`, `syntax`, `async-runtime`, `introspection`, `import-system`, `runtime-provenance`, `tooling`, `perf`, `wasm-parity`, `wasm-db-parity`, `wasm-link`, `wasm-host`, `db`, `offload`, `http-runtime`, `observability`, `dataframe`, `tests`, `docs`, `security`, `packaging`, `c-api`.
- `owner`: `runtime`, `frontend`, `compiler`, `stdlib`, `tooling`, `release`, `docs`, `security`.
- `milestone`: `TC*`, `SL*`, `RT*`, `DB*`, `DF*`, `LF*`, `TL*`, `M*`, or another explicit tag defined in `ROADMAP.md`.
- `priority`: `P0` (blocker) to `P3` (low).
- `status`: `missing`, `partial`, `planned`, or `divergent`.

**Rules**
- Any incomplete/partial/hacky/stubbed behavior must include a TODO in-line and be mirrored in `docs/spec/STATUS.md` + `ROADMAP.md`.
- If you introduce a new `area` or `milestone`, add it to this list or the ROADMAP legend in the same change.

---

*Molt = Python's Dynamism + Systems Engineering Rigor + AI-Augmented Optimization.*
