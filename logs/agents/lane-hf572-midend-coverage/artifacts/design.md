# HF-572: complete frontend mid-end coverage

Owner: lane/hf572-midend-coverage, slot-03. Parent owns integration and ledger.
Base: 45c442523be8f74df85ab61671a84ec647026307 (origin/main at start).

## Problem and measured evidence

The parent retained a native bench_chainmap build at db54a2f968:
22 module stats records, 1,144 stdlib functions bypassed in 21 modules;
user module SCCP cap hits 15 and expanded CSE fallback 1. This is historical,
not a current-base measurement. On the selected base, inspection confirms the
stdlib early return, size thresholds (1,500 default, 650/500 under pressure),
policy work-budget degradation and SCCP limits counted as block visits but
configured as tiny per-tier numbers. A whole program's apparent optimization
coverage cannot be inferred from small user probes. Baseline runs must identify
the complete transitive lowered program, not only its entry module.

Startup: clean assigned branch; context exit 2 reports stale external proof
custody and a missing unrelated scratch checkout. No lane proof has run yet.
Slot image Python is 3.12.15. The performance oracle must separately resolve
and attest exact CPython 3.12.16 without modifying the image.

## Existing authority, history, and prior art

HF-549 is landed: Rust has one iterative Semi-NCA engine and interval queries.
Python cfg_analysis.DominatorTree already uses dense CHK immediate parents,
iterative DFS and O(1) interval queries. Reuse this implementation and its
path-search oracle; do not introduce another graph authority or replace it
merely because the brief predates it. Measure residual dataflow costs.
Existing midend_policy_outcomes_by_function flows through frontend_integration
into cli/build_diagnostics._build_midend_diagnostics_payload. This is the
coverage authority; no independent production registry is added.

Primary sources consulted:
- Cooper/Harvey/Kennedy, A Simple, Fast Dominance Algorithm:
  https://www.clear.rice.edu/comp512/Lectures/Papers/TR06-33870-Dom.pdf
  CHK is quadratic worst case, efficient in practice; do not claim a strict
  near-linear bound. Existing O(1) interval queries eliminate ancestor walks.
- LLVM GenericDomTreeConstruction:
  https://llvm.org/doxygen/GenericDomTreeConstruction_8h.html
  Semi-NCA also has a quadratic worst-case bound. Algorithm choice must follow
  observed graph costs, not an asymptotic claim that neither solver guarantees.
- Go SCCP implementation:
  https://go.googlesource.com/go.git/+/a98e6e1013c473cdaf150aa91a98503a5631f4a9/src/cmd/compile/internal/ssacompile/sccp.go
  finite lattice transitions and executable-edge worklists provide termination;
  a fixed number of visited blocks cannot establish a fixed point.
No upstream implementation is copied; existing Molt algorithms are reused.

## Chosen mechanism and invariants

1. Record coverage at _run_ir_midend_passes, once per emitted function. Keep the
original input op count as denominator (not surviving optimized ops). Store
status, reasons, input/output ops and relevant counter deltas on the existing
function outcome. Aggregate whole-program functions/ops and fractions in existing
build diagnostics; report zero-denominator fractions explicitly. Module skips,
size skips, profile omissions, work budget, SCCP/CSE caps, verification rejection,
rollback and expanded fallback must not count as fully optimized. Categories
are disjoint: skipped, fallback, degraded, full; reasons may overlap. Failed
builds never claim complete coverage. A pass reaching a fixed point without a
rewrite still counts as optimized. Existing stats text exposes the same data.
Original functions before lowering and functions eliminated by closed-world
reachability are outside the denominator; explicitly describe that limitation.

2. Remove the stdlib and oversized admission gates after recording the baseline.
All admitted functions run canonicalization, SCCP and DCE regardless of source,
size or monolith pressure. Reuse iterative dominance/DFS. Sweep sibling skip
consumers, environment configuration, docs, tests and diagnostics together.
Replace tests that preserve the old gate with semantic and coverage obligations.
Large-function tests use independent observable results/path search, not an
assertion that a helper was called. Keep expensive optional optimization budgets
only where measured; expose every omission in coverage.

