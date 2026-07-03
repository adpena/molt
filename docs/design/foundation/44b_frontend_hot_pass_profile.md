# 44b Frontend Hot-Pass Profile

Status: live R5c profiling contract, added 2026-07-03.

This document pins the frontend profiling authority for the R5c iteration-loop
lane. The source of truth is `tools/frontend_hot_pass_profile.py`; it runs the
existing `SimpleTIRGenerator` telemetry over a deterministic source corpus and
writes both JSON and Markdown artifacts. Do not hand-rank profiler snippets or
copy stderr from `MOLT_MIDEND_STATS` as evidence.

## Command Contract

Default core-corpus profile:

```powershell
uv run --active --project . --python 3.12 python tools\frontend_hot_pass_profile.py --manifest tests\differential\basic\CORE_TESTS.txt --optimization-profile release --top 12 --fail-on-error
```

The tool fails closed for missing manifests, missing source entries, empty
corpora, and source errors when `--fail-on-error` is set. It emits a compact
verdict with the JSON and Markdown artifact paths:

```text
frontend-hot-pass-profile rc=0 sources=17 statuses={'pass': 17} json=... md=...
```

Artifact schema:

- `ranked_midend_passes`: aggregate pass table from the frontend's existing
  `midend_pass_stats_by_function` authority.
- `ranked_frontend_functions`: cProfile attribution filtered to
  `src/molt/frontend/**`, used to identify the shared primitive beneath hot
  passes.
- `sources`: per-source status, elapsed time, source hash, function count, op
  count, pass stats, and policy outcomes.

## Current Evidence

Command:

```powershell
uv run --active --project . --python 3.12 python tools\frontend_hot_pass_profile.py --manifest tests\differential\basic\CORE_TESTS.txt --optimization-profile release --top 12 --fail-on-error
```

Artifact:
`logs/frontend_profile/profile_20260703T024706Z/frontend_hot_pass_profile.json`

Revision: `b188a55137f1e48a29826885970c2f07c93cfead`

Corpus: 17/17 pass, total frontend elapsed `32623.8123 ms`.

| rank | pass | total_ms | p95_ms | attempted | accepted | degraded | sources |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | cse | 16743.460801 | 671.6759 | 104 | 16 | 6 | 17 |
| 2 | sccp_edge_thread | 4090.8857 | 149.2093 | 88 | 0 | 0 | 17 |
| 3 | prune | 1967.3465 | 61.2928 | 88 | 0 | 0 | 17 |
| 4 | cfg_precanonicalize | 1566.337201 | 143.6485 | 66 | 11 | 0 | 17 |
| 5 | verifier | 1003.404401 | 45.612 | 88 | 82 | 0 | 17 |
| 6 | post_cse_dce | 863.5984 | 30.526 | 98 | 0 | 0 | 17 |

Top shared frontend functions:

| rank | function | cumulative_ms | self_ms | calls |
| --- | --- | ---: | ---: | ---: |
| 1 | `serialization.py:to_json` | 31669.3485 | 12.6555 | 17 |
| 2 | `serialization.py:map_ops_to_json` | 31642.4024 | 88.0999 | 65 |
| 3 | `midend_pipeline.py:_run_ir_midend_passes` | 31231.2671 | 2.3347 | 65 |
| 4 | `midend_pipeline.py:_canonicalize_control_aware_ops` | 30882.376 | 126.4755 | 63 |
| 5 | `midend_pipeline.py:_canonicalize_control_aware_ops_impl` | 30314.296 | 462.2095 | 66 |
| 6 | `midend_pipeline.py:_run_cse_canonicalization_round` | 15969.1435 | 2517.9606 | 96 |
| 7 | `midend_dataflow.py:_compute_sccp` | 7061.2609 | 1695.159 | 362 |
| 8 | `cfg_analysis.py:build_cfg` | 5811.3707 | 66.9352 | 1874 |
| 9 | `midend_canonicalization.py:_canonicalization_state_signature` | 4205.7366 | 1788.1338 | 183664 |
| 10 | `midend_canonicalization.py:_canonicalize_block_with_state` | 3401.6662 | 1159.6708 | 40620 |
| 11 | `midend_dataflow.py:merge_states` | 2980.9108 | 1179.5993 | 18098 |
| 12 | `cfg_analysis.py:_compute_dominators` | 2373.4608 | 958.1581 | 518 |

## First Rust Candidate

The first Rust lowering candidate is the CFG primitive, not the whole CSE pass.
The pass table says CSE is hottest, but cProfile shows `cfg_analysis.build_cfg`
and `_compute_dominators` are repeatedly rebuilt under CSE, SCCP, prune, and
precanonicalization. That boundary is pure data-in/data-out, already has a
small public Python shape (`BasicBlock`, `ControlMaps`, `CFGGraph`), and can be
ported behind one equivalence gate before touching CSE semantics.

Candidate cut:

- Move the hot CFG construction and dominator kernel behind a Rust extension
  helper with the existing Python dataclass shape preserved at the boundary.
- Keep Python as the contract projection until the Rust path proves identical;
  then delete the duplicate Python algorithm and leave only thin projection code.
- Do not special-case CSE. Every midend pass that asks for CFG facts must read
  the same accelerated authority.

Differential gate design:

1. Unit equivalence: run Python CFG and Rust CFG on synthetic structured,
   irreducible, loop, try/except/finally, async, and empty-function MoltOp
   fixtures; compare blocks, predecessors, successors, control maps, dominators,
   and reachability byte-for-byte.
2. Frontend IR equivalence: serialize post-midend JSON for
   `tests/differential/basic/CORE_TESTS.txt` with Python CFG and Rust CFG, then
   compare canonical JSON exactly.
3. Product differential: queue a native differential shard for the same manifest
   with `--jobs 1`, because the frontend JSON equivalence gate proves compiler
   shape while `molt_diff.py` proves runtime parity.
4. Performance: rerun `tools/frontend_hot_pass_profile.py` on the same manifest
   and require the CFG rows plus total frontend elapsed to improve or remain
   within noise; no correctness regression may be traded for profiler movement.

The next profiler run must cite the artifact path and this command line before
claiming an R5c acceleration.
