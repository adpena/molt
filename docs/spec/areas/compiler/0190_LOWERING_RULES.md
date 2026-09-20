# Molt Lowering Rules

**Status:** Canonical (compiler-facing)
**Purpose:** Define deterministic, testable transforms from Python AST → Molt IR for supported idioms.
**Audience:** Compiler engineers, optimization authors writing compiler passes.

## Context-manager scope custody

`TryScope` owns nonlocal cleanup for `return`, `break`, and `continue` in both
synchronous and suspended functions. An async manager stores its captured
special `__aexit__` callable in a typed `AsyncContextExit` action before invoking
`__aenter__`. Successful entry establishes the protected scope before assigning
the `as` target; failed assignment therefore exits the manager too.

Normal, exceptional and nonlocal exits use one body/cleanup emitter for both
`SyncContextExit` (runtime-owned manager) and `AsyncContextExit` (captured callable).
It closes the body region and consumes its exception frame exactly once before
calling the captured exit. Exceptional cleanup retains the original exception
across suspension, supplies its actual traceback, and keeps it active during
the exit call, await and suppression truth test. A separate protected cleanup
region restores the enclosing context if any of those operations fails; errors
cannot re-enter the exited manager. Suspended pending return values use one
scratch carrier passed through the cleanup scopes, not a persistent raw spill.
Normal cleanup preserves it; an escaping transfer or failed exit clears it,
and successful return consumes it. A loop inside a finally does not consume
that finally's guard, so its local break/continue retains the pending value.
Normal exits do not truth-test the ignored callback result. Synchronous pending
errors cross the body pop through an independently retained `BINDING_ALIAS`, not
the observer's region-owned MatchRef; suspended paths retain them in frame storage.
Explicit manager actions are the only frontend lexical cleanup authority. Generic
try/try-star scopes carry no context-depth marks or fallback stack-unwind calls.
Runtime context stacks still own manager retention and uncaught-error boundaries.
Entry failure releases captured exit storage without invoking exit. Successful
entry and failed-entry dispatch ignore suppression inherited from an abandoned
return path: their fresh guard and remaining outer continuation are authoritative.
Successful cleanup consumes manager/callback storage before invocation and saved-error
storage after suppression testing, including exceptional cleanup paths. Source
`await` and internal awaits share one state-transition/result authority; its
owned result load clears the hidden carrier before pending-error dispatch.
Handler target deletion is tied to the owning scope, so inner cleanup can still
observe an enclosing handler and a loop transfer cannot clear a handler it stays
inside. Runtime stack-depth restoration is not lexical-region closure.
Every saved handler/finally entry has an explicit owning scope; nonlocal cleanup
retains only entries whose owners remain live, including when an inner finally
overrides the original transfer. Pending-error save/restore slots are distinct
from handled-exception state: bare raise consults the existing runtime authority
used by `sys.exception()`, including dynamically enclosing callers.

Each source loop has one `LoopScope` containing its lexical break destination,
continue destination, cleanup depth and else-suppression flag. A try captures its
lexical loop scopes; inlined finalbodies restore those scopes even when emitted
inside younger loops. Source `break` and `continue` become explicit jumps, not
targetless transfers bound to the physical emission site. Continue shares the
normal latch and index update; break publishes its else flag only after cleanup,
so an overriding continue cannot suppress loop-else. Structured loop markers
remain balanced even when every body path terminates. No backend-specific
exception or loop-target fallback is needed.

Labeled `TRY_START`/`TRY_END` carry path-local exception-region custody, not
textual brackets. Every named frontend scope carries its handler in the same
first operand, including context managers and generator resumption; serialization
projects it directly to the IR label. There is no metadata-only context identity
or scope flag selecting another carrier. Named starts and ends require a defined
handler target, while anonymous IR regions have no named target. One region may
close on several alternative paths, including
an early return inside a still-open `IF` or loop. Frontend structural repair
must preserve those branches and every labeled close; it must not pair a start
with the first textual end or discard later closes. Frontend CFG/SCCP preserves
explicit pending-error transfers and conservative handler reachability; shared
TIR exception analysis owns region facts after control-flow construction.
LLVM emits only explicit exception-stack operations, not implicit frames for
TRY markers. WASM dispatch uses pending-state checks; native EH is selected only
for a structured, non-relocatable frame that actually emits an EH region. Luau
labelled transfers project the shared executable SimpleIR graph (excluding
verifier-only reachability edges), with explicit pending/handled exception state
in the coroutine-owned frame context. No backend pairs textual TRY intervals or
skips cleanup operations by matching source patterns.

