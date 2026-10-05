# Molt Lowering Rules

**Status:** Canonical (compiler-facing)
**Purpose:** Define deterministic, testable transforms from Python AST → Molt IR for supported idioms.
**Audience:** Compiler engineers, optimization authors writing compiler passes.

## Imported callable code identity

Import visibility and code-symbol custody are distinct. A guarded Python call
may name a `(module, function)` code address only when the module belongs to the
compiled partition and canonical source analysis declares that function. Named
imports, module-attribute calls and callable type hints share this rule. Public
API spellings, re-export lists and missing analysis cannot establish a symbol.

Runtime-published builtins, re-exports and other callable values use their actual
imported binding. Even with a proven code address, guarded dispatch retains that
live value; the shared runtime guard owns callable identity, closure and binding
eligibility. Defaults, positional/keyword arguments and decorator replacements
do not require per-function frontend binding lists. Native exports still require
their declared callable ABI metadata. This rule is independent of target/backend
and does not broaden import admission or the verified subset.

## Runtime protocol and exception identity

An `await` operand is an ordinary Python expression. Its callable, argument
order and keyword binding follow the same live binding authority as other calls;
spelling a callee `anext` does not authorize a separate lowering path. Async
iterator acquisition uses the async type protocol even when a value carries a
list, generator or iterator hint. Only an explicitly prepared iterator binding
can bypass acquisition.

Compiler-owned termination checks use canonical builtin exception tags and
inheritance. Source `except` expressions use their evaluated class values; every
entry in a flat handler tuple is validated before matching, without metaclass
instance/subclass hooks. `except*` shares that admission rule and additionally
rejects exception-group classes. Exception names and formatted messages remain
diagnostics, not classification inputs. Import execution propagates the original
exception object, including notes, context and custom attributes.

Async special methods bypass instance attributes and `__getattribute__`.
Iterator admission observes the `__anext__` type slot without binding its
descriptor. Descriptor failures follow the configured CPython version: 3.12 and
3.13 async slot wrappers replace lookup failures with `AttributeError`; 3.14
preserves the original error. This version rule belongs to the shared runtime
lookup authority, not individual frontend consumers.

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

Source `return`, `break` and `continue` publish block termination after their
cleanup emitters finish. A cleanup's exceptional continuation does not reopen
ordinary fallthrough after that transfer. Scoped block visitors return their
own completion while restoring the enclosing flag; try emitters consume that
result instead of rereading the enclosing state.

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
Suspension resumes inherit custody only from reachable saved-state producers.
An absent state after the resume fixpoint is unreachable, not a depth-zero
entry; producer, lexical-handler, release and pop-owner queries all use this
same rule. Reachable unresolved saved states are explicit analysis errors.
LLVM emits only explicit exception-stack operations, not implicit frames for
TRY markers. WASM dispatch uses pending-state checks; structured non-relocatable
functions may use native EH for local handler control. Python-tag exceptions
return through the shared activation epilogue as boxed None with the original
runtime exception pending; they never unwind through caller invocation, recursion,
reference-count or Rust runtime guards. Native EH does not replace runtime
exception push/pop state or pending-error checks. Foreign exceptions retain their
identity when propagated through activation cleanup. Ordinary return, suspension
and failed construction share one boxed-result epilogue, so anchor cleanup grows
with the number of anchors rather than anchors times exits. Statically known
forward WASM dispatch edges use their enclosing block labels directly, including
pending-error checks and ready await continuations. The operation-to-block map
also owns saved-state lookup; only backward or dynamic resume edges reenter the
dispatcher. An ordinary implicit function exit uses the shared activation
epilogue; stateful fallthrough is rejected because suspension protocols require
an explicit terminal return. Unknown saved states, out-of-range operation
indices and invalid dispatch-table entries trap instead of entering unrelated
code or redispatching indefinitely.
Luau
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
Explicit exception transfers end their source basic block. Their canonical CFG
edge relation includes self-transfers: liveness, dominance, SSA argument
placement and iterator validity must observe the same re-entry, with edge
arguments captured at the transfer operation rather than a later definition.
CFG construction gives a re-entrant first source block a distinct invocation
predecessor. Declared parameters remain the invocation ABI in source order;
internal loop-carried values, including initially undefined locals, become
ordinary join arguments instead of additional or reordered function parameters.
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

