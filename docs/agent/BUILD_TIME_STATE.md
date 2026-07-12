# Build-Time State

## Ranked Rungs

| Rank | Rung | Live evidence | Gain / validation cost | Status |
|---:|---|---|---|---|
| 1 | Split-app Binaryen result reuse | R11 measured 1 -> 0 warm wasm-opt runs and split-runtime processing 115.665s -> 64.188s | High / medium | BUILD-TIME-R11 COMPLETE |
| 2 | Native-support import-scan reuse | R10 measured 91 -> 0 warm source parses and 91/91 persisted import-scan hits | High / medium | BUILD-TIME-R10 COMPLETE |
| 3 | Frontend lowering cache admission | R9 measured 0/145 -> 145/145 hits and 199.140s -> 0.000s re-lowered | Very high / medium | BUILD-TIME-R9 COMPLETE |
| 4 | Runtime-wasm cache-first combined-build ordering | Original 319.6s -> 259.8s signal did not reproduce after origin/main advanced | Marginal / medium | BUILD-TIME-R6 DROPPED |
| 5 | Import-strip / split restoration frontier | Every sample reaches linked validation after app/runtime generation | Correctness blocker / separate owner | OBSERVED |
| 6 | Runtime crate split for true source misses | Current-origin fingerprint population sample was 576.5s | Medium / high | OPEN |
| 7 | R73.3 provisioned extension archives | Cold ecosystem cross-compile remains outside warm iteration floor | Very high / very high | OPEN |

## Iteration Log

### BUILD-TIME-R12 - split-runtime whole-artifact parse reuse (awaiting witness)

- Static audit aperture: the native split-app path invokes contract restoration three times (`native-link`, `optimized-app`, and `publication-strip`). Each restoration walked the same unchanged post-restoration bytes once for every app contract entry (`molt_main`, memory, table), so the contract check alone performed 9 full `parse_wasm_module_facts` passes per native build. The export-only mutations do not alter function, table, memory, import, element, or code index spaces; the WebAssembly binary authority keeps exports in their own section, so one facts snapshot plus an updated export map is sufficient.
- Complete whole-artifact scan inventory for the split/publication path: app pre/post table-ref materialization; split-app optimizer input/output parse and serialization; memory-min rewrite; contract restoration; runtime canonical-export discovery and tree-shake input/output; linked/app/runtime publication-section stripping; linked debug stripping and standard-section canonicalization; app/runtime metrics; linked/app/runtime fail-closed structural validation. The new phase sidecar counts linker-owned `full_binary_parses`, `section_walks`, and `reserializations`, plus `redundant_parses_eliminated`; build diagnostics preserve these beside R11 `split_app_*` counts without changing R8 attribution, R9 warm-pair, or R10 graph counters.
- Rung: `_restore_split_runtime_contract_exports` now parses the restored module once, reuses that immutable facts snapshot across all three contract entries, and updates only the local export map after an export insertion. The superseded per-entry parse lane is deleted. Static operation count is 9 -> 3 full contract parses on the native witness path, eliminating 6 repeated whole-artifact parses; non-native split builds invoke two restoration stages and move 6 -> 2, eliminating 4.
- Light proof: focused contract/parser test records exactly one parser invocation and `redundant_parses_eliminated=2` per restoration; diagnostics extraction preserves all integer `split_app_*` and `wasm_whole_artifact_*` counters. Focused tests pass (`2 passed`; diagnostics `3 passed`); the broader pure-Python linker file is `140 passed, 3 failed`, where the three failures are existing cache-sensitive/synthetic-fixture issues outside this rung (R11 optimizer cache hits bypass a monkeypatched runner in two tests; one synthetic byte suffix is not a valid wasm section for `wasm_metrics`).
- Heavy verification is intentionally parked while the Fable E1 witness owns the shared-cache build slot. Required completion evidence after quiescence: one R12 witness row with emitted operation counts, byte-identical `output.wasm`/`app.wasm`/`molt_runtime.wasm`, unchanged-or-better execution frontier, held-bench/determinism and fail-closed gates.

### BUILD-TIME-R11 - split-app Binaryen result reuse

