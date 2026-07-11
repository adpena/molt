# Performance Authority

Molt has one citable performance authority:

```text
tools/perf_scoreboard.py --set core --backend native --backend llvm \
  --profile release-fast --samples 5 --warmup 2 --repeat 5 \
  --classify --require-quiescent --quiescence-wait-s 180 \
  --quiescence-poll-s 15
```

That gate owns the release-fast performance contract because it records cold
and warm timings, native+LLVM backend parity, repeat-CI classification,
quiescence, provenance, and stale-tree status. It is the only lane allowed to
publish `authoritative=true`.

`tools/release_exit_gate.py` treats every `status: pass` criterion as a typed
receipt, not a generic file attachment. E1 must include a
`pact-witness-acceptance` receipt with a real `candidate_outputs.npz` path and a
`pact witness acceptance PASS` verdict. E2 must include a canonical
`cpython_floor_scoreboard` JSON artifact that is schema-valid, `authoritative:
true`, fresh under `DEFAULT_STALE_DAYS`, on `origin/main` ancestry, and has
`summary.gate_fails == false`. The receipt command must be the canonical
native+LLVM release-fast gate above (`--set core`, `--backend native`,
`--backend llvm`, `--samples 5`, `--warmup 2`, `--repeat 5`, `--classify`,
`--require-quiescent`, `--quiescence-wait-s 180`, and
`--quiescence-poll-s 15`) and the scoreboard itself must contain classified
release-fast cells for both native and LLVM across the full canonical core suite
(`bench_suites.BENCHMARKS`), with backend binary identity receipts for both
backends. E3 must include a
`tools/parity_gate.py` receipt with the no-Tier-1-violations PASS verdict.

The same release gate treats E4 `status: pass` as typed structural evidence, not
a generic JSON attachment. E4 must include the canonicalization contract JSON,
the structural-audit JSON, the degrade-to-slow gate report, and a fail-closed
gate receipt. The two metric artifacts are compared against their checked-in
baselines and fail closed on any regression; the poison receipt must contain the
`fail-closed gate: OK` verdict.

## Non-Canonical Lanes

`tools/bench.py` and `bench/harness.py` still measure useful development
signals, but their JSON outputs are not the perf contract. They must stamp a
top-level `provenance` object from `tools/perf_authority.py` with:

- `authoritative: false`
- `source: "non-canonical"`
- `lane`: the emitting tool path
- `profile`: the actual measured profile
- `canonical_gate`: the full `tools/perf_scoreboard.py --set core --backend native --backend llvm --profile release-fast --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent --quiescence-wait-s 180 --quiescence-poll-s 15` command

These lanes are for debugging, triage, and local comparison. Do not cite them as
release performance evidence.

## Ratio Rule

All non-canonical lanes must compute speedup through
`perf_authority.safe_speedup(cpython_time, molt_time)`.

`safe_speedup` returns `None` whenever either timing is missing, non-finite, or
non-positive. A build failure, daemon crash, runaway, or missing `molt_time`
must render as `n/a`, never as a finite regression or win.

The direction is fixed:

```text
speedup = cpython_time / molt_time
```

Values greater than `1.0` mean Molt is faster. The inverse field
`molt_cpython_ratio` must remain `molt_time / cpython_time`.

## Freshness Rule

Historical markdown snapshots are routing context, not current evidence. A perf
document whose recorded `git_rev` is not on `origin/main`, or whose generated
timestamp is stale relative to `perf_authority.DEFAULT_STALE_DAYS`, must be
treated as non-authoritative and point readers back to the canonical gate.

Checked-in root `bench/scoreboard/*.json` CPython-floor boards are current
evidence only when they are schema-valid, generated at the current `origin/main`
tip, `authoritative: true`, fresh, and `summary.gate_fails == false`. Any older,
red, non-authoritative, or schema-legacy board must carry the structured
`perf_authority` stale metadata; then it is only a historical fixture and cannot
serve as E2 proof.

See also:

- `docs/perf/SCOREBOARD.md`
- `docs/design/foundation/64_perf_scoreboards_and_harness.md`

## Witness Iteration Build Profile (2026-07-11)

Canonical machine-checkable record: `tools/perf_witness_iteration_attestation.json`.
The measured aperture is the real `pact-witness-acceptance` build through the
current runtime frontier. The acceptance run currently fails after codegen in
WASM import stripping, so replay is not measurable in this revision; build-path
numbers remain valid and queue-custodied.