Rust-source labelled control flow uses the existing structured-PHI rewrite and
shared TIR lift/lowering for SSA edge assignments, then the shared executable
SimpleIR graph for dispatch. Residual unstructured PHIs fail explicitly instead
of guessing a predecessor. A jump never
implies a return or selects the last stored value. Source-order alias hints cannot
cross dispatch blocks. This does not expand Rust's admitted object, exception or
coroutine capabilities; unsupported domains remain explicit admission failures.
The current Rust target policy also gates unstructured control and truthiness;
internal executable-emission tests do not confer checked-target support.

Shared TIR-to-SimpleIR lowering owns block-argument transport for every target.
Function invocation seeds entry join slots once, before the entry label; a
backedge to entry reloads its supplied arguments without rerunning that prologue.
Explicit and structured branches use the same edge stores and block-entry loads,
including inlined arms and loop headers. Invocation is an external predecessor,
so structural inlining cannot consume the entry block. Missing edge operands or
labels fail at this shared boundary instead of borrowing a lexical value or
emitting a backend-specific fallthrough.

SSA operands also own copy sources after rewriting. Original source-name
metadata cannot override an operand selected by a join, substitution or loop
backedge. Named-store destinations remain destinations; they are not an
alternative source-value authority when projecting copies back to SimpleIR.
Named stores and deletes without their explicit destination fail at that
boundary; lowering must not invent a local slot from an SSA result name.
`SimpleValueNames` allocates injective value, local-slot and block-argument
transport names. Authored spellings remain provenance, not permission for
distinct SSA values to overwrite one another. ABI parameter names remain fixed;
mutable storage with the same authored spelling gets distinct transport.
Representation facts must follow the corresponding value or authored producer,
not be recovered from a renamed transport string.
For generic operations whose `var` field is a read, SSA lifting records the
exact resolved operand index. Lowering reconstructs that field from the indexed
SSA operand, never from its old spelling or an assumed last argument; unresolved
transport-only spellings do not consume an operand. Emission-only temporaries
share the same collision-safe namespace as values and storage.

Loop-invariant motion is owned by the shared TIR LICM pass, after control-flow
and SSA construction. Optimization loops require executable backedges; retained
lexical loop markers do not establish reachability or a valid preheader.
Hoisted operands must dominate the destination through the canonical CFG.
Source-ordered SimpleIR motion cannot prove dominance of
resumption edges into a loop and must not run before that analysis. Native,
LLVM and WASM preparation and the frontend midend no longer have a second
constant-hoisting pass;
Luau source postprocessing likewise does not perform textual LICM.
Deferred class calls keep live global lookup inside the executed loop body;
there is no frontend preheader class cache or name-based effect-proof override.
Zero-iteration loops cannot acquire, cache or raise from an unexecuted class read.
Serialization preserves the resulting definitions: identity comparisons do not
redefine `None` operands or invent values for undefined inputs. SSA construction
and verification, not serializer rematerialization, own def-use correctness.

Guarded handler/else/finally bodies each have one lexical cleanup continuation,
not recursive per-statement pending checks. Exit failures inside those bodies
join their cleanup before the enclosing finally runs. Nonlocal unwind exposes
only the remaining scope prefix while lowering cleanup; a typed executing-finally
flag prevents re-entry. Positional counts of already-popped scopes are not a
second unwind authority. A failure from a finally is dispatched while the next
outer region is still active, allowing its manager to observe or suppress it.

## Builtin shape and lifetime authority

Generic attribute reads keep boxed receivers and enter `molt_get_attr_object_ic`,
which caches only an owned interned name before invoking `molt_get_attr_name`.
Stable function/source-operation identity selects the name-cache site; the actual
name is checked on every hit. There is no wrapping frontend slot allocator,
class-only raw-offset cache or instance-independent result cache. Descriptor
precedence, instance shadowing, `__getattribute__`, exceptions and ownership remain
the canonical lookup's responsibility on every access.