- Warm re-attribution row `20260712T084410-pact-witness-acceptance-474acaca80b74c1d` retained 145/145 frontend-lowering hits and 91/91 persisted native-support import-scan hits. WASM link was 200.075s / 42.36% inclusive; split-runtime processing was the dominant link leaf at 115.665s / 24.49% of total, ahead of wasm-link core at 51.677s.
- Operation profile: every identical warm app issued one split-app optimization request and one Binaryen `wasm-opt` pipeline over the same 29,956,413-byte pre-optimization artifact. Binaryen documents that its tools are deterministic for identical inputs, so the missing authority was a content-addressed result cache keyed by app bytes, reference bytes, contract exports, optimize policy, executable, and Binaryen version.
- Fix: `tools/wasm_link.py` now owns a durable split-app optimizer cache beside the existing runtime tree-shake cache. The superseded unconditional repeated optimization lane is deleted. Link diagnostics publish request, cache-hit/miss, and wasm-opt-run operation counts; build diagnostics expose them as `wasm_link_operation_counts`.
- Population row `20260712T090036-pact-witness-acceptance-3a65c6530d2d46a0` recorded request=1, miss=1, wasm-opt runs=1 and is excluded. Authoritative warm row `20260712T091505-pact-witness-acceptance-d1dd6780b4504901` recorded request=1, hit=1, wasm-opt runs=0. Split-runtime processing fell from 115.665s to 64.188s (-51.477s / -44.51%); inclusive WASM link fell from 200.075s to 152.467s (-47.608s / -23.79%). Total fell from 472.297s to 442.764s, but operation counts are the primary host-noise-robust claim.
- Artifact identity held across baseline, population, and warm hit: `output.wasm` SHA-256 `9e37e67cbc91be7bd60088d1b56be13922eeee8eced67b30d6a7eddd5359847c`; `app.wasm` SHA-256 `91b9b9d64847989cf628acda4a593f7f9514159e6dcae5815ed0a34a267c77b1`; `molt_runtime.wasm` SHA-256 `4b1db262d828734fff9604ac65ab965fbd80cf74fca8336b6ae56346a97d3987`. Both patched rows reached the identical `_multiarray_umath` `Py_mod_exec` unhashable-type frontier.
- Focused proof covers cache miss then hit, exact output reuse, attestation reuse, and 1 -> 0 optimizer runs. Next prize: profile the remaining 64.188s split-runtime floor by full-binary parse/rewrite counts, especially repeated contract restoration, table-ref materialization, publication stripping, and validation scans.

### BUILD-TIME-R10 - native-support import-scan reuse

- Warm re-attribution row `20260712T063806-pact-witness-acceptance-2360b9d2baf44bb6` confirmed module graph as the dominant leaf phase: 263.006s / 40.78%, ahead of wasm link at 163.084s / 25.29%; frontend lowering remained 145/145 cache hits.
- Root cause: native extension support-source closure supplied precomputed imports by reparsing all 91 no-prune support files on every build, bypassing the existing persisted import-scan authority. This made warm graph discovery O(total support-source bytes) instead of O(91 metadata/content validations).
- Fix: one native-support slice authority now owns import extraction and optional pruned-source materialization. No-root support sources read/write the canonical persisted import-scan cache; the superseded unconditional warm AST parse lane is deleted. Build diagnostics publish machine-checkable iteration, request, persisted-hit/miss, parse, and prune counts.
- Population row `20260712T073638-pact-witness-acceptance-44c58a69249a47cb`: 91 persisted misses and 91 source parses, intentionally excluded. Warm rows `20260712T074833-pact-witness-acceptance-742721a8653d451a`, `20260712T075533-pact-witness-acceptance-7c5831db86444ef9`, and `20260712T080220-pact-witness-acceptance-752925bc516542c4`: each records 91/91 persisted hits and zero native-support source parses.
- Module-graph seconds were 235.949s, 227.895s, and 235.695s (median 235.695s), versus the current-tree warm baseline 263.006s: -27.311s / -10.38%. Total warm seconds were 411.547s, 397.993s, and 411.716s (median 411.547s); relative phase share rose because other warm phases contracted more, so the operation counts and phase seconds are the primary claim.
- Artifact identity held across baseline and all three warm rows: `output.wasm` SHA-256 `9e37e67cbc91be7bd60088d1b56be13922eeee8eced67b30d6a7eddd5359847c`; `app.wasm` SHA-256 `338e7f89685500f1c8921ae71c2fa5a3d576ce3921b8fa5389715e8df3562382`. Every row reached the same known `RuntimeError: unreachable` witness frontier.
- Focused proof: 19 import-collection/build-diagnostics tests passed. Gates: fail_closed_gate, check_table_drift, gen_wasm_abi --check, NumPy 2.5.1 seal verification, artifact_poison_gate, and determinism_perf_gate passed. Clean-main held-bench row `20260712T082140-perf-scoreboard-ec71b9b952574c46` was authoritative and quiescent: both warm smoke cells stayed green (`bench_sum` 3.19x, `bench_bytes_find` 2.80x; zero warm reds). The board remained non-green only on the independent `bench_sum` cold-start budget (453ms versus 380ms), so no cold-start improvement is claimed.
- Next prize: split-runtime processing / wasm link now dominates the remaining non-graph warm floor; profile split-runtime app/runtime validation and binary rewrites by operation count before selecting R11.

