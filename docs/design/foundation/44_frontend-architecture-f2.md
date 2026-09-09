<!-- Foundation design 44 — F2 frontend decomposition + separation-of-concerns end-state.
     Architect: read-only research+architecture agent, 2026-06-06. House style of docs 26–43
     (file:line at HEAD, IMPORTANCE×GAP scores, explicit refusals, provenance per borrowed idea).
     Audited HEAD: dc6965d8d8c41895afb6e486349710a1740e8bef.
     Doc number 44 is reserved by the supervisor — do NOT renumber.
     F1 = the landed move-only visitors/ + lowering/ mixin split (a0b9c8a9d, a460e7079).
     F2 = THIS program: phase separation (Parse→Bind/Sema→Lower→Serialize), single lowering
     authority per construct, state narrowing into explicit context objects, registry-derived
     tables, dissolution of the mixin-over-god-object shims. -->

# Frontend Architecture — the F2 Decomposition & Separation-of-Concerns End-State

**Status:** Design doc plus live F2 routing note. F1 (move-only mixin split) is landed and has expanded beyond the original four-mixin snapshot into the current local-binding, midend-optimization, serialization, analysis, visitor, async/generator, and statement-family mixin shell. F2 is the semantic decomposition that F1 deliberately deferred. F2b has begun: `frontend.sema.funcmeta` now owns `FunctionKind`, canonical function-kind normalization, yield/signature classification, and async-generator legality predicates consumed by lowering; `frontend.sema.classgraph` now owns static class-graph construction, local class-member facts with dynamic/decorated-member opacity, C3/static-MRO/reachability facts, class-body block-exec facts consumed by lowering. This doc remains the program spec for the unfinished end-state: phase separation, single-authority facts, registry extension, explicit lowering contexts, and dissolution of the mixin-over-god-object shims.

**Frame-elision update (2026-09-08):** The old static zero-argument `super()` fold and its sema method-set projection were deleted: visible-class MRO agreement does not prove immutable class-cell identity or executing-frame semantics. Method/constructor inline extraction now shares the generated effect authority through `python_inlining.py`; unknown callbacks, descriptors, operators, and scope references keep their real Python frame. Rejected candidates emit no speculative prefix.

**Date:** 2026-06-06. Every file:line anchor verified against HEAD `dc6965d8d`. Current-code note, 2026-06-26: F1 plus the first F2 authority cuts have reduced `src/molt/frontend/__init__.py` to a 302-line facade shell, not the final F2d facade. Function-shape spelling and suspension classifiers now have one semantic home in `frontend.sema.funcmeta`; static class graph, local class-member facts with fail-closed opacity, C3/static-MRO/reachability, class-body block-exec facts now live in `frontend.sema.classgraph`, and call/class lowering consume those facts through explicit sema inputs. The generator is still one shared-state lowering shell, so the F2 target below remains the required end-state: data contracts and phase separation, not permanent mixins over the god object.

**The verdict this engineers against** (supervisor, "engineered like Chris Lattner would?"): **NO, today.** At the original audit, `src/molt/frontend/__init__.py` was 27,071 lines, one class `SimpleTIRGenerator` with 538 `def`s (`__init__.py:211`), assembled from four MRO-mixins (`SerializationMixin, PatternMatchMixin, CallVisitorMixin, ClassDefVisitorMixin, ast.NodeVisitor` — `__init__.py:211-217`) that shared its ~150 mutable instance fields. Current F1 has moved more families out of the file, including `LocalBindingMixin` and `MidendOptimizationMixin`, and the first F2 sema cuts have removed the duplicate function-shape and static-class-graph authorities. The architectural defect remains: scope binding, IC index allocation, exception-edge insertion, const handling, augassign-kind selection, and emitted class metadata are still partly recomputed or supplied during the lowering walk behind one shared generator state. The cost is measured below in §6.

---

## 0. Scope, non-goals, and what F1 already proved

### Current source-ordered name authority (2026-09-05)

`compiler_analysis/python_binding_flow.py` owns source-point name identity,
binding status, invalidation, and truth-callback ordering. Its immutable
`PythonBindingIndex` is retained for a module's frontend lowering; import flow
is a projection of that same index, not a separately recomputed classifier.
The schema/digest/semantic-policy single-flight cache owns reuse. Target Python,
platform, package/spec identity, and execution kind remain part of the policy.

`PythonExpressionFact.binding_invalidated` gates cached module-name values,
direct calls, and imported-module attribute provenance through shared queries
in `frontend/lowering/local_bindings.py`. `binding_is_bound` prevents builtin
spelling from overriding lexical cells, parameters, or source module bindings.
`OTHER` alone proves neither invalidation nor builtin identity. Binding stores
and deletions publish their new state before releasing the previous value;
that release can invoke a finalizer which replaces or resurrects the binding.
Clean rebinding restores source facts only when the previous value's release
is proven inert. Callback epochs include published closure cells, write-only
nonlocals and class namespace slots, but do not taint uncaptured fast locals.
Previously absent slots remain clean until their namespace crosses a callback
epoch; deferred history must retain callbacks that can insert an absent name.
Clean conditional joins retain guarded native-call specialization only where
the binding authority permits it.

Known identity is not a lifetime proof. Private aliases can outlive the module,
function, locals mapping or frame that originally owned their value. Release
effects therefore retain callback barriers for those identities; `UNBOUND` is
neutral when joining genuinely inert alternatives. Plain-local replacement
captures the old slot, publishes the new slot and locals-cache projection, then
emits its release boundary. Runtime closure replacement follows the same
retain/publish/release transaction, including self-assignment and reentrant
replacement. Dictionary and list-cell stores retain their shared transaction
authorities rather than adding frontend-specific release paths.

Truth conversion runs before either successor, including short-circuit,
conditional-expression, loop/filter, assertion-message and match-guard paths.
An arbitrary `__bool__` can replace a callable and return false: a subsequent
global-existence check alone does not authorize a native direct call. Identity
comparisons have primitive boolean results; genuinely invalidated builtins use
the existing dynamic call emitter until stronger facts prove their identity.

Statement execution uses `PythonCompletionFlow` in `python_binding_facts.py`:
normal, return, raise, break and continue have separate successor states; missing
successors denote analyzed unreachability, not unknown facts. Binding and import
metadata share sequence, loop fixed-point, finally/context unwind and exception-
group routing. Handler-raised exceptions remain pending while later `except*`
handlers run; subclass split/derive operations are independent Python callback
boundaries, so dynamic package anchors require explicit runtime import custody.
Only raised exits can be suppressed by a context manager. Repeated
visits join source facts, while temporal history records actual transfers rather
than replaying old completion summaries. Deferred scopes/jobs have stable source
identity and retain their joined execution context.

Statement-loop iteration facts come from the evaluated iterable and the shared
protocol-effect authority. Finite exact string alternatives are one abstract
value, not per-element body replay; aliased mutable containers do not acquire
literal provenance. The shared loop scheduler releases the iterator before
`else` and on terminal exits, never on a backedge. Target assignment exceptions
remain independent of iterator purity. Finite namespace-key stores use one
batched may-write update in the non-relational binding domain, so key count does
not multiply flow states.

Exact globals item syntax and bound `__setitem__`/`__delitem__` identities share
one namespace publication/release transfer. Import metadata consumes admitted
call and assignment facts; globals/setattr spellings cannot manufacture exact
anchors. Unknown old-value releases retain runtime import custody even after a
literal assignment. The regression home is `tests/test_python_loop_metadata.py`.

Import metadata projects those completions with correlated package/spec/name
states and lexical write ownership. Calls observe metadata after earlier callee
and argument evaluation; loop backedges revisit import sites. Scanner consumers
honor explicitly empty site states, including absolute imports, static source
execution and runtime-protocol discovery; absent facts retain conservative
fallback. Pattern irrefutability and capture-only matching are shared source
facts, not scanner/lowering-local classifiers. Focused regression homes are
`tests/test_python_binding_completion_flow.py` and
`tests/cli/test_cli_python_import_authority.py`; these unit consumers do not prove
native/WASM emitted-program matrix cells.

Static graph identity is separate from mandatory runtime import execution. An
exact explicit package remains the successful-resolution anchor even when
`spec.parent` is unknown: CPython captures the package before consulting that
callback. The graph retains the known candidate while lowering preserves runtime
lookup, failure and warning behavior. A genuinely unknown package still requires
explicit runtime import custody rather than an invented graph root.