`compiler_analysis.python_builtin_shapes` describes builtin constructors,
methods, file modes, `range`, and `len`. `PythonBindingIndex` transports their results
through source-ordered bindings, aliases, joins and loop fixpoints using
`StaticExpressionResult`; frontend spelling and annotations are not proofs.
Arbitrarily exposed mutable containers retain exact kind but not concrete
contents, cardinality or truth facts. Publishing a tracked allocation into a
binding preserves its current contents while removing unexposed freshness;
the binding's owner token and all subsequent mutation boundaries govern those
facts. Homogeneous element results have a separate lifetime:
object writes and callbacks expire them, including mutable descendants inside
immutable owners. Iteration and frontend specialization consume that shared
projection; annotations and method spelling cannot manufacture element facts.
Receiver-kind preservation never authorizes restoring a name rebound by an
argument or invocation callback. Method effects include index/comparison/iteration
protocols and displaced-owner finalization before post-call facts are admitted.
Publication also invalidates callback-free release for object-bearing mutable
containers, including through immutable tuple/frozenset owners: an alias may
insert a finalizable object. Exact bytearray storage cannot contain Python
objects, so its release safety survives publication independently of its shape.
Kind facts mean exact builtin types, not subclass compatibility: callbackful
`str`/`bytes` conversions can return user subclasses and cannot publish an
exact-kind or inert-release fact without stronger operand/protocol evidence.

Allocation custody is separate from result shape. Persistent binding states
carry evaluated-allocation owner tokens through aliases; disagreement at a join
or invalidated binding yields unknown ownership. A read cannot turn unknown
ownership into a new allocation. Receiver mutation may exempt a replacement
allocated after callee capture, but still expires mutable descendants inside
that replacement, both per-member and homogeneous element projections, and
their reference-release safety. Loop convergence includes loss of owner custody before
publishing facts for subsequent iterations.

`PythonCallSiteFact` separates evaluated callee/argument effects, invocation
effects and post-invocation cleanup. A known normal-result kind does not imply
a callback-free invocation. `callee_elision_safe` is the authorization for
specialized calls and fused range/length consumers: identity, argument effects,
invocation, and release ordering must all permit it. Otherwise lowering loads
the actual callable before evaluating arguments and uses ordinary object call
dispatch, retaining any independently proven normal-result kind. Fresh lookups
consume builtin-member invalidation; aliases retain their captured object
identity independently of later changes to the builtin slot.
Argument cleanup also retains each argument's exposure to later argument
callbacks. Inert invocation does not erase that history or make the old name
an owner again; callee and argument release safety remain independent.

Normal-result transport uses that same expression-result authority for every
generic call, including imported or captured `open` aliases. There is no second
frontend file-mode classifier or replacement of an evaluated alias with a new
builtin wrapper. Builtin identity/member projections derive from the same
normal-result catalog, including callbackful operations such as `open`; catalog
membership alone never authorizes intrinsic replacement. File-mode facts
describe the returned file family, not exact iteration elements or the identity
of an instance's `read`, `write`, `close`,
or `flush` attribute; those calls retain ordinary descriptor lookup and argument
binding. Text decoders can return string subclasses, and unbuffered file
iteration calls the live `readline` attribute. Exact element specialization
requires an independently proved iterator result, not a text/binary mode hint.
The same rule applies to `contextlib.nullcontext`, `contextlib.closing`, and
`math.trunc`: module/member spelling cannot replace the evaluated callable or
its signature. Context construction belongs to the actual library callable;
there is no second frontend constructor or serialization lane for those names.
File iteration and context protocols retain their callback effects (including
custom codecs) independently of their normal item/entry result. Normal-result
exactness applies to the evaluated value, not automatically to its destination
name: assignment publishes the new value before releasing the old one, whose
finalizer can rebind a callback-visible destination. A later name load must use
the post-cleanup binding fact. Specialization requires that fact or an explicit
runtime guard; neither a yielded type nor a previous annotation can restore it.

`split` uses one str/bytes/bytearray emission family. Exact source-point receivers
can use the intrinsic directly. Otherwise, canonical builtin class identity
guards capture a tagged target before arguments: an exact receiver for a builtin
arm, or the actual generic descriptor result. The generic arm does not retain the
original receiver after lookup; its finalizer may run before the arguments.
Positional and keyword values are evaluated once in source order, then mapped to
the intrinsic parameters. Unknown/expanded/duplicate signatures retain ordinary
runtime binding and errors. Generic keyword calls use the shared argument builder;
only exact arms carry list-element facts and the joined result remains unknown.
Suspended target and argument storage is consumed and cleared before invocation,
so temporary frame references do not postpone cleanup.
List mutators and set algebra/update families share this retained-receiver
argument capture. All arguments precede set iteration/mutation, including when
later arguments suspend. Malformed positional-only method signatures stay on
the real runtime binder rather than discarding keywords or expansions.

