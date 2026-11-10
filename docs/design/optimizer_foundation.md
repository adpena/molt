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

Status: settled round-two design, 2026-11-10. The integrator's 07:05Z Correction
authorizes implementation in steps (a)–(d). The settled decisions immediately
below override conflicting proposals in the recorded measurement discussion.
The earlier 01:00Z Correction
supersedes the first proposal's structural-retention list. Step (a) is complete;
step (c) implements heap-literal reuse, with the qualification limits below. The findings ledger owns qualification; Claude
integrator owns review, integration and retirement. The assigned worker owns
the measurements and proofs. D0 remains diagnostic-only; guarded-call admission
and the inliner's execution-context gate remain HF-824's work.

### Settled implementation decisions (07:05Z)

Implement in order, with one commit per step and a stop on the first failed
oracle: (a) compare existing module-slot promotion diagnostics for A/on and
B/raw on the nine native and thirteen LLVM above-envelope programs; capture
LLVM A/B loop anatomy for bench_sum and bench_dict, including runtime calls,
reference-count sites and literal materializations. (b) Fix additional B
refusals at promotion's compensation owner or by ordering check_exception_elim
relative to promotion in build_default_pipeline. Do not extend string-keyed slot
matching. Prove no on-arm regression above A/A envelopes across the corpus;
report raw-arm closure or shrinkage per program and backend. (c) Only residual
above-envelope cells authorize heap-literal GVN. (d) Delete the Python boundary
and all consumers in one commit only when no unaccepted above-envelope cell
remains; stop at green precheck. Claude owns integration and retirement.

GVN owns literal identity and its existing `Replacements`
(`passes/ownership_lattice_min/replacement.rs`) owns ownership:
ConstStr, ConstBytes and ConstBigInt may enter the existing dominator-scoped
table; a replacement stays transparent only when both literals are unbound.
Terminal drop insertion releases at the alias class's last use. None and small
integers are immediate words. The existing `bound_local` string-keyed attribute
is debt with a typed home outside this slice; add no new string-keyed attribute.
There is no shared lifetime oracle and no new refcount_elim mechanism. Rust's
op_kinds.toml literal roles and gvn/keys.rs are the authority; delete the Python
literal_identity.py only with its last reader: live namespace/static-truth
analysis also reads it, so its independent language-lowering duty must survive
mid-end retirement. Rust does not depend on it. Heap-read aliasing and bigint folding
have existing owners but no demonstrated buyer here; they are not prerequisites.
Future bigint folding belongs in an arbitrary-precision SCCP ConstVal variant.
Backend literal materialization is HF-507's reserved work; native anatomy does
not explain LLVM losses. A cell whose A/A envelope exceeds its delta is unknown,
never a performance pass.

Findings 7–9 complete the consumer inventory:

- Keep FRONTEND_EFFECT_PURE_KINDS and FRONTEND_EFFECT_CONTROL_KINDS and their
  effect authority / [[frontend_effect_kind]] rows for traceback correctness in
  lowering/frame_locations.py. Migrate FRONTEND_EFFECT_CLASS readers in
  tools/check_correspondence.py and tests/test_lean_python_correspondence.py /
  test_lean_rust_correspondence.py; the effect map cannot disappear with CSE.
- Also retire optimizer-only FRONTEND_OPERATOR_OPCODE, frontend_operator_facts,
  _FRONTEND_OPERATOR_EFFECTS, frontend_predicate_facts,
  COMPARISON_SCALAR_DOMAINS, FRONTEND_COMMUTATIVE_DOMAINS,
  frontend_can_reorder_commutative and FRONTEND_INTRINSIC_SCALAR_ARITIES.
  Include tools/op_kinds/primitive_effects.py in GoldfishContext. Heap READS/WRITES
  kind sets have test readers only. Regenerate frontend Python and TIR Rust
  op_kinds_generated through tools/gen_op_kinds.py; preserve every Rust-consumed
  row, including predicate semantics read by value_identity.rs.
- Include src/molt/cli/entrypoint_parser.py stage choices, debug_helpers.py,
  config/public_contract_v1.surface.json, tests/translation_validation/README.md;
  _new_tracked_ops(count_function=True) callers in module_lifecycle.py and
  function_lifecycle.py; tools/agent_coordination.py and
  tests/test_agent_coordination.py's frontend proof selector;
  formal/lean/MoltTIR/Passes/{JoinCanon,EdgeThread}.lean;
  tools/bench_wasm.py's retired knobs and
  tests/test_bench_wasm_runtime_build_flags.py; docs/spec/STATUS.md,
  foundation/56_dx_buildspeed_tooling.md, dx_doctrine_100yr.md,
  cli-reference.md and 0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md.

Findings 10–11 correct the mechanism: SCCP refuses a fold when ANY outgoing edge
targets a loop header, not merely the selected edge. type_guard_hoist,
refcount_elim and dead_store_elim also precede SCCP. module_phase.rs reruns the
whole pipeline on promoted functions. Baseline GVN never replaces a constant
with Copy, even in one block; step (c) admits only generated heap code literals. Correct owner paths are
passes/alias_analysis/mod.rs and tir/type_refine.rs. The serializer skeleton's
bases must match its six existing serialization mixins. Ordering is not a
prerequisite without a measured buyer. Keep all earlier numerical observations
bound to their recorded source; they do not prove these proposed fixes.

### Step (a) result and step (b) disposition

VERIFIED at the settled design's source, with no compiler changes: the nine
native and thirteen LLVM above-envelope programs were built in both arms.
All 44 builds completed with closed guards. Existing promotion counters report
zero promoted slots in every observed module; the refusal logs show no new B
refusal. Their user module chunks hit the same function-wide ModuleGetGlobal
wildcard gate. That gate is stage-3 work outside this slice. The missing
compensation hypothesis is not supported by these cells. Step (b) requires no
change; do not broaden the string-keyed gate.