3. SCCP defaults drain the deterministic worklists to a fixed point. Audit lattice
monotonicity, predecessor-state dependencies and repeated definitions before
removing its default visit ceiling. Explicit diagnostic limits, if retained,
return conservative facts and report coverage loss. Distinguish SCCP transfer
visits from outer rewrite rounds. Do not silently raise a tiny cap and call it
convergence. Regress with long acyclic CFGs and cycles and CPython differentials.
For residual large-function cost, profile first, bound retained facts by liveness
where safe, and use sparse queued dependencies rather than whole graph sweeps.
No new cache. Do not turn off valuable CSE paths to improve compile time.

4. CSE aliases must refer to unique producers that dominate every rewritten use;
repeated-result mutable carriers and branch-local facts cannot become global
aliases. Inspect each corpus verifier failure at its first invalid transition,
fix the owning precondition and add an adversarial regression. Per-pass verifier
rejections, not only final expanded fallbacks, are coverage loss. Retain safe
failure handling only when explicitly reported; do not remove validation to make
coverage appear complete. A semantic ambiguity is returned to the parent rather
than widening Python scope. Facts retain current SSA identities; this work does
not create a new name-keyed optimization fact family.

## Consumers and bounded implementation sequence

Canonical sources: frontend/lowering/midend_pipeline.py, midend_policy.py,
midend_dataflow.py and midend_canonicalization.py; frontend/_types.py policy
fields and generated protocol projections when fields change; cfg_analysis.py
only for a demonstrated residual algorithmic defect. CLI build_diagnostics.py
and the existing diagnostics renderer consume the coverage projection.
Direct tests: test_frontend_midend_passes.py, frontend CFG and operation-registry
readers, cli build diagnostics/integration and determinism tests. Update the
owning optimizer_foundation design with coverage and TIR migration sizing.
proof_plan.toml changes only if a new test obligation has no selected cell.
No reserved paths are edited. Every extension is listed in the handoff.

Slices: reviewed design commit first; coverage with baseline; enable/fix solver
and alias causes; consumer migration and docs; measured qualification; exact-path
implementation commits. Scratch/builds remain under lane TMPDIR. Canonical
artifacts live in molt-src/logs/agents/lane-hf572-midend-coverage/artifacts.
The design is committed as an explicitly requested artifact at the identical
lane-relative logs path (normally ignored); retained canonical copy has the same
content. Routine logs are not added to source.

## Proof and measurement plan

Before each long command/commit reread the entire brief for corrections. Retain
command, cwd, source identity, relevant child environment, exit status, duration,
counts and log paths. Resolve native/LLVM/WASM compiler/runtime/runner availability
before expensive proofs. Only this worker launches proof commands.

Coverage corpus: examples/hello.py; canonical loop program; all bench/bench_*.py;
all ten CLBG kernels; existing differential stdlib programs for json, re,
collections and dataclasses. Use actual frontend/import collection and lowered
function op counts. Record the exact manifest and failures. Baseline is the
pinned origin/main source, head is final lane source; tools/temporary probes
must work unchanged against both. Do not synthesize zero skips from missing data.

Correctness: changed direct-reader pytest at <=4 workers; existing stdlib corpus
CPython differential on native, LLVM and WASM with mid-end enabled; large-function
and CSE adversarial fixtures; existing SCCP, effect and definite-assignment tests;
structural verifier and IR determinism. Stop dependent batches on first failure,
classify product/harness/artifact/unknown and retain receipts before retry.

Cost: five alternating base/head pairs for bench, CLBG and stdlib-heavy set;
exact CPython 3.12.16 output oracle, instructions retired from the existing PMU
measurement authority, compile wall time and /usr/bin/time -l peak RSS. Record
arguments/stdin, backend/profile, hardware, runtime/toolchain identity and artifact
custody. A missing PMU/toolchain/oracle is an unknown, never a performance win.
Compilation can share matching immutable runtime artifacts; frontend/module
artifacts must correspond to each source. No competing Cargo writers.

Run land --static-only, and the canonical land --precheck-only at most once from
the plain landing environment (unset CARGO_TARGET_DIR and UV_PROJECT_ENVIRONMENT,
TMPDIR on data volume). Inspect matching receipts first. Fix exposed in-scope
failures. Report integration/release qualification separately from local proof.

## Structural TIR end state (estimate to refine from live census)

This lane does not move the optimizer. One typed TIR owner requires preserving
structured exceptions, mutable-carrier semantics, source spans, generator resume
edges and definite assignment through frontend-to-TIR construction; then retire
each Python fact family as shared TIR proves equivalent behavior and measured
cost. Order: boundary/identity/effects and differential census; direct TIR
construction or lossless typed ingress; SCCP/CFG/DCE; GVN/memory facts; delete
Python policy/optimizer and old SimpleIR counterparts with consumers. Estimate
several cross-layer slices, not a one-file refactor; size and counts will be
refined against the live producer/consumer census in the final owning design.