Runtime split/rsplit entrypoints share validation and directional implementations
across str, bytes and bytearray. Receiver admission precedes maxsplit's index
protocol; separator admission and all mutable storage borrows follow it. Index
callbacks may resize or mutate bytearray receivers/separators, and splitting
must observe their resulting contents. Out-of-range maxsplit values raise rather
than saturating. Capped right-whitespace splitting preserves leading whitespace
in the unsplit remainder. String whitespace scanning accepts surrogate-containing
WTF-8 and uses Python's whitespace definition; byte scanning uses one ASCII
whitespace predicate for scalar and vectorized paths. The replayable
`split_protocol_order.py` corpus owns these protocol/error cases, separately from
callable capture and finalizer cases in `builtin_shape_lifetimes.py`.
The same generated Unicode-space authority and bidirectional WTF-8 traversal
govern `strip`/`lstrip`/`rstrip` and `isspace`, including custom surrogate trim
characters. Text ASCII SIMD classification includes Python's U+001C..U+001F;
bytes/bytearray classification does not. Scalar/vector agreement is checked
against the generated table, not Rust's independent whitespace definition.
All string predicates share descriptor admission and WTF-8 scalar traversal.
Alphabetic, cased/titlecase and identifier properties are generated from the
selected CPython authority alongside numeric, whitespace and printable tables;
Rust Unicode properties and case-conversion allocations are not classification
authorities. Lone surrogates are ordinary uncased/nonprintable code points, not
a reason to reject the surrounding string. `string_predicate_protocol.py`
owns the replayable classifier/subclass/receiver corpus. Host reference output
or table generation alone does not prove compiled native/WASM parity.

Numeric float conversion has one type-level protocol authority: `__float__`,
then `__index__`, with callback exceptions and strict-subclass warnings retained.
The constructor honors subclass overrides and separately admits text parsing;
numeric consumers (`%f`, memoryview packing and version-gated `float.from_number`)
use an existing float-subclass payload before invoking protocols and never parse
text. Integer overflow remains distinct from an actual floating infinity.
Memoryview packing alone translates numeric TypeError/OverflowError to its
format-specific TypeError/ValueError; boolean packing preserves callback errors.
`float_protocol.py` owns the cross-consumer differential corpus.

Private helper names do not confer compiler privileges. In particular,
`_load_optional_intrinsic` is an ordinary Python callable: assignments evaluate
the live helper, preserve its effects and bind its actual result. Its string
arguments neither reserve external symbols nor fabricate runtime-function type
facts. Intrinsic lowering uses the explicit imported intrinsic protocol instead.

Namespace identity and exact dictionary kind are separate facts. Lexical module
bootstrap pins an exact builtin namespace, so exact `CURRENT_GLOBALS` aliases
in module scope carry dictionary kind without content, truth or freshness facts.
Synthetic module code has no callable target and cannot be rebound by
`FunctionType`. Deferred/rebound function namespaces can be dict subclasses;
their namespace identity alone never authorizes builtin dictionary methods.
A runtime storage tag likewise does not prove exact builtin class identity.

Callable code and activation namespaces are independent. `FunctionType` can
install foreign globals and captured builtins on existing code; same-source
module history does not certify a deferred body's global names or imports.
It can also supply different closure cells: lexical origin is not executable
value custody for a deferred body's free/nonlocal inputs. Proven assignments
inside the current activation are distinct from supplied closure values.
Function, lambda, deferred annotation and generator-expression activations share
external-slot discovery and entry widening: compatible cells may hold arbitrary
values or be empty. A generator expression's first iterator is acquired in its
creating scope; only its deferred body uses foreign activation inputs. Inline
class and eager comprehension scopes inherit their enclosing activation's
custody, but class preparation and other callbacks still invalidate exposed
cells. Callback exposure is independent of lexical storage: an empty lexical
cell raises instead of consulting builtins. Proven lexical values are
distinct from globals and must not lose precision merely because code is
callable. Any future namespace-specialized body needs an explicit activation
guard; guarding a constructor alone cannot certify its downstream result facts,
branch pruning, receiver methods or loop fusion.

