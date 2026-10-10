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

The gate's `release-fast` label selects the public CLI's `--build-profile release`
while its shared profile selection explicitly pins the `release-fast` guest/runtime
and independently built `release` host compiler. Compiler lookup uses that host
coordinate and the platform's executable suffix. Measurement records selected
compiler content from build publication observations rather than relying on a
pre-build path probe. These observations prove selection and publication; they
are not loaded-daemon attestation. An authoritative receipt must bind the backend
that actually ran, including its profile, feature set and runtime identity, and
fail closed when that execution identity is absent. Profile declarations and
source-checkout measurements do not qualify the installed release matrix.

LLVM is required by E2 and the verified-subset policy. The production compiler
feature set includes LLVM through the admitted, pinned static SDK path. That
build policy is not a six-host execution or performance result. Installed
availability requires the compiler features/profile and exact physical runtime
cell; acceptance still requires the actual logical backend/profile products.
`config/release_acceptance_matrix.toml`, read by `molt.release_lanes`, owns those
lanes for runtime production, installed consumers and performance selection.
A native-only pass cannot replace LLVM evidence. The canonical command and
its acceptance requirements remain unchanged.

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
backends. E3 must include the exact source-bound verified-subset receipt closure
for every required CPython-version, OS, architecture, backend and guest-profile
coordinate. The policy requires LLVM, native and WASM with `dev` and `release`
guest profiles.
Each receipt binds the coordinate-specific test projection, raw/resolved/backend
outcomes, reference interpreter, host, toolchain, and CI execution identity.

The same release gate treats E4 `status: pass` as typed structural evidence, not
a generic JSON attachment. E4 must include the canonicalization contract JSON,
the structural-audit JSON, the degrade-to-slow gate report, and a fail-closed
gate receipt. The two metric artifacts are compared against their checked-in
baselines and fail closed on any regression; the poison receipt must contain the
`fail-closed gate: OK` verdict.

The development proof-supervisor bootstrap shares one repository-observer scope
across setup probes and one Cargo generation producer across direct and queued
consumers. A same-machine Windows development comparison observed unchanged
warm selection at 14.2–14.5 seconds before the closure (two samples), versus
5.4–6.0 seconds afterward (four unprofiled samples). Each selection reported
zero compiled and 41 fresh Cargo artifacts. Bounded source and local-dependency
edits with original mtimes rebuilt only owning local crates and dependents;
registry artifacts stayed fresh. These are development bootstrap observations,
not installed-user compile latency or E2 performance acceptance.

## v1.0 Acceptance Scope

The existing core gate is necessary, not sufficient, for v1.0. A native/LLVM
core pass does not certify WASM, ecosystem workloads, frontend/build latency,
memory, temporary disk usage, or binary size. Missing target coverage and stale
measurements remain release gaps; they are not implicit passes. The current
startup budget has null ceilings, and the cold-start table leaves the shipped
`release-output` runtime unseeded and has no WASM cell. Existing native/LLVM
baseline ceilings still apply; the missing numeric product budgets must be
declared and measured through this authority.

WASM development benchmark commands disable V8 tier-up and suppress host
warnings. Their engine settings and stderr treatment differ from public runs
and must be explicit in each claim; results cannot be relabelled as default
installed-user behavior.

Component diagnostics must retain their pipeline scope. Shared exceptional
cleanup factoring reduced a preserved module-metadata entry from 52,161 to
1,425 post-insertion TIR operations. The entry has 741 source operations and
1,232 signature-only siblings. Three serial samples per production compiler,
on the same host with the same native input and archive-callable binding, gave
guarded median compile times of 82.359 s before and 31.532 s after. Maximum
sampled compiler RSS fell from 2,208,509,952 to 97,107,968 bytes; the native object
fell from 3,695,361 to 128,893 bytes. CLIF output was disabled for timing.

