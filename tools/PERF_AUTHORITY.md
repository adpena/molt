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

Methodology v12 removes the inactive frontend codec option from benchmark setup
and cache partitions. Batch request admission, pipe writes and response waiting
share one deadline through the existing compiler service. These can change cold
build measurements or failure verdicts; v11 boards retain their original
comparison class. This version change makes no performance-equivalence claim.
The [frontend and transport contract](../docs/design/foundation/66_compat_cpython_parity.md)
owns those boundaries.

Methodology v11 includes current WASM deployment admission and typed resolver/OS
error domains. The runner refuses incompatible manifests before loading modules
and preserves resolver-versus-OS failure outcomes. These change execution
verdicts, so v10 boards remain historical in their original comparison class.

Methodology v10 separates requested profile labels from the build producer's
observed compiler/runtime selections. Baseline and trend comparisons refuse
missing or different observations, including historical records labelled
`release-fast` from a logical `release` request. This changes comparison verdicts,
so v9 boards remain historical in their original methodology class. It does not
change the canonical scoreboard's declared lanes or prove loaded-byte admission.

Methodology v9 preserves the selected immutable Python image and bytecode policy
during conformance/benchmark setup. This can change measured build startup;
v8 boards remain historical and are not a comparable baseline.

Noncanonical benchmark provenance records `requested_profile` as a request
label. It does not infer a physical compiler or runtime profile. Each regular
benchmark cell retains the CLI producer's `build_observation.selected_profiles`
(backend, target, guest request, runtime Cargo profile and compiler Cargo profile).
Reports show missing historical observations as `unbound`; baseline comparisons
require matching observed coordinates and timing settings. Historical files are
not rewritten or upgraded from logical labels. Native logical `release` normally
selects `release-output`; the canonical scoreboard pins `release-fast` explicitly
through the shared release-lane authority. Publication observation still does
not prove loaded compiler/runtime bytes, and these records remain noncanonical.
The legacy `bench/harness.py` retains raw timings and status checks but does not
capture a build observation. It explicitly refuses baseline timing comparisons;
its request label cannot supply missing physical selection evidence.
The regular baseline gate, `perf_regression.py` (including trends), Molt metrics
in `bench_diff.py`, and `bench_dashboard.py` reuse the shared observation policy.
Standalone producers without a retained observation remain unbound, including
historical and current `bench_wasm.py` records. Their raw measurements remain
readable, but cannot authorize cross-run Molt timing comparisons. External-only
metrics in `bench_diff.py` retain their independent success/profile rules, but
require equal admitted source workloads, like every other cross-run metric.
Fresh `bench_individual.py` and `bench/harness.py` CPython records carry this
same source identity; their missing Molt build observations remain unbound.
The Rust-transpile tool emits a separate list of nested millisecond results,
which the shared flat-map reader already refuses before metric comparison.