Exact user-class layout follows the actual lowered result, never a constructor
name recovered from the AST. Only a constructionally exact result may carry
that identity through local aliases; a live call, guarded target hint, type
annotation, or lexical class declaration cannot authorize fixed-offset field
loads/stores. Descriptor lookup, augmented assignment and deletion retain the
ordinary object protocol when that result identity is unproved.
Named reloads use the current binding-flow fact, so a producer annotation cannot
resurrect identity after a merge, rebinding or mutable-cell load. Arbitrary
initializer callbacks also invalidate post-construction exactness; only a
proven inline initializer can preserve the allocation's original class fact.
Exactness is lifetime-bound, including facts on already-evaluated temporary
operands and loop guards. Later argument evaluation, an augmented-assignment
RHS/operator, a descriptor, or an owner release may change the receiver class;
consumers must validate the fact at the actual access, not reuse an earlier
boolean decision. Dataclass field offsets obey the same authority as ordinary
class offsets. `getattr` evaluates all arguments, including an unused default,
before lookup; field specialization cannot elide or move those effects.

Fixed boxed-allocation shape is selected by the generated op-kind layout rule,
shared by typed-slot analysis and scalar replacement. Raw zeroed storage and
class missing-value storage have distinct initial values and dictionary tails;
exact operand counts, payload sizes and offsets remain required. Layout facts
do not establish callback-free construction or lifetime. Escape analysis uses
the terminator's canonical direct-value and edge projections, so new control
forms cannot silently omit an ownership obligation.

Native cleanup follows executable ownership, not the order in which branches
are emitted. Functions already processed by TIR drop insertion allocate no
native cleanup tokens. Direct native tracking carries each boxed ownership root
through Cranelift SSA: acquisition publishes the new owner before releasing a
displaced owner, release consumes the current path's token, and return transfers
it. Borrowed inputs start without an owned credit; returning a borrowed value
acquires the caller's credit. Joins and loop backedges carry predecessor state,
so cleanup in one branch cannot suppress a sibling's obligation.

Generated alias facts distinguish shared-root value moves from independent
retained results. BoxVal and UnboxVal retain independent ownership when projected
to boxed SimpleIR; equal bits do not imply one credit. Mutable bindings and their
snapshots cannot be conflated by a static alias map. Candidate liveness lists
schedule cleanup but do not constitute a second release-state authority. Raw
scalar carriers remain outside boxed-object cleanup.

No-result explicit retains are credits for the current binding epoch. Rebinding
starts a new epoch and cannot spend the previous object's explicit credit
against its replacement. The old credit remains an explicit IR obligation,
balanced through a surviving handle; native tracking does not invent or release
external ownership. Result-carrying stores publish their destination and result
exactly once, including when the destination is a raw or stack-backed carrier.
The canonical def/use visitor distinguishes a binding-only `out` destination
from an optional value result with a distinct explicit `var` destination.
Store-family results are not metadata, and a destination cannot be counted
twice as both a binding and an independent result.

Iterator fusion has one shared SSA authority. Native lowering consumes the
declared iterator, value/done and unpack operations; it must not scan a later
source window, skip consumers, or replace an observable tuple with a key or
done flag. A multi-result operation publishes every generated result field,
including trailing unpack outputs, through the same ownership path as ordinary
single-result operations.
Eligibility also proves the generated conditional-result contract: an item read
must follow a not-done edge for that same dynamic iterator step. A prior loop
guard, bypass, exception or resume path cannot establish validity. Fusion must
preserve exhaustion-payload lifetime across intervening effects; preserving the
effect instruction alone is not enough. Invalid candidates remain materialized.

Luau coroutine wrappers retire their exact execution-context lookup on terminal
completion, failure or explicit close through the shared frame authority.
Weak indexing handles abandonment; terminal retirement does not depend on GC
pressure, collection timing or eventual disappearance of retained wrappers.

Callable capability requirements are independent of exact callable identity.
Live global loads union the requirements of possible imported provenance and
builtin fallback, without substituting a callable or stamping an exact runtime
symbol. Unknown attribute receivers consume the generated requirement union for
both protected callables and reflection-gateway suffixes. Constructionally exact
nonmodule receivers may suppress that generic acquisition requirement, but a
lexical class name or callback-invalidated result is not such a proof. The
operation registry owns these masks; frontend consumers do not mirror the names.
Captured cells are mutable activation inputs across functions, generators,
coroutines and lambdas. Their enclosing import origin contributes only possible
requirements at entry, never exact module/callable identity. An import or write
executed in the current activation may establish fresh binding facts; copying an
enclosing import map cannot override canonical binding-flow widening.