SimpleIR read analysis and in-place source rewriting share the generated
field-role walk in `molt-ir::tir::simple_def_use`. For `copy_var` and `load_var`,
nonempty `args` supply the source and `var` is metadata; absent or empty `args`
use `var`. Binding destinations, positional results and trailing unpack results
are never rewritten as reads, even when their names collide. Native slot/PHI
planning and source-backend declarations consume these same read/definition
roles. Alias elimination requires exactly one source and immutable names;
mutable bindings and repeated definitions retain their load points. Object
identity alone does not authorize erasing guards or owned-reference operations.

Scalar store-target sets are projections of the completed representation facts
and canonical all-source binding edges, not a second opcode-driven fixed point.
Checked arithmetic's value and overflow results retain independent types through
result-carrying bindings. A semantic integer fact does not prove a bounded raw
carrier. Function-wide integer constants use generated opcode membership and
require one unambiguous producer; parameter names, repeated definitions and
rebinding cannot inherit a first-definition constant. Native literal dispatch,
preanalysis and scalarized tuple indexing share those authorities rather than
maintaining a backward constant scanner.
Field-role knowledge alone does not admit an operation or a raw carrier:
unsupported binding spellings retain their backend rejection, and only
registered checked-arithmetic operations authorize value/overflow result pairs.

Rust-source labelled flow preserves the canonical separation between ABI
parameter names and mutable local storage. Storage declarations live outside
dispatch arms, parameter-backed slots start with their incoming values, and the
existing source-backend writeback protocol uses the canonical slot mapping.

Native list-buffer caching consumes the same typed operation effects and
executable CFG dominance facts. Ordinary and indexed loops share one preheader
producer for scope publication and storage/layout loads. Nested loop effects
count; indexed preludes do not create a second loop. Unknown or arbitrary-heap
effects fence all cached buffers, even for calls without list arguments, and
rebinding a list invalidates
its cached identity. Ordinary observations are local to their defining native
block; cross-block reuse requires an explicitly certified loop lifetime and
expires at its boundary. Map membership alone proves neither initialization
on a sibling path nor validity after an intervening mutation. A lexical
definition before a loop is not evidence that it dominates a resumable entry.
Implicit owner releases can invoke finalizers: cache consumers check cleanup
emission custody, including releases within an operation, and loop certification
accounts for backedge cleanup. Conditional Boolean shadows snapshot the loaded
value and its storage tag in their defining block, rather than consulting a
subsequently mutable list layout. Fast and fallback indexing must transport the
same shadow representation, including negative indices into Boolean lists:
boxed `False` is not a raw truth bit. There is no backend mutation-name whitelist or
separate alias graph supplying a weaker safety rule.

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

Source binding invalidation removes value and specialization facts, never the
lexical storage owner. Name, callee and attribute-receiver evaluation retain
their local/cell loads and unbound guards after callbacks; only names whose
canonical binding fact selects global lookup re-read the module namespace.

Frontend consumer witnesses follow the value through its semantic owner.
Assignment-expression capture precedes `FRAME_HOME_STORE`, which publishes the
binding and releases its displaced owner; `STORE_VAR` transports only the
borrowed view. Code-name tables follow lexical compiler visitation before dead
branch removal, while executable imports and statements follow reachability.
Fused dictionary increments consume their kernel's boolean admission result and
run the original statement only on decline. Published module calls retain the
loaded callable before argument effects and select positional or CallArgs
dispatch from actual syntax; import spelling does not select an FFI lane.
Opcode serialization fixtures remain distinct from these producer/consumer
checks, so a valid wire opcode is never evidence that a source pattern must emit
it. `test_frontend_ir_alias_ops.py` checks these ownership, ordering and operand
links; code metadata uses an independent CPython reference.

Code-name projection receives the target Python version and future-annotation
mode separately. Eager namespace annotations visit their annotation expression;
future-string annotations do not. Both record `__annotations__` at a simple
named annotation, even in a dead branch; parenthesized names, attributes and
subscripts do not write that dictionary. Python 3.14 module metadata prepends
`__conditional_annotations__` when its lexical body contains an annotated
assignment and records deferred `__annotate__` publication after the body when a
simple annotation requires it. These are name-table facts, not permission to
execute dead statements. Function-local annotations contribute only evaluated
target/value operations. Nested class and function bodies remain separate
lexical regions; inline class annotation storage keeps its existing namespace
owner. Module/callable metadata oracles do not claim standalone class code-object
support.

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

Persistent binding storage carries ordinary and preserve-owner mutation frontiers
derived only from `expression_result_without_mutable_contents`, including both
per-member and homogeneous mutable descendants. Radix branches summarize those
chunk masks; mutation visits candidate storage rather than sweeping all bindings.
These are raw-result susceptibility facts, not cached cleanliness: namespace taint
domains can grow after an older state was interned, so epoch/domain cleanliness
is resolved at the point of use. Raw stored owner tokens are likewise distinct
from public unknown ownership on dirty bindings.

