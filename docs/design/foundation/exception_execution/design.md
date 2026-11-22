# Local exception execution

Goldfish review candidate for stage 5 of [Call Anatomy](../call_anatomy.md).
HF-906 and HF-561 in the [V1 ledger](../../../agent/V1_HANDOFF_FINDINGS.md)
own the outstanding work and qualification. HF-207 owns builtin exception identity.

## Problem and targets

Molt's compiled code still makes runtime calls to construct, normalize, record,
match, and release exceptions even when the compiler can prove which handler
will catch them and which fields the handler will read. A `try` also installs
handled-state storage before its body runs. The November 21 release LLVM
instruction sweep recorded the following whole-program counts on macOS arm64.

| Workload | Molt | CPython 3.15 `-I` | Molt / CPython |
| --- | ---: | ---: | ---: |
| `bench_exception_typed_fields` | 6,016,788,850 | 1,816,552,896 | 3.31× |
| `bench_exception_heavy` | 8,712,829,986 | 3,898,797,151 | 2.23× |

Both workloads must reach the CPython instruction count on Cranelift and LLVM
with identical outputs. The C instruction target stays open until measured
against an equivalent clang `-O2` implementation. The typed-fields program
constructs and reads three builtin exceptions without throwing. It qualifies
constructor and field scalar replacement independently of handler lowering.

Retained LLVM in-process samples used byte-identical workload sources and
earlier binaries than the ranked runs. In 442 heavy-loop samples, the nearest
runtime entry inside a compiled frame was `int_from_str_of_obj` in 23.5%,
traceback prepend in 18.6%, refcount decrement in 8.4%, and one-argument
construction in 6.1%. Handler match, context chain, push, pop, and context set
accounted for 2.5%, 2.0%, 2.0%, 1.6%, and 1.6% respectively. In 506 typed-fields
samples, positional construction accounted for 20.2%, attribute lookup for
11.9%, refcount decrement for 9.3%, and keyword construction for 9.1%. That
profile still reached generic attribute lookup. The ledger records a subsequent
typed-field load migration. Samples without a compiled frame remain unattributed
to entries. Refresh both profiles at the implementation base before assigning a
current share or selecting a secondary optimization.

The existing `local_raise_transfer` pass is present in `430daa171e`. It passes
a prepared exception as a block argument and skips setting, reading, and
clearing pending state on the successful local transfer. It still normalizes the
raise operand, prepends a traceback, chains handled context, and enters the
shared dispatch that matches at runtime. The checked failure of the context
operation still reaches the original pending handler. Reuse this pass and its
proven transfer shape to finish S1. Preserve the verified subset and the raised
error ABI at calls, callbacks, and foreign boundaries.

## Architecture

### Local transfer and handler entry (S1, I2)

Reuse `local_raise_transfer`'s split between the pending landing and the shared
handler body. Every reachable incoming edge still closes its lexical region and
passes the handler's live SSA arguments. Extend transfer eligibility using the
exact constructor result and the generated `BuiltinExc` relationship. Prove
that `ExceptionPrepareRaise` receives a successfully constructed instance and
that its cause is omitted before removing it. Preserve the origin of a
constructor error and any explicit cause normalization. Dispatch may bypass
only clauses with fixed, validated builtin class bindings, in source order.
An evaluated handler expression, an invalid handler, or a nonmatching catch
sequence retains its evaluation, error chaining, and propagation.

Move frontend handled-state acquisition to handler entry, after the pending
read and clear and before the first clause expression. The S1 direct edge
enters the same acquisition point. A `TryScope` tracks the phase of the emitted
path using a closed type. The protected body and `else` arm carry lexical
cleanup only. A caught handler or exceptional `finally` owns a handled level.
The normal `finally` preserves the prior handled level. Route normal and
exceptional predecessors to the correct cleanup even when both later join.
Restore the level on every exit, including unmatched dispatch, return, break,
continue, re-raise and suspension/resumption. `except*` keeps subgroup creation,
ordering and recombination on the existing group authority, with acquisition on
exceptional entry. Context-manager exits and runtime synthetic handlers keep
their distinct suppression and save/restore obligations.