Generated arbitrary-heap effects are independent of local memory effects.
`LOAD_VAR` is a slot read and `BINDING_ALIAS` retains an existing value without
releasing an owner, so neither alone invalidates class lifetime. `STORE_VAR`
remains a boundary because slot-backed stores can release a displaced value.
These facts live in the existing operation registry; consumers must not keep
their own opcode exceptions.

Lexical cells have one mutable object representation, distinct from sequences.
The shared `cell_new/get/set` runtime primitives own the retained value and empty
sentinel; compiler local/free-variable guards own their contextual exception.
Cell replacement publishes the retained new value before releasing the old one,
so a finalizer observes the new value and may reenter the same cell. Public
`cell_contents` operations use that storage, with `ValueError` for an empty read;
list protocols are not a cell API. Function reconstruction preserves the supplied
closure tuple and cell identities, and code replacement validates cardinality
before publishing any new state. Executable closure calling convention is an
explicit function-layout fact, not a test for nonzero closure storage: a supplied
empty tuple remains visible as `__closure__` without adding a hidden argument.
Code identity preserves physical call provenance: positional, lexical-cell
context, or opaque runtime context. Compiler closures and internal runtime
contexts both pass a hidden argument but are not reconstructible in the same
way. `FunctionType` and `__code__` replacement reject opaque-context code before
publication; an empty `co_freevars` tuple cannot certify a positional ABI.
Code clones preserve provenance. Dispatch, binding caches and direct-call
admission consume the published scalar, without inspecting the closure tuple.
Valid `__code__` replacement changes the executable entry, signature and task
policy together while retaining the function's globals, builtins, defaults and
closure cells. Constructor attachment checks identity; executable replacement
is a distinct validated publication, not attachment of a mismatched code edge.

The shared packed callable metadata appends ordered free-variable and cell-variable
name tuples after execution kind. Code objects own those immutable tuples;
capture ordering, executable operands and introspection must agree. Code clones
retain this metadata, and GC traverses/drops its object edges through the code
layout authority. Native and WASM consume the same runtime implementation.
Source backends without actual cell storage and closure transport reject the
generated `LEXICAL_CELLS` requirement before emission; a plain function wrapper
is not a closure implementation.

The JavaScript WASM import fixture also refuses lexical cells; closure behavior
must execute against the linked runtime, never a host-emulated list payload.
LLVM direct runtime calls and preserved operations share generated object-value
ABI facts, argument boxing and result handling. Machine `i64` carriers alone do
not distinguish objects from raw integers, addresses or opaque handles. The
runtime manifest owns representation contracts; machine declarations and
dedicated raw/mixed lowering do not confer generic boxed-call eligibility.
Descriptor construction (`classmethod_new`, `staticmethod_new`, `property_new`,
and `bound_method_new`) uses that shared boxed route, including exact arity,
selected-runtime symbol availability, typed argument materialization and owned
result transfer/release. There is no separate descriptor signature/lowering table.
Direct and preserved boxed calls share selected-runtime availability admission
before symbol declaration or argument materialization; a generated ABI fact does
not prove the symbol exists in the selected runtime profile.
Boxed runtime calls borrow arguments: temporary owners minted when boxing raw
integers are released after the call independently of result ownership, while
already-boxed operand owners remain with their original SSA values.
Class allocation publishes initialized storage before exposing an owned result;
generator locals registration preserves raw function addresses alongside boxed
metadata. Borrowed closure edges acquire a reference only when retained as an IR
result, while discarded owned results use the shared release primitive.
Unknown symbols, wrong arities, and unresolved internal targets must not acquire
invented signatures.

Immediate builtin method calls and first-class bound-method acquisition consume
the same source-point exact receiver kind. A known Python method/property target
or receiver-layout guard does not certify its annotated return value. Only an
actual inlined/proven result or explicit result guard can authorize its lane.

Reference release composes two independent proofs: a retained identity root or
a recursively safe result shape can each exclude finalizer/weakref callbacks.
Coarse `OTHER` identity never overrides precise callback-free result facts.
Likewise `INERT_VALUE` describes a value, not a retained lifetime root; only
the result's release facts can establish safety for unrooted values. Cached
publication-release stability follows the shared result DAG without expanding
repeated descendants, participates in semantic equality and joins, and survives
idempotent publication after descendant shape is erased. Immutable owners retain
their own truth and length even when mutable descendants lose content facts.
Unknown identity and unknown result remain conservatively callback-bearing.