## Review record

Pending: two independent sequential codex exec -s read-only Goldfish reviews,
gpt-6.1-sol at high, each with only this file path and reviewer role. Findings
will be answered here before the design commit and before implementation.

### Review 1: design critic (completed, exit 0)

Six findings, all accepted and resolved below. The reviewer read only this file;
full transcript and final findings are retained in review-design.log/.txt.
Correction to initial inspection: the live default size skip is **800**, not
1,500 ops (the pressure thresholds are 650/500). No baseline result is claimed.

D1, high, denominator: serialization.to_json's functions are the expected set.
Reconcile each run_midend=True map_ops_to_json invocation and function identity
with policy outcomes in the producer, then reconcile the serialized function
list before reporting the module. Include generated module chunks. A missing
outcome is skipped with reason missing_outcome, never absent from denominator.
Repeated serialization of the same function replaces its final record; attempts
inside one invocation accumulate defects. All transitive module outcomes are
qualified by existing module::function identity in frontend_integration.
Coverage describes serialized lowered functions before backend reachability;
the tool will explicitly report that denominator, not source function coverage.

D2, high, termination: SCCP has UNKNOWN -> exact immutable constant -> OVERDEFINED
scalar lattice, with missing sentinels overdefined; executable edges only grow.
Heap observations are must-facts killed by the canonical effect authority and
joined only when all executable predecessors agree. A block reevaluates when
an input predecessor state changes or an incoming edge is first executable.
Default completion requires both queues empty. Any retained explicit diagnostic
limit returns empty facts and ALL CFG blocks/edges executable with NO branch
choices, so no pruning can use partial solver results. Repeated definitions are
invalid under the current verifier and cannot establish a unique SSA fact.
Audit monotonicity against the actual non-SSA carrier boundary; no default cap
is removed until cyclic tests settle. Outer rewrite rounds fail closed on
non-convergence, separately from solver coverage; they do not emit a partial
successful build. A non-monotone fact is repaired at its join/transfer first.

D3, high, CSE semantics: reuse must additionally satisfy the existing
_op_effect_class / _op_may_access_arbitrary_heap authorities and literal_identity
exact-type/allocation-identity rules. midend_canonicalization invalidates heap
reads across writes and arbitrary callbacks; no dominance-only reuse is added.
Preserve effectful/raising computations even when results are dead. Existing
callback, heap-mutation, alias, NaN/signed-zero, MISSING and raising-DCE tests
are direct readers; add regressions for newly exposed first invalid transitions.

D4, high, CFG semantics: cfg_analysis.build_cfg is the one authority, including
NORMAL/EXCEPTION/RESUME edge flags. TRY_START and CHECK_EXCEPTION retain declared
handler reachability; RAISE falls through to explicit routing; STATE_YIELD
terminates and STATE_SWITCH reaches resume blocks. TRY_END custody is handled by
try_regions, not textual interval guesses. Unknown effects remain effectful and
exception edges stay conservative; that is conservative analysis, not an omitted
pass, and is not a degraded status. A disabled pass or verification rejection
is degraded. Differential obligations include raise-only handlers, finally,
generator suspend/resume, and callback invalidation alongside stdlib cases.

D5, high, cost: current verifier retains O(B*D) definition sets despite already
requiring unique producers and dominance. Replace this redundant dataflow with
one definition index plus interval queries (O(ops+uses) excluding CHK construction)
if the existing independent failure oracle confirms equivalence. SCCP/CSE full
state maps may retain O(B*F) facts; that is not called near-linear. Profile large
acyclic and cyclic functions at geometrically increasing sizes, require no
recursion failure, report scaling and peak memory, and investigate >3x cost for
a doubling. Ordinary whole-program compile/RSS regressions >20% are investigated
and reported to parent with real coverage gained, never hidden by a size skip.
The 8 GB host requirement remains binding; no acceptance by raising it.