### BUILD-TIME-R9 - effective frontend lowering cache admission

- Root cause: every witness invocation runs Molt from a fresh uv overlay installation (`C:\Molt\.uv-cache\builds-v0\.tmp*`). `_compiler_root()` therefore changed every run, and the source-tree fingerprint fallback embedded stat metadata (`mtime`/`ctime` and file count) from that ephemeral copy into the frontend semantic compiler fingerprint. Identical compiler bytes produced a different context digest for all 145 modules. The final residual miss was the same provenance bug in `_module_lowering_cache_key`, which keyed the generated namespace module on its run-specific physical output path.
- Fix: source-tree cache fingerprints are now content identities only (schema, scope, extra inputs, root-relative file names, and tracked bytes). Module-lowering slots key on source content plus module/package/target identity instead of physical path. The old stat/root and path-provenance identity lanes are deleted; payload context digest and source-sha validation remain the correctness authorities.
- Final population row `20260712T003540-pact-witness-acceptance-43899784ba3f4b1e`: hits=0 misses=145 reused_s=0.0 relowered_s=199.140232 (intentional one-time schema/key population).
- Final no-change warm row `20260712T004636-pact-witness-acceptance-3d3aa3098f9e46fa`: hits=145 misses=0 hit_rate=1.0 reused_s=199.140232 relowered_s=0.0. The build reached the identical known split-runtime native direct-symbol frontier.
- Byte identity: cold and warm `output.wasm` both have SHA-256 `9E37E67CBC91BE7BD60088D1B56BE13922EEEE8ECED67B30D6A7EDDD5359847C`.
- Anti-recurrence: `tools/build_health_gate.py --cold-diagnostics COLD --diagnostics WARM --strict` hard-fails when the paired warm hit rate is below 90%, the sample is too small, or observed module counts differ. Focused fingerprint/cache/gate tests cover ephemeral install roots, mtime-only churn, generated output-path churn, persisted-result equality, and the measured gate floor.

### BUILD-TIME-R8 — durable per-phase attribution

- Landed one canonical phase_attribution schema inside build diagnostics with per-phase seconds, share-of-total, ranked phases, child-link timings, and fail-closed flushing. tools/build_phase_attribution.py validates and prints the machine-checkable schema.
- Link instrumentation records inclusive wasm-link time plus split-runtime processing, publication stripping, and fail-closed validation. Unreached phases are explicit zeroes; an early split-runtime failure records partial split processing instead of losing the sample.
- Representative attributed witness row: 20260711T232839-pact-witness-acceptance-c8bde803db9842d4. The build reached the same known frontier, Split-runtime app is missing app-owned function export molt_main after symbol restoration at optimized-app; no support or wall-clock improvement is claimed.
- Share breakdown from tmp/pact_witness_acceptance_queue/runs/20260711T232839-pact-witness-acceptance-c8bde803db9842d4/build/build_diagnostics.json: frontend lowering 82.40% (457.23s inclusive), wasm link 15.77% (87.52s inclusive), split-runtime processing 10.35% (57.43s), wasm-link core 5.42% (30.08s). The frontend aggregate contains IR lowering 39.93%, module graph 37.22%, and module analysis 5.02%.
- The decisive cache evidence is frontend_lowering_cache: hits=0 misses=145 reused_s=0.0 relowered_s=199.822393. Backend/final-app codegen was a cache hit in this sample and is explicitly 0.0%; seal, strip, and final validation were not reached and are explicitly 0.0%.
- Artifact invariance is covered by the split-runtime linker regression: identical linked/app/runtime bytes with phase timing enabled versus disabled; the timing sidecar contains only diagnostics.
- Highest-value next rung: repair the frontend lowering cache admission/fingerprint path that made all 145 modules miss. Do not return to absolute witness cohorts until the host is quiescent.

