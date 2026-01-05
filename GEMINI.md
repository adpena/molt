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
- Provide extra handholding/step-by-step guidance when requested.
- Default to production-first implementations; avoid short-term hacks unless explicitly approved.
- Use stubs only if absolutely necessary; build lower-level primitives first and document any remaining gaps.
- Maintain full parity between native and wasm targets; close gaps immediately and treat wasm regressions as blockers.
- Keep Rust crate entrypoints (`lib.rs`) thin; factor substantive runtime/backend code into focused modules and re-export from `lib.rs`.
- Standardize naming: Python modules use `snake_case`, Rust crates use `kebab-case`, and paths reflect module names.
- Do not weaken or contort tests to mask missing functionality; surface the gap and implement the correct behavior.
- Aggressively and proactively update `ROADMAP.md` and the specs in `docs/spec/` when scope or behavior changes.
- Proactively and aggressively plan for native support of popular and growing Python packages written in Rust.
- The project vision is full Python compatibility: all types, syntax, and dependencies.
- Prioritize extending features and refactor current implementations when required to meet roadmap/spec goals.
- For major changes, ensure tight integration and compatibility across the project.
- Document partial or interim implementations with grepable `TODO(type-coverage, ...)` or `TODO(stdlib-compat, ...)` tags and record follow-ups in `ROADMAP.md`.
- Whenever a stub/partial feature or optimization candidate is added, update `README.md`, the relevant `docs/spec/` files, and `ROADMAP.md` in the same change.
- When major features or optimizations land, run benchmarks with JSON output (`python3 tools/bench.py --json`) and update the Performance & Comparisons section in `README.md` with summarized results.
- Install optional benchmark deps with `uv sync --group bench --python 3.12` before recording Cython/Numba baselines (Numba requires <3.13).
- Treat benchmark regressions as build breakers; iterate on optimization + `tools/dev.py lint` + `tools/dev.py test` + benchmarks (`uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json`) until the regression is gone and no new regressions appear, but avoid repeated cycles before the implementation is complete.
- Sound the alarm immediately on performance regressions; prioritize optimization feedback loops before shipping other work without overfitting to tests mid-implementation.
- Favor runtime performance over compile-time speed or binary size unless explicitly directed otherwise.
- Treat `docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md` as the source of truth for stdlib scope, tiering, and promotion rules.
- Keep stdlib modules import-only by default; promote to core only with spec + roadmap updates.
- Capability-gate any OS, I/O, network, or process modules and record the policy in specs.
- Use `ruff format` (black-style) as the canonical Python formatter before builds to avoid inconsistent quoting or formatting drift.
- When a potential optimization is complex or needs extended focus, add a fully specced entry to `OPTIMIZATIONS_PLAN.md` and propose a detailed evaluation plan (alternatives, checklists, perf matrix, regression gates, and research references; prefer papers and modern algorithms).
- Use `AGENT_LOCKS.md` for multi-agent coordination and keep communication explicit about scope, touched files, and tests.
- Agents may use `gh` and git over SSH; commit after cohesive changes and run lint/test once at the end rather than in repeated cycles.
- After any push, monitor CI logs until green; if failures appear, propose fixes, implement them, push again, and repeat until green.
- Always run tests via `uv run --python 3.12/3.13/3.14`; never use the raw `.venv` interpreter directly.

---

*Molt = Python's Dynamism + Systems Engineering Rigor + AI-Augmented Optimization.*