D6, medium, classification: priority skipped > fallback > degraded > full.
Any retained input/identity with no completed pass pipeline is skipped. A final
weaker rerun is fallback even if it verifies. Any budget/cap, required-pass
omission, rejected candidate or rollback in the invocation is degraded even if
a later attempt succeeds. Full requires canonicalization/SCCP/DCE/CSE completed
at the configured profile's fixed point without these losses; a configured
profile that omits CSE is degraded. Effects that prove CSE inapplicable are not
an omission. Counts sum functions and original input ops per disjoint status;
shares divide those sums by all input functions/ops. Empty denominators produce
null fractions, not 1 or 100%. Reasons are independent tallies with their own
function/op sums; do not add reason counts as category totals.

### Review 2: implementation readiness (completed, exit 0)

Five findings accepted; closures below. This is the second and final review in
the brief's bounded review process; no claim of reviewer approval is made.

R1, acceptance: implementation acceptance requires zero stdlib/size skips on
every successful coverage-corpus build, coverage reconciliation with no missing
module/function records, no unreported loss, changed direct readers green and
native/LLVM/WASM differential acceptance. Optional profile omissions/budgets may
remain only when explicitly classified, with a reason and measured cost; they
cannot support a 100% full claim. Any unknown backend, incomplete corpus, failed
precheck, unresolved semantic defect or unmeasured >20% cost regression blocks
ready-to-land status. The lane may return incomplete only at the explicit brief
stop conditions (unsettled semantic decision, reserved conflict or 300 minutes),
with retained evidence and remaining obligations. Parent owns release acceptance.

R2, verifier oracle: a test-only path-search oracle removes the candidate
definition's block and asks whether entry can still reach each ordinary use;
same-block uses additionally require def_index < use_index. Reachable duplicate
definitions are rejected separately. For exception and resume paths, CFG splits
at CHECK_EXCEPTION and STATE_YIELD/SWITCH; values defined after a check cannot be
used in its handler and values after a yield cannot dominate a dispatch-entered
resume block. Enumerate small CFGs and compare to the old set-based verifier,
then independently assert accepted/rejected use sites. Include unreachable
predecessors and before/after-check examples. PHI semantics remain the existing
frontend contract; this lane does not weaken validation to accept new PHI forms.

R3, inventory: frontend execution's selected module graph is the expected
transitive lowering inventory; its integration step consumes actual function
records for each module, including generated chunks. At that boundary reconcile
functions with outcomes before merging. Missing coverage is an explicit skipped
record derived from the function inventory, including when an older persisted
module result lacks coverage. Duplicate identities are rejected by existing
function integration. Module execution completion already requires every selected
module's integration; any missing module fails the build rather than publishing
complete stats. Verify that authority in the existing integration tests; do not
invent a second module registry. Reused function/module artifacts retain original
source-matched coverage receipts; absence is unknown coverage, never full.

R4, SCCP: exact scalar equivalence is literal_identity (exact Python type and
float bit identity, preserving signed zero/NaN distinctions). Each block's scalar
output must accumulate by join with its previous output: UNKNOWN < one exact
constant < OVERDEFINED, never regain a narrower scalar. Heap relations have a
finite per-function key domain (only tags of existing result/operand identities).
Their ordering is unobserved, exact tag, unavailable; after a processed block
loses a relation it cannot regain it. Transfers can generate local observations
but the output accumulation must not resurrect killed relations. A new incoming
edge schedules its destination even if scalar maps agree. Every output change
schedules executable successors. These finite transitions plus finite edges
bound rescheduling; block transfers and joins still cost the retained-fact set.
If live tests expose an ordering that cannot preserve a useful fact, keep the
sound conservative result and measure it rather than adding a retry or cache.
Cyclic fixtures independently expect mutations/callbacks to invalidate class
facts and scalar arithmetic to widen at backedges; never infer allocation
identity from equal values. The implementation must satisfy these invariants
before the default visit cap goes. Explicit diagnostic caps remain conservative
and counted. No claim of closure rests solely on a terminating test run.

R5, baseline: baseline is exactly 45c442523be8f74df85ab61671a84ec647026307,
not a moving origin/main. A temporary observer wrapping the existing serialization
and pass entry/return boundaries records original ops, function identities,
existing policy events and counter deltas; it does not change outputs or policy.
Missing baseline-only telemetry stays unknown (especially internal verifier
rejections not already counted). Report full as a lower bound when unavailable
loss fields prevent exact classification. Coverage observer overhead is excluded
from paired performance by running clean CLI builds separately. Performance
manifests bind both source SHAs, exact interpreter, arguments/stdin and artifact
identities. The baseline observer is retained outside tracked production source.