Methodology v8 binds source and ancestry to original Git objects, ignoring
replacement refs and legacy grafts. Performance freshness resolves the current
`origin/main` rather than retaining a process-lifetime ref or ancestry verdict.
These checks follow [Git's replacement semantics](https://git-scm.com/docs/git-replace)
and apply to development/release evidence, with no work in emitted programs.
The null-device graft source is portable across Git hosts. Its specific deprecation advice is disabled through [Git’s per-process configuration protocol](https://git-scm.com/docs/git-config#Documentation/git-config.txt-GITCONFIGCOUNT), preserving inherited settings and all other diagnostics.
Historical boards retain their recorded methodology; this correction does not
authenticate missing compiler/runtime execution identities or full-matrix evidence.

Methodology v7 routes benchmark build targets through the compiler's admitted
target authority. Exact WASI and freestanding targets retain their distinct
deployment rules. This can change selected builds or admission outcomes, so v6
boards remain a separate comparison class.

Methodology v6 includes complete typed build requests, validation before compiler
service startup and lazy benchmark-service creation. The first batch build's
timing now includes that startup, and invalid requests retain build-admission failure classification.
These can move recorded build numbers or verdicts, so v5 boards remain historical.
Request transport and compiler option admission retain their compiler-source owners;
concrete compiler/runtime/deployment identities remain separately recorded.

Methodology v5 includes environment-name semantics and the complete typed
invocation-environment migration in its content lock. These can change sample
execution or selection and therefore start a new comparability class; existing
v4 boards remain historical. The v4 declaration of host-tool names and their
classifier remains in the lock. Both POSIX discovery and Windows classification include the
standalone WASM inspector when determining quiescence. This can change authority
decisions, so earlier methodology versions remain separate comparison classes.

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

WASM scoreboard execution records one admitted Node plan per invocation in
schema-v6 board and cell `execution_host` facts. The path names the exact selected
executable; comparability uses engine kind, observed version, byte size and
SHA-256 rather than its installation path or basename. A different engine image
or version splits history and refuses merging. Missing historical observations
are unqualified; no legacy board is relabelled by rebuilding a summary.

This follows [pyperf's runtime metadata model](https://pyperf.readthedocs.io/en/latest/api.html#metadata),
which records executable, implementation and version. Molt adds canonical content
identity because a version string cannot distinguish locally rebuilt engines.
Admission hashes once and probes once for a WASM invocation, then reuses the typed
plan across cells. Native-only invocations do neither. Pre-build and surrounding
sample metadata fences detect observed replacement without repeated image reads:
POSIX uses `lstat`, Windows queries ChangeTime through a handle. Their cost is
constant per boundary and independent of image size; the initial read scales
with image bytes. No added work runs inside the guest, and ordinary Node consumers
retain their existing lightweight resolver. This is observed execution-host
identity, not loaded-byte attestation or installed-release qualification.

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

## Finding the limiter

Development-loop latency is always P0: every faster edit, build, diagnosis and
proof compounds across subsequent work. Use the existing build phase diagnostics
to attribute the current limiter, and compare the same unchanged
or edited workload before and after each load-bearing change. Use
`tools/dx_build_timer.py` for build-loop measurements and
`MOLT_CLI_PROFILE` for attribution; keep instrumented timings separate from
uninstrumented latency. The timer currently requests JSON and diagnostics, which
disable the native build-record cutoff; ordinary CLI latency requires a separate
uninstrumented invocation until that timer contract is repaired. Classify a new
failure before repeating an expensive proof.

A bounded macOS arm64/CPython 3.12.15 observation of native `dev` builds of
`bench_calls.py`, in the same retained project/cache/output, found metadata-only
import-root probes reporting shared parent-directory contents. That false input
refused every record under unrelated scratch churn: three unchanged builds took
2.350–2.362 s. Reporting the exact positive lookup instead restores reuse.
Twenty subsequent ordinary CLI observations on the matched source have p50
281 ms and nearest-rank p95 290 ms; 19 explicitly report unchanged inputs.
Restoring that source immediately before the 2.599 s prime triggered the fixed
two-second racy-window refusal. The first observation therefore rebuilt in
2.327 s and remains in those statistics. A real function-body edit rebuilt in
2.541 s, the next unchanged build took 2.334 s after the same racy refusal,
and the following cutoff took 283 ms. Original and edited executables match the
same interpreter's output. These are bounded compiler-latency observations,
not a runtime speedup, cross-platform qualification, E2 acceptance, or proof of
the 100 ms no-change and one-second edit targets. HF-250 remains open.

The shared `regalloc2` move planner uses one maintained source for native
Cranelift and the Wasmtime host (pin and invariants:
[`MOLT_PATCHES.md`](../vendor/rust/regalloc2/MOLT_PATCHES.md)). A captured
212,627-byte WASM workload exposed 1,664,708,150 incoming-parameter comparisons
and 40,826,978,304 usable-capacity units at nonempty source-map clears. A
range-local monotone cursor reduced the comparisons to 18,967,661; typed vreg
stamps remove per-vreg clears. An instrumented allocator replay changed from
17.67 to 15.04 seconds with identical ordered allocations, offsets, edits,
spill-slot counts and debug locations.

One subsequent uninstrumented macOS arm64 `dev-fast` host precompile/serialize
pair took 28.24 seconds before and 26.67 seconds after. Both produced the same
36,371,064-byte serialized module (SHA-256
`b325859c3d4e1f987ec91df04a51f7942b7986300cefdf9272ce26c71a52f345`).
Direct-child rusage peaks were 3.374 and 3.372 GiB respectively; this does not
establish a material memory reduction.
This is one captured-workload component pair, not a repeated-sample, public CLI,
installed compiler or cross-platform latency qualification. The complete symbolic
checker on the large input exceeded its 90-second diagnostic budget; exact
output differential and focused independent unit oracles are the executed claims.
The source cache initializes lazily and uses bounded `O(blocks)` storage; sparse
large graphs can use more storage than the old sparse map. Stable managed slots
retain the dependency build across lanes. Keep cold dependency rebuild cost,
compiler execution latency and ordinary edit-to-test latency separate.

Repository audit tests now share one streaming import-policy observation and
leave full-tree subprocess acceptance to the existing mandatory source-policy
gate. The copied-default scan-record oracle uses a real isolated inventory,
including Python and shell findings, exact cold/warm equality and a rejected
source mutation. One serial macOS arm64 comparison reduced the selected test
bundle from 21.05 to 8.58 seconds guarded wall time. The restructured bundle
retains all three import policies and adds missing-position controls; full-source
acceptance executes separately in the mandatory gate. This is one test-cost
comparison, not a whole-integration, compiler-latency or cross-platform result.
The per-file snippet index removes repeated source-prefix extraction; its
initial same-node macro comparison alone established no material improvement.

A measured allocator landing spent 277.31 seconds in changed-package Clippy
selection and execution; the Python cohorts already overlapped that work. The
old root-manifest/local-patch rule selected all ten declared Clippy cells. The
shared locked dependency projection now selects five unchanged cells for that
same change, omitting unrelated runtime and WASI checks that consumed over 100
seconds in the recorded run. This is verified selection reduction, not a measured
new end-to-end gate speedup. No Cargo feature sets are merged and no additional
Cargo writers contend for the shared target. Both base and current dependency
closures participate; unknown identities retain full coverage. This follows
[Cargo's locked package graph](https://doc.rust-lang.org/cargo/reference/resolver.html)
and the [rustc incremental model](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html):
make dependencies explicit and avoid work whose inputs cannot have changed.

That selector landing also exposed a 759-test development-driver cohort taking
149.39 seconds: changing the leaf selector selected lane-claim, checkout and
landing simulations. The proof plan now assigns selector and source-closure
leaves their own test owners. The selector cohort contains its 49 owning tests;
shared authorities still select the union of consumer tests. The source-closure
producer now explicitly selects its own oracles, named backend-cache and compiler
identity consumers, and the selector consumer, closing missing producer triggers. Test-only source-closure edits no longer
compile zlib, while provider edits keep both compiled gates and source oracles.
These are verified obligation changes, not an end-to-end latency comparison.

Managed Python images now publish under the schema-v9 `cpython-source-only.v1`
policy. Their ordinary bytecode read namespace is physical and empty, and every
child disables bytecode writes before startup. The retired three-variant eager
cache producer and its derivation memo are gone. Publication and admission still
verify dependency inputs, source contents, executable images and the empty
namespace's final membership fence. Borrowed Python environments retain their
ordinary cache validation. The [image contract](../docs/ops/INTEGRATION.md)
owns these boundaries; timing the removed eager builder does not measure the
current image policy.

The retired schema-v7 Python image publisher derived three ordinary bytecode variants per
source. One publication action retained trusted derivation hashes for its fresh
receiver capture, eliminating duplicate compilation while preserving full input
and output reads. It also retained CPython-derived destinations and created each
parent directory once. Two alternating before/after pairs on macOS arm64 over
3,383 real sources and 10,149 byte-identical caches measured publication plus
fresh source/cache inventory and verification at median 16.28 seconds before and
10.19 seconds after (37% lower). Median process peak memory rose from 155.3 to
156.4 MiB; source/code buffers remained bounded to one module and the action table
scaled with cache variants. This component excludes full environment/runtime
inventory, dependency installation and process startup; it does not measure the
entire dev loop. Cold publication also removed one child and JSON round trip.
These historical schema-v7 measurements do not qualify the current source-only policy.

Automatic checkout retention uses the existing detached supervisor and immutable
controller source, retaining source and interpreter use through terminal
publication. A recent daily stamp costs one observation and a small immutable
epoch value, with no process or inventory. Due status and attempt identity come
from that same observation. Due dispatch reuses the controller's existing source
publisher without fetching or installing dependencies; discovery and deletion
continue under the existing daily and per-root locks after command return.

One source command previously spent 46.73 seconds after its 11.26-second guard
in synchronous daily collection. A controlled macOS arm64 fixture now returns
while its collector is independently blocked, removes the originating lane, then
executes actual eligible-root reclamation and retains both GC and pressure logs.
Dispatch with a new source projection took 4.95 seconds; reuse from the surviving
pinned controller took 0.47 seconds in that fixture. A separate source-only
publication of 2,095 files / 37.54 MB took 1.43 seconds. These are component
observations, not paired whole-compiler latency measurements. Background I/O
remains, failed startup can still wait for custody closure, and environment
admission is a separate cost. The [retention contract](../docs/design/foundation/56_dx_buildspeed_tooling.md#26-checkout-state-retention-hf-313)
owns policy, startup and failure boundaries.

WASM policy separates typed build intent, target placement, effective captured
Cargo incrementality and shared publication. The ordinary nonincremental default
is retained: one controlled `dev-fast`, micro-runtime WASM source edit took
32.31 seconds without incrementality versus 34.14 seconds with it (Cargo compile
17.98 versus 21.17 seconds), with retained incremental state verified. Both paths
used isolated targets and suppressed shared publication, so this is a component
comparison, not ordinary default-loop or cross-platform qualification. Explicit
iteration controls remain available; only bound nonincremental products may
publish to the shared cache. [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#incremental)
own effective settings after canonical sccache normalization.

Local filtered pytest cohorts now carry their own typed prerequisite selection.
On the retained 39-command assembly-policy aperture, all commands and collection
requests remain identical; only the 34-test Python GPU frontend selector loses
its inherited Rust/Cargo requirements. That session occupied 6.05 seconds in the
previous Cargo chain and can now overlap the real writer. This is a verified
classification and dependency reduction, not a new end-to-end timing comparison.
Unfiltered consumers that build native fixtures keep their prerequisites, and
writer serialization, pytest-session limits and memory envelopes are unchanged.

Explicit unfiltered cohorts use `all = true` with nonempty targets in the same
`pytest_select` authority as filtered cohorts. A complete-file consumer that
executes Cargo keeps its Cargo prerequisite even when its surrounding CI rule
requires only Python. The leaf and shared-input rules name the same cohort, so
the existing command identity deduplicates their combined selection. Ordinary
`pytest = []` still means no work; an empty explicit cohort is invalid.

Test fixture edits select their owning consumer without requalifying unrelated
producers. WASM single-compile fixtures retain their runtime/Cargo owner;
native-link benchmark fixtures retain the complete benchmark test file. Native
transfer leaves select transfer, Nightly bundle and Nightly preparation tests.
Changes to shared frontend, extension, CI and transfer authorities still select
their consumer union. This follows [LLVM's individual regression tests](https://llvm.org/docs/TestingGuide.html#unit-and-regression-tests)
and [Cargo's package and target selection](https://doc.rust-lang.org/cargo/commands/cargo-check.html):
the dependency boundary determines the work. Selection reduction alone does not
establish an end-to-end latency improvement.

Integration source fences obtain HEAD and cleanliness in one fresh porcelain-v2
status observation. Closed one-reference symbolic-ref queries and tree reads use
the existing metadata path, preserving managed-state leases and keeping mutations,
pager launches and recorded-output executions guarded. Two alternating before/after
pairs of the real queue-bisection test on macOS arm64 kept all 42 source fences and
five gate executions while reducing Git commands from 334 to 292. Median test-call
time was 10.43 seconds before and 9.41 seconds after; process wall time was 14.03
and 12.31 seconds. These two-sample medians are observations of one local workflow,
not full compiler-loop, cross-platform or release performance qualification.
A separate warmed metadata replay over 20 branch/tree reads measured median 0.659
seconds before and 0.236 seconds after, with exact known outputs in both paths.
Freshness and source mutation refusal remain part of correctness; command-count
reduction does not authorize a cached HEAD or worktree verdict.

`tools/opt_explorer.py` explains a number before anyone optimizes it. It never
publishes a performance result.

- `anatomy prog.py` builds `prog.py` with symbols and reports, for every
  compiled function and every natural loop, the instruction count, the branches
  and the calls by callee. Calls through a register resolve when the function
  materialized the address (`adrp` + `add`, or a GOT slot).
- `profile prog.py --callers SYMBOL` samples one run with macOS
  `/usr/bin/sample` and splits self time into cost buckets: compiled code,
  runtime entry, reference counting, allocation, attribute access, call
  dispatch, exceptions, other runtime and system libraries. It lists the
  hottest symbols, demangled, and the direct callers of a symbol.

`tools/sample_profile.py` is the one parser of `sample` reports; the scoreboard
and the explorer both read through it. A claimed optimization names its limiter
from these outputs, then shows the change in the canonical gate above.

## v1.0 Acceptance Scope

The existing core gate is necessary, not sufficient, for v1.0. A native/LLVM
core pass does not certify WASM, ecosystem workloads, frontend/build latency,
memory, temporary disk usage, or binary size. Missing target coverage and stale
measurements remain release gaps; they are not implicit passes. The current
startup budget has null ceilings, and the cold-start table leaves the shipped
`release-output` runtime unseeded and has no WASM cell. Existing native/LLVM
baseline ceilings still apply; the missing numeric product budgets must be
declared and measured through this authority.

The scoreboard executes linked WASM through its compiler-declared deployment
manifest and the public Node runner, without ambient engine flags or warning
suppression. Its optional guest-stderr capture retains exact guest bytes and
keeps Node diagnostics separately in the cell log. Each WASI instance owns its
capture descriptor until guest close/renumber or host teardown; ordinary runs
allocate no capture. Child launch-to-exit timing excludes guard cleanup, but
includes fresh-engine startup, compilation and instantiation. Warm samples are
filesystem/host-warmed process invocations. Historical measurements with forced
engine settings or combined stderr retain their original scope. The methodology
version separates the new measurement boundary from those earlier boards.

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

WASM dispatch planning projects the final executable graph rather than admitting
every physical block as an ordinary dynamic target. A paired macOS arm64 component
diagnostic compiled the same retained 2,365-operation locale dictionary function
through the production mandatory TIR/WASM pipeline. Its emitted table narrowed
from 5,898 to 1,179 targets; the other 72,724 decoded instructions and all 8,499
ordered runtime calls were identical. The dispatcher loop and forward scopes
remained because final ownership cleanup introduced backward transfers.

Single compile-only observations with the same retained Wasmtime host and default
engine policy measured translation at 93.293 s before and 8.292 s after. The
baseline exceeded the unchanged 180 s bound before codegen completed; the changed
module completed in 113.276 s, including 103.693 s for the function's compilation,
with 4.965 GiB peak sampled/direct-child RSS. The timeout is censored, not a finite
whole-compile speedup. These are unlinked component observations with diagnostic
logging, not linked-module execution, installed-user latency, quiescent medians
or full-module/release acceptance.

Replacing per-literal WASM anchor locals with one invocation-owned scratch region
further reduced this same component from 4,488 to 3,546 locals and from 246,843 to
228,793 bytes. Its 944 anchors occupy 7,552 checked bytes. The other 91 function
bodies, all non-code sections, constructor inputs and the 1,179-target dispatch
table remain identical. With the same retained host, cache-disabled engine policy,
diagnostic logging and 180 s bound, single compile-only observations measured
113.276 s before and 28.575 s after: function translation fell from 8.292 to
4.625 s and compilation from 103.693 to 22.627 s. Peak sampled/direct-child RSS
fell from 4.965 to 3.396 GiB; serialized native artifacts shrank from 41,569,600
to 36,371,200 bytes. The earlier receipt does not retain exhaustive ambient
environment or numeric guard limits, so these are development observations rather
than a fully controlled benchmark or canonical performance score.

This accepts a compile-latency/runtime-allocation trade: a literal-bearing
invocation or poll performs one resource-accounted allocation/free pair, uses
8 bytes per anchor plus allocator overhead and linear initialization/cleanup
traffic, and can fail resource admission. A function without anchors allocates
nothing for this storage. A historical one-anchor, two-million-call Node
diagnostic measured literal process-wall medians of 5.340 and 5.447 s. Every sample
started a fresh process, and the after-sample spread exceeded the estimated
control-adjusted signal; it establishes neither isolated allocator latency nor a
current runtime regression percentage. Cold/steady runtime, retained memory,
concurrency, linked-module execution and the wider release matrix remain
unqualified. Ownership and failure cleanup follow
`docs/spec/areas/compiler/0190_LOWERING_RULES.md`.

Direct literal constructor operands remove the remaining per-output raw pointer
and length staging from both WASM emitters. On the same component this reduces
declared locals from 3,546 to 1,190 and module bytes from 228,793 to 212,627,
without another runtime allocation or call. Existing Wasmtime pass telemetry
identified register allocation as the remaining dominant cost: 20.008 s of
22.253 s of function compilation before the deletion. The corresponding single
instrumented observations took 28.001 s before and 30.197 s after, with register
allocation at 22.002 s afterward. This establishes a smaller representation and
removal of staging, not a compile-latency improvement. Further work must target
the measured limiter and whole edit-to-result latency.

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
- `requested_profile`: the request label, never an inferred physical profile
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

Its attestation failed the Variant-II audit below and was withdrawn (HF-188); the
measurements here are dated history, not a citable perf claim.
The measured aperture is the real `pact-witness-acceptance` build through the
current runtime frontier. The acceptance run currently fails after codegen in
WASM import stripping, so replay is not measurable in this revision; build-path
numbers remain valid and queue-custodied.

| Rank | Phase | Cold / miss path | Warm incremental path | Inherent floor | Ranked waste |
|---:|---|---:|---:|---:|---:|
| 1 | Frontend graph + analysis + lowering | 390-604s across same-machine diagnostics; 0-24% lowering hits | 467.6s avoidable before this landing; unique output path changed all contexts | Reuse unchanged 145 module lowerings; lower only changed modules | 467.6s eliminated (61.3% wall, 2.59x) |
| 2 | Backend prepare/codegen | 198-410s, including first population of thousands of functions | 295.0s total build to the current import-strip frontier; only 12 uncached TIR functions in the steady sample | Rebuild the changed runtime/compiler cone and changed functions | Next target: function/object cache misses plus import-strip frontier |
| 3 | Runtime wasm cargo compile | 23.9-265.5s historical; 124.7s recent cold sample | Target/shared reuse is fingerprint-controlled; final patched sample did not rebuild runtime | One changed runtime crate compile | Audit configured vs effective runtime-wasm hydrate hit rate |
| 4 | Runtime reloc link | 3.831193s median warm isolated relink for the 69.5MB runtime; target-staticlib reuse previously linked before cache lookup | 1.287784s exact-cache hydrate median after cache-first ordering | One relink on a true cache miss | 2.543410s eliminated (66.39%, 2.975x) |
| 5 | Seal / validate | Isolated NumPy 2.5.1 seal validation was 1.441451s median across five fresh processes; 1.182s cumulative was repeated relocation-root discovery across 132 objects | 0.869927s median after one resolver precomputes manifest relocation roots once | Hash each source once; discover relocation roots once per manifest | 0.571524s eliminated (39.65%, 1.657x) |
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

Its attestation failed the Variant-II audit below and was withdrawn (HF-188); the
measurements here are dated history, not a citable perf claim.
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
and a recorded memory-ceiling run. An attestation is checked in only when it
passes this audit. Five July 2026 attestations (perf goals r3, r4, r5, the
witness iteration and the WASM publication strip) failed it on dev-profile,
proxy or single-run evidence and were withdrawn (HF-188). The four
`tools/opt_matrix_r*_attestation.json` records pass it.

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

## Linux sealed release replay provider

`tools/release/consumer_replay.py` owns one post-uninstall native/WASM replay
contract. Its Linux provider is `SealedRootTransport` in `tools/cross_run.py`:
a system-manager-owned import and ephemeral execution root, using the existing
Linux proof supervisor for actual guest executable identity and complete process
receipts. It does not provision Docker, a new VM, a scheduler, or an end-user
dependency. Generic Docker cross-platform smoke transport is a separate producer.

The initial reviewed provider source coordinate is systemd `259.5-0ubuntu3.4` on native
Linux aarch64 or x86_64. The installed Ubuntu security package includes the
fixes in [USN-8626-1](https://ubuntu.com/security/notices/USN-8626-1)
(published 2026-08-10); no package install or seven-day age exception is needed
for that recorded coordinate. Package versions are provenance, not a permanent
single-patch admission gate. Every invocation must supply the required effective
API properties; unsupported settings fail without retry or fallback. Recheck the
runner before dispatch. A VM name does not establish its ISA. The host kernel,
system manager and root-owned control tools form the trusted execution base.
Recorded tool hashes identify selected files, not the manager's loaded image;
they must never be projected as compiler/runtime used-byte admission. macOS
remains ineligible for this Linux execution-root contract.

The orchestrator validates the exact sealed tar and sends its captured bytes
through a write/grow/shrink/seal-locked memfd. The manager opens that descriptor
while its owner keeps it alive; a constrained, bounded tar service imports into
a fresh root-owned runtime directory. Exact payload inventory, archive binding,
root ownership and permissions are checked before execution. Each guest service
uses `RootEphemeral=yes`, a read-only root, private PID/user/network/IPC/mount
namespaces, no capabilities, no-new-privileges, and bounded noexec tmpfs scratch.
No host root or host executable bind is admitted. Guest stdin is null; only the
owned stdout/stderr capture descriptors cross the manager boundary.

This follows systemd's maintained [execution sandbox contract](https://www.freedesktop.org/software/systemd/man/259/systemd.exec.html)
and [transient service interface](https://www.freedesktop.org/software/systemd/man/259/systemd-run.html).
The [v259.5 executor](https://github.com/systemd/systemd/blob/v259.5/src/core/exec-invoke.c)
implements the ephemeral directory snapshot with a copy fallback on ext4.
The [service lifecycle](https://github.com/systemd/systemd/blob/v259.5/src/core/service.c)
implements `ExitType=cgroup`: main-process exit alone is insufficient. No custom
namespace, capability-drop or privileged Python helper replaces these owners.

The receiver requires the retained typed D-Bus Unit and Service observations,
including empty properties; omitted fields do not mean default isolation. It
checks the exact supervisor argv, transient invocation, resource configuration,
terminal cgroup-owned state, successful blocking stop job, and invocation-bound
retirement. The existing supervisor receipt verifier independently checks actual
guest image bytes, policy, stdout and complete process closure. An uncertain
creation/stop retains the imported source and nonce-bound intent for recovery;
a failed lookup is never proof that a delayed creation cannot arrive.

Cold cost is bounded archive capture plus one import, one snapshot and two
inventory passes per guest, selected-tool hashes, and manager control queries.
Archive input is capped at 1 GiB. Import has 2 GiB memory, no swap, one CPU and
16 tasks; guest services have 1 GiB memory, no swap, one CPU and 64 tasks.
Scratch totals 96 MiB and captured streams are bounded to 16 MiB each. Existing
cross-run guards own control clients and captures. These are development proof
costs; emitted programs receive no added work. Actual latency and peak memory
remain a real-run measurement obligation.

A successful Python protocol fixture is not kernel isolation, native/WASM product
success, performance admission, or authenticated H0. Acceptance still requires
an explicitly authorized real Linux run of the exact provider postimage, existing
supervisor generation and installed native/WASM subjects, followed by retained
receipt replay and independent archive/identity/omission rejection. This provider
prerequisite does not close the missing consumed compiler/runtime identity
producer or H0 signing producer, change thresholds, or shrink the release matrix.

The same provider has a typed compilation projection with explicit memory,
workspace and runtime budgets. It gives compiler scratch a writable noexec `/work`; imported
compiler, frontend, toolchain and installed runtime inputs remain read-only.
The compilation projection leaves captured `/tmp` installation paths read-only
instead of masking them with replay's disposable `/tmp` mount; `TMPDIR` points
to `/work/tmp`.
Preparation and import are shared across the declared cells of one installed
frontend coordinate. Each invocation has a fresh workspace and independent
supervisor policy, journal and output export. Replay retains its original fixed
limits. Compilation stdout carries the release ZIP export and is bounded by its
workspace budget and the existing ZIP32 archive limit; stderr retains the 16 MiB
supervisor transport bound. Both streams are checked again after pipe completion.

Raw linker reproducers and the final-link publication receipt are checked before
compiler/runtime content can be joined to actual supervisor-observed images.
The controller and cell configuration are themselves reconstructed from source;
sealing an arbitrary controller cannot authorize a caller-written summary.
These source mechanisms do not remove the current performance and full-release
refusals: installed caller coverage, measurement/artifact binding, real kernel
execution and authenticated H0 still require their owning producers and receivers.

Native admission capture also retains the C driver dependency report. Its
header paths are preparation selections, not consumed-byte claims. The canonical
Make parser retains lexical and resolved coordinates: installed roots copy the
selected contents at the lexical header names, while existing extension/Cython
content closures continue to deduplicate resolved files. Selected driver bytes
must already belong to the admitted toolchain root and join the native journal.
CRT objects and link libraries are copied from the LLD reproducer buffers. The
fresh supervised build still supplies the actual link-consumption evidence.

The shared `bench_evidence.METRIC_POLICIES` declaration binds each Molt metric's
success gates and native/WASM lane. Comparison admits the observed target
through the compiler's target authority; an identical wrong-lane observation
does not qualify, and a mixed native/WASM metric cell cannot share one build
observation. A new Molt metric requires a declared policy. Exact native triples
and all admitted WASM/WASI intents preserve their target distinctions; no
runtime profile is inferred from a metric name or target.

The current methodology governs the shared comparison policy and each comparison
verdict owner (`bench_reporting`, `bench_diff`, `bench_dashboard`,
`perf_regression` and the legacy harness), including the target-admission
dependencies. They are not unlocked report-formatting mechanics. The canonical
bump must run after final formatting, and historical versions remain separate.

Scoreboard workload facts use the [admitted source-generation contract](../docs/design/foundation/64_perf_scoreboards_and_harness.md#admitted-benchmark-source-generations). The full release matrix retains each canonical benchmark's entry digest from its existing source-authority capture as `benchmark_sha256`; a measured workload must name that exact entry and digest. Cross-cell consistency does not substitute for this matrix binding. This is source-record consistency, not loaded-byte attestation.


Regular native and WASM benchmark JSON producers carry the same admitted local
Python-source workload record as the scoreboard. Their source generations stay
held through measurement and close before the atomic result replacement. Shared
baseline, diff, dashboard and trend comparisons require equal valid workloads,
including comparisons of external-runtime-only metrics; matching names or
profiles cannot substitute for source identity. Historical records and separate
legacy harness formats without that contract remain displayable but cannot be
compared under this policy. This does not attest loaded bytes or undeclared data,
and does not upgrade noncanonical benchmark provenance or missing build-profile
observations. Source checks run outside each measured child interval and still
have controller/descriptor/IO costs that must be measured separately.

The legacy harness keeps its status-only baseline decision on its existing
`results[suite][name]` records. Source-bound flat `benchmarks` keys serve metric
readers; they do not replace or re-key that status coordinate. Thus old harness
status evidence remains useful without assigning it a workload identity or
qualifying its unbound Molt timing observations.



Input collections are validated before matching benchmark keys, so a renamed
historical basename cannot disappear from an acceptance gate as an empty match.
Baseline and threshold gates refuse zero actual comparable measurement pairs;
ordinary added/removed reports remain available without a performance verdict.
Current producers consume their admitted Molt/run argument lists. WASM and the
individual runner reject nonempty guest run arguments, which their current APIs
do not execute; the legacy harness additionally checks its actual Molt command
tail against the admitted list before either runtime starts. These refusals do
not add a new argument parser or silently change the executed program.