The runtime-support graph producer supplies that custody for its explicitly
resolved importlib implementation sources on registry-capable targets. The
immutable catalog contains source-backed existing dispatch roots (and their
parents) plus the support roots, not every discovered application module. Exact
owner name/path and AST digest authorize dynamic metadata projection to the
whole catalog; no lexical package is substituted. Discovery validates source
identity and retains those rows in runtime dispatch. The same object reaches
full frontend analysis through the import plan. Custodied owner scans use a
separate per-build cache identity and never populate strict persisted scans or
analysis; other sources and protocol-alias identity analysis remain strict.
Names outside the compiled dispatch surface retain runtime import failure
behavior. Source emitters without that registry cannot claim this custody.

The invariant-metadata fast path also consumes completed assignment effects,
including release, deletion and named-expression callbacks. An unrelated
deferred return must not decide whether import metadata is sound. Regression
coverage lives in `tests/cli/test_runtime_import_scan_custody.py` and
`tests/test_python_binding_flow.py`; emitted native/WASM behavior remains a
separate consumer proof.

Intrinsic dependency classification is not runtime graph admission. Its shared
`StdlibModuleImportEvidence` retains independently proven import edges and
explicit unresolved site plans. Only the proven edges establish same-package
intrinsic/support relationships; unknown metadata never contributes guessed
dependencies or catalog alternatives. A string ending in `.py` is not an import
dependency and cannot qualify a private module as intrinsic support. Direct
intrinsic evidence remains valid despite unresolved imports. The strict
static-import projection still
rejects those obligations with the owning module, path and source line. Compiler
enforcement and the audit command use this same evidence authority.
Classification returns statuses and the analyzed import evidence together;
compiler failures and audit text/JSON report unresolved sites without a second
analysis. Missing, unreadable or target-incompatible source fails with its
module, path and Python target rather than becoming empty import evidence.

Expression results are owned by `compiler_analysis/static_truth.py`:
known truth, exact scalar value, required evaluation, and structurally shared
display segments are separate facts. Source-bound names and members project
from `PythonBindingIndex`; unknown facts never fall back to TYPE_CHECKING or
platform spellings. The frontend keeps required tests in normal conditional
ownership lowering while pruning only impossible successors, matching import
closure. Binding analysis and effect projection share container hash/unpack
boundaries; annotation scopes retain distinct live-class lookup semantics.
`compiler_analysis/python_lexical_scope.py` owns definition/header regions for
declarations, deferred dependencies and frontend assignment projections. Generic
defaults belong to the enclosing scope, before type-parameter construction.
The ordered assignment collector owns the walk; unordered queries project its
set. Collection remains unpruned for Python local-name rules, including eager
function-local annotation declarations even when their reads never execute.
The shared `PythonDependencyAuthority` memoizes transitive dependencies for both
binding analysis and frontend closure/storage planning. Function, lambda,
annotation and comprehension capture queries project that authority; there is
no second frontend free-variable walker. Header evaluation, nested defaults,
class lexical barriers, comprehension parameters and escaping walrus writes
retain distinct scope ownership. Name-lookup facts remove global-only reads
from frontend captures without losing a sibling's same-spelled lexical capture.
Annotation evaluation and lexical participation follow target-version and future
flags. Dependency caches are scoped to the immutable source index and policy;
repeated subtree queries reuse summaries.

For supported Python 3.12+ targets, materialized comprehensions retain the
enclosing Python frame (PEP 709), regardless of unpacking, multiple clauses,
nesting or suspension. `_comprehension_scope` owns typed SSA/frame-slot storage
and masks only target-name projections, restoring outer state on exit. Real
closure captures alone allocate cells; nested eager reads do not. First-iterable
evaluation precedes target shadowing. `current_python_first_arg` identifies the
executing source frame's live positional argument zero, distinct from transport
parameters and method optimization facts. A real generator expression has its
own iterator argument; frame-observing reductions retain that frame instead of
fusing it away. All eager collection forms share `_emit_materialized_comprehension`;
no collection-to-generator rewrite remains. Regressions live in
`tests/test_python_execution_frame.py`, `tests/test_class_function_lifecycle.py`
and the `super_comprehension_frame_ownership.py` differential capsule.

The executing runtime `FrameEntry` owns a typed argument-zero transport
(`NoArgument`, value, or actual cell) and the actual implicit class cell.
Frontend entry, rebinding/deletion, and generated repoll continuations publish
these facts through `molt_frame_context_set`. Temporary argument shadowing and
inline class namespaces restore the outer context on normal and exception
edges; class namespace prefix/suffix writes remain inside the class context.
`super_from_current_frame` is the shared resolver for direct, aliased and
builtin-callback invocation. It owns error precedence and receiver validation;
the constructed super retains the resolved receiver class instead of repeating
proxy `__class__` lookup during attribute access. Native local-frame entry and
guarded exit are semantic operations independent of optional call tracing,
matching WASM ownership. Runtime symbol requirements are projections of the
same generated semantic-role rows as opcode requirements, not a separate
symbol-policy list. `super_runtime_frame_context.py` covers the dispatch family;
frontend/static receipts alone do not establish emitted native/WASM parity.

Deferred annotations and lazy type evaluators use that same executing-frame
and lexical-closure authority. Their namespace lookup scope is not a class-body
frame. Python 3.12/3.13 lazy evaluators have no Python argument zero; 3.14
evaluators expose their format argument. That argument has a typed transport
identity separate from any source binding named `format`, including a lexical
cell or class namespace entry with that spelling. Annotation dependency
projection retains implicit class-cell demand, and capture construction shares
`_capture_lexical_closure` with ordinary functions and comprehensions; type
parameter values and namespace/execution-map captures are explicit inputs.
The public 3.14 evaluator format check preserves CPython's generated comparison
protocol before annotation expressions. Its actual argument is not assumed to
be an integer; comparison callbacks observe the evaluator frame.

`OpIR` owns runtime-requirement aggregation. Executed canonical calls participate
in active-frame dominance; acquiring a frame-sensitive callable only contributes
target requirements and does not itself require an executing Python frame.
Receiver validation observes the receiver's actual type after a `__class__`
getter returns. Releasing a rejected claim is a separate callback boundary, so
error diagnostics read the actual type again afterward. The differential capsule
`super_runtime_frame_mutating_receiver.py` covers these distinct boundaries.

Every function-like definition captures through `_capture_lexical_closure`;
class mappings are not lexical frames. `class_cell_required` is the class-owned
dependency fact, separate from an outer same-spelled free variable. Syntactic
`super` loads request the cell even when `super` is shadowed; local/parameter/
global `__class__` declarations suppress that implicit demand. Nested class
headers may consume an outer cell while their methods capture the new class's
own cell. Class methods, including conditional and repeated definitions, are
created at their source point through `_emit_class_function_definition`.
Decorator expressions precede defaults and apply in reverse order. Runtime
bindings are authoritative; `MethodDescriptor` is only a proved optimization
fact, retired by rebinding or deletion.

Class lookup effects follow namespace ownership, not source spelling. Prepared
mappings and initially exact dictionaries exposed to mutation can execute Python
on reads, stores and deletions. Class-visible annotations consult the owning
namespace through type-parameter scopes; current-scope type parameters and
inherited class `global` directives bypass it. Nonlocal reads can consult that
namespace while nonlocal writes target the closure. Eager 3.12/3.13 and deferred
3.14 annotation scopes retain their distinct lookup order.
`PythonExpressionFact.class_namespace_lookup` projects the same scope decision
to lowering from a single typed name-lookup discriminator, including
`class_lexical` versus `class_global` fallback. Invalidating a value never changes
its storage owner, and a class-global annotation read must not retain an unused
outer same-spelled object. Runtime-backed
class reads use `molt_namespace_get`, which probes arbitrary mappings, recognizes
only `KeyError` (including subclasses) as absence, and retains every other error.
The exact-dictionary path avoids constructing an exception on an ordinary miss.
Deletion uses `molt_namespace_del`: CPython `DELETE_NAME` replaces every mapping
deletion failure with `NameError`, unlike the load operation's KeyError-only
fallback. Successful deletion returns `None`, never a borrowed namespace owner;
custom deletion discards its owned callback result. Lookup result merging
reuses the condition-flow authority (PHI/COPY
for synchronous code, suspension-safe storage for asynchronous code), not a
separate per-read heap-cell lane.
Global declarations bypass the mapping; missing class-local names fall back to
globals, while free/nonlocal names can fall back to their lexical cells.
Comprehension payloads and nested functions are lexical barriers; the first
comprehension iterable remains in the enclosing class scope. Function state
capture/reset owns the class-namespace stack and depth. Regressions live in
`tests/test_frontend_class_namespace.py` and the replayable
`tests/differential/basic/class_namespace_lookup.py` capsule. Frontend acceptance
and CPython reference execution are not native/WASM parity receipts.

