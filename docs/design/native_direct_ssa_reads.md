# Native direct SSA reads

## Decision

Native reads of immutable SimpleIR names use the existing `NativeSsaValues`
definition and execution-dominance authority. Stateful and loop-index functions
use that authority too; only carriers written by native lowering outside their
SimpleIR definition remain mutable. There is no second SSA analysis.

Predecessor-driven early sealing was implemented and measured, then reverted.
It does not survive as disabled code, another seal registry, an environment
switch, or a completion-error API. Native lowering retains its original sealing
boundary and Cranelift finalization. The direct-SSA read path removes avoidable
variable lookup independently of that policy.

The measured workload is `asyncio_events__molt_module_chunk_6`: 7,402 input
SimpleIR operations, 27,193 final operations, `drop_inserted=true`, 159,585 CLIF
blocks. Rust 1.99.1, Linux x86_64, release-fast, pinned Cranelift 0.137.1,
CLIF verifier explicitly enabled:

| Variant | DFG values | Peak RSS, KiB | Replay wall seconds |
| --- | ---: | ---: | ---: |
| Base | 54,161,531 | 3,251,392 | 35.59 |
| Early sealing | 54,157,757 | 3,606,132 | 38.15 |
| Direct SSA | 19,752,308 | 2,475,620 | 20.40 |
| Both | 19,751,408 | 2,606,700 | 17.84 |

These initial wall measurements were noisy. Two alternating runs per variant on
one pinned CPU resolved the choice: direct SSA took 18.89–20.97 seconds at
2,453,952–2,476,796 KiB; both took 23.66–24.92 seconds at
2,626,040–2,626,236 KiB. Early sealing adds memory and shows no repeatable latency
advantage. The four variants emit byte-identical objects. These are workload
measurements, not a universal bound: the remaining approximately 19.75 million
DFG values and multi-gigabyte peak are still substantial.

## Authority and invariants

`SimpleDefinitionFacts::unique_definition` owns definition identity.
`CFG::execution_points` owns executable availability, including exceptional and
resume edges. `NativeSsaValues::can_bind_directly` requires both dominance and
an earlier textual emission point. A definition that dominates a backward use
but has not yet been emitted still needs variable transport.

The canonical liveness name table indexes definitions, retained values and
representation bindings. A dense Variable-to-name-ID projection supplies the
native read boundary; it does not establish identity or infer facts. Existing
stack/frame-backed names keep memory custody and do not enter this projection.

Native operand readers pass through `native_ssa::FunctionBuilder::use_var`,
including direct readers in scalar carriers and the boxed-operand helpers.
Proven reads return the actual retained CLIF `Value`. Other reads use Cranelift's
variable mechanism, including internal carriers and names without a dominating
definition. Definitions still supply that mutable backing for paths requiring it.
No map keyed by source spelling stores SSA facts.

The builder owns the existing retained-value authority during instruction
emission. Parameters are captured after the invocation prologue. Each operation's
final value is captured before lowering the next operation or switching labels;
structured phis are captured at their actual merge writer. Skipped operations,
including replaced sum-loop bodies, never acquire an emitted entry merely from
their declared result. The textual-availability check still forbids using a
materialized phi before its logical definition point. Capturing at
the first `def_var` would be unsound for operations lowered through multiple
internal branches. Capture reads temporarily bypass retained values so they see
the completed native definition, rather than a stale earlier binding.

Label plans precede lowering. If a direct definition was never captured because
its lowering was skipped or terminated, label materialization reads its existing
Cranelift Variable backing. It does not invent a retained SSA value or panic.
After the source operation loop, the builder clears its operation point so native
epilogues read their actual backing rather than the last body operation's facts.

Debug builds check every ordinary native Variable definition against any captured
value, resolving CLIF aliases before comparison. A different value requires a
mutable exclusion or a known rebind boundary. Incoming block-parameter transport
has one explicit boundary that selects an actual parameter of the current block;
ordinary writes cannot silently bypass the check. The check adds no release-build
analysis or representation.

Native writes outside SimpleIR definitions are excluded at their existing owners:

- Binding destinations can be initialized, rebound at structured merges or
  deleted. Their value snapshots can still be immutable.
- Stateful invocation parameters have resume/frame ABI writes; their neighbouring
  temporaries remain eligible when execution dominance proves availability.
- Loop-index carriers have native backedge writes. Existing loop-body entry
  initialization facts exclude sentinel-initialized carriers. Existing sum-loop
  recognition excludes the accumulator/result names rewritten by native unrolling.
- Legacy native cleanup can scrub operation-produced boxed owner carriers.
  Its existing RC authority and representation plan identify those carriers.
  Borrowed invocation parameters do not acquire such cleanup roots.

These exclusions refine the same definition facts. They do not disable every
name in a stateful function or every function containing an index loop, and do
not add a separate control-flow or definition analysis.

## Prior art and replaced mechanism

Cranelift's pinned frontend recommends sealing a block when its last incoming
branch is known. Its `ssa.rs::find_var` can allocate transient parameters while
resolving a variable through predecessors, even when those values later alias
an existing definition. Retained block-parameter counts therefore do not measure
allocation cost; `func.dfg.num_values()` records the relevant DFG allocation total.
The [upstream SSA implementation](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/frontend/src/ssa.rs)
is the algorithm reference; this change copies no upstream implementation.

The experiment used the existing shared CFG to close incoming sources, recorded
native-owned seals on a builder boundary, and checked native-owned completion in
every profile. Private blocks created by pinned `Switch::emit` stayed helper-owned;
no private Cranelift state or dependency fork was used. Measurements overturned
that mechanism's benefit for this workload, so its edge tracker, checked error
API, threaded consumer migration and seeded-seal tests were reverted together.
The integrator's settled extension instead follows the non-SSA-local principle:
values already proven immutable and available need no variable lookup.

## Consumers and verification

The changed production boundary is shared by the function compiler,
`BlockTransportPlan`, scalar/boxed operand readers, and native test builders.
Explicit label transport reads retained values from the same authority now owned
by the builder. Its former separately threaded `NativeSsaValues` argument is
removed from label/return lowering and fixtures. The retained compiler diagnostic
uses the existing `MOLT_DEBUG_SEAL` selector; no new setting or runner is added.

The existing native regression authority verifies an immutable read across an
unsealed block chain creates no additional DFG value and still passes the CLIF
verifier. Existing branch/exception parameter tests preserve actual invocation
values without incoming phi transport. The native differential regression covers
joins, loop break/continue, handlers, finally paths and generator send/throw/resume,
using CPython output as its independent oracle.

Qualification uses the proof plan's whole-library profile/feature cells for the
native backend and reverse dependencies, the native basic and stdlib differential
families, a cold aio program-cache comparison and the ordinary handoff precheck.
Run-specific receipts and current failures remain in the lane's private report;
this design does not claim unexecuted platform/backend cells or release acceptance.