These are development component measurements, not the public full-module
pipeline, installed-user latency, linked artifact size or E2 acceptance.
The cleanup compiler checkpoint's public native and linked-WASM ownership
stdout matches the independent CPython 3.12 oracle with Python absent from PATH.
Native stderr matches; the public Node WASI host warning is preserved separately
in the WASM assessment. The subsequent production SSA verifier repair uses
shared executable program-point dominance, also used by DropInsertion's split
continuation remapper. Focused correctness checks and public native/linked-WASM
stdout replay pass on this integrated change. Native stderr matches; the public
Node host warning is retained and independent WASM guest-stderr attribution
remains open. No performance gain is claimed for the verifier/remapper repair.
Native immutable-value transport now uses shared definition/execution facts and
the actual emitted values; mutable joins and stack/frame/resume custody retain
explicit transport. Liveness and transport share canonical name IDs with sparse
per-operation sets. A separate CLIF diagnostic reduced explicit block
parameters from 154,957 to 8 across the same 1,396 blocks. Three alternating quiet samples per retained
production compiler, on the same input and archive-callable binding, measured
28.390 s before and 28.890 s after. Maximum sampled compiler RSS was
96,759,808 and 96,473,088 bytes respectively; both produced the same
128,893-byte native object, byte for byte. These results establish the
representation reduction, not a compile-latency, executable-size or material
memory improvement. Focused liveness and native codegen checks pass, along with
public native/linked-WASM stdout replay of the transport implementation against
CPython 3.12 with
Python absent from guest PATH. Native guest stderr matches the oracle;
independent WASM guest-stderr attribution and the wider declared matrix remain
open.
These remain development component results, with CLIF output disabled for
comparison timings. Public-build cold/warm/edit measurements must include actual
module discovery, lowering, linking, delivered bytes and memory.

Build-Python capture now binds inventory rows within the shared stable-file
transaction and batches capture and final verification. An isolated Windows
CPython 3.12 capture diagnostic reduced scheduled futures from 5,382 to 169 and
`lstat` calls from 24,961 to 19,491. These are profiled operation counts, not an
end-to-end latency claim. The representative public native and linked-WASM
ExceptionGroup executions match the CPython oracle with Python absent from the
guest PATH. Warm source-checkout timing samples had materially different host
load before and after the change and establish no compile-latency improvement.
Comparable cold/warm/edit and installed-user measurements remain required.

Build diagnostics use one terminal snapshot after artifact finalization and
publication, including cache-hit link admission and requested artifact analysis.
Their `build_preamble_to_terminal_result` interval excludes interpreter startup,
compiler identity enrichment and diagnostic serialization/publication; the outer
process measurement remains the compile-latency authority. Sequential phases may
be summed; overlapping attribution aggregates must not be summed. A reporting
failure preserves the primary build failure, or returns nonzero after a successful
artifact publication while retaining that artifact. This instrumentation change
does not establish a product speedup.

The shared native object/archive cache decoder now uses native Unicode scanning
and reuses lexical validation within each payload. A Windows CPython 3.12
development diagnostic of the same 4,781-member cached archive measured
1.141 s before and 0.178 s after (median of five samples after two warmups).
Every decoded member table round-tripped to the original payload; sampled
process-tree RSS increased from 148,635,648 to 150,257,664 bytes. This measures
cache decoding, excluding JSON I/O and artifact/toolchain custody. The public
native and linked-WASM ExceptionGroup authority executions still match the
independent CPython oracle, with empty guest/runner stderr and Python absent
from guest PATH. These observations establish a component improvement, not a
whole-build, installed-user, generated-program, or release-performance claim.

Runtime source/tooling admission now uses one live tree index per resolver and
bounded file batches across native, shared/relocatable WASM and standalone WASM
ABI builds. On identical copied source/tooling bytes (1,195 files), a Windows
CPython 3.12 component diagnostic reduced scheduled futures from 2,390 to 96.
Sequential samples under host load observed median capture time of 0.888 s
before and 0.763 s after (five samples after two warmups), with identical source
and publication summaries. These are development component observations,
excluding Python/toolchain capture, archives, lowering and linking. They do not
establish quiescent or installed-user compile latency. Fresh post-build and
final-link observations retain membership and byte-change detection; no tree
index is reused across those boundaries.

Direct dictionary snapshot materialization shares the canonical list backing
authority and avoids the temporary view used by the view-to-list path. Seven
alternating correctness-checked component samples on Windows x86_64, using the
same `dev-fast` runtime test executable and 32 repetitions per sample, observed:

| Entries / operation | View-to-list median | Direct median |
| --- | ---: | ---: |
| 64 keys | 10.597 us | 4.425 us |
| 64 values | 10.709 us | 4.456 us |
| 64 items | 246.463 us | 196.078 us |
| 4096 keys | 64.456 us | 42.809 us |
| 4096 values | 66.316 us | 43.034 us |
| 4096 items | 15,040.913 us | 12,097.359 us |