Point writes, multi-slot writes, tainting, and mutable-content expiry use one
batch publisher. Each changed chunk and affected existing radix ancestor is
copied at most once per batch; joins construct chunks through the same frontier
authority. Ordered write-history payloads remain independent of structural
sharing, including same-shaped stores and owner-only transitions.
`slot_updated_between` retains those events; semantic fixpoint equality never
substitutes for owner-custody comparison.
Default multi-slot batches compare each input with the preceding staged write
to that slot, so an A-to-B-to-A sequence preserves both effective writes while
publishing A. Explicit `record_writes` also retains same-shaped writes. This
staging exists only within one publication call, never across taint-domain growth.

Exceptional observations retain their complete, ordered lexical histories.
Their incremental joins fold only newly observed distinct states, in original
state order, and restart only when an earlier original state is observed out of
order. Joined raw custody is domain-independent: the clean bit records conjunction
of outside-domain cleanliness, and the clean epoch is the joined namespace epoch
only if every parent would be clean inside the domain, otherwise -1. Parent/child
namespace epochs are nonnegative and monotone. Shared storage and re-materialized
storage therefore project identically after any later domain admission, including
nested joins and point transfers. No domain-dependent join key or fold rebuild is
needed. Default writes refresh stale namespace custody even when their current
outside-domain payload is unchanged; staged duplicate writes remain no-ops.
Domain-dependent lexical-history summaries refresh their event and initial
projection caches on domain growth or appended history before reuse. Equal
environments never authorize returning an existing state in place of a join; all
ancestor writes and same-shaped/owner-only transitions survive.

Storage diffs deliberately skip shared storage, even across different epochs.
Semantic equality, join-history fallback and ownership comparison instead share
one changed-slot frontier: all structurally changed slots, plus only live-domain
slots in shared storage when epochs differ. Shared radix subtrees outside that
frontier are skipped without projecting their bindings. Absent live-domain slots
share one canonical empty-slot projection per epoch; they are still emitted
individually when that projection changes. Domain masks must be nonnegative;
invalid masks are rejected before mutation or traversal. Closure history cannot depend on whether
equivalent chunks happen to share an allocation. Deferred module activation
uses full module history or a nonempty explicit activation tuple; no synthetic
module-exit join or empty-tuple fallback supplies another authority.

Static binding facts use canonical typed literal identity (plus their existing
symbolic parameter/string-alternative identities), including in state interning,
publication, expression facts, joins and projection comparisons. Python numeric
equality must not conflate boolean and integer facts. Result joins use the same
semantic-key authority as result construction, preserve normal-only absence
semantics, and absorb an equal existing result in deterministic input order.
NaN conservatism, signed-zero bits, graph edges, release stability and source
write history are not weakened by payload or custody fast paths.

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
a reason to reject the surrounding string. `str_predicate_protocol.py`
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
creating scope and transported in the Python-visible `.0` frame slot. Both
synchronous and asynchronous loop lowering consume that acquired value without
re-evaluating the outer expression or borrowing its source-position binding
facts for a fabricated name. The original lexical regions own capture: only
the deferred body uses foreign activation inputs. Inline
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

Every canonical control arc carries custody. A terminator arc binds its target's
block arguments, and a `CheckException` binds its operands to its handler's
arguments when it raises. A `TryStart` registers its region: it keeps the
handler reachable and binds nothing. An owned root that the target does
not read moves into its first argument; any other owned binding is retained on
its arc, by a check's landing on the exceptional path only. After a move, an
explicit release or an operation that adopts its own `+1`, the root's name owns
nothing until its definition runs again, and an argument that nothing reads is
released on entry. A Python-bound local, a transferred parameter and an
explicitly released root keep their objects to a Python boundary. A block
argument that takes one of them keeps the same boundary
through its transparent aliases and intervening blocks, so its last textual use
does not end the owner's lifetime.

A source Python call instruction adopts each operand whose typed custody is
`Transferred`, and a bind adopts the CallArgs builder it frees, on both
continuations. The holder moves a dead, non-lexical owner's own `+1` into the
first adopted position that names it and retains one right before the call for
every other position (`drop_insertion/transfers.rs`). The plan and last-use
releases read one last-read projection. A raw carrier holds no reference there.