The runtime uses stack depth for more than `sys.exception()`. Audit the
`exception_last_public_bits` path, activation save/restore and `block_on` before
removing lexical pushes. In particular, the `block_on` epilogue currently uses
the presence of a handler level to decide whether to unwind context managers
after restoring the caller. Caller cleanups must be left to caller edges. A
terminal runtime scope may unwind the context stack it owns. Tie this decision
to the existing context/activation boundary, and prove nested awaits and a
synchronous caller's `try` preserve their context managers. Do not save a new
runtime `try`-presence bit. The compiled push/pop pair and the existing
MatchRef release follow the same explicit handler region. After wiring all
edges, check the region verifier and drop insertion for duplicate or missing
pops. Normal paths through a `try` execute no handler-state operations.

### Lazy traceback, chaining, and handled state (S2)

For a fresh caught value, carry the raise's typed origin and incoming SSA value
through the local edge. The origin belongs to the raise and its activation.
Use the existing location table and origin/propagation distinction, including
spliced frames. Upon materialization, use the original location with the same
frame identity and live/frozen bindings CPython exposes. The handler's current
source position cannot supply the traceback line. Use the same traceback
payload and reader for eager and deferred paths.

Work from uses of the exception and effects of intervening ops. Track the
prior handled exception until chaining can be committed without losing its
owner or changing context identity. Materialize before the first observer
or boundary that may invalidate this proof. Observers include traceback or
context reads and writes, escaped references, new or repeated throws, handler
expressions, callback-capable operations and decrements, async polls, frame
inspection, foreign calls, and suspension. Reuse `RaiseFacts`, the shared
effect classifier, frame-observation predicates, alias analysis and region
facts. A no-raise proof alone cannot establish absence of callbacks or state
reads. Where the existing poll split proves the fast edge services no work,
publish before servicing the poll on its cold edge. A transfer that would
delay context lookup past a change of the prior
handled level, or delay work that itself may raise or run Python, uses the
existing eager point. Preserve mutation of existing exception instances and
cause suppression independently of handled state.

Materialize into ordinary SSA blocks on the edges that need it. A handler path
with no observer completes without publishing its handled state or allocating
its traceback. An edge that materializes a handled level owns its corresponding
pop. Use dominance and ownership at joins to materialize once per activation,
with no runtime representation test to distinguish virtual from published
state. At a join that cannot preserve visibility or ownership within the
rewrite budget, materialize on the incoming edges at the latest proved point.

### Scalar replacement (S3)

Keep an allocation's identity in the compiler as its SSA definition. Admit
constructors by `BuiltinExc`, exact argument shape, and modeled constructor
effects. Reuse the alias and escape authority for copies and block arguments.
An unrecognized use materializes the object before that use. Replacement of a
field result preserves its ownership, default/deleted-field semantics, and
representation. A tuple-valued `args` use must preserve the identity of repeated
reads and any ownership/callback effects. A read of one proven tuple element
can use that element's SSA value directly. Preserve conversion errors and
argument release order. If an allocation failure is observable under the
owning execution contract, retain the operation that can raise it.

Use the existing generated exception schema and layout for accepted constructor
forms and fields. Extract shared constructor projection rules used by both the
runtime materializer and compiler. A compiler projection must not independently
encode keyword validation, OSError subtype/argument normalization, Unicode
index conversion, or missing-value policy. For the three benchmark families,
prove typed keyword bindings for `AttributeError`, the concrete exact integral
errno and platform policy for `OSError`, and validated exact arguments for
`UnicodeEncodeError`. Validate that each rule also works off the benchmark,
including omitted fields, unknown errno, invalid arguments and index conversion.
Keep unmodeled constructor effects at their current evaluation point.