Separate allocation diagnostics observed one fewer runtime object per direct
snapshot: keys/values allocate one instead of two, and items allocate `N + 1`
instead of `N + 2`. Timing disabled profiling; allocation diagnostics enabled
it. These are paired implementation paths on one source, not historical
before/after builds. Quiescence was not established, and the object counters
exclude native buffers and process memory. They establish neither whole-program
nor installed-user nor release performance. The fixture is
`c_api::tests::dictionary_snapshot_materialization_components`.

Relocatable WASM runtime metadata admission now performs one `wasm-ld -r`
preflight on the custodied immutable input; the pipeline consumes that same
admitted input without repeating the preflight. Five alternating serial pairs
on a 60,578,040-byte development runtime with LLVM 22.1.8 measured median
component time of 1.801794 s for two invocations and 0.899768 s for one,
eliminating 0.902026 s. Input, linker and output content identities were equal.
This measures the removed admission work, excludes the rest of linking and
compilation, and establishes no installed-user or whole-build speedup.

Source-build Python admission retains the observed interpreter for one build
operation, checks fresh startup selection at the next boundary, and closes
through the existing guarded-command custody. Five alternating Windows
component pairs measured 6.022947 s for two complete captures versus 5.602407 s
for retained admission, fresh selection, verification and closure: 0.420540 s
removed. The semantic identity was equal. This excludes guest compilation and
does not establish public-build, installed-user or cross-platform latency.
Installed distributions use shipped runtime cells and bypass build-Python and
Cargo preparation; their admission cost needs separate measurement.

Installed runtime admission now carries captured generations through callable
binding, layout, hydration and final-link custody, with lazy member facts and
one WASM acceptance report. Five serial Windows CPython 3.12 component samples
on identical frozen development artifacts measured native admission medians
of 1.300023 s before and 0.726186 s after, and WASM admission medians of
0.959667 s before and 0.788988 s after. Semantic outputs matched. Maximum
process-tree job commit fell from 176,267,264 to 136,699,904 bytes. Instrumented
reads fell from 803,451,895 to 404,583,523 bytes and from 25,259 to 12,701 calls;
these counters exclude mmap page traffic and metadata syscalls. The WASM
component covers pair read, binding and split layout; it does not cover the
complete non-split export/structural admission path. These are development
component measurements, not public CLI latency, installed qualification,
generated-program speedups or release performance. The full public-build and
installed-user comparisons remain required.

Each claimed workload and target needs reproducible correctness-checked
measurements, explicit resource/latency budgets, and input-size scaling. Extend
the existing scoreboard and typed release receipts when a dimension is missing;
do not create a second performance authority. Profile outliers and resolve
known budget violations or pathological scaling before release. Aggregate
speedups cannot cancel a failing workload, target, or resource budget.

For v1.0, every individual canonical benchmark at every required target,
backend, profile, and reference-CPython version coordinate must statistically
demonstrate a speedup greater than 1.0 against matched CPython. A point estimate
or aggregate is insufficient: noisy or inconclusive measurements and unmeasured
coordinates remain acceptance blockers. Keep the declared workload and target
coverage intact; do not remove benchmarks, narrow the verified subset, or count
skips as wins. Report runtime, build latency, startup, and memory separately.

The semantic acceptance matrix retains CPython 3.12, 3.13, and 3.14 across the
declared verified subset. Asyncio, threading, multiprocessing, stdlib, and
advertised third-party package coverage require coordinate-bound correctness
evidence before their performance results can establish release acceptance.


A retained Windows native attribute workload measured 46.485 s before and
43.031 s after the integrated class-storage changes (median, seven fresh-process
samples after two warmups per artifact). Every sample matched its independent
CPython 3.12 output with empty stderr and Python absent from guest PATH. Both
artifacts used the development guest/runtime profile and the same workload;
compilation is excluded. The 7.4% wall-time reduction is a development comparison
of integrated source checkpoints, not attribution to one change, current-source
qualification, a release-profile result, or E2 acceptance.