### BUILD-TIME-R6 — cache authority before combined Cargo prepopulation — DROPPED

- Candidate aperture: probe exact/compatible shared runtime caches before combined Cargo target prepopulation.
- Pre-rebase exploratory signal: current-main warm 319.6s versus candidate samples 276.6s, 258.1s, 259.8s (candidate median 259.8s, apparent -18.7%).
- Rebased baseline fingerprint-population row `20260711T211139-pact-witness-acceptance-c9cb3e40044046a9` was 576.5s and was excluded from the warm cohort.
- Valid current-origin warm cohort: 337.0s, 331.6s, 327.5s; median 331.6s. Queue rows `20260711T212204-pact-witness-acceptance-7812586b8cb9436a`, `20260711T212746-pact-witness-acceptance-6a0c404ffb4a49c1`, `20260711T214317-pact-witness-acceptance-126a6c3a95964047`.
- Valid rebased-candidate warm cohort: 328.7s, 324.3s, 323.4s; median 324.3s. Queue rows `20260711T214908-pact-witness-acceptance-78dd3c4d44504c5b`, `20260711T215450-pact-witness-acceptance-95fdd4a0ca9c4c07`, `20260711T220032-pact-witness-acceptance-5a0fcb9dba234db8`.
- Rebased median delta: -7.3s (-2.2%), not the original -59.8s (-18.7%) effect and not strong enough to separate from concurrent-host variance. The candidate was dropped under M05; no runtime/performance claim or A12 landing was made.
- Correctness frontier stayed identical across all six cohort rows: final linked validation rejected missing exported memory after split app/runtime generation.
- The code and regression test were restored to rebased `origin/main`; no compatibility lane remains.

### BUILD-TIME-R7 — backend function/object cache and final app codegen

- Profile rows `20260711T220848-build-time-r7-profile-faa1f45cb3644726` and `20260711T221606-build-time-r7-profile-f3018a2515324658` showed an effective module-cache miss and 62+ seconds in backend compilation before link/post-link work.
- Source audit found `_prepare_backend_cache_setup` passed `module_cache_payload_digest` into `_function_cache_key`, bypassing the function-payload digest authority.
- Candidate moved function digest ownership back to `_cache_backend_payload_ir`; focused proof was 43 passed, 1 skipped.
- Population row `20260711T222819-pact-witness-acceptance-dddc5db7a0bb4b5d` was 345.0s and excluded.
- Valid candidate warm cohort: 324.0s, 326.0s, 321.8s; median 324.0s. Queue rows `20260711T223429-pact-witness-acceptance-ebf4721cd54445e6`, `20260711T224014-pact-witness-acceptance-4b9bd29355dc4c13`, `20260711T224555-pact-witness-acceptance-fc4b887192c64625`.
- Delta versus the valid 331.6s baseline median: -7.6s (-2.3%). This did not separate from concurrent-host variance and did not justify held-bench/determinism expenditure or an A12 landing. Candidate code was restored; no duplicate digest lane remains.

## Next Actions

1. Profile the remaining split-runtime processing floor by full-binary parse/rewrite counts and remove the next repeated whole-artifact scan class.
2. Preserve R8's share-of-total schema, R9's warm-pair cache-effect gate, R10's module-graph operation counts, and R11's link operation counts for every future build-time landing.
3. Preserve the 331.6s historical current-origin cohort only as pre-R9 archaeology; current absolute comparisons require a same-fingerprint warm cohort.