Specialize `str(e)` and `IntFromStrOfObj` using the exact builtin render slot
and constructor arguments. A `BaseException`-style single exact string can
supply the string value. For default-base `int(str(e))`, an exact int operand
can pass through when the existing range facts prove the decimal round trip
cannot fail or trigger the digit limit. This proof excludes `bool`, subclasses,
format overrides, explicit base arguments, and effectful conversions. A string
that still requires parsing uses the existing parser and preserves failures.
Reuse of an equal exact integer follows the [verified subset's immutable-number
identity contract](../../../spec/areas/compat/contracts/verified_subset_contract.md#identity-of-immutable-numbers).
If a cold edge needs the exception object, reuse its canonical constructor and
publish deferred state before its first observer. Retain enough owned operands
to materialize without re-evaluating expressions or extending user-visible
destructor lifetimes across the old release point.

Prefer an expansion to existing TIR primitives before frame observation,
ownership/drop insertion and final check elimination. The shared native walker
lowers those primitives for both backends. Any new low-level layout operation
belongs to one `molt-codegen-abi` generator and uses the generated runtime
binding. The compiler plan uses per-function typed `BlockId`, `ValueId` and
closed enum facts. It has no guest representation. Keep work proportional to
candidate uses and reachable edges, with a fixed budget for any CFG expansion.

## Alternatives and trade-offs

- LLVM `invoke` and DWARF landing pads address native unwinding. Adopting them
  would introduce ABI-specific exception paths and still require explicit
  Python handled state, traceback semantics and a Cranelift solution. Swift
  SIL illustrates explicit error successors and owned block arguments, which
  fit Molt's existing CFG on both backends. [LLVM EH](https://llvm.org/docs/ExceptionHandling.html),
  [Swift SIL](https://github.com/swiftlang/swift/blob/main/docs/SIL/SIL.md).
- Use a preallocated exception for repeated throws, as HotSpot can for selected
  intrinsic exceptions when stack traces can be omitted. Python exception
  instances have mutable args, context, causes, notes, and user-visible identity.
  Reuse could disclose one raise's state through another. Use the local jump
  technique with a fresh instance wherever identity can be observed.
  [OpenJDK `GraphKit`](https://github.com/openjdk/jdk/blob/master/src/hotspot/share/opto/graphKit.cpp).
- Keep eager thread-state publication and make it cheaper. That benefits opaque
  raises, while retaining stores and releases on provably unobserved paths.
  CPython's exception table removes normal-path stack work. Graal's partial
  escape analysis materializes at an escaping edge. Molt can use these ideas
  with static effects and ordinary cold CFG edges, without JIT deoptimization.
  [CPython exception handling](https://github.com/python/cpython/blob/main/InternalDocs/exception_handling.md),
  [Stadler et al., CGO 2014](https://ssw.jku.at/Research/Papers/Stadler14/Stadler2014-CGO-PEA.pdf).

## Invariants and skeleton

1. Lexical protection and handled ownership are distinct. A reachable handled
   exit restores one level acquired on its path. MatchRef ownership follows
   that level. Opaque and exceptional transfers retain their original cleanup
   order, raised owner, and handler argument arity.
2. The first possible observer sees the same exception identity, prior handled
   state, traceback frame and origin, including when a decrement runs Python.
   One activation materializes a virtual identity at most once on any path.
3. A replacement derives only from typed producer facts and generated class and
   layout identities. It preserves evaluation order, failure edges, and owned
   SSA lifetimes. Unproved uses stay on the canonical materialized path.
4. One TIR pass owns local edge selection. One schema owns constructor rules
   and fields. Both native backends consume the same TIR and ABI lowering.
   Existing generator checks and pass-boundary SSA/region verifiers enforce
   projection synchronization and edge validity.

```python
class TryPhase(Enum):
    PROTECTED = auto()
    HANDLING = auto()
    NORMAL_CLEANUP = auto()
    EXCEPTIONAL_CLEANUP = auto()


def _emit_handler_dispatch(
    self, handlers: list[ast.ExceptHandler], exc_val: MoltValue, scope: TryScope
) -> None:
    """Requires a landing with the caught value in scope. Ensures acquisition
    precedes clause evaluation and generated cleanup restores its owner (1).
    Raises frontend rejection for unsupported input through existing rules.
    """
    ...
```

```rust
enum ExceptionUse {
    Transport,
    ClassMatch,
    Field(ExceptionTypedField),
    Render,
    ParseRenderedInt,
    Materialize,
}

enum LocalExceptionState {
    Deferred { owner: ValueId, origin: ExceptionOpPosition },
    Published { owner: ValueId },
}

/// Requires typed SSA and canonical region facts. Ensures a local rewrite
/// preserves every exceptional successor and commits state by its first
/// observer (1–4). An unavailable proof leaves that candidate's CFG intact.
/// Raises: no guest error here. Generated error edges retain their origin.
pub fn run(func: &mut TirFunction) -> PassStats {
    todo!()
}

/// Requires a handler region, a dominating origin, and an owned caught value.
/// Ensures every observation is dominated by the required publication and every
/// published edge releases exactly one level (1, 2, 4). Raises: none during
/// compilation. Declines a rewrite when the origin's state cannot be reproduced.
fn place_handled_state(
    func: &mut TirFunction,
    entry: BlockId,
    state: LocalExceptionState,
    regions: &ExceptionRegionFacts,
) -> bool {
    todo!()
}

/// Requires an admitted exact builtin constructor and its SSA value. Ensures
/// known uses resolve to projections with owned SSA results and materializes
/// once before any remaining observer (2–4). Raises: none during compilation.
/// Guest constructor, conversion, and materialization errors use their
/// original handler edges and origins.
fn scalar_replace_exception(
    func: &mut TirFunction,
    allocation: ValueId,
    kind: BuiltinExc,
) -> bool {
    todo!()
}
```

## Proof and rollout

- Implement I2 and complete S1 around the existing transfer boundary. Establish
  the observer proofs and substitute the dominant scalar consumers before
  claiming S2 eliminates traceback capture on the heavy benchmark. An opaque
  consumer may force publication even on the local transfer. Reprofile after
  scalar replacement before attributing the residual to any other lane.
- Refresh the two LLVM release profiles with the existing unstripped in-process
  PC sampler before source changes. Use its scope and call census to distinguish
  removal of hot-path work from a change in relative shares. Measure Cranelift
  and LLVM release outputs in interleaved base/head runs, best of three, using
  `/usr/bin/time -l` on the same macOS arm64 host. Keep exact build/runtime
  profile, workload bytes, counter, CPython identity, outputs, all three raw
  counts, and source/receipt identities. Repeat attribution after each slice
  that changes the dominant path. Keep sampler and census instrumentation off
  instruction-counting builds. Compare the no-throw body with and without the
  protected region for I2 and compare both benchmark outputs to CPython.
- Extend `exception_local_raise_transfer.py` and `exception_typed_member_loads.py`
  when the existing cases lack an independent CPython oracle for the change.
  Include a cold observer after a local throw, a callback from a destructor,
  original traceback line after the handler advances the frame, outer context
  restored across nested dispatch, and a mixed virtual/materialized join. Pin
  deallocation and identity on the escape edge. Use rebound handlers, handler
  expressions that raise, failed construction, same-instance re-raise,
  explicit cause, `finally`, `except*`, and resume to falsify skipped effects.
  Verify `try`/`else` and returns, breaks and continues both before and after
  acquisition. The heavy benchmark output alone would miss all these defects.
- Run the brief's sorted, deduplicated `basic/*exception*`, `*raise*`, and `*try*`
  differential selection with `tests/molt_diff.py --target native,llvm --jobs 2`.
  Include the owning context, async activation and memory tests when those
  boundaries change. A changed target-neutral path also exercises the owning
  WASM differential entry. Check generated sources, run the consumer gate
  on exact paths, and run the plain-environment landing precheck, including
  the declared whole-lib and reverse-dependency Rust cells. Record failures
  and unverified platform/profile cells in the ledger evidence. Land measured,
  semantically green slices through the lane's assigned integration procedure.

## GoldfishContext

| Boundary | Owners and consumers to inspect |
| --- | --- |
| Lexical lowering, handler ownership and forced exits | `src/molt/frontend/_types.py`, `_protocol.py`, `lowering/exception_lowering.py`, `lowering/serialization_exception_ops.py`, `lowering/loop_lowering.py`, `visitors/statement_control_flow.py`, `visitors/functions.py`, `visitors/async_gen.py`. Review every explicit push/pop, including context-manager guards, and each reader of `try_scopes` and `try_handler_scopes`. |
| Typed TIR, frame effects, and ownership | `runtime/molt-passes/src/tir/passes/local_raise_transfer.rs`, `check_exception_elim/{classify,flow,engine,tests}.rs`, `exception_observation.rs`, `effects.rs`, `escape_analysis/{analysis,classify,mod}.rs`, `alias_analysis/copy_kind.rs`, `frame_location_placement.rs`, `frame_home_placement.rs`, `spliced_frame.rs`, `generator_fusion/clone.rs`, `drop_insertion/{exception_edges,exception_region}.rs`, `runtime/molt-passes/src/tir/{exception_regions/mod.rs,exception_regions/path_state.rs,exception_regions/tests.rs,pass_manager.rs}`, and `runtime/molt-passes/tests/tir/ownership_memory/drop_insertion/{exception_regions,exception_edges}.rs`. Audit pass-order tests and fixtures for regions, pending observers, ownership and callback effects. |
| Constructor and render rules | `runtime/exception_kinds.toml`, `runtime/molt-codegen-abi/src/exception_layout.rs`, `runtime/molt-runtime/src/builtins/exceptions.rs`, `builtins/exceptions/{exception_payload,storage,rendering,raise_protocol}.rs`, `runtime/molt-runtime/src/call/class_init.rs`, `object/{ops_convert,ops_format}.rs`, `src/molt/frontend/lowering/{attribute_access,serialization_object_attr_ops,serialization_collection_ops}.py`. Use HF-207's generated class relationship and the layout field tags. |
| Runtime observation and restore | `runtime/molt-runtime/src/builtins/exceptions/{exception_state_abi,state,activation,raised_state}.rs`, `builtins/frames.rs`, `builtins/frames/{records,locations,spliced}.rs`, `builtins/context.rs`, `async_rt/scheduler/mod.rs`, `object/{mod,ops_sys_traceback}.rs`, `cpython_abi_hooks/native_raised_state_tests.rs`. Check the public pending reader, scheduler exit, native raised state, and terminal/unraisable paths separately by ownership. |
| Backend readers and generated contracts | `runtime/molt-backend-native/src/codegen_ssa/{walk,object,preserved,builder}.rs`, `llvm_backend/lowering/field_slot.rs`, `llvm_backend/lowering/tests/preserved_ops.rs`, `runtime/molt-codegen-abi/src/builtin_field.rs`, `runtime/molt-ir/src/tir/{ops.rs,op_kinds.toml}`, `runtime/runtime_abi.toml`, `tools/{gen_exception_kinds.py,check_table_drift.py,generator_manifest.toml}`, and `tools/op_kinds/`. Follow the manifest to generated Rust/Python op kinds, exception kinds, runtime callables, attached exports, and WASM ABI projections when their authority changes. |
| Proof selection and fixtures | `tools/proof_plan.toml` rules for table drift, builtin identity, and Rust library profiles, the `native.jit-aot-twins` list and `.github/proof-plan.generated.json` if selectors change. Audit `tests/differential/basic/CORE_TESTS.txt`, `tests/differential/stdlib/TESTS.txt` and `tests/test_proof_plan.py` for new or moved cases, `tests/{test_gen_exception_kinds,test_frontend_context_unwind,test_frontend_lowering_contracts,test_frontend_builtin_call_lowering,test_frontend_op_kind_registry}.py`, `tests/differential/basic/{exception_local_raise_transfer,exception_typed_member_loads,exception_handler_identity}.py`, `tests/differential/stdlib/sys_exception_across_resume.py` and `tests/differential/memory/exception_raise_catch_loop_leak.py`. Existing synthetic TIR fixtures must describe the same handler boundary as the production producer. |
| Measurement and artifact identity | `tests/benchmarks/bench_exception_{heavy,typed_fields}.py`, `tools/{bench_suites,perf_instructions,perf_scoreboard}.py`, `tools/PERF_AUTHORITY.md`, `src/molt/cli/runtime_features.py`, `runtime/molt-runtime/src/state/{mod,pc_sampler}.rs`, and the call-census module. Reuse the current sampler/census flags, runtime feature fingerprint, build retention and measurement entry points. This design adds no environment variables, capture lists, fingerprint roles or measurement runner. |

The landing extends the initial aperture into frontend scope/exit producers,
shared TIR exception-region and ownership consumers, existing runtime exception,
frame and context authorities, and their tests and generated projections. If a
required producer or consumer falls inside a reserved lane path, return the
specific dependency to the integrator before changing it.
