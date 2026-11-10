# molt Optimizer Foundation — the engine for "Mojo/Julia/Codon/Nuitka, but better"

Status: NORTH-STAR ARCHITECTURE (2026-06-03). Supersedes the narrower framing in
`generator_fusion.md` (generator fusion is now a *consequence* of this foundation, not the
architecture). Origin: user vision — "bleeding edge, like Mojo and Julia but better and faster;
like Codon but more ecosystem compatibility / language support / flexibility; like Nuitka but
way faster and even more flexible."

Support and release claims are bounded by the
[verified subset](../spec/areas/compat/contracts/verified_subset_contract.md)
and its qualified target cells. The north-star comparisons below describe the
architectural goal.

## Mid-end coverage and bounded work

Every serialized lowered function receives frontend mid-end coverage accounting,
including stdlib bodies and large functions. User functions enter the optimizer
regardless of size. Stdlib bodies bypass the Python optimizer as a measured cost
policy owned by HF-773, while the backend Rust TIR pipeline still processes them.
This is a cost decision, not a correctness gate: the 2026-11-05 round-2 native
release measurements found cold builds growing from 51.66 to 111.58 seconds for
`bench_math_numeric`, 59.05 to 109.70 for `bench_colorsys`, and 40.91 to 77.04 for
`binary_trees`, for 0–3.4% fewer runtime instructions across the measured corpus.
Receipts: `logs/agents/lane-hf572-midend-coverage/artifacts/round-2/report.md`
in the integration checkout. These are whole-lane observations, not an isolated
attribution of every instruction delta to stdlib optimization. HF-773 must carry
the measured opportunities into the existing typed SSA optimizer and retire the
overlapping Python fact families. It owns full stdlib differential qualification.
The round-3 design and two sequential read-only review records are retained in
the same lane's `artifacts/round-3/`; the first review's six findings were answered
and the implementation-readiness review found no remaining design gap.

The build's existing mid-end diagnostics
record original input ops, output ops, status and reasons per function; a
program summary is also emitted without detailed timing diagnostics. The
denominator includes generated module chunks and precedes backend reachability.
It is not a count of source functions or surviving optimized operations.

The disjoint statuses are `full`, `degraded`, `skipped`, and `fallback`, with
skipped taking precedence over fallback, and fallback over degraded. A fixed
point without a rewrite is full optimization. A verification rejection,
rollback, solver limit, disabled required pass or deterministic optional-pass
budget is coverage loss even when a later attempt verifies. Reasons may
overlap; their counts must not be added as category totals. Missing cached
outcomes retain their function inventory as skipped with unknown input ops;
op fractions are null until the denominator is complete. Empty denominators
also have null fractions. Failed builds do not establish program coverage.
Stdlib bypass records `skipped` with reason `stdlib_cost_policy`, including empty
functions, original/output op counts and zero executed passes. The historical
`midend_module_skips` counter counts per-function invocations of this policy;
program function/module inventories provide their own denominators. Skipped
stdlib functions are never counted as fully optimized by the Python pipeline.

Required outer rewrites and SCCP's default worklists drain to a fixed point.
Tier/profile round counts calibrate optional CSE's work budget, not required-pass
admission. Backend benchmark and differential runners impose no hidden solver
or outer-round diagnostic limits. An explicit diagnostic visit
limit returns no value or branch facts and marks all CFG blocks and edges
executable; its loss remains in coverage. Successive scalar outputs widen by
the exact-literal lattice, and killed heap relations cannot resurrect around
a cycle. The shared value-liveness analysis bounds retained block facts to
values used locally or later. Joins project needed values before merging:
a shared exception handler must not join every dead temporary from every
predecessor. The verifier uses unique producer sites, instruction order and
dominator intervals rather than a definition set for every block. MISSING
taint follows PHI use dependencies iteratively.
Authored `LOOP_INDEX_START`/`LOOP_INDEX_NEXT` are explicit pre-SSA loop carriers:
an update requires its own loop's unique dominating start. They do not admit
ordinary repeated SSA definitions or flow-insensitive literal facts.

Optimized stdlib functions are products of the existing source-qualified
module-lowering record, together with function metadata and original coverage
receipts. Serial and parallel readers consult that product before constructing
the frontend generator. Its identity must include module bytes, semantic
compiler identity, target Python/platform, source-body selection, dependency
facts, profile/PGO and every mid-end policy setting. Reusing the product executes
no mid-end passes; restored pass timings describe its original production,
identified by the existing cache-hit worker mode. There is no separate stdlib
optimization cache. Cold compile cost and warm product reuse are distinct
measurements; backend artifact reuse is another boundary.

