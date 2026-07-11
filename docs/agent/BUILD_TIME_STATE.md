# Build-Time State

## Ranked Rungs

| Rank | Rung | Live evidence | Gain / validation cost | Status |
|---:|---|---|---|---|
| 1 | Final app codegen and link/post-link pipeline | Current-origin warm witness median is 331.6s; function-digest correction did not move the floor beyond variance | High / high | BUILD-TIME-R8 NEXT |
| 2 | Runtime-wasm cache-first combined-build ordering | Original 319.6s -> 259.8s signal did not reproduce after origin/main advanced | Marginal / medium | BUILD-TIME-R6 DROPPED |
| 3 | Import-strip / split restoration frontier | Every sample reaches linked validation after app/runtime generation | Correctness blocker / separate owner | OBSERVED |
| 4 | Runtime crate split for true source misses | Current-origin fingerprint population sample was 576.5s | Medium / high | OPEN |
| 5 | R73.3 provisioned extension archives | Cold ecosystem cross-compile remains outside warm iteration floor | Very high / very high | OPEN |

## Iteration Log

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

1. Instrument final app codegen, wasm link, strip, split-runtime bridge, and validation as separate durable phase timers even on fail-closed exits.
2. Attribute the remaining ~324s floor to one dominant consumer before changing code; the next rung must exceed host variance before held gates.
3. Preserve the 331.6s current-origin baseline cohort as the comparison authority unless origin/main or toolchain fingerprints change.