Deferred class evaluators capture one explicit namespace cell, not a class-name
rewrite or a parent-function SSA value. The binding index records which class
owners have a deferred namespace reader; body entry allocates their cells once,
before conditional definitions or loops. Evaluator construction only consumes
that owner and cannot lazily allocate branch-local transport. Eager annotations,
future strings and nested lexical-only readers request no namespace cell.
Before type construction the cell holds
the prepared mapping; class finalization publishes the actual copied dictionary
before class callbacks. Each read reloads the cell, so class-name rebinding and
mutation of an abandoned prepared mapping cannot redirect the evaluator.
Unpublished classes use the existing allocation drop guard on all failure paths.
The ordinary type-call adapter and metaclass-winner path share `molt_type_new`.
Its finalizer validates `__qualname__` before publishing compiler cells, normalizes
plain Python special methods centrally, and publishes the class cell before the
dictionary cell. Descriptor hooks consume one retained ordered snapshot via
special-method lookup, followed by one MRO-based `__init_subclass__` dispatch.
Frontend lowering neither refills cells nor replays callbacks. It verifies the
original class cell against a returned type before decorators/publication;
non-type metaclass results are a distinct valid path. Constructor regressions
live in `call/bind/class_constructor_tests.rs` and the static differential
`class_constructor_cell_callbacks.py` capsule.
Eager and future-string class annotations set up `__annotations__` before the
body (including annotations in dead branches), preserve a prepared mapping, and
reload the live mapping after RHS publication and annotation evaluation. Method
annotation attachment follows body order, without a postbody overwrite lane.
Annotation formats 1/2 share one body; free-variable cell reads reload closure
transport in their consuming block rather than reusing branch-local SSA.
The shared lexical-region validator rejects lambdas/comprehensions inside
class-visible annotation scopes for target 3.12, including dead branches.
Targets 3.13+ permit them; ordinary eager annotations and function-body aliases
remain distinct regions. This gate is based on installed CPython reference
compilation, not on the compiler host parser accepting the AST.
Regression homes are `tests/test_class_annotation_namespace_lowering.py`,
`tests/test_function_annotation_lowering.py`, and
`tests/differential/basic/class_annotation_namespace.py` (3.12+ common semantics)
with `class_annotation_namespace_313.py` (static 3.13+ nested annotation scopes
and type-parameter defaults). Canonical `MOLT_META` version admission excludes
inapplicable sources before parsing; runtime `exec` is not a syntax gate.

Import storage and provenance share owner-aware publication for ordinary,
from, child-module and synthetic imports. Class-local imports cannot leak
metadata into enclosing scope; global publication projects module ownership,
and nonlocal publication invalidates stale enclosing import provenance.
`tests/test_frontend_class_imports.py` checks the emitted store and subsequent
consumer, including boxed and loop-bound module slots.

Frontend SCCP scheduling is owned by executable CFG edges and predecessor-state
changes. The redundant global value-notification queue is removed. Newly
executable edges still schedule PHI reevaluation even for equal predecessor
states. The unchanged growth stress consumer measured 47.74855s before and
0.89006s after this change; this is a profiled stress-case result, not a
whole-frontend speed claim. Deterministic diamond scheduling budgets and the
equal-state late-edge regression live in `tests/test_frontend_midend_passes.py`.

`static_comparison_result` also owns each comparison's result facts, so binding
flow applies comparison, truth and release effects before later chain operands
and skips proven unreachable tails. A constant member result never licenses
discarding its owner evaluation, including walrus stores. Executed typing
imports stay in runtime closure even when a TYPE_CHECKING branch is unreachable;
there is no scanner-only import omission.

`frontend/lowering/condition_flow.py` owns value versus syntax-condition
short-circuit emission, including comparison-result identity and PHI/COPY/async
merges. CPython 3.14 omits a stopped nested BoolOp value's outer retest;
3.12/3.13 retain it. Comparison and conditional-expression value boundaries
retain their retests in all three versions. Unary `not` has distinct value and
condition behavior as well. These choices use the target Python version, never
the compiler host version. `tests/test_frontend_condition_semantics.py` executes
the emitted structured expression operations against stateful Python callbacks;
`tests/differential/basic/condition_flow_truth_custody.py` is the cross-backend
replay corpus. This frontend-only execution is not backend conformance evidence.
Result ownership also distinguishes inert builtin displays from containers
holding values whose release can execute Python; dictionary values are included
even though their membership shape describes only keys. Hashing and unpacking
effects consume these shared result facts instead of a second literal classifier.

Accumulated key effects belong to `python_effects.AccumulatedKeyEffects`:
inserting an exact incoming key is not callback-free when a retained dictionary,
set, or keyword key can override equality. Binding analysis and effect summaries
carry that fact across later insertions and expansions; proven empty expansions
introduce no collision callback. Nested walrus values preserve their result shape
without losing the mandatory store. Scoped walrus collection visits immediate
defaults and headers but excludes deferred bodies.

Runtime call builders own one keyword dictionary. Binding acquires an ephemeral,
pinned `PreparedCallArgs` projection after all argument effects; no borrowed
keyword arrays survive expansion. `BoundCallSlots` owns values through every
binding callback and failure exit. Keyword matching and canonical `**kwargs`
insertion precede positional arity/default resolution; each missing keyword-only
parameter rereads the live defaults dictionary. No extra-keyword rebuild lane
exists. Inline-cache admission and foreign calls read the canonical dictionary.
Iterator descriptor lookup and invocation share one
exception boundary; sequence acquisition probes slot presence without binding,
and exhaustion retires the target before releasing callback-capable references.
User-defined iterators may resume after StopIteration; generated sequence
iterators remain exhausted. Cached result tuples publish both cache and caller
ownership before releasing old elements; finalizer reentry may clear or replace
the cache without mutating or freeing the caller's result. ABI-observed tuples
use fresh replacement under the existing ABI ownership authority.
These contracts are retained in the registered
`call_argument_expansion_custody.py` differential corpus; reference traces alone
do not establish native/WASM conformance.

`compiler_analysis/python_call_arguments.py` owns the call/class evaluation and
expansion schedule used by binding effects and argument lowering. A sole call
star operand is expanded after keywords, mixed positional stars are expanded
immediately, and consecutive named keyword values are evaluated before merging.
Class construction has implicit positional operands and therefore never defers
its sole starred base. Print, dictionary construction/update with keywords, and
class keyword assembly use the common argument builder; their old independent
keyword-merge emitters are removed. A sole call star is materialized through the
runtime tuple constructor; mixed stars use list-style accumulation. Tuple
conversion skips length-hint callbacks starting in target Python 3.14, while
list conversion retains them. The frontend no longer implements tuple conversion
as list conversion or treats list/tuple annotations as exact-type proofs.
Splat builtin calls enter the shared binder before per-builtin specialization,
so cardinality, mapping callbacks, and duplicate-key errors retain source order.
Async argument custody uses shared scratch
load-then-clear operations so successful consumption does not retain values in
compiler frame slots. `locals()` snapshot selection follows target Python
(PEP 667 at 3.13), not the compiler's host interpreter.

Required condition evaluation remains in the live-statement projection even
when a successor is impossible. An underscore is not a Python visibility
boundary: an unreferenced private module function remains present. Deleted
helper pruning additionally requires an unobserved lifetime with exactly one
definition and deletion, including earlier escaped live namespace views. The
binding index owns namespace observation facts; pruning and class stability
consume that projection rather than maintaining spelling-only escape scans.

`tests/test_static_expression_result.py` pins value/truth distinctions and
linear display-fact storage. `tests/differential/basic/expression_result_authority.py`
covers evaluation, temporary finalizers, lookup errors, comparison results and
construction segments. Target and CPython-version receipts are required before
claiming matrix closure; source/IR checks alone do not establish it.

Focused authority/IR proofs live in `tests/test_python_binding_flow.py` and
`tests/test_frontend_ir_alias_ops.py`. Replay capsules are
`tests/differential/basic/branch_truth_bindings.py` and `import_star.py`.
Native/WASM execution of these capsules is pending; local fact/IR checks do not
establish CPython-version, OS, architecture, or backend matrix closure.

**In scope.** The decomposition of `SimpleTIRGenerator` into named phases with explicit data contracts; the rule that makes scope-divergent lowering structurally impossible; the extension of the *already-landed* op-kind registry (`tools/gen_op_kinds.py`, doc 25) to absorb hand-kept frontend tables/effect oracles; the phasing that lands move-only structure before any semantic change and dissolves the mixin shims by the end.

**Out of scope (covered elsewhere, cross-referenced only).** The per-construct *semantic* gaps (metaclass `__prepare__`, `__slots__` layout, `__index__` coercion, match-as-CFG) are doc 30's portfolio and its commissioned docs (#40–#42); F2 makes those fixes *land in one place* but does not re-specify them. The op_kinds.toml *schema* is doc 25; F2 extends its **output**, not its design. Generator/coroutine lowering is doc 26.