Retained operands cross callback boundaries in evaluation order; cleanup uses
the published result, not a stale pre-callback shape. Loop and eager-comprehension
cleanup includes reachable body effects. Generator creation retains its first
iterator rather than finalizing it. `PythonIterationFact` owns yielded result
facts as well as finite string alternatives, so target replacement and backedges
use result lifetime proofs. Unknown expansions invalidate finite alternatives;
mutable iteration exposure invalidates unstable element facts.

Binding joins preserve absence in the identity mask without treating it as an
unknown normal object. Only bound alternatives contribute value/result facts;
a bound unknown value still widens them. Name lookup then applies its real
protocol: an unbound lexical cell raises, whereas module/class misses can load
a different value from the fallback namespace. Fallback results must join before
any exact-kind, truth, or lifetime fact reaches lowering.

Result contents retain shared sequence-expansion provenance; constructors do
not flatten a compact fact graph into its potentially exponential runtime
cardinality. Semantic hashing, equality, publication stability, membership,
iteration alternatives, and key-effect analysis must preserve this sharing.
Exact iteration protocol does not imply callback-free element hashing or
comparison: tuple and frozenset keys include recursive collision effects,
while an exact bytearray key is unhashable even though its elements are inert.

This contract does not prove imported dataclass/statistics identities, mutable
container element facts, or target execution parity; those need their own
runtime identity/ownership and native/WASM receipts.

---

## 0. Terms

- **Pattern:** A recognizable AST/IR shape (e.g., `list(range(n))`).
- **Lowering:** Converting a Pattern into a smaller set of IR primitives (loops, alloc, calls).
- **Tier:** Optimization/semantic level (Tier 0 static, Tier 1 guarded, Tier 2 dynamic).
- **Guard:** Runtime check that enables a fast path (Tier 1).
- **Deopt:** Transfer of control from fast path to dynamic semantics when a guard fails.

---

## 1. Canonical IR primitives
These are the *only* primitives idiom lowerings may emit (v0.1). Everything else should be expressed in terms of these.

### 1.1 Control
- `Loop(counted)` — counted loop with canonical induction var `i: i64`
- `If(cond)` — branch
- `Break/Continue`
- `Trap(reason)` — compilation/runtime trap with reason

### 1.2 Memory / containers
- `AllocVec(len, dtype)` — allocate contiguous vector
- `VecSet(vec, idx, value)`
- `VecGet(vec, idx)`
- `AllocDict(capacity_hint)`
- `DictSet(dict, key, value)`
- `DictGet(dict, key)`
- `AllocTuple(len)`
- `TupleSet(tuple, idx, value)`

### 1.3 Arithmetic / comparisons
- `Add/Sub/Mul/Div/Mod`
- `CmpEq/Lt/Le/Gt/Ge`
- `And/Or/Not`

### 1.4 Calls / effects
- `CallKnown(fn_id, args...)` — direct call to known function
- `CallDyn(obj, args...)` — dynamic call (Tier 2 only)
- `Raise(exc)` — raise exception
- `Return(value)`

### 1.5 Range primitive
- `RangeTriplet(start, stop, step)` — normalized triplet
- `RangeLen(triplet)` — computed length (may guard for overflow)
- `RangeAt(triplet, i)` — ith value

---

## 2. Lowering cookbook (idioms)

Each rule includes:
- **Pattern**
- **Tier eligibility**
- **Lowering steps**
- **Correctness notes**
- **Tests**

### 2.1 `for i in range(n): body`
**Pattern:** `For(target=i, iter=Call(range,[n]), body=...)`
**Tier:** 0/1

**Lowering:**
1. Normalize: `trip = RangeTriplet(0, n, 1)`
2. Length: `len = RangeLen(trip)`
3. Emit counted loop `Loop(i=0..len-1)`
4. Inside loop: `i_val = RangeAt(trip, i)`
5. Bind `i := i_val`
6. Lower `body`

**Correctness notes:**
- Python `range` supports negative steps; must normalize.
- `RangeLen` must match Python semantics (empty ranges allowed).
- Overflow behavior: in Tier 0 require `n` fits i64 or raise compile-time error; Tier 1 guard.

**Tests:**
- `n=0,1,10`
- `n<0` (range empty)
- `n` near i64 boundary (Tier 1 guard fail → deopt)