VERIFIED LLVM loop anatomy, with text sections identical to all four measured
release products: bench_sum A/B have 337/433 static loop instructions, 2/6
string materialization calls and 11/14 release slow-path call sites. bench_dict's
first loop has 259/360 instructions, 1/5 materialization calls and 8/12 release
sites; its second has 366/469 instructions, 2/6 materializations and 12/15
release sites. LLVM inlines RC fast paths with runtime slow calls; these are
static sites, including conditional paths, not dynamic per-iteration totals.
Full per-module refusal/counter records and loop call inventories stay in the
private report. They authorize the settled step (c), not a backend edit or a
claim that literal construction alone explains the whole instruction delta.

### Step (c) implementation and measured limits

GVN now uses the existing generated owned-literal classifier to admit ConstStr,
ConstBytes and ConstBigInt to its scoped table. Immediate constants retain
block-local numbering and backend representation. Replacements collects a
HashSet<ValueId> of original unbound heap literal results only when GVN has
replacements, then leaves two such literals transparent. Either bound producer
keeps the ordinary independent-owner rule. Other passes retain their existing
record/finish path and pay no new full-function literal scan. No new string-keyed
attribute, registry, lifetime oracle, pass ordering or backend path is added.

INFERRED representation/cost: the existing scoped HashMaps and iterative
undo-based dominator walk remain. The added ownership-admission scan is O(ops)
with expected O(literals) HashSet work/storage; payload hashing, key allocation,
representation, dominator and alias-analysis costs remain explicit. There is no
new fixpoint or cache. VERIFIED optimized replay on the same largest raw Regex
Redux input: 2,366 functions / 489,412 input operations; 29,306 GVN replacements,
0.529530 s GVN, 5.690126 s summed ordinary pipeline intervals and 412,797 output
ops. Child wall 15.60 s, guarded tree peak 1,619.55 MiB. Baseline ordinary GVN
was 0.266632 s with no replacements and normal pipeline 5.652728 s / 466,720
output ops. These are single observations under host load, not a latency
improvement, memory attribution or a statistical comparison. New per-table
allocation and stack attribution remain unmeasured.

VERIFIED: 19 selected GVN lib tests pass in dev-fast/default features, including
real terminal-drop ownership and exception-only/sibling scope boundaries;
these are not whole-crate/reverse-dependency qualification. The six retained
fixtures plus midend_literal_materialization_identity pass explicitly on native,
LLVM and WASM (21 backend observations), with a closed guard. All 246 opcode
generator tests pass and both opcode projections are current.

Historical first continuation: the integrator's 08:51Z Correction accepts beneficial changes beyond the
A/A envelope: neutrality means no above-envelope regression. Nine current
native cells were completed with five alternating baseline/on/raw triples and
exact CPython stdout/exit checks. Eight have no above-envelope regression;
bench_while is the first failure: new-on/base-on -1.458010%, raw/on +0.161167%,
raw/base-on -1.315332%, against its unchanged 0.011276% envelope. Both new arms
improve on baseline, but this does not waive the separate raw/on obligation.
The batch stopped at that cell. The other thirteen native cells and current
LLVM/WASM instruction cells remain unqualified; no deletion or implementation
precheck was run.

Historical lost fact, corrected below: the raw loop retains a pending-exception observation at its
header before the first ModuleGetGlobal; on eliminates it. Text-matched native
inspection has 289/293 loop instructions and 56/57 branches, with identical
runtime-call maps (four release sites; no retain/literal constructor sites).
The extra raw header is ldur/ldrb/tst/b.ne, reached by the backedge. Supplementary
normal-pipeline replay preserves the distinction: raw source-op 17 is a
resultless Copy origin=line before the loop and source-op 20 remains a header
CheckException; on has an outside-loop check and removes the header check.
The effects oracle conservatively treats non-value-forwarding Copy as impure,
so the non-raising LINE fact used by Python coalescing is not available to
check_exception_elim. INFERRED causal owner is canonical metadata effects and
check_exception_elim/ordering; no product counterfactual or fix is qualified.
Asynchronous-work polling and traceback/exception custody must remain intact.
The 09:24Z disposition requires correcting this loss before deletion. It admits
only a generated nothrow Copy-kind fact, preserving the impure effect-free and
consistent floors and the existing independent callback projection.

VERIFIED runtime premise: molt_dec_ref_obj enters dec_ref_ptr_validated
(runtime/molt-runtime/src/object/ops.rs:1394), whose rc-zero path calls the
finalizer at object/mod.rs:4426. Ordinary __del__ binding/calling runs inside
run_unraisable_with_policy (object/mod.rs:3749). The transaction reports the
captured error, discards current raised state (builtins/exceptions/unraisable.rs:
211), then restores the original raised/handled state at :215; take_raised
clears the pending flag (raised_state.rs:45). Generator and async-generator
finalizers use the same transaction (async_rt/generators.rs:982,1161).
This prevents a new finalizer error escaping; an incoming exception is preserved.
Cranelift's native tracking mode may release at Line outside loops, so Line is
not pure. Its terminal TIR mode drains no native cleanup candidates
(native_backend/simple_backend/cleanup.rs:80), and both native finalization
modes run terminal drops before codegen (program_pipeline.rs:108-136). LLVM
and WASM Line handlers emit no work. The fact does not authorize deleting or
moving Line markers, weakening asynchronous polls or modifying reserved paths.

The generated nothrow Copy-kind projection is now read by the existing effects
oracle. Five effects tests, all 24 check_exception_elim tests and 249 opcode
generator tests pass in the plain development environment. These include the
loop-header obligation, preservation of the marker and impure floors, and
rejection of invalid or duplicate table entries. They do not qualify the
remaining instruction corpus or the final landing matrix.

The 09:24Z nothrow implementation is measured at `9c8a163c40`. Native
bench_while now has on/base -1.635583%, raw/on +0.006651% and raw/base
-1.627254%, within its unchanged 0.011276% raw/on envelope. All 22 native
rows completed without an above-envelope regression. LLVM bench_calls has
on/base -5.353879% and raw/on +0.001416%, against a 0.596212% envelope.

The batch stops at LLVM bench_chainmap: on/base +1.568592% exceeds its
0.072614% envelope. Raw/on -0.044060% is within the envelope; raw/base is
+1.288231%. All five on/base pairs regress beyond that envelope; all fifteen
executions match CPython stdout/exit and close their guards. This fails the
separate on-arm neutrality obligation. Twenty LLVM and 21 available WASM
instruction cells remain unmeasured, with baseline WASM Regex Redux still
unavailable. High-noise cells remain unknown for small performance differences.