CSE propagates aliases only for values live out of each block. Substitutions
for that block's eliminated definitions remain at their producers until final
operand rewriting, including substitutions that are dead in the original
value graph. This bounds propagated alias entries by the sum of live-out names,
plus one local substitution per defining operation, instead of copying every
earlier eliminated definition through every later block. Available expressions
remain a separate fact: a dead representative may eliminate a future repeated
computation. Their removal cannot be justified by value liveness alone.
Scalar and type facts propagate for live values, live alias targets and all
available representatives; dead eliminated aliases do not retain duplicate
facts in every later block. This projects the existing fact domain without
dropping expression availability or changing effect invalidation.
CSE compares the four closed fact maps directly, using target SSA identity for
aliases and available expressions. It retains no sorted signature projection
or mutation-invalidated memo alongside those maps.

With `MOLT_MIDEND_STATS` present, existing pass statistics also record actual
call counts and sampled compiler-process peak RSS, including entry/exit samples
and a 10 ms observer. Nested SCCP/CSE solver calls have their own records; their
inclusive wall times and resident peaks overlap outer stages and must not be
added as a total. RSS includes retained inputs and allocator pages, rather than
only allocations made by the pass. Peak growth is relative to its first valid
sample; unavailable observations remain null with explicit missing-sample counts.
Disabled or rejected-before-execution stages have zero calls, independently of
their recorded optimization attempts. The observer is owned by one function's
mid-end invocation and closes on success or failure. Normal builds start no
observer. Restored module-product timings remain historical receipts, so fresh
optimizer profiling disables lowering-cache reuse.

Frontend dominance queries use intervals and retained tree storage is linear.
The CHK construction itself has a quadratic worst case: a chain whose final
node has edges back to every earlier non-entry node makes repeated intersections
walk long parent paths. A macOS arm64 / slot Python 3.12.15 measurement on
2026-11-05 took 0.056/0.213/0.827/1.283 seconds at 4k/8k/16k/20k nodes
(2N-3 edges), with a 155.9 MiB process high-water RSS at 20k including Python
and imports. This bounds that measured case only; arbitrary graph construction
is not claimed to be near-linear. Required frontend passes still admit every
user function.

PHI operands belong to incoming merge edges, including normal continuations
through exception checks after a structured merge. One linear CFG edge table
feeds liveness, SCCP, trimming and verification.
The structured wire boundary additionally requires PHIs immediately after
their `END_IF`; branch tail merging must retain common checks and metadata
inside the branches when a PHI follows. CFG validity alone does not prove that
backend attachment contract.
Suspended values use explicit
local homes, fresh SSA results on reload, and the existing backend SSA
construction. Checked producers publish into a home only on the success path.
Branch transport captures borrowed frame views before storing them in compiler
locals, so a returned selection survives the source frame exit.
Branch transport uses the same typed local homes; the retired frontend `COPY`
identity has no remaining producer or consumer. The backend wire `copy` remains
part of its own SSA lowering contract. Heap-read CSE invalidates available reads at effects and joins
only identical available results; numeric mutation epochs are unnecessary and
would grow indefinitely on loop backedges. Deterministic work budgets may
disable the superlinear cross-block CSE optimization, with a coverage reason;
they do not truncate required CFG/SCCP/DCE work or skip a function.