**What F1 proved (the precedent F2 extends).** F1 split the 27K-line class across files **move-only** — beginning with `a0b9c8a9d` (serialization + pattern_match) and `a460e7079` (visit_Call + visit_ClassDef families), then continuing through local-binding, midend-optimization, analysis, async/generator, comprehension, expression, function, assignment, control-flow, and scope families. Each family remains a body relocation with the `_GeneratorProtocol` (`_protocol.py:54`) restoring cross-file `self.<attr>` type-checking. F1's own headers are explicit that this bought *file boundaries, not semantic ones*: classes.py:1-9 — "Move-only extraction… every method here is, transitively, called only from within this family. self.<method>/<attr> references resolve through the SimpleTIRGenerator MRO at runtime." Doc 30:20 states the verdict precisely: the visitors are "F1-phase move-only extractions with **no independent semantic content**." F2 is the phase that gives them independent semantic content — or dissolves them.

**The existence proof that F2's target shape is reachable** lives in the same package today: `cfg_analysis.py` (416 lines) is already the end-state shape — free functions (`build_cfg`, `_collect_control_maps` at `cfg_analysis.py:44`) over frozen dataclasses (`BasicBlock`/`ControlMaps`/`CFGGraph` at `cfg_analysis.py:12/19/31`) taking an `OpLike` Protocol (`cfg_analysis.py:7`). Zero `self`, zero god-object state, fully testable in isolation. F2 makes the rest of the frontend look like `cfg_analysis.py`.

---

## 1. The end-state architecture (the Year-5 shape)

### 1.1 The phase ladder and its contracts

The end-state is a four-phase pipeline. Each phase is a separate module with an explicit data contract; state is narrowed to the context object each phase owns; the semantic phases **annotate, never emit**, and the lowering phase **consumes annotations, never re-derives them**.

```
ast.Module
   │
   ▼  PARSE            (already external: Python's own ast)
   │
   ▼  BIND / SEMA      frontend/sema/                       ── ANNOTATES, never emits
   │     • ScopeTable        : per-scope symbol kind (local/cell/free/global), the
   │                           closure-cell index map, comp-scope isolation set
   │     • ClassGraph        : static bases, C3 linearization, reachability
   │     • ClassFacts        : class-body block-exec ids, class-member
   │                           methods, descriptor/slot facts
   │     • ConstEnv          : statically-known module dicts, const-int facts
   │     • Legality          : compile-time warnings (~bool, finally-flow), the
   │                           refuse-to-fold-a-raising-const decision
   │   OUTPUT: a SemaResult — immutable annotation tables keyed by AST-node id.
   │
   ▼  LOWER             frontend/lower/  (the thin walk)    ── CONSUMES annotations, emits MoltOps
   │     • ONE authority per construct (one function per AST node kind), parameterized
   │       by a LowerCtx (scope cursor + op buffer + sema handle), NOT by self-state.
   │     • Emits MoltOp(kind=UPPERCASE) into an OpBuffer. No scope re-analysis here.
   │   OUTPUT: per-function MoltOp streams (the funcs_map).
   │
   ▼  SERIALIZE         frontend/lowering/serialization.py  ── already separable
   │     • map_ops_to_json: MoltOp(UPPERCASE) → JSON kind (lowercase) wire contract.
   │   OUTPUT: the JSON IR the backend consumes.
   ▼
backend (Rust)
```