Exceptional liveness enters at the observing operation, separately from normal
terminator demand. After ordinary lifetime releases are placed, an exception
edge releases available owned roots whose normal continuation was abandoned,
while preserving handler live-ins and transferred payloads, and retains each
payload that its handler argument cannot take by move. Cleanup operands
travel as explicit block arguments; a definition below the check cannot supply
them. Identical cleanup states share a landing block. ExceptionRegions carries
pending-error custody through that block into the original handler. Lowered
state-machine activations expose suspension as ordinary Return exits before
this analysis. Frame stores/loads own persistence; invocation-local references
use the same release and transfer rules on completion, suspension and error.
The saved-state dispatch map is transported explicitly, independently of label
numbers and physical block order. Backends do not synthesize suspension returns
or a second activation ownership policy.

Release placement names an owned root only where one authority,
`drop_insertion/availability.rs`, finds it available. The root's definition
must reach the point on every path, including an exception edge that leaves a
block before the definition. A conditionally-valid result must also lie inside
the region its producer initializes. The root's name must still own its object:
no path to the point may pass a move, an explicit release or an adoption of
it, unless its definition ran again afterwards. Straight-line observations,
entry and arc releases, phi retains and lexical boundaries ask this query.
Landings ask it without custody, because final liveness already excludes a root
that gave up its object. A Return or join may be entered by arcs that carry a
root and by arcs that do not, like the exit that `raise; jump exit` shares with
every check. Such a block does not release the root itself. The release moves
to the normal arcs where custody ends, and landings release it on the
exceptional entries that still have it. Landing labels are fresh in the whole
exception-label namespace. CPython frame and traceback lifetime is not modeled
for named locals on exceptional paths, or where lowering emits no scope-exit
cleanup (module code, closures, boxed and async locals). There a release may
precede a handler or the exit's statements, and landings follow reverse
creation order.

An absent result binding does not erase an operation's effects. BoxVal/UnboxVal
retain their canonical operation identity across SimpleIR, TIR and LIR;
full-width integer boxing still materializes and retires its temporary owner
when discarded, preserving allocation failure. Pure representation extraction
does not create an owner for an absent result. Consumers must distinguish
semantic result arity from the presence of an observable result binding.
Rust-source and Luau use their existing value carriers for the same conversions;
their shared representation handlers preserve reserved `none`, omit discarded
bindings, and reject malformed operand counts even when no result is bound.

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
`simple_ir_binding` is the borrowed destination/result view shared by def/use,
allocation and backend emission. A `none` output is discarded, not a second
destination. Raw native, WASM and source-backend stores preserve the incoming
snapshot independently of later binding changes; SSA/LLVM consumers use the
same single semantic result. Backend-local `out.or(var)` result classifiers
must not reinterpret this contract.
In WASM frames the reserved `none` operand reads the initialized constant-cache
singleton; discarded result fields write only to the typed dead-result sink.
Result-slot selection must not use the operand lookup. Owned runtime results
are released when their name is absent, `none`, or allocated to a dead-result
sink, rather than published. Discarded internal calls cannot become tail
returns of their unobserved result. Physical-slot constant facts are invalidated
by every canonical definition before emitter dispatch and at generated control
boundaries; early-returning handlers cannot preserve stale facts across writes.
Frame planning consumes the same semantic read/definition visitors as liveness:
iterator results carried by `var` and unpack results carried by trailing `args`
are not reads merely because of their wire field. Unknown-operation transport
metadata becomes an SSA read only when it resolves to an actual SSA name.

WASM temporary-slot reuse spans the first definition through the last access,
including later dead writes. Ranges intersecting repeated execution widen to
the enclosing iteration regions derived from generated control facts; resumable
state-machine functions retain distinct locals. Merge iteration regions once
and locate intersections by binary search. Allocate the lowest available slot
deterministically using expiry and free-slot heaps, rather than repeatedly
scanning all live slots. Region metadata alone is not a backward transfer.

Numeric op-loop emitters carry the selected typed runtime import through to the
shared result sink, including guarded inline branches and in-place variants.
The generated numeric selector table drives execution coverage for live,
absent, sentinel and dead results; no separate numeric ownership classifier or
untyped call-and-store helper owns this boundary.