This coverage contract does not qualify all backend/profile cells by itself.
The findings ledger owns current qualification. The
[single typed IR migration](foundation/77_architecture_convergence.md#b-one-typed-ir-as-the-backend-input)
owns retirement of the overlapping Python and Rust optimizers.

## Python SCCP/CSE retirement (HF-773 slice E)

Status: proposed, 2026-11-09; implementation requires the integrator's Goldfish
review. This section specifies the migration; the coverage contract above
describes the current implementation. The findings ledger owns qualification.
The integration owner lands and retires the lane; the assigned worker owns its
measurements and proofs. D0 remains diagnostic-only. This migration does not
implement guarded-call specialization or change the inliner's execution-context
admission (HF-824).

### Problem, evidence and alternatives

The frontend and TIR both compute constant, expression-identity and executable-
edge facts. Retiring the Python owner must remove repeated analysis and its
selection machinery while preserving observable behavior and generated-program
performance. The target is one optimizer, rather than another faster Python
solver or a new Rust service called separately from the TIR pipeline.

Historical measurement, retained in HF-572's round-2 report: a full-coverage
native release build of `loop` took 113.653 s and 1,794.17 MiB guarded process-tree
peak RSS. Inclusive Python pass observations were CSE 24.600 s (including its
23.061 s solver), SCCP solver 22.992 s (also nested in other stages), edge
threading 11.065 s, prune 5.809 s, DCE 1.431 s, post-CSE DCE 1.283 s and verifier
0.840 s. These times overlap; summing them is invalid. Compiler-process sampled
RSS and guarded process-tree RSS have different scopes. This is VERIFIED
historical evidence at the report's recorded producer bytes, not a measurement
of the current default build.

Current-source inspection is INFERRED until measured on the merged source:
`_run_ir_midend_passes` bypasses stdlib bodies before coalescing or optimization.
Other bodies enter the fixed-point driver. Module entries normally select A,
ordinary functions B and bodies of at least 1,800 ops C; explicit overrides and
PGO promotion can change those choices. All profile/tier defaults enable CSE
and deep edge threading. Required SCCP/CFG/DCE work drains by default; nominal
round counts calibrate optional CSE's deterministic work budget. Exhausting that
budget disables CSE. Explicit diagnostic caps and verifier rejection/fallback
are separate loss mechanisms. A tier label alone therefore cannot establish
which passes actually executed.

Reuse the landed Wegman-Zadeck SCCP and its copy/publication and nullness facts,
the existing scoped GVN, memory GVN, effect/ownership authorities, analysis
invalidation and pass timing. Prior design and review: the integrator's
`logs/agents/hf773-sccp/design.md`, revision 2, and its slice-2 consumer/precision
inventory. Its proposal to leave Python edge-thread consumers with empty or
structural facts is superseded here: generic optimizations are retired with
their consumers; only demonstrated wire-format obligations remain in Python.
[LLVM SCCP](https://llvm.org/docs/Passes.html#sccp-sparse-conditional-constant-propagation)
combines value propagation and dead-edge discovery; its
[GVN](https://llvm.org/docs/Passes.html#gvn-global-value-numbering) removes
redundant computations. These support reusing the existing TIR owners; they do
not prove parity for Molt's Python effects, exceptions or ownership.

Alternatives and their concrete costs:

- Keep Python SCCP as CSE's seed: retains duplicate fact ownership and nested
  solving. Rejected even if a small sample's compile time looks acceptable.
- Port the Python object-graph pipeline verbatim: preserves name-keyed maps,
  repeated CFG walks and a second pass manager. Rejected; extend existing TIR
  passes only for measured gaps.
- Delete the entire frontend mid-end immediately: can discard valuable heap-
  read CSE or wire PHI/exception/region normalization. Rejected before A/B and
  ingress proof. A narrow first deletion remains admissible only with complete
  consumer retirement and evidence for its own fact family.
- Emit TIR directly or migrate SimpleIR/backend lowering in this slice: a wider
  ingress/ABI migration with reserved-path owners. Deferred to architecture
  convergence; do not solve an exposed backend conflict by opening another lane.

### Mechanism and global invariants

1. **One fact owner.** SCCP owns immutable constants and executable-edge value
   facts; GVN owns pure computation identity; memory GVN/effects own reusable
   heap observations. A measured missing fact extends its existing TIR owner,
   never a Python compatibility solver. No cache, environment knob or second
   optimizer is added. Registered op/effect rules remain generated from
   `runtime/molt-ir/src/tir/op_kinds.toml`.
2. **Semantics before deletion.** Preserve exception order, callbacks, missing
   values, arbitrary-precision arithmetic, singleton/object identity, ownership,
   mutation and resume edges. An annotation is not an exact-type proof. A read
   separated from another by a possible writer is not reusable without the
   existing memory-state proof. Retain distinct semantics at sibling sites.
3. **Structural ingress remains explicit.** Frontend lowering must still emit
   admissible wire regions, labels, PHIs, exception routing and suspended-value
   transport. A structurally required rewrite stays at its current owner until
   the ingress changes. No branch/value fact is fabricated to satisfy a retired
   API. Verification needed by retained lowering remains; verification serving
   only a deleted speculative rewrite is removed with that rewrite.
4. **Typed facts and existing boundaries.** TIR facts use `ValueId`, `BlockId`,
   `OpCode`, `TerminatorEdge` and existing closed effect/lattice types. New closed
   roles belong at their existing generating authority. Python `MoltOp` keeps
   its current typed frontend kind at the wire boundary. No new string-keyed
   facts, untyped dictionaries, `Any`, validators-as-types or spelling-based
   lowering are introduced.
5. **Indivisible retirement.** Delete the displaced algorithms, their private
   helpers/state, policy choices, caps, counters, environment registrations,
   cache-key fields, worker transport and implementation-pinning tests together.
   Preserve unrelated language semantics tests and build diagnostics. Generated
   protocols/registries are regenerated from their sources. Retained diagnostics
   describe actual lowering or TIR work; Python pass absence cannot be reported
   as a full execution of that pass.
6. **Evidence has an identity.** Record source/patch, toolchain, environment,
   workload/input, backend/profile, artifact and counter scope for every A/B.
   An old D0 or HF-572 receipt establishes only its recorded cell. A missing
   counter or failed run is unknown/failure, never zero cost or preserved speed.

The skeleton binds existing owners and signatures; placeholder bodies below
are specification only. No executable stub is added. The Python entry retains
only obligations admitted by invariant 3; it does not call the retired CSE or
SCCP pipeline. Specific TIR transfer-function changes require a measured missing
fact and a settled semantic contract, so none is invented in this skeleton.

```python
# src/molt/frontend/lowering/midend_pipeline.py
class MidendPipelineMixin(GeneratorMixinBase):
    def _run_ir_midend_passes(self, ops: list[MoltOp]) -> list[MoltOp]:
        """Normalize frontend wire obligations, without optimizing SSA facts.

        Requires: language lowering has emitted typed MoltOp kinds (I-3, I-4).
        Ensures: retained region/PHI/exception/resume ingress obligations hold;
        constants, CSE and executable-edge facts have only TIR owners (I-1,
        I-2, I-3). No policy tier or budget selects an optimizer here (I-5).
        Raises: the existing lowering diagnostic on malformed structural input;
        no speculative optimizer rollback hides a lowering defect (I-3).
        """
        ...
```

```rust
// Existing public entry signatures, in sccp/mod.rs, gvn/engine.rs, dce/engine.rs.
// Each is shown inside its existing module; no replacement module is created.

/// Requires: valid typed SSA, full exception edges and metadata (I-2, I-4).
/// Ensures: immutable value/edge facts only; missing values, loop carriers,
/// effects and observable identity remain sound (I-1, I-2).
/// Panics: only on an existing violated IR invariant, never as feature detection.
pub fn run(func: &mut TirFunction) -> PassStats { todo!() } // SCCP

/// Requires: analyses belong to this function's current CFG (I-2, I-4).
/// Ensures: replacements dominate uses and preserve ownership/effects;
/// heap observations use the existing memory authority (I-1, I-2).
/// Panics: only on an existing violated IR invariant.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    todo!()
} // GVN

/// Requires: valid use-def, exception and ownership metadata (I-2, I-4).
/// Ensures: removes only unobservable dead work; preserves metadata roots
/// and fallible operations whose exceptions remain observable (I-1, I-2).
/// Panics: only on an existing violated IR invariant.
pub fn run(func: &mut TirFunction) -> PassStats { todo!() } // DCE
```

Closed facts in the skeleton are the existing Rust IDs/enums and Python kinds;
`list[MoltOp]` is an ordered transport sequence, not a fact dictionary. No new
string/`dict`/`Any` fact carrier is proposed. Existing legacy name maps disappear
with their solver; remaining wire names are an ingress limitation, not a second
source of optimization facts. Existing generators and consumer gates check
projections and stale contracts; source review must additionally establish
single ownership, which behavioral output tests alone cannot prove.

### Representation, cost and admission sequence

No TIR pass is changed by this proposal commit. The following inventory is
INFERRED from source, with no fresh largest-program cost claim. For every pass
actually changed, record its measured elapsed time, allocations/RSS limitations,
and largest program/function dimensions before and after. Reuse `PassStats.elapsed`
and existing pass-delta facilities rather than adding a profiler.

| Existing owner | Representation and algorithm | Cost obligation |
| --- | --- | --- |
| SCCP | Typed IDs in hash maps/sets; edge and SSA-value vector worklists; incremental meet | Work follows changed lattice uses and executable edges; bounded lattice height alone does not bound operand/constant evaluation or guard queries. Measure those costs. |
| GVN | Scoped hash tables and undo vectors; iterative dominator-tree traversal | Expected linear table work in visited operations/operands, plus key construction, dominance and exact-type analysis; no worst-case linear claim. |
| DCE | Hash-map use counts, per-block vectors; reverse scan, at most ten global rounds | Up to ten whole-function use/operation scans plus metadata reachability; a round cap is not proof of a complete fixed point. |
| Copy propagation | Typed-ID hash map/set; at most twenty chain-flattening rounds | Repeated map scans plus operand rewriting; measure chain and largest-program behavior if changed. |
| Canonicalization | Typed-ID constant/source maps; function scans | Account for operand/key/type-analysis cost, not just operation count. |
| Block-argument pruning | Hash-set liveness, ordered dead-position map; repeated edge scans to stability | Cost scales with propagation depth times edge-payload visits; preserve terminal ownership-phase placement. |
| Exception-check elimination | Typed-ID maps/sets; whole-CFG pending-state scans to stability | Include map reconstruction and iteration count; preserve exception-target entry state. |

1. After review, preflight the actual child toolchain, selected interpreter,
   target runtimes, counters and source/artifact identity before expensive work.
   Measure unchanged default `loop`, `bench/bench_chainmap.py` and
   `examples/hello.py` builds, recording the exact loop source from retained
   evidence (do not silently substitute a similarly named workload). Use fresh
   per-program artifact/cache roots under lane scratch. Record cold product
   state separately from warm compiler/toolchain and OS caches. Use
   `MOLT_MIDEND_STATS=1` and `MOLT_BUILD_DIAGNOSTICS_FILE` for profiled runs;
   report observer overhead separately from uninstrumented default wall/RSS.
   Report actual per-function coverage, passes, tier/budget/degrade/fallback
   decisions and exclusive outer mid-end time, without summing nested solvers.
2. Keep an exact scratch-only disabling patch and its digest. Disable the
   coupled Python SCCP/CSE/edge-thread transformations, including the CSE solver's
   nested SCCP and facts used by prune; retain structural normalization and
   unrelated lowering. Add no flag/environment variable. The patch and resulting
   ingress must be audited before comparing runtimes. Use an authorized scratch
   source snapshot and existing build custody, not a second integration lane.
3. Compare the same CPython-running `bench/` workloads in alternating A/B pairs,
   preserving source, inputs and backend/profile. Measure program instructions
   and runtime calls per iteration where a hot path is implicated. On macOS,
   `/usr/bin/time -l` counters cover the measured process; multiprocessing needs
   a demonstrated whole-workload scope. Record exact outputs/exit and applicable
   stderr policy against the independently identified CPython oracle before
   admitting a pair. Use the existing performance runners and methodology;
   scratch diagnostics do not publish authoritative release performance.
4. Run owning differentials with the pipeline disabled on native, LLVM and WASM.
   For each instruction regression, use existing TIR dumps/pass deltas to name
   the lost fact, op kinds, effect premise and canonical Rust owner. Include
   bigint, singleton/heap identity, MISSING, type/truth predicates, heap reads,
   loops, exception joins and suspension siblings in this analysis. Do not assume
   pure GVN subsumes Python heap-read CSE. A fact requiring a reserved path or
   unsettled semantic decision returns to the integrator with measured evidence.
5. Close admitted gaps in existing Rust passes with CPython differential
   programs, then delete the displaced Python family and every consumer in
   one change. Compare final deletion against the same base; a disabling patch
   is not the final implementation's performance receipt. Classify all remaining
   frontend helpers as required lowering or optimization awaiting retirement,
   with specific producer/consumer evidence. Do not move SimpleIR-required work.

Before implementation, `_align_phi_args_to_cfg_predecessors`,
`_ensure_structural_cfg_validity`, `_canonicalize_structured_regions_pre_sccp`,
`_normalize_try_except_join_labels`, label cleanup and exception-check coalescing
are candidates for retained ingress obligations, not a blanket retention list.
Their current composition in `_canonicalize_cfg_before_optimization` mixes
normalization and optimizations; prove each retained action. Generic branch
pruning, tail merging, unreachable-region removal, loop threading, value
numbering and trivial-constant DCE belong to the retirement inventory. Audit
all callers before deleting definite-assignment verification or unique-result
analysis: their existence in a mid-end module does not establish sole ownership.

### GoldfishContext: consumer and proof closure

The review must challenge the following concrete authorities and their real
readers, rather than treating deletion of a pipeline call as complete migration.
Paths below extend the starting aperture where necessary; reserved descendants
remain excluded. No new registry, runner or proof cell is proposed.

- Algorithms/assembly: all seven `src/molt/frontend/lowering/midend_*.py`
  modules; `src/molt/frontend/__init__.py`, `_types.py`, `cfg_analysis.py`,
  `value_liveness.py`, and `lowering/serialization.py`. Remove SCCPResult,
  CanonicalizationState and sentinel/helper families only with their last reader.
- TIR owners: `runtime/molt-passes/src/tir/passes/{sccp,gvn,dce,
  block_arg_prune,check_exception_elim}`, and its `mem_gvn.rs`, `copy_prop.rs`,
  `canonicalize.rs`, `effects.rs` and `value_identity.rs`;
  `runtime/molt-passes/src/tir/{type_refine.rs,pass_manager.rs}`.
  `runtime/molt-ir/src/tir/op_kinds.toml` and `tools/gen_op_kinds.py` own any
  measured new opcode property; do not edit reserved runtime-function outputs.
- Protocol sibling/import lists: `tools/gen_protocol.py` and
  `tools/generator_manifest.toml`; regenerate frontend `_protocol.py` and
  `_protocol_attrs.py`. Registry: `src/molt/environment_registry.toml`,
  `tools/gen_environment_registry.py`, generated `src/molt/_environment_registry.py`
  and `docs/environment-variables.generated.md`. The same generator also owns
  `src/molt/guard_stems_generated.py`,
  `runtime/molt-host-process/src/host_process_environment_generated.rs` and
  reserved `runtime/molt-runtime/src/runtime_flags_generated.rs`; inspect its
  output and return any required reserved-path change to the integrator.
  Remove all retired MIDEND/SCCP/CSE
  keys with their readers; `MOLT_MIDEND_STATS` survives only if retained work
  still consumes it meaningfully. No new environment variable is proposed.
- Cache/fingerprint and capture/closure transport: `src/molt/cli/module_cache.py`
  (`midend_policy` key and persisted coverage), `cache_fingerprints.py`,
  `build_inputs.py`, `models.py`, `frontend_worker.py`, `frontend_pipeline.py`,
  `frontend_execution.py`, `backend_output_pipeline.py`, `build_diagnostics.py`.
  Preserve semantic compiler/source/target identity and reject stale products;
  do not accept fabricated empty optimizer outcomes as old full coverage.
  Inspect `_FRONTEND_COMPILER_INPUTS` roles and source-closure discovery in
  `cache_fingerprints.py` when deleting a module; do not retain a dead producer
  row or classify an optimization producer as reporting to avoid invalidation.
- Hand-written proof lists and projections: `tools/proof_plan.toml` authority
  inputs, frontend path globs/test lists and `tir-midend` cell;
  `tools/gen_proof_plan.py` and its manifest outputs if those inputs change;
  `tools/generator_manifest.toml` source lists and
  `tools/generator_manifest_baseline.json` if an owned generator retires.
- Direct tests/fakes: `tests/test_frontend_midend_passes.py`,
  `test_frontend_literal_identity.py`, `test_frontend_package_composition.py`,
  `test_opcode_literal_payloads.py`, `determinism/test_ir_determinism.py`,
  `model_based/test_model_smoke.py`; under `tests/cli/`,
  `test_cli_module_frontend_shared_cache.py`,
  `test_cli_build_diagnostics_authority.py`,
  `test_cli_frontend_execution_authority.py`,
  `test_cli_frontend_integration_authority.py`,
  `test_cli_frontend_scc_scheduling.py`,
  `test_cli_backend_output_pipeline_authority.py`,
  `test_cli_binary_image_closure.py`, `test_cli_import_collection.py`,
  `test_cli_smoke.py`, `test_cli_runtime_callable_symbols_authority.py` and
  `test_native_object_publication.py`.
  Their fake generators, worker results and persisted cache records must use
  the real retained boundary. Preserve language/identity/resume tests; retire
  assertions whose only contract is Python CSE policy or optimized frontend shape.
- Existing tools/models: `tools/frontend_hot_pass_profile.py`, `bench_wasm.py`,
  `translation_validator.py`, `quint_trace_to_tests.py`, `check_formal_methods.py`,
  `formal/quint/molt_midend_pipeline.qnt`, `molt_optimization_pipeline.qnt`,
  `tests/test_translation_validator_unit.py`, `test_check_formal_methods_tool.py`
  and `tests/tools/test_formal_correspondence_truth.py`. Retire obsolete Python
  pipeline correspondence and last-consumer paths; preserve models/validators
  still proving the Rust fact family. Generic pass names alone are not obsolete.
- Public contracts: this document, `docs/OPERATIONS.md`, `BENCHMARKING.md`,
  `docs/spec/areas/compiler/0100_MOLT_IR.md`, the frontend architecture designs
  and architecture convergence section B. Inspect
  `config/public_contract_v1.surface.json` for affected symbol references.
  The HF-773 row in `docs/agent/V1_HANDOFF_FINDINGS.md` alone owns progress.

Use existing consumer gates, generator write/check and changed-reader pytest
selection (at most four workers). Test plausible defects with independent
language programs: constants across feasible/infeasible joins and loops;
overloaded arithmetic versus exact scalars; mutation/callback invalidation;
exceptions from apparently dead values; missing/unbound reads; PHI transport;
generators/coroutines resuming through exception regions; literal identity.
Run the owning programs through `tests/molt_diff.py` for native, LLVM and WASM;
backend/profile/target prerequisites must be observed before admission.

For changed Rust crates, the existing changed-crate gate owns whole-library
tests and reverse dependencies in declared feature/profile cells, plus selected
Clippy/build checks. Use the plain landing environment, not sourced lane
selectors, for changed tests and `molt_dev.py land --precheck-only`. Run that
handoff gate once on the final implementation, and repeat only for a changed
input, new failure or uncovered obligation. The design-only review handoff is
not a precheck-green implementation claim.

### Risks, limits and completion

The principal risks are hidden wire normalization, Python facts not yet modeled
in TIR, heap/effect unsoundness, stale cached products, altered diagnostics and
loss of verification around retained lowering. Invariants 1–6 and the consumer
closure above address these at their owners. Full default and historical
full-coverage comparisons answer different questions; neither substitutes for
the other. Largest-program TIR cost, default wall/RSS share and instruction
parity remain unmeasured in this proposal. No security boundary, supported
dynamic-execution policy or host-interpreter dependency is expanded.

The post-review checkpoint consists of preserved D0, completed default and
scratch-disable measurements, and the first coherent deletion those results
justify, with complete consumers and a green landing precheck. Full retirement
of SCCP/CSE/edge-thread and after/base qualification is the assigned end state;
a partial checkpoint must identify remaining families without claiming that end
state. Reserved-path conflicts or a measured fact needing an unsettled semantic
decision return to the integrator. The required design review is a separate
handoff before implementation; its settled findings arrive in the live brief.

## What "better than each" technically requires
- **vs Nuitka** (full CPython compat, but slow — inherits CPython's runtime/object model via
  libpython): molt keeps the compatibility but has its OWN runtime, so the win comes from an
  optimizer that escapes the object model — **SROA** removes the per-operation allocation, the
  **inliner** removes the call barrier. Tiny standalone binaries, no libpython.
- **vs Codon** (fast AOT Python, generator inlining, but a restricted dialect — weak C-extension /
  package ecosystem, less dynamism): match its codegen + generator elision while keeping FULL Python
  semantics + ecosystem reach. Same engine (inline + coro-elide + specialization), fewer compromises.
- **vs Julia** (world-class numerical speed via specialization + inlining + escape analysis feeding
  LLVM, but JIT warmup + its own language): molt's typed-IR/`Repr` work is the specialization half;
  the **inliner + SROA** are the other half — delivered AOT (no warmup) over the Python ecosystem.
- **vs Mojo** (MLIR progressive lowering, ownership, SIMD/GPU, zero-cost abstractions, but NOT real
  Python): molt already has progressive lowering (TIR→SimpleIR→Cranelift/LLVM/wasm/luau), `vectorize`/
  `polyhedral`, and the tinygrad GPU lane — on REAL Python. SROA supplies the zero-cost abstractions.

All four converge on one root cause and one fix.

## The structural gap (measured 2026-06-03)
- `run_pipeline(func: &mut TirFunction)` is **strictly per-function** (passes/mod.rs:72; called
  per-function at main.rs:2199, wasm.rs:2116, parallel.rs:28). There is **no whole-program pass
  phase** — molt has inter-procedural *summaries* (`compute_return_alias_summaries`) but no body-level
  inter-procedural access. **Every call is an optimization barrier.**
- molt has **no function inliner** and **no mem2reg/SROA** (pass list passes/mod.rs:156-229).
- Consequence: Python's call-heavy, allocation-per-operation style can't be optimized across
  boundaries → the project is pushed toward hand-written native objects per stdlib function (the
  CPython C-extension treadmill) — the exact thing this foundation exists to make unnecessary.

The inliner + SROA/mem2reg are, in LLVM and Julia, THE load-bearing passes: they're what make every
other pass (GVN, LICM, SCCP, escape analysis, the `Repr` specialization) pay off across call and
allocation boundaries. molt has the downstream passes already; it's missing the two that feed them.

## The foundation (build order — each a complete, perf-gated, differential-tested piece)
1. **Inter-procedural pass phase + call graph.** A whole-program step before the per-function
   pipelines, with access to all `TirFunction`s + a call graph (call site → callee). Reuse the
   existing summary precedent. This is the architectural unlock; the inliner and all future IPO
   live here. (Driver: main.rs ~2199 + wasm.rs ~2116 collect funcs; add the phase before the loop.)
2. **Function inliner** — the bridge from inter- to intra-procedural. CONSERVATIVE-CORRECT first cut:
   inline only small, non-recursive, directly-called leaf functions with simple CFGs; profitability +
   size-growth gated; recursion-cycle-safe; identical semantics (params→args, return threading, value/
   block-id remapping, refcount/exception correctness). Then EXPAND by measurement. All backends. This
   alone is likely molt's single biggest perf lever (kills call overhead + unlocks cross-call opt).
3. **mem2reg / SROA.** Promote non-escaping memory + aggregates (stack slots, non-escaping heap
   objects, tuples/small structs, coroutine frames) to SSA values. Drives the allocation tax to zero.
   **Coroutine-frame elision lives here** (generator frame slots → loop-carried phis; SCCP folds the
   resume `STATE_SWITCH`). Built on the existing `escape_analysis` (extend it to `AllocTask`/`Alloc`).
4. **Specialization / monomorphization.** Deepen the typed-IR/`Repr` convergence with inlining so
   call sites specialize on argument repr (Julia-class type-stable native code), AOT.

## Consequences (fall out of the foundation — not separately engineered)
- **Generator fusion** = inline the generator `poll` (pass 2) + elide the frame (pass 3) = exactly
  LLVM's `CoroElide`. The earlier bespoke `generator_fusion` devirt pass is RETIRED in favor of this
  (it would have reimplemented half an inliner anyway). os.walk-as-Python is the *proving ground*.
- **Stdlib stays Python.** Stdlib bodies inline into user code + SROA the allocations → no native
  iterators. Retire the bespoke itertools/os.walk natives → Python.
- **os.walk** rewritten as the CPython-verbatim Python generator (iterative work-stack; top-down +
  bottom-up; in-place `dirnames` pruning + `onerror` + `followlinks`), fast via fusion — fixing its
  eager-OOM + deep-tree-SIGSEGV in Python, once passes 2+3 prove ≥ CPython on the walk benchmark.

## Engineering discipline (bleeding edge ≠ reckless)
The best inliners/SROA (LLVM, Julia) were built as conservative-correct first cuts that EXPAND by
measurement and NEVER miscompile. Hold that bar: absence of an optimization is a perf bail, never a
correctness change; differential-test every shape against CPython; the perf contract (≥ CPython on
every benchmark × target × profile) gates each landing. No half-passes on main — each phase lands
complete or leaves a clean baton.

## Risk / honesty
Passes 1–3 are genuinely multi-week, high-correctness-risk foundational compiler work (the inliner's
remap + the frame-SROA's SSA reconstruction are the hard parts). This is the correct investment: it's
the engine the entire 5-year vision runs on, and it subsumes the per-function-workaround approach.
Until pass 3 lands, os.walk's eager-OOM + deep-tree-SIGSEGV remain open (accepted; no native stopgap).