VERIFIED localization: baseline/current LLVM on-arm frontend inputs are
byte-identical (SHA-256 15ad0baaa10c0b55facbd79d9538e416bcd0730457765a59923b88e3addc831b)
and use the same immutable runtime generation. The failure is downstream of
Python lowering. Supplementary actual-backend fact graphs are byte-identical
for ChainMap.__getitem__ and get; the caller and __contains__ have fewer
string producers and release sites. Retained archive-candidate anatomy shows
850/692 static caller-loop instructions and six/one string materialization
sites, while full-caller stack-memory sites rise 241/267. Linked-text equality
and a causal dynamic attribution are not established. Extended literal lifetime
or register placement is a hypothesis, not a named proven lost fact. The report
explicitly leaves the cause unlocalized rather than inventing one.

No deletion or precheck runs after this failed oracle. An unapplied deletion
draft based on `9c8a163c40` is preserved privately; its rendered projections and
Ruff checks do not qualify its migrated readers or final wire boundary. Claude
integrator owns the regression disposition, integration and retirement. The
typed nothrow fix does not waive the separate LLVM regression.

VERIFIED native bench_calls inspection products have text identical to both
new measured arms. Their loops have 429/431 static instructions and identical
call-site maps (one inc-ref, six dec-ref sites per loop region), compared with
baseline 554/723 instructions. Conditional/cold paths are included; neither
static sites nor the instruction change prove dynamic calls per iteration.

### Problem and measured scope

Two implementations compute constants, expression identity and executable-edge
facts: the Python object-graph mid-end and the Rust TIR optimizer. The desired
architecture has one optimizer on typed SSA. Removing the Python implementation
must preserve behavior and compiled-program performance, rather than simply
remove its entry point and call a green producer sufficient evidence.

VERIFIED historical context: HF-572's full-coverage loop build took 113.653 s;
its inclusive CSE observation was 24.600 s, including nested solving. Those
observations describe their recorded source and coverage policy. They are not
current default costs and their nested pass times cannot be added.

VERIFIED current source and measurements at `204e3fe607`: default stdlib bodies
return before Python optimization. Actual cold-product default builds, with the
lowering cache off and the normal scheduler and daemon policies, selected
native/release, release host compiler and release-output runtime. The existing
suite owner enclosed the commands and drained their warm daemons. The first
per-command attempt did not own that lifetime correctly and is retained as an
unqualified harness attempt, not silently counted successful.

On hello, 2 of 95 serialized functions, containing 92 of 17,488 original ops,
entered Python optimization; on ChainMap, 2 of 1,348 functions, containing 401
of 221,637 ops, did. Both user pairs selected tiers A/B; no degradation,
promotion, fallback or structural canonicalization was observed. With the
memory profiler, their user mid-end calls totaled 0.036777 s / 25.49 s build
and 0.106599 s / 61.41 s, respectively. These are instrumented observations,
not an overhead estimate or latency distribution. Plain default observations
were 32.23 and 79.24 s. Separate serial timer-only observations were 0.013654
and 0.081606 s. Host load and first-launch stalls are uncontrolled.

Per-pass RSS is a sampled compiler-process level and transient lower bound;
guarded build RSS is the sampled descendant-tree peak. Neither is an additive
allocation measurement or a causal percentage of peak build memory. The
requested historical private loop input is unavailable at its retained path
(SHA-256 `42628e79e2f8425bf9cd75fb4df0aa4c6fb6e3961733b708e5ce0bcde2982820`);
its cell remains unmeasured. The public `bench_while` is a separate workload.

The scratch B arm applies the stdlib early return to **every** function, before
coalescing or structural repair. It also bypasses all nested SCCP consumers,
PHI/loop/prune rewrites, CSE solving, DCE and verifier rollback. It introduces
no product switch. Variant artifact/cache roots are isolated; lowering and
whole-program backend cache lookup are disabled. Attestations identify the
real imported checkout module and the scratch replacement method. The earlier
external-module loader was rejected by import custody and provides no semantic
result. Both failed attempts and their repairs remain in private evidence.

VERIFIED: raw lowering passed `bigint_ops`, `midend_phi_and_resume_semantics`,
`midend_resume_exception_transport`, `sccp_class_predicates`,
`sccp_join_constants` and `sccp_none_refinement` under `tests/molt_diff.py` on
native, LLVM and WASM: 18 explicit backend passes. This does not prove all
frontend shapes, traceback locations, literal identity or release cells.

Regex Redux cannot run on this Node/WASI host's descriptor-passing model:
both A and raw B build successfully, then fail with the same multiprocessing
IPC diagnostic. Its WASM instruction/performance cell is unqualified; this
is not a Python-retirement regression or a semantic pass. Independent
remaining cases continue after the failure is classified.

VERIFIED completed cells; raw/on median instruction delta / observed A/A envelope. Unavailable cells remain explicit. WASM counters include Node/engine; native/LLVM Regex Redux counters do not qualify the whole multiprocessing workload.