Runtime import return custody is generated from the shared boxed-call ABI and
explicit non-boxed import declarations in `wasm_abi_manifest.toml`. An `i64`
carrier alone does not imply an object or a borrowed result. WASM direct,
generated and manual sinks consume `WasmRuntimeReturn`; native and LLVM boxed
calls consume the same owned/borrowed/poll facts. A bound borrowed return is
retained before releasing temporary arguments; discarding it releases no owner.
Poll returns own ready values, while the pending immediate is not a heap pointer.
Poll-table membership establishes that protocol for scheduler callbacks; direct
channel/network calls declare the same protocol without acquiring a table slot.
Raw status flags, invocation-depth tokens and GPU primitive handles are excluded
from Python-callable admission even when their machine signatures use i64.
Object-facing send wrappers box ready success at that boundary and preserve
Pending and raised errors; the underlying raw transport ABI is unchanged. Its
signed zero exception sentinel is not success while an exception is pending:
conversion checks the current error state before boxing a ready zero.
Unpublished allocations, sized scratch storage and execution tokens require
their specific publication/free/leave protocol, never the generic object sink.
Actual emitted reference operations determine import roots.

Public channel, stream and WebSocket-pair constructors share boxed integer
capacity admission, including the index protocol, negative rejection and checked
target-width conversion. Host adapters box capacities through the runtime and
retire the borrowed argument's temporary owner. Raw transport entrypoints retain
their explicitly raw ABI. Unpublished opaque stream/WebSocket handles use their
resource-specific drop operation, including rollback after tuple allocation or
host argument-cleanup failure; generic object decref cannot release them.
The generated host-export and final-output essential-export policies retain
that complete host stream protocol even without direct guest stream imports.
This retains stream-core reachability, not optional network/crypto features.

Multi-call constructors preserve their owner through initialization and release
unobserved results afterward. Dict/set/frozenset insertion results never replace
the container owner: failed allocation skips entries, and a failed insertion
releases the partial container and temporary scalar boxes, preserves the pending
exception, and skips later boxing/hash/equality calls. Native, WASM and LLVM
construction follow this same transaction. Native and LLVM entries materialize
lazily in entry order: a source repeated in a later entry reuses its first box, and minted
boxes stay owned until construction ends, so a shared box is never released
while a later entry borrows it. Fixed boxed LLVM container operations use the shared runtime
call emitter; specialized builders and out-buffer protocols retain explicit
custody for their additional resources.
Fixed-arity lists, tuples and dataclass field values use canonical runtime constructors
that borrow a contiguous word range (`molt_list_from_values`, `molt_tuple_from_values`,
`molt_dataclass_new_from_values`). The constructor refuses to allocate while an
exception is pending, copies and retains every word in one constructor transaction, and
returns None only with an exception pending, so no builder owner, per-element
failure branch or partial publication exists. Native Cranelift passes a stack
slot and LLVM one static entry-block slot (as for unpack, class-definition and
dataclass ranges), so construction in a loop never grows the stack. WASM passes
a scratch allocation private to that one construction and freed before the next
operation; no static buffer is shared with a reentrant callback, another
activation or another thread. Dynamic streaming construction retains the list
builder protocol. Native materialization uses one operation-scoped operand
transaction over the shared representation plan to box each input once,
preserve repeated operand identity, release only minted full-i64 boxes, and
skip all dependent work after failure. Such temporary boxes
never enter scalarized tuple aliases. Borrowed scalarized tuple views are
published as None on construction failure, so releasing source owners cannot
leave a later projection holding a freed element. Slice construction passes its
three bounds to `molt_slice_new` as direct arguments, with no range storage; an
omitted bound is None. Native slices use the same materialization transaction,
as do subscript and dict/set operations, including views and fused counting
operations. Borrowed returns acquire any escaping credit before operand cleanup;
owned discarded returns are released. Proven raw-index list lanes retain their
raw access, and a failed checked read does not materialize a result object.
Boxed results enter scalar homes only through the shared carrier boundary.
List, tuple, dict,
set and slice construction are throwing operations in the canonical effect
table: allocation can fail, and hashing/equality can raise. Their results retain
every operand, so an operand's finalizer sensitivity extends to the constructed
object. Frontend exception edges and optimizer
check retention consume that same fact; construction failure must transfer to
the handler before any dependent Python operation.
Native and WASM scalar parsing use the same object parser for literal and dynamic inputs.
The runtime alone owns input-type admission and exceptions; lowering must not
reparse raw literal bytes through a second, name-based path. This removes the
redundant out-buffer allocation and its ownership/exception transitions.

`visit_simple_ir_results` retains the positional `Var`, `Out`, or trailing
argument role even when a result has no retained name. Def/use consumers filter
that view to live names; SSA allocation and multi-result emitters must not pack
live names and infer their roles from the shortened sequence. Discarding a checked
arithmetic value or iterator value does not turn the overflow/done flag into the
first result. Unpack admission still requires all declared results to be named.
Single-result sinks use the ordinary-output projection of that same visitor.
An effect's output metadata cannot publish or retain its runtime return, even
when its spelling matches a live operand or binding.

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

