# Performance Goal Loop State

Standing objective: grind the witness-iteration waste ladder from
`tools/PERF_AUTHORITY.md` top-down until every rung is either
`ATTESTED-IMPROVED` or `DOCUMENTED-BLOCKED`.

## Rung Status

| Rank | Rung | Status | Evidence | Next action |
|---:|---|---|---|---|
| 1 | Frontend graph + analysis + lowering | ATTESTED-IMPROVED | `tools/perf_witness_iteration_attestation.json`; commit `5636182fca` | Done. |
| 2 | Backend prepare/codegen | DOCUMENTED-BLOCKED | `tools/perf_goal_r2_backend_blocker.json` | Re-establish a current-main, same-SHA baseline with at least two runs and isolated backend phase/counter breakdown before changing code. |
| 3 | Runtime-compile shared-cache effective hit rate | ATTESTED-IMPROVED | `tools/perf_goal_r3_runtime_cache_attestation.json`; indexed compatibility hydrate is 7.33x faster at 116 entries with 7/7 effective hits before and after | Done. |
| 4 | Seal/validation isolated instrumentation | ATTESTED-IMPROVED | `tools/perf_goal_r4_seal_validation_attestation.json`; NumPy 2.5.1 validation is 1.657x faster across five before/after fresh-process runs | Done. |
| 5 | Relink | ATTESTED-IMPROVED | `tools/perf_goal_r5_relink_attestation.json`; exact cache hydrate now precedes target-staticlib relink, 2.975x faster | Done. |

## Iteration Log

| Iteration | Date | Claim | Rung | Verdict | Summary |
|---:|---|---|---:|---|---|
| 1 | 2026-07-11 | PERF-GOAL-R2 | 2 | DOCUMENTED-BLOCKED | Canonical evidence lacks backend phase breakdown and a legal two-run before baseline, its queue artifacts are unavailable in this worktree, and current `origin/main` has advanced beyond the recorded import-strip frontier. Unblock contract is machine-readable in `tools/perf_goal_r2_backend_blocker.json`. |
| 2 | 2026-07-11 | PERF-GOAL-R3 | 3 | ATTESTED-IMPROVED | Replaced the `O(N)` compatibility sidecar scan with one atomic per-profile compatibility index while retaining a legacy migration fallback. On the same machine with 116 cache entries and one configured compatible artifact, median effective hydrate fell from 17.3408 ms to 2.3644 ms (7.334x, 86.365% reduction); both paths delivered 7/7 effective hits. |
| 3 | 2026-07-11 | PERF-GOAL-R4 | 4 | ATTESTED-IMPROVED | Profiled the real NumPy 2.5.1 seal gate and found 132 object checks recomputing the same relocation roots, producing 3,089 path resolutions and 1.182s cumulative hot-path time. A manifest-scoped resolver moved that invariant out of the object loop; five fresh-process medians fell from 1.441451s to 0.869927s (1.657x, 39.65% reduction) with all source hashes and custody checks unchanged. |
| 4 | 2026-07-11 | PERF-GOAL-R5 | 5 | ATTESTED-IMPROVED | Profiled the real 69.5MB reloc runtime and found target-staticlib reuse forcing `wasm-ld -r` before exact shared-cache lookup. Reordering the existing authorities preserves the link fallback while reducing three-run median phase wall from 3.831193s to 1.287784s (2.975x, 66.39% reduction). All ranked rungs are now attested-improved or documented-blocked. |