| Program | Cranelift | LLVM | WASM |
| --- | --- | --- | --- |
| bench_calls | +1.649% / 0.264% | +8.066% / 0.596% | +2.150% / 1.216% |
| bench_chainmap | -2.026% / 0.157% | +1.006% / 0.073% | +0.094% / 0.039% |
| bench_colorsys | +0.605% / 21.978% | +4.077% / 0.282% | +0.537% / 1.380% |
| bench_dict | +3.168% / 0.268% | +15.929% / 0.267% | +3.693% / 0.409% |
| bench_fib | +0.777% / 0.274% | +3.050% / 0.626% | -0.111% / 0.679% |
| bench_float | +2.798% / 0.518% | +14.036% / 0.232% | +2.960% / 0.897% |
| bench_list | +2.954% / 0.551% | +15.122% / 0.526% | +4.089% / 0.821% |
| bench_math_numeric | +1.286% / 0.228% | +7.455% / 0.248% | +1.404% / 0.652% |
| bench_method_defaults | +1.351% / 0.252% | +7.021% / 0.609% | +2.405% / 0.241% |
| bench_string | +1.269% / 2.708% | +7.845% / 2.301% | +0.550% / 2.227% |
| bench_sum | +3.114% / 0.078% | +14.879% / 0.058% | +4.373% / 0.130% |
| bench_while | +3.006% / 0.011% | +14.270% / 0.722% | +3.844% / 0.280% |
| binary_trees | +0.340% / 1.667% | +1.941% / 1.672% | +0.450% / 0.498% |
| fannkuch_redux | +0.126% / 4.749% | -0.001% / 4.803% | +0.081% / 0.200% |
| fasta | +0.488% / 0.860% | +0.295% / 1.055% | -0.134% / 1.571% |
| k_nucleotide | +0.038% / 0.849% | +0.238% / 1.060% | -0.054% / 1.675% |
| mandelbrot | -0.013% / 0.204% | -0.019% / 0.086% | -0.272% / 0.676% |
| nbody | +0.083% / 2.167% | -0.026% / 1.171% | +0.374% / 2.243% |
| pidigits | +0.741% / 12.734% | +0.712% / 13.383% | +0.276% / 0.491% |
| regex_redux | -0.288% / 9.334% | +0.278% / 7.440% | host model unavailable (A/B) |
| reverse_complement | -0.714% / 12.179% | -0.021% / 11.222% | -0.642% / 2.808% |
| spectral_norm | +0.019% / 1.086% | -0.021% / 0.565% | +0.190% / 1.632% |

### Mechanism and invariants

1. **One owner per displaced fact family.** SCCP owns constants and executable
   edges; GVN owns reusable pure computation and admitted constant identity;
   memory GVN/effect and ownership analyses own reusable heap observations.
   Extend these existing owners for measured gaps. Do not port the Python
   solver verbatim, call a separate Rust optimizer service, or add a cache,
   policy knob or string-keyed fact table. Registered rules remain generated
   from `runtime/molt-ir/src/tir/op_kinds.toml`.
2. **Behavior, identity and ownership bind reuse.** Preserve exception order,
   MISSING/unbound reads, callbacks, arbitrary-precision arithmetic, singleton
   and heap identity, releases and resume transport. Value equality alone does
   not prove that two fallible heap materializations may be merged. Reuse must
   satisfy Rust `op_kinds.toml` and `gvn/keys.rs` and the native optimization
   contract's immutable-literal identity rule, producer-derived exact facts,
   representation and ownership rules, and both exception-aware dominance and
   strict-CFG use reachability. The existing
   `foundation/20_rc-ownership-drop-insertion.md` contract owns reference
   boundaries. Use existing ownership replacements; never
   introduce an alias without the existing Replacements proof for two unbound
   heap literals. An annotation
   or a runtime tag hint that returns mismatched inputs is not an exact-type
   or nullness proof.
3. **Raw lowering, no permissive repair.** Delete `_run_ir_midend_passes`, its
   call and coverage block in `serialization.py`, and `run_midend`. Do not keep
   any of the six proposed structural candidates: check coalescing duplicates
   `check_exception_elim`; PHI alignment collapses equal arguments; structured
   regions, try joins and labels are cleanup; `_ensure_structural_cfg_validity`
   is a repair. A measured raw-ingress rejection names the actual producer and
   backend/TIR-construction boundary. Fix the producer and, when necessary,
   check it fail-closed there. Do not retain a repair, optimizer rollback,
   fabricated edge facts or empty compatibility outcome.
4. **Precision and order are different obligations.** `build_default_pipeline`
   alone owns ordering and analysis invalidation. It currently runs SCCP after
   canonicalization, unboxing, block versioning, GVN, LICM, memory GVN and SROA,
   type_guard_hoist, refcount_elim and dead_store_elim, with no later GVN.
   A precision fix cannot be claimed complete if the useful
   fact arrives after its consumer. Measure a finite ordering change against
   the buying program; keep one pass implementation and one manager. Do not
   reproduce the Python fixed-point driver or add new convergence/budget
   machinery. The scratch early-SCCP/DCE experiment below is an investigated
   alternative, not an automatic new default or a demonstrated runtime fix.
5. **Typed SSA and one-way facts.** Use existing `ValueId`, `BlockId`, `OpCode`,
   `TerminatorEdge`, constant/lattice and ownership types. No ported fact goes
   into `TirOp`'s `HashMap<String, AttrValue>` attributes. Do not re-lift
   serialized IR or key facts by source names. Add a necessary closed rule at
   its canonical generating authority, with all consumers and projections.
6. **Indivisible retirement and honest evidence.** Remove displaced algorithms,
   private state, knobs, caps, telemetry, transports, cache fields and pinning
   tests together. Regenerate projections; reject stale records at the owning
   schema boundary. No noop API, alias, permissive validator or fabricated
   coverage may hide a remaining consumer. Record source, toolchain, target,
   profile, workload/input, artifact, counter scope and guard closure. A failed
   or unavailable observation is not zero cost or preserved performance.

The binding Python signature is the existing serializer with the retired switch
removed. This is a specification skeleton, not an executable stub.

```python
# src/molt/frontend/lowering/serialization.py
class SerializationMixin(
    SerializationBasicOpsMixin,
    SerializationFunctionOpsMixin,
    SerializationExceptionOpsMixin,
    SerializationObjectAttrOpsMixin,
    SerializationCollectionOpsMixin,
    SerializationLoopStringAsyncOpsMixin,
):
    def map_ops_to_json(
        self,
        ops: list[MoltOp],
        *,
        function_name: str | None = None,
        module_init_symbol: str | None = None,
    ) -> list[SimpleIROp]:
        """Serialize language lowering without entering a generic optimizer.

        Requires: producers emit valid typed operations and complete label,
        exception and suspension references (I-2, I-3, I-5).
        Ensures: the existing wire boundary is preserved; no Python SCCP, CSE,
        edge threading, repair or speculative rollback is invoked (I-1, I-3,
        I-6). Generic remaining serializer optimizations are explicitly inventoried.
        Raises: the owning producer/serialization diagnostic for invalid input;
        it does not fabricate missing labels or repair malformed control (I-3).
        """
        ...
```