| Rank | Phase | Cold / miss path | Warm incremental path | Inherent floor | Ranked waste |
|---:|---|---:|---:|---:|---:|
| 1 | Frontend graph + analysis + lowering | 390-604s across same-machine diagnostics; 0-24% lowering hits | 467.6s avoidable before this landing; unique output path changed all contexts | Reuse unchanged 145 module lowerings; lower only changed modules | 467.6s eliminated (61.3% wall, 2.59x) |
| 2 | Backend prepare/codegen | 198-410s, including first population of thousands of functions | 295.0s total build to the current import-strip frontier; only 12 uncached TIR functions in the steady sample | Rebuild the changed runtime/compiler cone and changed functions | Next target: function/object cache misses plus import-strip frontier |
| 3 | Runtime wasm cargo compile | 23.9-265.5s historical; 124.7s recent cold sample | Target/shared reuse is fingerprint-controlled; final patched sample did not rebuild runtime | One changed runtime crate compile | Audit configured vs effective runtime-wasm hydrate hit rate |
| 4 | Runtime reloc link | 3.831193s median warm isolated relink for the 69.5MB runtime; target-staticlib reuse previously linked before cache lookup | 1.287784s exact-cache hydrate median after cache-first ordering | One relink on a true cache miss | 2.543410s eliminated (66.39%, 2.975x); `tools/perf_goal_r5_relink_attestation.json` |
| 5 | Seal / validate | Isolated NumPy 2.5.1 seal validation was 1.441451s median across five fresh processes; 1.182s cumulative was repeated relocation-root discovery across 132 objects | 0.869927s median after one resolver precomputes manifest relocation roots once | Hash each source once; discover relocation roots once per manifest | 0.571524s eliminated (39.65%, 1.657x); `tools/perf_goal_r4_seal_validation_attestation.json` |
| 6 | Replay / parity | Not reached: current frontier is WASM import stripping | Not reached | One replay | Outside this build-time landing |

Root cause: the synthetic `_molt_native_runtime_python_imports.py` entry lived
under each queue run's unique output directory but was named against source roots.
That turned `tmp/pact_witness_acceptance_queue/runs/<run>/build/...` into seven
namespace pseudo-modules. Because `known_modules` is a lowering-context input,
every run invalidated the whole module set: `O(M)` relowering for output-path
churn. The synthetic artifact root is now the first module-naming root, so the
entry keeps its stable logical module name and output-directory churn invalidates
zero contexts. The regression test uses an acceptance-shaped nested output path
and rejects any `tmp.acceptance` module admission.

## WASM Publication Strip (2026-07-11)

Canonical machine-checkable record: `tools/wasm_publication_strip_attestation.json`.
This aperture removes publication-only custom sections without touching code or
data reachability. Final artifacts run the export-contract rewrite first, then
the canonical strip, then link validation.

| Artifact contract | Before | After | Removed | Reduction |
|---|---:|---:|---:|---:|
| app final (`output.wasm`) | 34,442,899 B | 25,819,855 B | 8,623,044 B | 25.0358% |
| deploy runtime final (`molt_runtime.wasm`) | 41,915,494 B | 18,812,380 B | 23,103,114 B | 55.1183% |
| relink runtime cache input (`molt_runtime_reloc.wasm`) | 78,546,783 B | 42,089,702 B | 36,457,081 B | 46.4145% |

The reloc-runtime decision is structural: relink consumers read the `linking`
symbol table and code/data/element relocation sections, but not DWARF, debug
relocations, or the `name` section. Those debug families are stripped before
cache publication while the real relink authority remains intact. The live
`C:/Molt` inventory contained 81 content-addressed reloc runtimes totaling
4.442 GiB; applying the measured ratio projects 2.380 GiB retained and 2.062
GiB reclaimed.

The former dual-profile smell is closed by making the keyed integrity sidecar
the single live contract for a published filename. A new publication deletes
the retired unkeyed pin and every sibling keyed pin; profile variants remain
separate only in the content-addressed runtime cache.

Task #22 retains the code/data optimization frontier. Its measured map is
20,032,474 B code, 5,275,490 B data, 416,470 B exports, and 52,230 B elements.
This landing intentionally does not tree-shake those sections.

### WASM startup metadata scan

`tools/opt_matrix_r1_wasm_metadata_attestation.json` records the A12-citable
release differential for the Node pre-instantiation metadata path. A single
`parseWasmMetadata` section walk now produces both import descriptors and
export function signatures; the superseded second full-module walk is deleted
from `run_wasm.js`. Seven serial fresh-process samples on a 9,720,086 B
final-form release runtime improved the median from 36.3275 ms to 22.6561 ms
(1.6034x), with metadata parity and a 77,664,256 B maximum RSS ceiling.

## Variant-II Landing Acceptance

`tools/powerplay_acceptance.py` is the acceptance authority for perf landings.
A citable landing requires a positive serial differential on the real authority,
a release profile, at least three samples, held-bench never-regress evidence,
and a recorded memory-ceiling run. Checked-in legacy attestations are parsed by
the canonical perf workflow but remain advisory until all fields needed by
`CorrectnessDemonstration` are present; this prevents historical shape drift
from blocking unrelated work while refusing proxy evidence for a perf claim.

The current checked-in attestations validate as follows:

- `perf_goal_r3_runtime_cache_attestation.json`: parsed; dev-fast compatible-cache proxy, not Variant-II citable.
- `perf_goal_r4_seal_validation_attestation.json`: parsed; real five-run seal differential, missing explicit release-profile and memory-ceiling evidence.
- `perf_goal_r5_relink_attestation.json`: parsed; real three-run artifact differential, missing explicit release-profile and memory-ceiling evidence.
- `perf_witness_iteration_attestation.json`: parsed; acceptance-shaped build evidence, not a complete correctness/memory attestation.

### Backlog gain per validation cost

The ranking helper reads this intentionally small table; it does not create a
second backlog system. Gain and cost are relative planning estimates.

| Item | Expected gain | Validation cost |
| runtime-wasm cache authority | 8 | 2 |
| witness lowering-context stability | 10 | 4 |
| seal relocation-root reuse | 4 | 2 |