---

### 2.2 `list(range(a,b,s))`
**Pattern:** `Call(list, [Call(range,[a,b,s])])`
**Tier:** 0/1

**Lowering:**
1. Normalize: `trip = RangeTriplet(a,b,s)`
2. `len = RangeLen(trip)`
3. `vec = AllocVec(len, i64)` (dtype may generalize; start with i64)
4. Emit `Loop(i=0..len-1)`:
   - `v = RangeAt(trip, i)`
   - `VecSet(vec, i, v)`
5. Return `vec`

**Correctness notes:**
- In Python, list of range yields ints; Molt may use i64.
- If `a/b/s` not statically known, Tier 1 emits guards:
  - args are ints
  - step != 0
  - length computation does not overflow

**Tests:**
- `list(range(5)) == [0,1,2,3,4]`
- `list(range(5,0,-2)) == [5,3,1]`
- `step=0` raises `ValueError` (must match)

---

### 2.3 `tuple(range(...))`
Same as `list(range(...))`, but:
- allocate tuple
- set via `TupleSet`
- tuple is immutable after construction

---

### 2.4 List comprehension over range
**Pattern:** `[EXPR(x) for x in range(...)]`
**Tier:** 0/1

**Lowering:**
1. Lower range to `trip/len`
2. `vec=AllocVec(len, <dtype_of_expr or Any>)`
3. Counted loop:
   - bind `x`
   - lower `EXPR(x)` into `v`
   - `VecSet(vec,i,v)`

**Notes:**
- If `EXPR` may throw, exceptions propagate normally.
- If dtype unknown, start with `Any` or specialize later.

**Tests:**
- simple arithmetic
- function call inside expr (Tier 1 guard if `f` is known)

---

### 2.5 `sum(range(...))`
**Pattern:** `Call(sum,[Call(range,...)])`
**Tier:** 0/1

**Lowering options:**
- **Analytic** (preferred): if step constant, use arithmetic series:
  - `len = RangeLen(trip)`
  - `first = RangeAt(trip,0)` if len>0 else 0
  - `last = RangeAt(trip,len-1)` if len>0 else 0
  - `sum = len*(first+last)/2` (careful with overflow)
- **Loop** fallback: accumulate in counted loop

**Guards:**
- overflow checks → deopt
- numeric type must be int

**Tests:**
- compare with CPython for random ranges
- huge ranges to force overflow guard/deopt

---

### 2.6 `any(iterable)` / `all(iterable)`
**Tier:** 0/1 if iterable is recognized (range, list, tuple) and predicate is implicit truthiness.

**Lowering:**
- Short-circuit loop with truthiness checks.
- For `range`, exploit emptiness quickly.

**Notes:**
- If iterable is generator or side-effectful, Tier 2 only.

---

### 2.7 `enumerate(range(...))`
**Pattern:** `for (i,x) in enumerate(range(...))`
**Tier:** 0/1

**Lowering:**
- use one counted loop
- `i` is induction var
- `x = RangeAt(trip,i)`

---

### 2.8 `zip(range(a), range(b))`
**Tier:** 0/1 if both are ranges and no `strict=True` behavior required (Python 3.10+ has strict in itertools, but built-in zip has no strict).

**Lowering:**
- compute `len = min(lenA,lenB)`
- one counted loop
- compute each `RangeAt`

---

## 3. Lowering constraints

### 3.1 No hidden allocations
If a lowering allocates, it MUST be explicit in IR (`AllocVec`, etc.).

### 3.2 No dynamic calls in Tier 0/1
Tier 0/1 lowerings cannot emit `CallDyn` except inside a deopt block.

### 3.3 Deterministic lowering
Given the same AST and Tier configuration, lowering must be deterministic.
No heuristic-only transforms without a controlling flag.

---

## 4. Validation and testing

### 4.1 Golden tests
For each idiom, maintain:
- input source snippet
- expected IR (pretty-printed)
- expected runtime behavior vs CPython oracle (where applicable)

### 4.2 Differential oracle
For Tier 2 fallbacks and deopt exits, use CPython as the oracle for:
- result
- raised exception type/message class (message may be loose)
- side effects ordering (within supported subset)

---

## 5. Extension points (how to add a new idiom)
A new idiom requires:
1. Pattern match spec (AST + type facts used)
2. Tier eligibility
3. Lowering steps to IR primitives
4. Guard/deopt plan
5. Test suite additions