Frame entry is a checked lifecycle operation in typed frontend IR, not a late
serialization prepend. `trace_enter_slot` is followed by its `check_exception`
before exception-stack baselines, locals mutation, metadata or body execution.
Its failure path owns only the entry attempt: it balances `trace_exit` without
reading state that would have been initialized after successful entry. Module
entry additionally preserves publication rollback; see the import contract.

Backend scheduling must preserve this edge. Native heap-literal preparation
initializes every cleanup anchor to boxed `None` before entry, but fallible
literal constructors follow the leading entry check's success continuation.
Early failure therefore releases only initialized anchors and consumes exactly
one frame attempt. An IR-positioned module entry cannot be hoisted ahead of the
code/global binding that it requires. Partitioning must keep the checked entry
and its failure cleanup with the frame owner; inherited chunks neither enter
nor exit that frame.
Chunk budgeting counts complete statements, including the terminal statement,
and retains the last legal cut before extending a chunk past its target. SSA
transport prefixes must not strand an oversized tail when a legal earlier cut
exists. Every selected cut reuses the same control-target and cleanup-live-in
admission; an indivisible region exceeding the hard limit refuses atomically.

SSA materializes missing reaching definitions only at surviving uses after edge
repair. One local `None` definition dominates the consuming block's operations
and outgoing edge arguments, including disconnected roots. An unused SSA
placeholder must not emit instructions before checked entry or escape into
published operands or type facts.
Executable exception transfers end their CFG blocks at the transfer program
point. Success-continuation definitions cannot reach an earlier failure edge;
SSA dominance, liveness and block-argument placement consume the same augmented
exception/resume graph rather than independently reconstructing its successors.
One source liveness solution seeds implicit-edge environments and pruned phi
placement; argument seeding does not recompute unchanged source defs/uses.

Counted-loop recognition follows ordered guard and body paths through executable
exception observations. The same descriptor owns recurrence facts, unroll cost,
operation cloning and region retirement; it is not a fixed three-block shape.
Unrolling preserves the final failed guard and refuses substitutions that would
change carried values observed on a side exit.

Integer range facts distinguish global value bounds from program-point bounds.
A loop IV's global hull includes its final failed-guard value; a true-guard body
hull is valid only where the continuing edge is proved. Pre-guard operations,
exception observers and bypass paths cannot inherit a body-only bound. Derived
values consume operand ranges at their defining site; bounds elimination and
raw arithmetic admission consume facts valid at the actual use. Speculation
must prove the destination site as well. This applies equally to bounds,
zero-divisor, shift-count and integer-representation safety on all backends.

The function's explicit `return_abi` owns the linkage result independently of
return payloads and inferred semantic types. Python functions, pollers, and
callable helpers have a value ABI even if optimization removes every value
exit. Void module/entry wrappers declare their convention at construction.
SimpleIR/TIR roundtrips and cache/extern projections preserve this fact without
synthetic signature instructions or a scan of optimized returns. Empty returns
in a value ABI lower to boxed `None` at the machine boundary, keeping
`trace_exit` adjacent to its return in the IR. A Python `None` type is not a void
calling convention. See `SIMPLE_IR_JSON_SCHEMA.md` for the transport contract.
WASM straight-line, jumpful and stateful SimpleIR paths share the return emitter;
dispatch selects control edges, not a separate payload or ownership convention.
In particular, an empty value-ABI return is boxed `None`, never raw integer zero.
An empty extern declaration is not a body proving a `None` result: its value
ABI has an unknown boxed semantic result, and lowering keeps the declaration
free of synthetic executable signature operations.

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
Every LLVM consumer of borrowed boxed runtime operands uses one operation-local
custody: fixed and hash constructors; boxed runtime, dynamic, method and
builtin calls; attribute, subscript, module, conversion, generator and await
operations; and direct compiled calls into boxed parameters. Each distinct SSA
operand is boxed once, at its first request; a failed box prevents all later
boxing and the consumer call and leaves the first exception pending. Static
owner slots are initialized before the first failure edge and retired on both
paths. Hash construction requests each entry only after the previous insertion
succeeded. Already-boxed operands remain borrowed from their original SSA
owners, and an operation whose operands cannot mint an owner adds no branch or
release. A bound borrowed return acquires its independent credit before
argument cleanup; discarded owned returns are released, while setters whose
every path returns the immortal None bind it without a release. Values a
consumer stores or returns (task payloads, yielded pairs, `and`/`or` selections)
take their own owner instead of a borrowed one. Raw ABI words (addresses,
lengths, tags, capacities) pass unboxed, and object-address parameters receive
unboxed pointers. A preserved kind whose runtime entry is exactly `molt_<kind>`
with a generated boxed row takes the shared admitted route, with no
spelling-based call path; this includes preserved `slice_new` and the CallArgs
push and expansion steps. LLVM-built CallArgs reserve every positional slot, so
their pushes cannot fail; a consuming bind frees its builder even when it
observes an exception left pending by an earlier step. Native fixed constructors, hash construction and
subscript read, write, delete and slice share one native operand transaction
with the same ownership invariant, including both borrowed ranges of a class
definition. Its owner slots precede the first failure edge; its join publishes
None on failure and carries internal CFG cleanup tracking to the final block.
Subscript lanes that read a proven list through a raw-int index box it only on
a cold path whose runtime call needs a boxed key; that path opens its own
transaction and rejoins before the lane merge. Discarded owned subscript
results are released, and statement returns borrowed from the container acquire
nothing. These
operation-local rules do not establish identity across separate boxing sites;
that requires the shared representation plan to preserve the source identity.
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