Megafunctions are structural review triggers: isolate semantic responsibilities
and ownership, remove repeated analysis, and measure compiler and generated-code
costs. Splitting lines into wrappers is not acceptance; neither is faster
compilation that changes observable semantics, duplicates authority, or moves
the cost into runtime memory or code size.

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
- `docs/agent/CODEGEN_RUNTIME_OPT_CATALOG.md` — the codegen + runtime hot-path lever
  catalog (Agner-grounded), tagged LANDED/OPEN + determinism-safety. This authority
  covers build-time + publication; that one covers the emitted-code / runtime surface
  (loops, dependency chains, int/float division, NaN-box bit-tricks, SIMD, memory
  layout). Opt-matrix rung selection should read both.

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

The former dual-profile smell is closed by making one source-attested pair
pointer the live contract for shared and reloc runtime publication. The atomic
pointer binds the full source/config/toolchain identity to two immutable,
content-named members. Fixed filenames are derived consumer projections, never
validation authority; cache hydration validates and copies only the immutable
members and rechecks their staged hashes before advancing the destination
pointer.

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

### Exact runtime-WASM shared-cache hydrate

`tools/opt_matrix_r2_runtime_wasm_hydrate_attestation.json` records the
A12-citable release differential for an exact-identity shared runtime cache hit.
The cache source retains full structural/export validation before hydration;
the superseded second validation of the byte-identical atomic-copy destination
is deleted. Seven serial alternating samples on a real 45,871,431 B release
runtime improved the median from 1,111.2847 ms to 554.2158 ms (2.0051x), with
byte identity, corrupt-source rejection, copy-failure rejection, and a
144,908,288 B maximum RSS ceiling.

### Linked WASM metadata materialization

`tools/opt_matrix_r5_linked_metadata_attestation.json` records the A12-citable
release differential for linked Node startup. Linked execution needs import
descriptors but does not consume app export function signatures, so
`parseWasmMetadata` now skips the function and export payloads in linked mode
while direct-link and auto-split mode retain the full shared parser contract.
Seven serial alternating samples on the 9,720,086 B release runtime improved
the median from 10.6805 ms to 3.0449 ms (3.5077x), preserved all 90 function
imports, skipped 4,409 unused linked exports, and stayed below a 65,130,496 B
maximum RSS ceiling.

### Native multi-byte reverse search

`tools/opt_matrix_r4_bytes_rfind_attestation.json` records the A12-citable
release differential for repeated multi-byte `bytes.rfind` calls. The shared
bytes/string/bytearray reverse-search primitive now uses `memmem::rfind`
directly; the superseded forward `find_iter` enumeration that visited every
match only to retain the last index is deleted. Three serial release samples
on 200 searches of a 10,000,000-byte overlapping haystack improved the median
from 5,062 ms to 266 ms (19.0301x), preserved exact output semantics, and
reduced observed peak RSS from 64 MiB to 40 MiB.

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

### OPT-MATRIX-R7 WASM release memory floor

- Aperture: host RSS for the real 9,720,086 B release runtime, separated into Node/V8 baseline, artifact plus metadata residency, exact imported linear memory, and module compilation.
- Profile: seven serial fresh processes measured medians of 38,072,320 B baseline, 61,435,904 B artifact/metadata, 61,882,368 B artifact plus the exact 48-page memory, and 73,060,352 B compiled module RSS.
- Linear-memory fact: both Node and browser hosts allocate from the parsed WASM import minimum. The 48 pages reserve 3,145,728 virtual bytes but add only 446,464 B committed RSS over the identical artifact/metadata phase.
- Verdict: DOCUMENTED-BLOCKED. No Molt-owned committed linear-memory allocation clears the 1 MiB or 10% admission threshold. The measurable RSS is artifact and V8-code driven, so its structural removal belongs to Task #22/Binaryen tree shaking rather than a fake initial-page reduction.
- Unblock contract: remeasure after the reserved `wasmld-toolchain` authority lands, or identify one removable committed allocation above the admission floor; browser-specific claims require Chromium process-tree RSS on the real release artifact.
- Evidence: `tools/opt_matrix_r7_wasm_memory_profile.json` and `tools/opt_matrix_r7_wasm_memory_blocker.json`.

### Backlog gain per validation cost

The ranking helper reads this intentionally small table; it does not create a
second backlog system. Gain and cost are relative planning estimates.

| Item | Expected gain | Validation cost |
| runtime-wasm cache authority | 8 | 2 |
| witness lowering-context stability | 10 | 4 |
| seal relocation-root reuse | 4 | 2 |