This is a deliberate borrowing of the **Clang Sema/CodeGen separation** (`Sema` builds a fully type-checked, annotated AST; `CodeGen` is a thin walk that *consumes* `Sema`'s decisions and never re-decides) and **CPython's own `symtable`→`compile` split** (`symtable.c` computes the scope/symbol binding for every name *before* `compile.c` emits a single bytecode; the code generator reads `PySTEntryObject` flags, it does not recompute scope). Provenance and how molt diverges from each is in §1.4.

### 1.2 The four data contracts (the load-bearing decision)

The phase boundaries are only real if the contract between them is **data, not a shared mutable object**. The four contracts:

| Phase | Owns (mutable) | Reads (immutable) | Produces (immutable) |
|---|---|---|---|
| **Bind/Sema** | its own work-stacks | `ast` | `SemaResult` (tables below) |
| **Lower** | `LowerCtx` (scope cursor, `OpBuffer`, label/var counters) | `ast` + `SemaResult` | `funcs_map: dict[name, MoltOp stream]` |
| **Serialize** | a local fold/fusion cursor | `funcs_map` + `SemaResult` | JSON IR |

`SemaResult` is the keystone artifact — the analog of Clang's annotated AST / CPython's `symtable`. Concretely:

```
@dataclass(frozen=True)
class SemaResult:
    scopes:      dict[int, ScopeInfo]      # keyed by AST-node id (FunctionDef/Lambda/Module/comp)
    classes:     dict[str, ClassFacts]     # block-exec class ids, class-member facts, slots, fields
    const_env:   ConstEnv                  # module const dicts, const-int facts, refused-fold node ids
    legality:    LegalityReport            # deferred warnings, finally-flow violations
```

`ScopeInfo` carries what `__init__.py` today smears across `locals/boxed_locals/closure_locals/comp_shadow_locals/free_vars/free_var_hints/global_decls/nonlocal_decls/scope_assigned/del_targets/unbound_check_names/async_locals/...` (fields at `__init__.py:270-356`). In the end-state those are **per-scope immutable facts computed once by Sema**, not 18 mutable dicts on one object re-keyed every time the walk enters a function.

### 1.3 Does `SimpleTIRGenerator` survive?

**Decision: it dissolves.** It does not survive as the lowering shell, because "the shell" with 150 fields *is* the problem. The end-state has:

- `frontend/sema/` — free functions + small dataclasses (the `cfg_analysis.py` shape), producing `SemaResult`.
- `frontend/lower/` — a `Lowerer` that is a **thin `ast.NodeVisitor`** whose only instance state is a `LowerCtx`. Its `visit_X` methods are the single authority per construct (§2). It holds *no* scope dicts — it reads `self.ctx.sema.scopes[node_id]`.
- `frontend/lowering/serialization.py` — `map_ops_to_json` becomes a free function `map_ops_to_json(funcs_map, sema) -> json`, not a mixin method (it already takes no construct-level `self`-decisions it could not take from its arguments; §5 row 6).
- `frontend/__init__.py` — a **thin façade** (~150 lines): `compile_to_tir(...)` wires Parse→Sema→Lower→Serialize and re-exports the public names. This is the exact shape of the backend precedent `34e3bddbf` (lib.rs 6,928 → 264 lines, "a thin facade of mod decls + re-exports… every public path and symbol preserved byte-identically").

The F1 mixins, including the current local-binding, midend-optimization, serialization, analysis, visitor, and statement-family mixins, **must be deleted** by the end of F2 — not re-homed as mixins, *deleted as mixins*. Pattern-match and call/class lowering become `Lowerer` method-families that take `LowerCtx` (the M1 precedent: `fe1454a03` lifted 10 op-families out of a 34K-line function into free `fn` handlers "taking the shared lowering state as explicit split-borrowed &mut params" — the Rust analog of exactly this). The phase that deletes the mixin base classes is **F2d** (§4); naming it explicitly is the anti-half-measure commitment per CLAUDE.md.

### 1.4 Provenance (per borrowed idea; GPL = ideas only)

- **Clang `Sema`/`CodeGen` separation** ("Clang Internals", clang.llvm.org/docs/InternalsManual.html): the principle that semantic analysis produces a fully-annotated AST and codegen is a thin consumer. **Borrowed:** the annotate-then-consume contract; Sema never emits, Lower never re-analyzes. **Diverge:** molt's Sema is lighter — it does *binding + legality + static class facts*, not full type inference (molt's types flow as optional hints + the Rust midend's `type_refine`).
- **CPython `symtable.c` → `compile.c`** (CPython source; PSF license — studied, reimplemented, not copied): scope/symbol binding is computed for every name into `PySTEntryObject` **before** any bytecode is emitted; the compiler reads `ste_symbols` flags. **Borrowed:** `ScopeInfo` keyed per scope, computed once, read by Lower. This is the *direct* fix for molt's "scope analysis recomputed inline during the walk." **Diverge:** molt keys by AST-node id and produces an immutable dataclass rather than a mutable `symtable` object graph.
- **Swift `Parse → Sema → SILGen → SIL`** (swift.org/swift-compiler/; Apache-2.0): SIL is the *semantic IR* on which the diagnostic/optimization passes run; SILGen is a thin lowering from the type-checked AST. **Borrowed:** the idea that the IR (here: the MoltOp stream + JSON) is produced by a *thin* lowering from an *already-decided* representation, with the heavy analysis upstream. **Diverge:** molt has no separate SIL — the MoltOp stream is lowered straight to the Rust TIR; F2's `SemaResult` is the "decided representation," not a second IR.
- **Rustc `HIR → THIR → MIR` + query system** (rustc-dev-guide.rust-lang.org; MIT/Apache — ideas only): the query system computes a fact (e.g. `typeck`) **on demand, memoized, keyed by `DefId`**, and consumers *ask* for it rather than recomputing. **Borrowed:** `SemaResult` tables keyed by node id are the memoized-fact analog; class graph and block-execution decisions have sema-owned builders, and Lower reads their facts instead of re-running those analyses. **Diverge:** molt computes Sema eagerly per module (no lazy query engine — the module is small enough that eager is simpler and the DX is better; we reject importing a query framework, §5).
- **MLIR ODS / TableGen** (mlir.llvm.org/docs/DefiningDialects/Operations/; Apache-2.0): one declarative op definition generates the verifier, builder, and printer. **Borrowed:** the §3 registry move — one `op_kinds.toml` row generates the mapper arm, the effect oracle, *and* the frontend's canonical-spelling + raising-kind constants. This is **already half-built** in molt (`tools/gen_op_kinds.py`); F2 extends it.

---

## 2. The single-expression-lowering-authority rule

**The rule:** every Python construct has **exactly one** lowering function, parameterized by a `LowerCtx` that carries the scope. Scope-dependent behavior is a *parameter*, never a *fork in the code*. This is what makes scope-divergence (the task-#42 bug *class*) structurally inexpressible: there is no second site to drift.

### 2.1 The measured evidence: construct lowering forks on scope today

The single largest structural smell in `__init__.py` is the **88 occurrences of `self.current_func_name == "molt_main"`** — i.e. 88 places where the lowering of a construct *branches on whether it is at module scope or in a function*. (`grep -c 'current_func_name == "molt_main"' __init__.py` → 88.) Representative load-bearing sites: `__init__.py:3092, 3279, 3495, 3522, 3559, 3597, 3718, 3798, 4631, 4693, 4718, 4968, 5260, 5772`. Each is a hand-maintained "is this module scope?" fork inside a visit method. Eighteen-plus of these gate *variable storage* (`module_obj`-backed global vs frame-local), which is exactly the axis that produced the comp-walrus / env-misbind P0 history (doc 30:238, commits `99723d589`/`d19dfa588`/`c1faf79f7` — three separate frontend commits in the last week all unifying *storage* for the same construct across scopes). Each of those commits is a single-site patch on the *symptom* of "construct X lowers differently in scope A vs B." F2's rule makes the *cause* — two sites — impossible.

A second axis: the **async vs sync fork**. `visit_AugAssign` (`__init__.py:13961`) forks at `:13967` on `self.is_async() and node.target.id in self.async_locals`, producing a distinct value-load path (`_load_local_value` vs `visit(load_node)`). The same async/sync fork recurs across with/for/bool-op lowering. In the end-state, "async" is a property of the `ScopeInfo` the one lowering function reads — the storage strategy is selected by `LowerCtx.store(name, val)` dispatching on `ctx.scope`, not by an `if self.is_async()` inside every visit method.

### 2.2 The constructs that lower in >1 place (the authority-merge worklist)

| Construct | Forked today on | Anchors | End-state authority |
|---|---|---|---|
| **Variable store/load** | module vs function (`molt_main`); async vs sync | 88× `molt_main`; `__init__.py:13967` | `LowerCtx.store/load(name)` → dispatch on `ScopeInfo.kind[name]` |
| **Const handling** | the raising-fold refusal is a Sema decision today partly entangled with emission; `_RAISING_OP_KINDS` (frontend) duplicates the backend `may_throw` oracle | `__init__.py:1169` (raising set); `:1239-1346` (CHECK_EXCEPTION inverse set); op_kinds.toml `may_throw` (38 rows) | Sema's `ConstEnv` records "refuse to fold node N"; Lower emits the op unconditionally; *raising-ness* is read from the **generated** registry, not a hand list (§3) |
| **Class-body vs function-body statement** | `_class_body_depth` counter mutated mid-walk (`__init__.py:269`); nested-class binding fixed *twice* recently | `c1faf79f7` (class-nested classes); `classes.py:849` (class-body `visit_Assign`) vs `__init__.py:2670`/`2354` (other `visit_Assign`) | one `lower_assign` reading `ScopeInfo.kind` ∈ {class_body, function, module} |
| **Comprehension scope** | `comp_shadow_locals` set toggled around the comp (`__init__.py:276`); walrus-target storage unified *twice* last week | `99723d589`, `d19dfa588`; `__init__.py:276` | `ScopeInfo` for the comp node carries the isolation set + walrus-leak targets; one `lower_comprehension` |
| **f-string pieces** | conversion/format-spec assembled inline in `visit_JoinedStr`; the `{expr=}`-under-inlining multisite miscompile baton | doc 30:258 (`project_inliner_fstring_multisite_miscompile.md`) | one `lower_joinedstr` over a Sema-resolved piece list |
| **super() dispatch and frame elision** | Zero-argument construction reads the live runtime argument-zero and lexical class cell; no static MRO/class-name shortcut substitutes for those values. Frontend expression inlining requires callback-free effects and explicit parameter-only binding, preflighted before emission. | Runtime `FrameEntry` / `molt_super_from_frame`; `compiler_analysis/python_inlining.py`; method/constructor extractors and consumers | Preserve executing-frame observation for aliases, callbacks, class-cell mutation, suspension, and comprehension scope restoration. |

**Note on task #42 accuracy (important).** The `raising_const_expr_fold_matrix.py` regression (the task-#42 corpus) documents that the *two sites that actually dropped the raising op were in the Rust midend* — the `op_kinds.toml` `may_throw` mis-classification of `Shl`/`Shr`/`Pow` and SCCP's `eval_binary_pow` (test docstring, `raising_const_expr_fold_matrix.py:9-18`). That specific bug is **already fixed** at HEAD (the registry now carries 38 `may_throw=true` rows including the shifts; `d6c792454`/`f16740ca3` landed the registry). What the test *also* encodes — and why it crosses "module / function / method / comprehension / lambda" scopes (`:88-118`) — is that **the frontend has five distinct lowering paths per scope** whose divergence is the standing fragility. F2's §2 rule is the structural defense for that fragility; the frontend's `_RAISING_OP_KINDS` (`__init__.py:1169`) duplicating the backend oracle is the residual drift vector (§3). This doc corrects the brief's framing: task #42's *drop* was backend; task #42's *scope-matrix* is the frontend smell F2 targets.

### 2.3 Why `LowerCtx`-parameterization, not a flag

A construct lowered by `if scope == module: ... else: ...` inside one function is *not* single-authority — it is two authorities sharing a `def`. The rule requires that the *strategy* (how to store a name, whether a name is a cell) live behind a `LowerCtx`/`ScopeInfo` method whose *implementations* are the scope variants, so a visit method reads `ctx.store(name, v)` with no scope `if` at all. This is the difference between "one place that branches" and "one place" — only the latter makes the second behavior un-addable without touching the strategy object's contract (where the divergence is then *visible and tested*).

---

## 3. The registry extension (the ODS move) — and the HEAD surprise

**The brief assumed the registry generator does not yet emit a Python file. It does, at HEAD.** `tools/gen_op_kinds.py:50` already renders `src/molt/frontend/lowering/op_kinds_generated.py`, and `op_kinds.toml` already carries the `may_throw` column (`op_kinds.toml:140`, 38 `may_throw=true` rows). Today that generated Python file exports `MAPPER_CANONICAL_KINDS` + `canonical_kind()` (`op_kinds_generated.py:165/272`) — the wire-spelling vocabulary. The F2 move is therefore **not "build a generator"** — it is **"extend the existing generator's Python render to absorb hand-kept frontend tables/effect oracles, then delete them."** This is a sharper, lower-risk move than the brief anticipated.

### 3.1 The hand-kept frontend tables that must become generated

1. **`_RAISING_OP_KINDS`** (`__init__.py:1169-1229`, 60 entries, UPPERCASE MoltOp kinds). This is a hand-maintained copy of the `may_throw` knowledge the backend's `opcode_may_throw` already derives from `op_kinds.toml` (38 `may_throw=true` rows). **Two copies of one fact** — doc 25's exact bug class (#1, the `matches!`-default-false / ModuleImportFrom lesson). It is consumed at `emit()` (`__init__.py:1235`) solely to attach `_expr_col` to raising ops for traceback carets. **Generate `RAISING_KIND_NAMES`** from the `may_throw` column (mapped MoltOp-kind ↔ JSON-kind via the table's existing alias data) and import it.

2. **The `emit()` CHECK_EXCEPTION exclusion set** (`__init__.py:1258-1338`, ~80 entries). This is the *inverse* table: the set of op kinds after which `emit()` does **not** auto-insert a `CHECK_EXCEPTION`. It is logically `¬(may_throw)` for structural/const/pure kinds — a **third** copy of the throw-classification, drifting independently from both `_RAISING_OP_KINDS` and the backend oracle. **Generate it as the complement** of the raising set over the known-kind universe (with the structural/CFG kinds the registry already enumerates — doc 25 §2 "structural kinds").

3. **`_augassign_op_kind`** (`__init__.py:13924-13959`, a 13-arm `isinstance(op, ast.X)` → `"INPLACE_X"` chain). This is the AST-operator → kind map that doc 25 §1 flagged as drift-prone (the historical `floordiv`/`floor_div` schism, bug #5; and the augassign-inplace-dunder gap that was a live correctness bug fixed days ago in `1c15a8353`). The map `{ast.Add: "INPLACE_ADD", ...}` is pure registry data. **Add an `augassign_kind` column** (or a `binop_ast → kind` mapping section) to `op_kinds.toml` and generate the dict; `visit_AugAssign` imports it. The sibling `visit_BinOp` op-selection chain (`__init__.py:10723-10736`, `ast.LShift → "LSHIFT"` etc.) is the same shape and folds into the same generated map.

4. **The midend optimizer effect oracle** (`frontend/lowering/midend_canonicalization.py::_op_effect_class`, composed through `midend_optimization.py`). This is the pre-serialization sibling of backend `OpEffects`: it decides CSE/LICM/DCE barriers over UPPERCASE `MoltOp.kind` names before JSON serialization. It must be generated from `[[kind]]`, opcode `may_throw`/`side_effecting`/`purity`, `[[simpleir_control_kind]]`, `[[frontend_raising_kind]]`, and explicit `[[frontend_effect_kind]]` overrides so a frontend optimizer barrier cannot drift from the TIR registry.

### 3.2 Mechanism (extends doc 25 §5, does not replace it)

- **One table:** `op_kinds.toml` gains two columns on the relevant rows — `ast_binop` (the `ast.operator` subclass name this kind is the binary form of) and `ast_augassign` (the inplace form). The `may_throw` column already exists.
- **One generator:** `tools/gen_op_kinds.py` (already renders `op_kinds_generated.py`) additionally emits, into that same file: `RAISING_KIND_NAMES: frozenset[str]`, `CHECK_EXCEPTION_SKIP_KINDS: frozenset[str]`, `FRONTEND_EFFECT_CLASS: dict[str, str]` plus effect-class sets, `AUGASSIGN_OP_KIND: dict[str, str]` (keyed by `ast.operator.__name__`), and `BINOP_OP_KIND: dict[str, str]`.
- **One sync test:** `tests/test_gen_op_kinds.py` (already exists per doc 25 §5/§6) re-renders in memory and asserts byte-identity → drift becomes a test failure.
- **Three deletions:** `__init__.py:1169-1229`, `:1258-1338`, `:13924-13959` are replaced by imports from `op_kinds_generated`. The `visit_BinOp` chain (`:10723-10736`) keeps its *type-hint* logic but reads the kind string from `BINOP_OP_KIND`.

This is the **MLIR ODS principle** applied to the last hand-kept frontend tables: the op definition is the single source; the verifier (backend `opcode_may_throw`), the builder (frontend kind selection), and now the frontend's optimizer *effect* and *construction* vocabularies are all generated from it. The wire vocabulary (`MAPPER_CANONICAL_KINDS`) is already generated; F2 closes the loop on the *effect* and *construction* vocabularies.

### 3.3 The visitor-dispatch surface (refused as a generation target)

The brief asks whether "the visitor dispatch surface" should be generated. **Refused.** `ast.NodeVisitor`'s `visit_X` dispatch is already a clean, language-defined surface (one method per `ast` node type); generating it would add a layer without removing drift (the `ast` grammar is CPython's, already a single source). The decomposition value is in *who owns each `visit_X*` and *what state it reads* (§1–§2), not in code-generating the dispatch. Generating the *kind tables* kills a real bug class; generating the dispatch would be cargo-culting the ODS pattern past the point it pays.

---

## 4. Phasing (F1/M1 discipline: complete pieces, move-only before semantic, gates per phase)

The unit of work is the complete structural change (CLAUDE.md). F2 is a multi-week arc; the phases below are each a **complete structural piece** (not a partial fix toward the next), so intermediate commits are honest. Every phase carries a differential gate (`tests/differential/basic/` byte-identical on native + LLVM) and the registry sync test where it touches tables.

### F2a — Registry absorption of hand-kept frontend tables (THIS WEEK; collision-free)

**Scope.** §3: add the `ast_binop`/`ast_augassign` columns and frontend effect rows to `op_kinds.toml`; extend `gen_op_kinds.py` to emit `RAISING_KIND_NAMES`/`CHECK_EXCEPTION_SKIP_KINDS`/`FRONTEND_EFFECT_CLASS`/`AUGASSIGN_OP_KIND`/`BINOP_OP_KIND` into `op_kinds_generated.py`; delete the hand lists (`__init__.py:1169`, `:1258`, `:13924`, and `midend_canonicalization.py::_op_effect_class`'s local sets) and the `visit_BinOp` kind-literals (`:10723`), replacing with imports.
**Why it lands this week without colliding with in-flight arcs.** The in-flight frontend work is **`prepfix`** (metaclass `__prepare__`, live-uncommitted in `visitors/calls.py` per `git status`, and `visitors/classes.py`) and **`cfoldfix`** (const-fold paths). F2a touches *neither*: it edits `op_kinds.toml`, `gen_op_kinds.py`, `op_kinds_generated.py`, and three *disjoint* line ranges of `__init__.py` (the table definitions at 1169/1258/13924/10723 — none of which `prepfix` or `cfoldfix` touch, since those live in `classes.py`/`calls.py` and the SCCP/fold paths respectively). It is the M1 move "lift a hand table into the generator" — the exact, low-risk shape of `34e3bddbf`/`fe1454a03`.
**LoC/risk.** Generated-table expansion plus local-table deletion. **Risk: LOW** (byte-identical generated output is mechanically verifiable; the sync test is the gate). **Deletes:** the hand frontend registry/effect tables — concrete dead-code removal of F2.
**Gate.** `tests/test_gen_op_kinds.py` byte-identity + full differential corpus byte-identical (the generated sets must be supersets-equal to the hand sets; a diff *is* a latent drift the move surfaces).
**Audit-tool implication.** `tools/audit_op_kinds.py` parses the serialization dispatcher plus extracted `serialization_*_ops.py` handlers for the wire vocabulary, so serialization decomposition must update the audit file set in the same arc. The raising/augassign tables move under the same generator the audit already cross-checks.

### F2b — Extract Sema (the binding/legality/class-graph phase), additively

**Scope.** Create `frontend/sema/` with `scope.py` (the `ScopeInfo`/`ScopeTable` builder — lifts the closure-cell/free-var/nonlocal/global/comp-isolation analysis currently smeared across the `*Collector` nested classes and the 18 scope dicts), `classgraph.py` (lifts static class-graph construction, local class-member facts, C3/static-MRO/reachability facts, and class-body block-execution decisions), `constenv.py` (lifts `_collect_module_const_dicts` `__init__.py:2705`, the const-int facts, the refuse-to-fold decision), `legality.py` (lifts `_prescan_compile_warnings` `__init__.py:1450`). Each is **free functions over dataclasses** (the `cfg_analysis.py` shape). `SimpleTIRGenerator.__init__` *calls* Sema and stores the immutable `SemaResult`; the visit methods initially still read their old dicts, now *populated from* `SemaResult` (a shim layer).
**Why additive-first.** This is the move-only-before-semantic discipline: F2b *relocates* the analysis and introduces the `SemaResult` contract **without yet rewiring the walk** to read it directly. The walk's behavior is byte-identical because the old dicts are filled from the new tables. This de-risks the boundary before the semantic rewire (F2c).
**LoC/risk.** ~2,500 LoC relocated into `sema/` (the `*Collector` classes — there are ~25 of them, `__init__.py:2185-7227` — plus the MRO/const/legality helpers). **Risk: MEDIUM** (the relocation must preserve the exact population order; the `*Collector` classes mutate `self`-state today, so the relocation must thread a builder that returns facts instead — this is where the move stops being purely mechanical).
**Gate.** Full differential corpus byte-identical; the `SemaResult` tables asserted equal (in a new `tests/test_frontend_sema.py`) to the values the old inline analysis produced on the corpus.
**Deletes:** nothing yet (the shim keeps the old dicts). Dead-code removal is F2c/F2d.

### F2c — Rewire Lower to read `SemaResult`; merge the scope-forked authorities

**Scope.** §2: introduce `LowerCtx` carrying `(scope_cursor, op_buffer, sema, label_ctr, var_ctr)`. Rewrite the visit methods to read `ctx.sema.scopes[node_id]` instead of `self.locals/boxed_locals/...`; replace the 88 `molt_main` forks and the async/sync forks with `LowerCtx.store/load` dispatch (§2.3). Merge each >1-site construct (§2.2 worklist) into one authority. **This is the riskiest phase** (§4.1).
**LoC/risk.** Touches most of the 538 methods (the read-sites change even where the logic does not). **Risk: HIGH** — flagged §4.1.
**Gate.** Full differential corpus byte-identical, **per construct family** (land the variable-store merge, gate; then comprehension, gate; then class-body, gate; then f-string; then super). Each family is a complete piece.
**Deletes:** the 18 scope dicts (`__init__.py:270-356`) and the shim from F2b — *as each family migrates off them*. The last family to migrate deletes the field.

### F2d — Dissolve the mixins into the `Lowerer`; collapse `__init__.py` to a façade

**Scope.** Convert `SerializationMixin`/`PatternMatchMixin`/`CallVisitorMixin`/`ClassDefVisitorMixin` from mixins-over-god-object into `Lowerer` method-families taking `LowerCtx` (or, for serialization, a free function — §5 row 6). **Delete the four mixin base classes and the `_GeneratorProtocol`** (no longer needed once `self` is the small `Lowerer`/the explicit `LowerCtx`). Collapse `__init__.py` to the ~150-line façade (`compile_to_tir` wiring Parse→Sema→Lower→Serialize + re-exports), mirroring `34e3bddbf`.
**LoC/risk.** `__init__.py` 27,071 → ~150 (façade) + bodies relocated to `lower/`. **Risk: MEDIUM** (the F1 mixins are already separate files; F2d changes their *base* and their `self`-contract, which F2c already narrowed). **Deletes:** the four mixin classes, `_protocol.py` (817 lines), the god-object. **This is the phase the verdict demands: the mixin shims die here.**

### Phase ordering rationale

F2a is independent and lands now. F2b→F2c→F2d is the strict order: you cannot rewire Lower to read Sema (F2c) before Sema exists (F2b); you cannot dissolve the mixins (F2d) before the `self`-contract is narrowed (F2c). Splitting F2c's construct-family merges across commits is honest *only because each family is a complete authority-merge* (not a partial fix toward the next family) — each leaves the tree byte-identical and the codebase with one fewer scope-fork.

---

## 4.1 The riskiest phase: F2c (the Lower rewire)

F2c is highest-risk because it is the only phase that **changes behavior-adjacent code at ~500 sites** while the contract is that behavior does *not* change. The specific hazards:

- **Population-order coupling.** The inline analysis today runs *interleaved* with emission (e.g. `exact_locals.pop(...)` inside `visit_AugAssign` at `__init__.py:13965`; `const_ints[...]` written inside `emit()` at `:1245`). Some "facts" are *mutated by the walk itself*. F2c must prove each such fact is either (a) a genuine Sema fact (compute once) or (b) a *walk-local* cursor that stays in `LowerCtx`. Mis-classifying (b) as (a) is a miscompile. This is the line where the brief's "asymmetric coverage" trap lurks: migrating the int-lane store but not the async-lane store re-creates the env-misbind bug.
- **The `molt_main` forks are not all the same axis.** Some of the 88 are "module global storage" (genuinely scope-dependent, → `LowerCtx.store`); others are class-binding questions or "emit module metadata here" (a phase-ordering question). F2c must *triage* the 88, not mechanically rewrite them. A wrong triage is silent.
- **Differential coverage is necessary but not sufficient.** The corpus is ~480 files (doc 30:28); byte-identical output proves the *covered* paths. F2c must add targeted regressions for the *uncovered* scope crosses (the task-#42 matrix `raising_const_expr_fold_matrix.py` is the template — cross every migrated construct with module/function/method/comp/lambda). The gate is "corpus byte-identical **and** a per-family scope-cross regression added."

**Mitigation (the F2b additive shim is the de-risker).** Because F2b populates the old dicts from `SemaResult` *first*, F2c can migrate one read-site at a time with an in-place assertion that `ctx.sema.scopes[node].kind[name] == (the old dict's answer)` — verifying the invariant *while* completing the migration (the right use of a debug-gated assertion per CLAUDE.md: a verification tool *during* the migration, not a substitute for it).

---

## 5. Scorecard — current state per Lattner principle (IMPORTANCE × GAP, file:line evidence)

Scale: IMPORTANCE 1–3 (how load-bearing for a world-class AOT frontend), GAP 0–3 (distance from the principle today). House style of doc 30.

### 5.1 Phase separation — IMPORTANCE 3, GAP 3

The phase boundary is now partially real but incomplete. Parse is external (ast), and `frontend.sema` now owns module class-graph construction, local class-member facts, C3/static-MRO/reachability facts, const-env collection, function-shape facts, class-body block-exec facts. The remaining gap is that Lower still reads shimmed god-object dicts in many other places. The only fully clean separation in the package remains `cfg_analysis.py` — and it operates on the *already-emitted* op stream, i.e. it is a *post-Lower* analysis, not a *pre-Lower* Sema. **Gap remains high:** the architecture has a Sema phase, but most of Lower is not yet a thin consumer of immutable ClassFacts/ScopeInfo.

### 5.2 Single authority per construct — IMPORTANCE 3, GAP 3

The defining smell. **88 `molt_main` scope-forks** (§2.1) + the async/sync forks mean dozens of constructs lower in ≥2 places. The recurring-P0 history is the *measured* cost: comp-walrus storage unified **twice in one week** (`99723d589`, `d19dfa588`), nested-class binding (`c1faf79f7`), the f-string multisite baton (doc 30:258) — each a single-site patch on "construct X diverges across scope." Doc 30 names this directly: "recurring P0 classes ARE upstream quality gaps." **Gap is maximal.**

### 5.3 State narrowing — IMPORTANCE 3, GAP 3

`SimpleTIRGenerator` carries **~150 mutable instance fields** (`__init__.py:240-608`), and the `_GeneratorProtocol` enumerates **186 attributes + ~631 methods (817 declarations)** that the four mixins access on `self` (`_protocol.py`, `grep -c` = 817). Every "extracted" mixin can read and write all 150 fields — F1 bought files, not encapsulation. Process-global mutable state compounds it: the IC index allocator is a **module-global list** `_ic_counter` (`_types.py:43`), so IC slot assignment is shared across *all* compilations in a process, not owned by a context. **Gap is maximal:** the god-object is the abstraction.

### 5.4 Declarative tables — IMPORTANCE 2, GAP 1

**Best-scoring axis** — because doc 25's registry landed. `op_kinds.toml` + `gen_op_kinds.py` already generate the wire vocabulary (`op_kinds_generated.py:165`) and the backend effect oracle from one source, with a byte-identity sync test. GAP is 1, not 0, only because **three frontend tables remain hand-kept** (`_RAISING_OP_KINDS` `__init__.py:1169`, the CHECK_EXCEPTION skip set `:1258`, `_augassign_op_kind` `:13924`) — each a copy of knowledge the registry already owns (§3). F2a closes this to GAP 0. IMPORTANCE 2 (tables are real-but-bounded leverage vs the phase/authority axes).

### 5.5 Testability in isolation — IMPORTANCE 3, GAP 3

Semantic isolation is now real for the first F2 facts, but still thin. The class graph, local class-member facts, class-body block-exec facts, C3/static-MRO/reachability facts are unit-testable through `frontend.sema.classgraph`, and function-shape facts are unit-testable through `frontend.sema.funcmeta`; scope binding still requires full lowering or generator construction. The old outlier remains `cfg_analysis.py` (free functions over an `OpLike` Protocol — `cfg_analysis.py:7/44`) plus `gen_op_kinds.py`'s generated output. Sema-as-free-functions (F2b) is the path that makes scope binding unit-testable on a bare AST next. **Gap remains high:** the architecture has isolated sema islands, but not a complete pre-lower semantic contract.

### 5.6 IR contract explicitness — IMPORTANCE 3, GAP 2

The *cross-process* contract (the JSON wire kind) is explicit and now registry-governed (doc 25) — good. The *intra-frontend* contracts are implicit: the boundary between "analysis" and "emission" is the shared `self`, so there is no typed artifact saying "Lower depends on exactly these facts." `_protocol.py` is an *accidental* contract — it enumerates the 817-symbol coupling surface, which documents the *absence* of a narrow contract rather than providing one. `serialization.py` already takes a near-pure contract (it reads the MoltOp stream + a handful of `self` fields and produces JSON — `map_ops_to_json` at `serialization.py:396`), which is why §1.3 can make it a free function with low risk. GAP 2 (the wire contract is explicit; the phase contracts are not).

### 5.7 Explicit refusals

- **Refused: a full visitor-pattern rewrite in one pass.** Rewriting all 538 methods at once is the anti-pattern CLAUDE.md forbids (the un-reviewable mega-diff; the "land half this session" trap). F2 is phased move-only-first (F2a/F2b) precisely so the semantic rewire (F2c) lands one *complete construct family* at a time, each gated.
- **Refused: importing a query engine (rustc-style `DefId` query memoization).** molt modules are small; eager per-module Sema is simpler, has better DX (Go-like legibility, per `feedback_golike_dx_lattner_perf`), and avoids a framework. We borrow the *fact-keyed-by-id* idea, not the lazy-query machinery.
- **Refused: introducing a second IR (a molt-SIL between AST and MoltOp).** Swift's SIL pays off because it hosts many passes; molt's optimization passes run on the Rust TIR downstream. A second frontend IR would be a parallel source of truth (the compound-interest trap). `SemaResult` is *annotations on the AST*, not a new IR.
- **Refused: generating the `ast.NodeVisitor` dispatch surface (§3.3).** The dispatch is already single-source (CPython's grammar); generating it removes no drift and adds a layer.
- **Refused: re-homing the F1 mixins as mixins.** The verdict is that mixins-over-god-object is the defect. F2d *deletes* the mixin base classes; it does not relocate them.

---

## 6. Contention economics (quantified from this week's evidence)

**The monolith serializes agent work — measured.** In the last ~3 days the frontend took **12+ commits** touching `src/molt/frontend/` (`git log -- src/molt/frontend/`): `1c15a8353` (augassign), `99723d589`/`d19dfa588` (comp-walrus storage, twice), `3f5aa1135` (cross-chunk class-SSA), `c1faf79f7` (nested-class binding), `c683690ce`/`a7021a45f`/`c5d7e02f3` (F1 hash-order leaks), `a460e7079`/`a0b9c8a9d` (F1 extractions), `cedb4a9f8` (unbound-name parity). The overwhelming majority land in **`__init__.py`** — the single 27,071-line file. With "max 2 build-triggering agents" (CLAUDE.md) and three build agents + a parallel session live on this tree *right now* (`git status` shows `calls.py` uncommitted), every one of those commits is a potential merge/edit conflict on one file. The cost is structural: a 27K-line file with 150 shared fields means *any two semantic changes touch overlapping state*, so they cannot be developed in parallel without coordination — the file *is* the lock.

**The M1 precedent quantifies the analog.** `fe1454a03` records the identical pathology on the Rust side: "compile_func_inner (34,242 lines, ONE function) is the #1 incremental-build long pole because rustc's codegen-units partition at *function* boundaries — a 34K-line function is one indivisible codegen unit no matter how the file is arranged." The Python analog is exact: a 27K-line *class* with 150 shared fields is one indivisible *review/merge* unit no matter how F1 arranged the files across mixins (the mixins still share the one `self`). F1 split the *files*; it did not split the *state*, so it did not split the lock.

**How each F2 phase unlocks a parallel lane:**

- **F2a** (registry absorption) removes three tables from `__init__.py` and moves their evolution to `op_kinds.toml` — a *declarative* file two agents can edit on disjoint rows without semantic conflict. It also *prevents* a class of cross-file drift bug (the three throw-tables), so it removes coordination *and* a bug source. Lands this week, collision-free with `prepfix`/`cfoldfix`.
- **F2b** (extract Sema) moves ~2,500 lines (the ~25 `*Collector` classes + MRO/const/legality) out of `__init__.py` into `sema/` files that have **no `self`-coupling to the lowering state**. After F2b, scope-binding work and lowering work are in different files with different contracts — two agents, two lanes.
- **F2c** (narrow Lower's state) replaces the 150 shared fields with a per-walk `LowerCtx`. After F2c, two construct families (say comprehensions and classes) no longer share mutable scope dicts, so they can be edited in parallel — the 150-field lock is broken into per-family contracts. *This is the phase that actually breaks the merge lock*, which is why it is also the riskiest.
- **F2d** (façade) leaves `__init__.py` at ~150 lines — no longer a contention surface at all; new construct work lands in `lower/<family>.py` and `sema/<analysis>.py`, the way new backend op-families now land in `fc/<family>.rs` (the `fe1454a03` end-state).

**Net:** the monolith currently forces N agents through one file and one state object (serial). F2a→F2d converts that into a declarative table + a Sema package + per-family Lower modules + a façade — the same N agents on disjoint files with explicit contracts (parallel). The contention reduction is the *DX* payoff that sits alongside the correctness payoff (the killed scope-divergence bug class) — both are required, per the verdict.

---

## 7. Cross-references and relevant paths

| Arc | Relationship to F2 |
|---|---|
| **doc 25** (op-kind registry) | F2a *extends* its already-landed generator (`gen_op_kinds.py` → `op_kinds_generated.py`) to absorb the three frontend throw/augassign tables. Do not duplicate its schema. |
| **doc 30** (core-language portfolio) | Source of the per-construct *semantic* gaps (`__prepare__` #42, `__slots__`, `__index__`, match-as-CFG #40). F2 gives those fixes a *single place to land* (one authority per construct); it does not re-specify them. |
| **doc 26** (real async/generators) | Generator/coroutine lowering is its territory; F2's `LowerCtx` is the substrate a clean generator-fusion lowering would plug into. |
| **F1** (`a0b9c8a9d`, `a460e7079`) | The move-only file split F2 builds on; F2d deletes the mixin classes F1 created. |
| **M1** (`fe1454a03`) | The Rust precedent for "god-function → free-fn handlers taking explicit context params" — the exact model for F2c/F2d. |
| **`34e3bddbf`** | The façade precedent (6,928 → 264 lines) — the F2d shape for `__init__.py`. |
| **In-flight `prepfix`/`cfoldfix`** | F2a is sequenced to touch neither (`op_kinds.toml`/`gen_op_kinds.py` + disjoint `__init__.py` line ranges). F2b+ must re-sequence once those settle. |

**Frontend paths (all `/Users/adpena/Projects/molt/`):**
- `src/molt/frontend/__init__.py` — `SimpleTIRGenerator` (`:211`), the 4-mixin header (`:211-217`), ~150 fields (`:240-608`), `_RAISING_OP_KINDS` (`:1169`), `emit()` + CHECK_EXCEPTION skip set (`:1231-1346`), `_augassign_op_kind` (`:13924`), `visit_BinOp` kind chain (`:10723`), the ~25 `*Collector` nested classes (`:2185-7227`), `_prescan_compile_warnings` (`:1450`), `_collect_module_const_dicts` (`:2705`), the 88 `molt_main` forks.
- `src/molt/frontend/_types.py` — `MoltValue`/`MoltOp` (`:67/73`), `_next_ic_index` + module-global `_ic_counter` (`:43/47`).
- `src/molt/frontend/_protocol.py` — `_GeneratorProtocol` (`:54`), the 817-declaration coupling surface (deleted in F2d).
- `src/molt/frontend/cfg_analysis.py` — the end-state shape that already exists (`:7/12/44`).
- `src/molt/frontend/sema/classgraph.py` — static class graph, local class-member facts, class-body block-exec facts, C3/static-MRO/reachability facts.
- `src/molt/frontend/visitors/calls.py` — call dispatch consumes semantic binding facts; zero-argument `super()` uses the runtime executing-frame authority.
- `src/molt/frontend/visitors/classes.py` — class lowering, `__prepare__` gap (no `__prepare__` emission; `:574-755`).
- `src/molt/frontend/visitors/pattern_match.py` — match lowering.
- `src/molt/frontend/lowering/serialization.py` — `map_ops_to_json` (`:396`); becomes a free function (F2d, §1.3).
- `src/molt/frontend/lowering/op_kinds_generated.py` — generated; gains `RAISING_KIND_NAMES`/`AUGASSIGN_OP_KIND`/… in F2a (`:165` today).
- `tools/gen_op_kinds.py` (`:50` emits the frontend Python) / `tools/audit_op_kinds.py` / `runtime/molt-ir/src/tir/op_kinds.toml` (`:140` `may_throw`, 38 throwing rows).
