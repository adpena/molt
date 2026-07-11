# Optimization Matrix State

Persistent objective: optimize targets `wasm-browser split`, `wasm-server/wasi`, and `native exe` across artifact size, build wall-clock, runtime performance, startup, and memory. Release evidence follows A12 Variant-II acceptance; determinism, witness correctness, publication stripping, export contracts, package seals, and memory custody remain hard rails.

## Ranked Rungs

Ranking uses `tools.powerplay_acceptance.rank_backlog`: expected gain divided by validation cost. The largest size prize is temporarily dependency-blocked because the active `wasmld-toolchain` lane owns `tools/wasm_link.py` and `tools/wasm_toolchain.py`; it remains first for re-evaluation next iteration but cannot be attacked concurrently.

Ranking-helper input:

| Item | Expected gain | Validation cost |
| wasm code/data/export tree-shake | 10 | 2 |
| backend prepare/codegen unblock re-evaluation | 9 | 2 |
| duplicate WASM startup scans | 7 | 2 |
| runtime-wasm shared-cache hit rate | 8 | 3 |
| native hello below CPython | 7 | 3 |
| determinism-safe runtime hot loops | 8 | 4 |
| WASM section startup attribution | 5 | 3 |
| release memory ceilings | 5 | 4 |
| browser instantiateStreaming | 4 | 4 |

| Rank | Target | Axis | Rung | Expected gain | Validation cost | Ratio | Status |
|---:|---|---|---|---:|---:|---:|---|
| 1 | wasm-browser split + wasm-server/wasi | size | Provision Binaryen through toolchain custody and tree-shake the measured 20,032,474 B code / 5,275,490 B data / 416,470 B exports | 10 | 2 | 5.00 | DEPENDENCY-BLOCKED: wasmld-toolchain lane owns required files this cycle |
| 2 | wasm-server/wasi | build wall-clock | Re-evaluate backend prepare/codegen instrumentation unblock contract | 9 | 2 | 4.50 | DOCUMENTED-BLOCKED: re-evaluated 2026-07-11; required phase/counter fields remain absent outside the unblock contract |
| 3 | wasm-browser split + wasm-server/wasi | startup | Remove duplicate full-byte import/export-signature scans | 7 | 2 | 3.50 | ATTESTED-IMPROVED OPT-MATRIX-R1: 36.3275 ms -> 22.6561 ms median, 1.6034x |
| 4 | wasm-browser split + wasm-server/wasi | build wall-clock | Audit effective runtime-wasm shared-cache hit rate | 8 | 3 | 2.67 | ATTESTED-IMPROVED OPT-MATRIX-R2: 1,111.2847 ms -> 554.2158 ms median, 2.0051x |
| 5 | native exe | startup | Measure and reduce native hello below the CPython process median | 7 | 3 | 2.33 | DOCUMENTED-BLOCKED OPT-MATRIX-R3: current release median 11.954 ms vs CPython 167.438 ms; exposed duplicate-init candidate failed A12 (10.7531 ms -> 10.9078 ms) and was deleted |
| 6 | all | runtime perf | Profile target-specific hot loops under determinism-safe classes | 8 | 4 | 2.00 | UNATTACKED |
| 7 | wasm-browser split + wasm-server/wasi | startup | Attribute read/instantiate cost by section and active data | 5 | 3 | 1.67 | UNATTACKED |
| 8 | all | memory | Profile release peak RSS / linear-memory ceilings and remove proven excess reservation | 5 | 4 | 1.25 | UNATTACKED |
| 9 | wasm-browser split | startup | Measure browser `instantiateStreaming` independently | 4 | 4 | 1.00 | UNATTACKED |

## Iteration Log

### OPT-MATRIX-R1 — Node WASM metadata scan

- Aperture: split-runtime Node startup before V8 instantiation.
- Profile: import discovery and export-signature discovery each walked the entire module section table, `2 * O(module_bytes)` on the same bytes.
- Artifact: canonical publication stripping applied to the existing release runtime produced a 9,720,086 B final-form artifact; no reserved toolchain files were edited.
- Landing: one `parseWasmMetadata` authority produces imports and export function signatures in one `O(module_bytes)` walk; independent consumers retain thin views over that authority.
- Evidence: `tools/opt_matrix_r1_wasm_metadata_attestation.json` (seven serial alternating fresh Node processes per side; 36.3275 ms -> 22.6561 ms median, 1.6034x; metadata parity; 77,664,256 B maximum RSS).
- Gates: focused pytest 37 passed; link validation 116 passed; fail-closed, table drift, generated WASM ABI, determinism, NumPy 2.5.1 seal, final-form artifact poison, and strict A12 acceptance all pass. No full witness run was consumed because this landing changes loader analysis only and does not change compiled or published WASM bytes.

### OPT-MATRIX-R2 — Exact runtime-WASM cache hydrate

- Aperture: exact-identity release runtime-WASM hydration after a shared-cache hit.
- Profile: the cache artifact was structurally validated before copy and the byte-identical destination was structurally validated again, `2 * O(artifact_bytes)` validation plus one `O(artifact_bytes)` atomic copy.
- Landing: retain the source structural/export validation and atomic-copy failure boundary; delete the redundant destination validation because `_atomic_copy_file` copies the already-validated bytes to a temporary file and atomically replaces the destination only after copy success.
- Evidence: `tools/opt_matrix_r2_runtime_wasm_hydrate_attestation.json` (seven serial alternating release samples on a 45,871,431 B real shared runtime; 1,111.2847 ms -> 554.2158 ms median, 2.0051x; byte identity; 144,908,288 B maximum RSS).
- Teeth: focused cache tests assert one source validation, byte-identical hydration, corrupt-source rejection, and copy-failure rejection.


### OPT-MATRIX-R3 ? Native hello startup floor

- Aperture: native release process entry from the generated C launcher through `molt_main`.
- Profile: queue run `20260711T185428-opt-matrix-r3-startup-profile-1c835cc901814022` built the 4,193,792 B release native artifact successfully in 268.172 s. Seven direct serial launches measured an 11.954 ms median versus 167.438 ms for isolated CPython on the same machine; traced runtime initialization completed in 1.024 ms, with the largest stage 0.254 ms.
- Candidate: native `molt_main` redundantly called idempotent `molt_runtime_init` after the C launcher already initialized the runtime. A target-specific deletion retained WASM and `molt_host_init` initialization and rebuilt successfully in queue run `20260711T190943-opt-matrix-r3-native-rebuild-0349735db84c4a45`.
- A12 verdict: REJECTED. Nine alternating release samples measured 10.7531 ms before and 10.9078 ms after; the candidate produced no net improvement and was removed rather than landed.
- Blocker: native hello is already roughly 14x below the current CPython process bar, and the remaining runtime-init stages are below the Windows process-launch noise floor. Reopen only when profiling exposes a target-specific component costing at least 0.5 ms with a predicted >=2% end-to-end win, using at least seven alternating samples plus cold-start treatment.
- Harness finding: `tools/startup_bench.py` currently couples native and WASM builds, so the successful native artifact was discarded when the independent WASM linked build failed. The blocker contract requires target evidence to survive another target's build failure.
- Evidence: `tools/opt_matrix_r3_native_startup_blocker.json`; no source optimization was retained.
