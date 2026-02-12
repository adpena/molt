# Optimization Progress Log

Last updated: 2026-02-11
Canonical plan: `OPTIMIZATIONS_PLAN.md`

## Purpose
- Track optimization-program execution progress separately from planning scope.
- Keep a chronological, auditable record of baselines, experiments, outcomes, and rollback decisions.
- Make it clear when work is planning-only versus execution-complete.

## Current Program State
- Program phase: Week 1 (Observability + Hot-Path Attribution), with Week 0 baseline lock complete.
- Execution assumption: implementation work has started for instrumentation/attribution and wasm stabilization, with follow-on Week 2 specialization queued behind wasm parity/perf hardening.
- Status summary: runtime observability counters + profile JSON emission are landed/validated; refreshed uv-first linked-WASM artifact captured with 43/45 pass; wasm triage tooling now captures failure classes and optional wasmtime control-runner evidence.

## Week 0 Checklist
- [x] Build baseline captured (`tools/compile_progress.py`)
- [x] Native benchmark baseline captured (`tools/bench.py --json-out ...`)
- [x] WASM benchmark baseline captured (`tools/bench_wasm.py --json-out ...`)
- [x] Import-call and lowering-coverage snapshot captured
- [x] Baseline-lock entry published with artifact links

## Week 1 Checklist (Observability)
- [x] Runtime hot-path counters added for call IC, attr site-name cache, split lane selection, dict prehash lane, TAQ ingest, and ASCII int parse failures.
- [x] `molt_profile_dump` emits machine-readable `molt_profile_json` payload when `MOLT_PROFILE_JSON=1`.
- [x] Codon subset profile artifacts captured (`sum.py`, `word_count.py`, `taq.py`) with command reproducibility notes.
- [x] Correctness gate run: `cargo check -p molt-runtime -p molt-backend`.
- [x] Correctness gate run: `uv run --python 3.12 pytest -q tests/test_codec_lowering.py` (`33 passed`).
- [x] Add benchmark artifact diff tooling for run-to-run trend comparisons.
- [x] Normalize baseline lock artifacts (build/native/wasm) and publish Week 0 lock entry.

## Run Log

| Date | Phase | Action | Result | Artifact Paths | Next Step |
| --- | --- | --- | --- | --- | --- |
| 2026-02-11 | Week 0 kickoff | Created clean-slate optimization kickoff docs and cross-doc references. | Complete | `OPTIMIZATIONS_PLAN.md`, `ROADMAP.md`, `docs/spec/STATUS.md`, `README.md`, `docs/benchmarks/optimization_progress.md` | Capture baseline benchmark and compile artifacts. |
| 2026-02-11 | Week 1 observability | Added runtime perf counters + JSON profile emission and fixed compile wiring for call-bind instrumentation imports. | Complete | `runtime/molt-runtime/src/constants.rs`, `runtime/molt-runtime/src/call/bind.rs`, `runtime/molt-runtime/src/builtins/attributes.rs`, `runtime/molt-runtime/src/object/ops.rs` | Capture workload evidence and sync docs with initial hotspot findings. |
| 2026-02-11 | Week 1 observability | Captured Codon subset profile evidence with trusted-mode runs and deterministic sample input for file-driven cases. | Complete | `bench/results/optimization_progress/2026-02-11_week1_observability/sum_profile.log`, `bench/results/optimization_progress/2026-02-11_week1_observability/word_count_profile.log`, `bench/results/optimization_progress/2026-02-11_week1_observability/taq_profile.log`, `bench/results/optimization_progress/2026-02-11_week1_observability/summary.md` | Build `tools/bench_diff.py` and finish Week 0 baseline lock artifacts (build/native/wasm). |
| 2026-02-11 | Week 1 observability | Implemented benchmark artifact diff tooling (`tools/bench_diff.py`) and validated it against existing Codon subset snapshots. | Complete | `tools/bench_diff.py`, `bench/results/optimization_progress/2026-02-11_week1_observability/cluster12_vs_13b_diff.json` | Integrate diff tooling into benchmarking docs and weekly workflow. |
| 2026-02-11 | Week 0 baseline lock | Captured compile/native/wasm baseline artifacts and published a lock summary with pass/fail and lowering-density snapshots. | Complete (with recorded failures) | `bench/results/optimization_progress/2026-02-11_week0_baseline_lock/compile_progress/compile_progress.json`, `bench/results/optimization_progress/2026-02-11_week0_baseline_lock/bench_native.json`, `bench/results/optimization_progress/2026-02-11_week0_baseline_lock/bench_wasm.json`, `bench/results/optimization_progress/2026-02-11_week0_baseline_lock/baseline_lock_summary.md` | Open Week 2 specialization and dedicated wasm-stabilization clusters using locked baseline metrics. |
| 2026-02-11 | WASM stabilization | Hardened wasm backend/harness against baseline blockers: fixed missing-local panic class in wasm codegen and made `bench_wasm.py` continue after per-benchmark setup failures. Added wasm link diagnostics hardening (`tools/wasm_link.py`) and optional unlinked mode for targeted debugging. | Partial complete | `runtime/molt-backend/src/wasm.rs`, `tools/bench_wasm.py`, `tools/wasm_link.py` | Address remaining linked-run/runtime parity failures (`bench_async_await`, `bench_channel_throughput`, `bench_bytes_find`, `bench_str_join`) in dedicated wasm clusters. |
| 2026-02-11 | WASM stabilization | Re-ran full linked WASM suite with uv-first interpreter routing and published refreshed artifacts + diff versus week-0 baseline lock. `bench_bytes_find` and `bench_str_join` moved to passing; remaining failures are async/channel benches under Node with V8 Zone OOM during wasm compilation. | Partial complete (43/45 pass) | `bench/results/optimization_progress/2026-02-11_week1_wasm_stabilization/bench_wasm_uv_linked.json`, `bench/results/optimization_progress/2026-02-11_week1_wasm_stabilization/bench_wasm_uv_linked_summary.md`, `bench/results/optimization_progress/2026-02-11_week1_wasm_stabilization/bench_wasm_uv_linked_vs_week0.json` | Triage async/channel failures with wasmtime-native runner as control and reduce Node/V8 compile-memory pressure for linked modules. |
| 2026-02-11 | WASM stabilization | Extended `tools/bench_wasm.py` for focused async/channel triage: added benchmark selection (`--bench`), structured failure classification (`molt_wasm_failure_*`), optional control-runner reruns (`--control-runner wasmtime`), and Node heap tuning (`--node-max-old-space-mb` / `MOLT_WASM_NODE_MAX_OLD_SPACE_MB`). | In progress | `tools/bench_wasm.py`, `docs/benchmarks/optimization_progress.md` | Re-run `bench_async_await` + `bench_channel_throughput` with `--runner node --control-runner wasmtime` and publish fresh stabilization artifact bundle. |

## Experiment Template (Use For Each OPT Track)

```text
Date:
Track ID:
Hypothesis:
Change Summary:
Benchmark Command(s):
Parity/Correctness Gate Command(s):
Result Summary:
Regression Risk:
Rollback Switch:
Follow-up:
```