## Fused loops

A fused op runs items of a Python loop, or the statement
`d[k] = d.get(k, 0) + delta`, in a runtime kernel. It is an optimization of
the ordinary lowering, never a separate semantics: the frontend emits the
ordinary lowering after or beside the op, and the op leaves exactly the state
the code would leave after the items it ran, having run no Python code.

- **Admission is a runtime fact.** Source shapes and type hints only select a
  candidate. The kernel admits an item when, on the values the code actually
  reads, it provably runs no Python code: exact builtin containers, exact
  ints, bools and floats (no subclasses), `str` keys whose dict probe decides
  without Python equality, and a loop target whose previous value releases
  inertly (unbound, unboxed, or an exact `str`, `bytes`, `int` or `float`).
- **Long loops run as a chunked prefix.** A reduction (`vec_sum`, `vec_prod`,
  `vec_min`, `vec_max`) takes the loop's own iterator, acquired once where the
  loop acquires it, and consumes at most one bounded chunk per call (4096
  items, fewer once a big int total grows past 4096 bits); a counted
  `bytearray` fill writes at most 1 MiB per chunk. The calls run in a chunk
  loop whose back edge is an ordinary loop back edge, where the canonical
  eval-breaker observation (`async_work_poll`: pending calls, signals, GC
  finalizers) runs. Before that back edge the chunk's state is published: the
  loop target takes the last item and then the accumulator its result, the
  order in which the loop releases them, through the bindings' homes. The next
  chunk rereads both (and the fill its index and buffer) from those homes,
  since the serviced work may have rebound them. The chunk loop ends at the
  iterator's end or at the first item the kernel does not admit, which it
  leaves unconsumed; the ordinary loop then continues from that state on the
  same iterator or index, so it runs every remaining item, and no consumed
  item or callback is ever replayed. After the last chunk the ordinary loop
  finds the iterator exhausted (or the fill index at its bound) and ends,
  running an `else:` clause as the loop would.
- **Whole-loop kernels are bounded.** The split/count kernel runs a whole line
  of at most 4096 words or declines before any effect, after which the
  ordinary loop evaluates `line.split(sep)` itself; `dict_str_int_inc` runs
  one statement.
- **Results are the code's own.** Integers are exact at any size; float
  operations are the loop's IEEE operations in iteration order (no
  compensation, reassociation, closed forms that round differently, wrapping
  or saturation); int/float mixing follows the binary operators; min/max keep
  the source's strict comparison and the element object itself. A NaN meeting
  a NaN, or a big int meeting a float, is not admitted, since the operand
  order that picks the result or the error is not part of the op. No result
  depends on an earlier call, adaptive history or the environment.
- **Bindings are the code's own.** In a function body only (a module's loop
  target is a module global and a class body's a namespace entry, whose every
  store is observable), the loop target and accumulator are published after
  each nonempty chunk; a loop that runs no iteration leaves an unbound target
  unbound.
- **Moved reads are unobservable.** A fused op reads, before the code does,
  bindings the code reads later. Such a read uses the binding's current value
  (from 3.13 a callback may rebind a frame local through `f_locals`), and must
  not raise: the binding analysis proves the name bound, which for a fast
  local survives callbacks (they rebind but never delete it) and for a global,
  class-namespace or cell binding also needs a clean read. A key read that can
  raise (a dataclass field may be unset) happens only after an exact dict
  makes the source's earlier `d.get` lookup unobservable.