```rust
// runtime/molt-passes/src/tir/pass_manager.rs
/// Requires: target information admitted by the existing target/profile plan.
/// Ensures: one finite ordered pipeline and its existing mutation/invalidation
/// contracts own every fact consumer (I-1, I-4, I-5); any changed order has a
/// measured buying program and whole-program evidence (I-6).
pub fn build_default_pipeline(target_info: TargetInfo) -> PassManager {
    ...
}

// Existing entry in gvn/engine.rs, not a replacement optimizer.
/// Requires: valid typed SSA and producer-derived representation/effect facts.
/// Ensures: replacements prove identity, dominance, representation and ownership
/// (I-2, I-5), including constants whose existing admission proves reuse sound.
/// Inadmissible materializations remain distinct; no name-based exception or
/// backend-specific fact owner is introduced (I-1, I-2).
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    ...
}

// Existing ownership authority: passes/ownership_lattice_min/replacement.rs.
impl Replacements {
    /// Requires: original typed producers and their existing binding metadata.
    /// Ensures: generated owned-literal roles alone admit SSA ids; the extra
    /// scan and HashSet storage occur only for GVN with replacements (I-2, I-5).
    pub(crate) fn with_literal_reuse(func: &TirFunction) -> Self { ... }

    /// Requires: GVN proved exact payload identity and scoped dominance.
    /// Ensures: two original unbound literals share the terminal drop root;
    /// either binding or another producer retains ordinary record/finish
    /// ownership. No second lifetime mechanism or new attrs (I-1, I-2, I-5).
    pub(crate) fn record_equivalent(&mut self, replaced: &TirOp, leader: ValueId) { ... }
}
```

All new semantic facts in this design have closed Rust types and typed SSA
identities. `list[MoltOp]` and `list[SimpleIROp]` are existing typed transports.
Source spellings remain open program/diagnostic data at existing boundaries,
not optimizer identities. No new `str`, loose dictionary or `Any` fact carrier
is proposed; existing wire metadata is not permission to add one.

### Existing owners, gaps and measured alternatives