- **Results are owned by their contract.** A kernel's result tuple is a fresh
  owned object, as its runtime import's return contract states, and is
  classified `OwnedValue` so drop insertion releases it; `audit_op_kinds`
  category `owned_result_transparent_alias` rejects any new result-producing
  kind whose owned boxed return the classifier would alias instead.
- **Counted `for` over `range()`.** Only a call the binding analysis proves is
  builtin `range` counts. Its arguments are evaluated in order, then each
  given bound converts through `operator.index` (`operator_index`; start,
  stop, step, raising `range()`'s own TypeError), then the step is checked
  for zero. A bound the analysis proves an exact int converts to itself and a
  bool literal to its int, so the counted loop only ever sees exact ints of any
  size; a raw `i64` lane still needs a representation proof. A counted
  `while` guards its index the same way (an exact int start) before entering
  the counted lane.
- **Distinct operations stay distinct.** Builtin `sum()` is CPython's
  `builtin_sum_impl` for the target version: an int phase reading C `long`
  items into a `Py_ssize_t` total (the target's C data model: LP64 Linux and
  macOS, LLP64 Windows, ILP32 wasm32-wasi; `sys.maxsize` and
  `struct.calcsize('l')` report the same widths), a compensated float phase
  (3.14 also compensates the ints it meets there), from 3.14 a compensated
  complex phase, then generic `+`. It is not an explicit `+=` loop; inline
  `sum(<generator>)` keeps a running total only for items structurally proven
  exact ints and otherwise calls `sum()`. Builtin `min()`/`max()` compare each
  value to the best with one rich comparison (`value < best`,
  `value > best`); `sorted()` is `list.sort()` on the list its iterable
  makes, and `list.sort()` compares with `<` alone, asking the target
  version's comparisons in its order (CPython's timsort, whose run detection
  changed in 3.13, gh-116554).
  All of them hold their iterator and each item only as long as CPython does.
  Complex arithmetic with a real operand follows the target's rules (3.14:
  C99 mixed-mode arithmetic, gh-69639), as do complex products and quotients
  (Smith division; 3.14 recovers Annex G infinities and zeros).
- **Backend loop rewrites follow the same rule.** A backend may rewrite an
  admitted or ordinary loop (for example the native 4x unrolled `list[int]`
  sum) only with unchecked `i64` arithmetic that a value-range proof makes
  exact; a checked full-range carrier keeps its checked operations.

The ops are `vec_sum`, `vec_prod`, `vec_min`, `vec_max` (operands
`(it, acc, target)`, result `(result, last, count, more)`),
`string_split_ws_dict_inc`, `string_split_sep_dict_inc`, `dict_str_int_inc`,
the `operator_index` range-bound conversion and the counted
`while`/`bytearray` lanes (SimpleIR schema, "Fused Loops"). Targets that cannot
evaluate an admission check decline every fused op, consuming nothing.
Differential guests `vec_reduction_in_function.py`, `fused_loop_semantics.py`,
`range_bound_semantics.py` and `builtin_reductions.py` pin the observable
contract.

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
- The bounds are `range()`'s own conversions ("Fused loops": counted `for`
  over `range()`): exact ints of any size, converted once, in order, before
  the zero-step check. The index is a raw `i64` only where a value-range proof
  bounds it; otherwise it is a boxed exact int.

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
- In Python, list of range yields exact ints of any size; values outside the
  inline int window are boxed at full range, never truncated.
- The bounds are converted as for a counted `for`; the runtime range
  constructor then checks the step and computes the length exactly.

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

**Lowering:** builtin `sum()` over the range's iterator ("Fused loops",
distinct operations): its int phase adds C `long` values into a `Py_ssize_t`
total and leaves it exactly as CPython does, so the result's type and value
depend on the start and the target's C data model. An analytic closed form
would have to reproduce those phase transitions and any float start's
rounding; it is not used.

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

Function reservations, generated lambdas and generator expressions, module chunks,
and materialized bodies share one emitted-symbol namespace. Admission consults
the existing reservation, allocated-name, and function-body owners together.
Stateful function kind reserves its callable and poll targets atomically, before
lowering either body; a source function named `f_poll` cannot alias the poll target
of a coroutine or generator named `f`, in either declaration order. Only source
function materialization may consume a matching module reservation; generated
annotation functions and class-method symbols allocate their own identities.
A materialized reservation is consumed: a later definition with the same source
name gets a new target, preserving any retained earlier callable.

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