The existing Wegman-Zadeck solver, scoped GVN, memory SSA, effect/ownership
oracles, analysis manager and pass timings are reused. Prior SCCP revision-two
research and [LLVM SCCP](https://llvm.org/docs/Passes.html#sccp-sparse-conditional-constant-propagation)
and [GVN](https://llvm.org/docs/Passes.html#gvn-global-value-numbering) explain
these owners' purposes; they do not prove Molt equivalence.

INFERRED baseline-source precision inventory, to distinguish from measured causality:
Before step (c), GVN numbers constants only within a block and never performs constant
Copy replacements, even within that block, because of representation/exception-only reachability
constraints. Python CSE also merges admitted constant materializations across
blocks. Rust memory GVN covers proven typed-slot `LoadAttr`, not Python's
broader LEN, INDEX, CONTAINS, attribute and TYPE_OF alias classes. Python folds
CONST_BIGINT; Rust SCCP's `ConstVal::Int` is i64. Each measured buyer must reach
its existing fact owner; unknown or more precise annotations cannot fill a gap.

The first measured native buyer, `bench_calls`, has identical `inc` bodies but
127 versus 146 frontend module-body operations, including repeated constant
strings inside the loop. This is VERIFIED emitted input; attributing the
instruction increase to a particular constant/ownership rewrite remains
INFERRED until runtime-call/assembly evidence closes that link.
Across all nine native above-envelope cases, full serialized-input op-count
differences are confined to `const`, `const_none`, `const_str` and
`check_exception`; all other kind counts match. This narrows the source
candidates to admitted constant reuse and exception coalescing, without proving
identical branch operands or assigning dynamic cost to those counts.
Public `bench_while` has one versus three `const_str` materializations inside
its frontend loop region (66 versus 70 module-chunk ops). The existing GVN
constant-numbering rule does not itself delete fallible materializations, even
where it assigns local value numbers. Prove admissible reuse at that owner;
do not special-case a program name or infer runtime calls from frontend counts.

VERIFIED machine anatomy sharpens that mechanism: symbol-retaining builds of
both arms for calls/while have text sections identical to the measured release
artifacts. The raw calls loop has three additional inc-ref and three dec-ref
call sites; raw while has two additional sites of each. Neither adds a string
construction call site inside its natural loop. These are static sites,
including possible conditional paths, not dynamic call totals. Existing
allocation, dispatch and handle-resolution counters match across arms.

The native sites narrow the candidates but do not establish the dominant cause.
The settled implementation order above first tests module-slot promotion and
its compensation refusals. Only remaining cells authorize GVN heap-literal
reuse, whose ownership rule belongs in existing Replacements. No shared
lifetime oracle or refcount_elim extension is proposed. Independent rebinding,
deletion, callback, identity and exception oracles must prove its boundary.
Literal construction costs in reserved backend paths remain HF-507's work.

Loop threading is a separate **egress obligation**, not a missing SCCP solver.
`passes/sccp/mod.rs` refuses to fold a conditional when ANY edge targets a loop header
because `lower_to_simple` reconstructs structured loops. Record protected
folds and their cost separately from constant and ordering gaps. HF-821 owns
that egress; do not rebuild Python threading or cross its reserved lowering
paths. An actual required egress change blocks this lane until the integrator
coordinates an indivisible consumer migration. No unmeasured loop cost is
assigned to a generic instruction delta.

VERIFIED bounded egress census: keeping the guard and adding only a diagnostic
at its existing protected-fold branch observed **zero** blocked folds across
99 `bench_calls` bodies, 98 `bench_while` bodies and 2,366 Regex Redux bodies,
each with one ordinary SCCP invocation under `native_release_fast`. All other
copied pass-source bytes match the canonical crate. This finds no protected
opportunity in those per-function replays; it does not qualify module-phase,
other-target or hypothetical guard-removal instruction cost. HF-821 remains
the egress owner, without charging these generic deltas to it.

VERIFIED compile-only ordering experiment: on the largest captured native
program, Regex Redux, the ordinary per-function API with
`TargetInfo::native_release_fast()` processed 2,366 bodies
(including backend-preparation helpers), 489,412 lifted operations. Current
order ended at 466,720 operations; prepending the existing SCCP/DCE ended at
466,715. GVN reported no replacements in either. Current pipeline intervals
summed to 52.593 s; the alternative's ordinary intervals were 49.662 s plus
4.100 s for its extra early passes. Single samples under host load cannot
prove a latency improvement, and no generated-code gain is claimed. This does
not justify a blanket extra prefix as the solution. Precision and ordering
buyers must be distinguished before changing the canonical sequence.

| Scratch replay | Child wall s | User s | Guarded tree MiB |
| --- | --- | --- | --- |
| current | 146.72 | 129.28 | 1214.80 |
| early-prefix alternative | 157.07 | 128.45 | 1212.33 |

| TIR phase/pass | Current ms | Early-prefix alternative ms |
| --- | --- | --- |
| canonicalize | 1171.017 | 1120.456 |
| unboxing | 6580.845 | 6235.460 |
| canonicalize_post | 1054.734 | 977.859 |
| gvn | 2406.667 | 2285.389 |
| licm | 2042.030 | 1935.908 |
| mem_gvn | 4296.192 | 4088.217 |
| sroa | 7355.674 | 6964.295 |
| sccp | 2398.139 | 2263.135 |
| check_exception_elim | 1636.326 | 1542.516 |
| copy_prop | 84.865 | 80.811 |
| dce | 1782.993 | 1535.208 |
| terminal drop_insertion | 29221.097 | 27065.986 |
| terminal refcount_elim_post | 10776.703 | 10293.284 |
| terminal block_arg_prune_post_drop | 456.021 | 437.657 |
| terminal frame_location_placement | 15285.621 | 14518.222 |

Sampling on the same unoptimized scratch replay attributes 76,099 of 101,491 exclusive self samples to hash ancestry. Largest nearest-module contributors are drop insertion, frame placement, value identity and alias analysis; this is analysis-stack attribution, not a release-runtime percentage. The canonical parser/demangler and source inventory are reused. One disabled census entry sample and no census capture samples were observed. The older unoptimized stack capture lacks a separate child-success receipt and is observational stack data, not an end-to-end producer proof.

VERIFIED optimized scratch replay: repository release optimization settings (opt=3, thin LTO, four codegen units, unwind, debug=1), with symbols retained. This is the same input and API cell, not byte identity with the production host compiler.

| Optimized replay | Child wall s | Guarded tree MiB | Normal pipeline s | Output ops |
| --- | --- | --- | --- | --- |
| current | 15.72 | 1658.80 | 5.652728 | 466720 |
| early-prefix alternative | 24.37 | 1565.06 | 5.637218 | 466715 |

The alternative additionally spends 0.444411 s in its early SCCP/DCE prefix. These single observations do not establish a latency distribution or generated-code gain.

| Optimized phase/pass | Current ms | Early-prefix alternative ms |
| --- | --- | --- |
| canonicalize | 132.107 | 130.633 |
| unboxing | 676.787 | 677.531 |
| canonicalize_post | 110.944 | 111.446 |
| gvn | 266.632 | 264.832 |
| licm | 211.607 | 211.906 |
| mem_gvn | 444.372 | 456.909 |
| sroa | 739.990 | 755.961 |
| sccp | 245.398 | 240.592 |
| check_exception_elim | 178.889 | 165.287 |
| copy_prop | 11.954 | 11.887 |
| dce | 185.370 | 164.253 |
| terminal drop_insertion | 2705.249 | 4530.712 |
| terminal refcount_elim_post | 1173.796 | 1898.178 |
| terminal block_arg_prune_post_drop | 46.645 | 85.137 |
| terminal frame_location_placement | 1336.801 | 2108.021 |

Observed optimized sample: 3,011/11,500 exclusive self samples have hash ancestry. Nearest pass-module attribution: outside_passes 855, drop_insertion 538, frame_location_placement 239, liveness 150, value_identity 139, effects 124, refcount_elim 114, value_range 82, alias_analysis 80, counted_loop 69. Direct-child completion is retained in tir-release-sample-children.json; this is not descendant-tree qualification or a causal allocation percentage.

Representation/complexity inventory is INFERRED from the current source;
timings above are VERIFIED only for their recorded cell. SCCP has typed-ID
hash maps/sets, edge and SSA worklists: lattice/use work depends on lattice
height, plus constant evaluation, equality and guard-query cost. GVN uses
scoped hash tables/undo vectors and an iterative dominator walk: expected
linear table visits do not bound hashing, key construction or analyses. DCE
rebuilds hash-map use counts for up to ten whole-function rounds. Copy
propagation flattens typed-ID maps for up to twenty rounds. Canonicalization
scans typed-ID constant/source maps. Block-argument pruning repeats hash-set
liveness and edge scans to stability. Exception elimination repeatedly scans
pending-state maps to stability. None is claimed to be a dense-array pass or
worst-case linear solely from its name. Profile the changed pass, allocation,
snapshot, hashing and largest-function costs before any representation change;
add no cache. Keep drop-phase pruning terminal.

Rejected alternatives and concrete tradeoffs:

- Keep Python SCCP as a CSE seed, retain its policy layer, or call a separate
  Rust service: duplicate facts, repeated solving and two pass managers remain.
- Retain the six cleanup/repair candidates without a measured ingress rejection:
  preserves permissive repair and generic optimization in the frontend.
- Copy Python's object-graph algorithm into Rust: retains repeated CFG walks and
  name-keyed facts instead of extending existing SSA owners.
- Blind deletion or broad constant/heap CSE: loses measured instructions or
  changes identity, exception and ownership behavior.
- Add an early SCCP/DCE prefix by ritual: the observed largest-program result
  gives almost no extra simplification and no GVN gain; runtime benefit is unproven.
- Move SimpleIR egress or native/WASM lowering in this lane: conflicts with
  HF-821's reserved owner. Return the measured requirement to that integrator.

### GoldfishContext: consumer closure and proof plan

The implementation is one migration of the retired boundary. GoldfishContext
must include these producers, consumers, fakes and generated projections:

- All seven `src/molt/frontend/lowering/midend_*.py` modules; frontend
  `__init__.py`, `_types.py`, `cfg_analysis.py`, `value_liveness.py`, and
  `lowering/serialization.py`. Remove `_run_ir_midend_passes`, `run_midend`,
  `midend_stage`, SCCPResult, CanonicalizationState and dead sentinels/helpers
  with their last reader. Audit definite-assignment and unique-result helpers
  for other actual consumers before deletion.
- Required async/generator lowering in visitors/async_gen.py also calls
  _rewrite_aliases_in_arg from midend_canonicalization.py to substitute fresh
  frame-reload SSA values recursively in argument payloads. Preserve that
  operation at its lowering owner while deleting the optimizer helper/API;
  do not retain a mid-end compatibility mixin. No implementation is made yet.
- Pressure bookkeeping: `_TrackedOpsList` and tracking fields in `_types.py`,
  `lowering/generator_state.py`, `visitors/statement_scope.py`; their only
  pressure output is `monolith_pressure_level`. Delete bookkeeping and callers,
  not a wrapper whose callbacks silently stop doing work.
- Rust owners: `passes/{sccp,gvn,dce,block_arg_prune,check_exception_elim}`,
  `copy_prop.rs`, `canonicalize.rs`, `mem_gvn.rs`, `effects.rs`,
  `value_identity.rs`, `ownership_lattice_min/{mod,replacement}.rs`, `refcount_elim`,
  `passes/alias_analysis/mod.rs`, `tir/type_refine.rs` and `pass_manager.rs`. Preserve the
  module/terminal phase order and existing analysis-invalidation authorities.
- Existing immutable-literal identity authority:
  Rust `op_kinds.toml` and `gvn/keys.rs`, with
  `docs/spec/areas/compiler/NATIVE_BACKEND_OPTIMIZATION.md`. Value equality
  cannot replace its identity/representation admission.
- Opcode/property generation: `runtime/molt-ir/src/tir/op_kinds.toml`,
  `tools/gen_op_kinds.py`, `tools/op_kinds/{render_python,frontend_validate,primitive_effects}.py`
  and generated frontend tables. Retire optimizer-only
  FRONTEND_PREDICATE_SEMANTICS, FRONTEND_INTRINSIC_SCALAR_RESULTS,
  FRONTEND_TRUTHINESS_CONTROL_KINDS plus their rows
  of frontend-only metadata and validators, preserving opcode entities and
  properties still consumed by TIR. Keep CHECK_EXCEPTION_SKIP_KINDS and
  FRONTEND_ARBITRARY_HEAP_EFFECT, which language lowering reads.
- Protocol/generator inputs: `tools/gen_protocol.py`,
  `tools/generator_manifest.toml`, its baseline and generated frontend
  `_protocol.py` / `_protocol_attrs.py`. Search hand-written sibling, capture,
  closure and fingerprint-role lists before expensive validation.
- Environment authority: `src/molt/environment_registry.toml`, its generator,
  `tools/{gen,check}_environment_registry.py`, `src/molt/_environment_registry.py`,
  `docs/environment-variables.generated.md`, `src/molt/guard_stems_generated.py`,
  `runtime/molt-host-process/src/host_process_environment_generated.rs` and
  `runtime/molt-runtime/src/runtime_flags_generated.rs`. Remove retired
  MIDEND/SCCP/CSE keys and every
  reader. Inspect reserved runtime-flag projections before writing; a required
  reserved output goes to the integrator, never an unfinished compatibility key.
- CLI/cache transports: `module_cache.py` (MidendCoverage, policy and
  pgo_hot_functions cache fields), `cache_fingerprints.py` and
  `_FRONTEND_COMPILER_INPUTS`, `build_inputs.py`, `models.py`, `frontend_worker.py`,
  `frontend_pipeline.py`, `frontend_execution.py`, `frontend_integration.py`,
  `backend_output_pipeline.py` and `build_diagnostics.py`. Remove dead coverage
  and policy transport; reject old records through their schema authority.
- Public debug surface: `src/molt/debug/{ir,verify}.py`, `--stage pre-midend|post-midend`,
  all `midend_stage` callers, `tests/test_debug_ir.py`, `OPERATIONS.md` and
  `docs/design/foundation/call_anatomy.md`. Expose the real frontend serialization boundary;
  remove `--stage`, `VALID_STAGES`, `midend_stage`, the repeated snapshot and
  Python pass-stat payload together. Serialize once for the frontend dump;
  retire pre/post/all Python views without aliases, empty pass-stat records or
  pretending to show TIR. Existing backend diagnostics continue to own TIR.
- Remaining serializer optimizations: `_fuse_string_split_field_consumers_json`
  and `_scalarize_string_split_fields_json` in `serialization.py` are generic
  name-keyed optimizations, not required wire repairs. Inventory their effects,
  current TIR deforestation owners and measured value. They remain an explicit
  broader HF-773 retirement obligation; no claim of an optimization-free
  frontend is permitted while they remain.
- Readers/fakes: `tests/test_frontend_midend_passes.py`,
  `test_frontend_literal_identity.py`, `test_frontend_package_composition.py`,
  `test_opcode_literal_payloads.py`, `test_debug_ir.py`,
  `determinism/test_ir_determinism.py`, `model_based/test_model_smoke.py`;
  CLI module-cache, build-diagnostics, frontend-execution/integration/SCC,
  backend-output, binary-image-closure, import-collection, smoke,
  runtime-callable-symbol and native-object-publication tests. Update real
  records and fake constructors together. Preserve language oracles; retire
  tests whose only obligation is a Python policy/pass shape.
- Exact CLI fake/reader files under `tests/cli/`:
  `test_cli_backend_output_pipeline_authority.py`,
  `test_cli_binary_image_closure.py`, `test_cli_build_diagnostics_authority.py`,
  `test_cli_frontend_execution_authority.py`,
  `test_cli_frontend_integration_authority.py`,
  `test_cli_frontend_scc_scheduling.py`, `test_cli_import_collection.py`,
  `test_cli_lowering_cache_tooling_scope.py`,
  `test_cli_module_frontend_shared_cache.py`,
  `test_cli_runtime_callable_symbols_authority.py`, `test_cli_smoke.py` and
  `test_native_object_publication.py`. Preserve unrelated boundary obligations.
- Direct `run_midend`/`midend_stage` test consumers additionally include
  `tests/test_frontend_{op_kind_registry,native_export_dispatch,fused_binding_reads,ir_alias_ops,class_imports}.py`,
  `tests/test_gen_op_kinds.py` and `tests/tools/test_check_ir_structure.py`.
  `tests/tools/test_analysis_capsule.py` and
  `tests/cli/test_cli_frontend_integration_authority.py` exercise the tool and
  integration boundaries; migrate their fakes with the real removed fields.
- Remaining proof/policy readers: `tests/test_frontend_condition_semantics.py`,
  `tests/test_backend_parity.py`, `tests/test_committed_gate_policy.py`,
  `tests/test_proof_plan.py` and `tests/tools/test_frontend_hot_pass_profile.py`.
  A textual mid-end reference alone does not authorize deleting a semantic test.
- Existing tools: `frontend_hot_pass_profile.py`, `analysis_capsule.py`,
  `bench_wasm.py`, `bench/wasm_bench.py` (dead MOLT_MIDEND_DISABLE) and
  `tests/test_standalone_benchmark_memory_guard.py` /
  `tests/test_frontend_midend_passes.py` readers of that knob. No replacement
  runner, gate, registry or cache is added.
- Formal correspondences: `formal/quint/{molt_midend_pipeline,molt_optimization_pipeline}.qnt`, Lean `formal/lean/MoltTIR/Passes/{ConstFold,DCE,Pipeline,FullPipeline}.lean`, `formal/lean/MoltTIR/Compilation/CompilationCorrectness.lean`,
  `formal/lean/MoltTIR/Meta/Completeness.lean`, `tools/{check_correspondence,formal_coverage,check_formal_methods,quint_trace_to_tests,translation_validator}.py` and
  their correspondence/validator tests. Retire false Python-pipeline claims;
  preserve proofs still describing Rust, with their actual correspondence gaps.
- Proof selection/public contracts: `tools/proof_plan.toml` authority inputs,
  path globs and tir-midend cell, its generator/projections, README/onboarding,
  `BENCHMARKING.md`, compiler IR specification, architecture convergence,
  this owning design and the existing HF-773 ledger row. Inspect the public
  surface manifest for retired symbols. Keep proof logs private.
- Additional owning design references: `docs/design/foundation/{25_op_kind_registry,44_frontend-architecture-f2,44b_frontend_hot_pass_profile}.md`; update their actual Python-pipeline and
  profiling claims without turning historical measurements into current costs.

Regression corpus: the 22 `bench/` paths from the retained HF-572 corpus,
with fixed arguments and FASTA input, are compared separately for Cranelift,
LLVM and WASM. Do not combine backend deltas: LLVM's own optimization can hide
losses. Five alternating A/B pairs and three A/A pairs identify each observed
noise envelope (largest absolute paired A/A deviation, not a confidence bound).
Every above-noise regression blocks qualification until fixed in its Rust owner
or explicitly dispositioned by the integrator with the lost fact named. High
noise and unavailable counters remain unknown. Multiprocessing parent counters
are not whole-workload acceptance. Such disposition does not silently close a
broader V1 or target requirement.

The twelve `bench_*` rows are `bench/<stem>.py`; the ten CLBG rows are
`bench/corpus/clbg/<stem>.py`. Runtime arguments are binary_trees 10,
fannkuch_redux 7, fasta 10000, mandelbrot 256, nbody 10000, pidigits 100 and
spectral_norm 100; other rows use source defaults. K-nucleotide, Regex Redux
and reverse complement consume the retained FASTA-10000 bytes. Source/input
hashes and bytes stay in private evidence. Observed runtime cells use macOS
26.4 arm64, selected CPython 3.12.16 as oracle, release guest builds with CLI
optimization defaults, and Node 26.13.0 for the WASM consumer. Other OS,
architecture, Python, concurrency and optimization cells remain unverified.

Retain the six executed differential fixtures named above. Before deletion,
add independent compiled-language oracles where the changed facts require them:
`tests/differential/basic/midend_literal_materialization_identity.py` and
`midend_heap_read_effects.py` (planned), covering cross-block constants, bigints,
mutable/callback observations, overloaded truth/arithmetic, missing/unbound
values, exceptions and lifetime/identity. Extend the named
`loop_break_continue.py`, `loop_while_true_break_drops.py`,
`assignment_unboundlocal_global.py` and `dead_branch_scope_unboundlocal.py`
fixtures, plus the executed resume fixtures above, when their obligations change.
Run those exact selections with `tests/molt_diff.py` on native, LLVM and WASM.
The existing `nightly_differential` family owns full-basic discovery; confirm
new fixtures enter its digest-bound shards. Its coverage does not establish
every backend cell: `llvm.test.differential` currently selects integer-carrier
pytest cases, not these new files. Record the explicit migration receipts and
remaining scheduled backend cells; do not claim automatic LLVM/WASM coverage or
add a replacement runner. A producer test or
implementation-pinning frontend test cannot prove the migration.

Reuse matching receipts and one compiled producer per compatible proof cell.
For changed Rust, run the selected build/Clippy and whole-crate plus reverse-
dependency library obligations through `check_changed_crate_tests.py`. Run
changed readers with at most four pytest workers, source consumer gates and
source generators write/check. Before a land-ready handoff, run the existing
`molt_dev.py land --repo <checkout> --precheck-only` in the runner's plain
selected-image environment, without sourced lane selectors. No design-only
or scratch-disable result is a deletion or precheck-green implementation.

### Risks and exit

The main risks are constant identity/representation, missing heap effects,
consumer ordering, structured-loop egress, traceback/suspension transport,
stale products and hidden readers. Invariants 1–6 and the explicit closure
address them at their owners. No dynamic-execution policy, package substitution,
interpreter fallback or target convenience gate is introduced.

Checkpoint 1 needs preserved D0, completed available measurements, the first
coherent deletion those measurements permit, complete consumers and a green
plain-environment handoff precheck. The assigned full retirement also needs
final after/base performance and target proof. Current instruction regressions,
unavailable loop bytes and unverified cells are not resolved by this design.
The current Correction requests this measurement/round-two design handoff and
stops implementation for integrator review. Landing and retirement remain
separate integrator-owned facts.

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
