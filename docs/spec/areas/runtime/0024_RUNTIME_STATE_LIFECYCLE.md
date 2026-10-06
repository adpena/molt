Title: Runtime State Lifecycle and Shutdown
Status: Active
Owner: runtime
Last Updated: 2026-10-01

## Summary
`RuntimeState` owns builtins, interned names, module and exception caches,
capability state, and async registries. Explicit initialization publishes this
state; finalization revokes ordinary admission before retiring its owners.
This contract defines shared native/WASM callback custody, embedding state
reclamation, and executable process exit. Implementation and individual proof
cells do not by themselves establish leak-free or release-wide conformance.

## Goals
- Provide explicit `molt_runtime_init()` and `molt_runtime_shutdown()`.
- Allow full teardown of all runtime-global caches.
- Distinguish executable process exit from embedding teardown: native
  executables must run Python-level exit hooks and then hard-exit without
  C/Rust allocator or TLS destructor teardown.
- Preserve current fast paths (minimal overhead for steady-state execution).
- Enable Miri leak checks to pass without suppressing leaks.
- Integrate allocation diagnostics and cyclic collection with lifecycle custody.

## Non-Goals
- Replace ref counting with a tracing GC.
- Replace the existing cyclic collector with a second reclamation authority.
- Require pervasive API changes in generated code or wasm ABI (unless unavoidable).

## Performance + Concurrency Constraints
- The steady-state runtime must not add new locks or dynamic dispatch in hot paths.
- Initialization/shutdown must be explicit and rare; no hidden work on every call.
- Async/coroutine and channel paths must remain zero-cost at runtime when the
  lifecycle is already initialized.

## Owned Teardown Roots (Non-Exhaustive)
- Builtin classes (`BuiltinClasses`) and their `__bases__`/`__mro__` tuples.
- Interned names (`INTERN_*`) and method tables (OnceLock values).
- Module cache, exception cache, last-exception tracking.
- Parse arena and TLS caches retain allocations until shutdown/thread exit.
- Capability cache and hash secret storage.
- Async registries (task exception stacks, cancel tokens, per-task maps).

## Architecture
### Sealed class storage
`object::class_layout` owns managed solid-base selection and the immutable
physical-field projection. Native kind, shape, intrinsic declarations and own
slots determine storage lineage; dictionary/weakref policy and inferred
attribute names do not introduce a competing solid base. The same selection
governs admission and public `__base__`.

Class sealing prepares the namespace, offset map, typed physical rows and size
before publishing them together with the finished state. Displaced owners are
released after publication. Allocation, initialization, GC, dictionary exposure
and class transfer consume this projection; they do not reconstruct storage
from ancestor namespaces. Integer and float payload words have intrinsic kinds,
independent of user attribute names.

Slot descriptors retain the declaring class and resolve its sealed offset.
Hidden, redeclared and duplicate slots remain distinct physical owners; private
names are mangled when captured. Dataclass vectors keep their logical field
prefix and append hidden slot owners through a derived typed projection. Every
value and retained projection name participates in GC and clear. The
`DictSubclass` shape owns one backing slot shared by access and lifecycle;
corrupt backing reports an error without replacement. No side table supplies
missing physical capacity.

### Native descriptors and Python class identity

Native C descriptors use the physical type's descriptor slots at both C-API
attribute lookup and compiled Python crossings. Class namespace entries remain
owned across lookup and callback reentry, together with the original type and
any instance dictionary queried. Missing get/set slots, callback
failures, a NULL receiver/delete operand, and an explicit Python `None` value
are distinct states. Optional lookup suppresses only `AttributeError` and its
subclasses; other lookup and descriptor failures retain their original exception.
Native callback results and statuses share the call boundary's exception
contract: a failure requires an exception; a success with an exception becomes
`SystemError` with the original exception retained as cause and context. An owned result
is released before that contract violation is propagated.

Public C-API exact and inclusive predicates consume the same live Python class
identity as `Py_TYPE`. A native storage tag or physical ABI-view header never
proves exact builtin identity. Physical `ob_type` still owns layout and native
slot dispatch; this separation preserves native subclass storage without
introducing a second class registry or a stale predicate cache.
Runtime truth testing and specialized length entry points likewise require
builtin class identity before using storage-only fast paths. Subclasses use
the shared special-method lookup; explicit base descriptors retain physical
payload semantics.

### Native list storage and Python class state

List subclasses use the existing LIST heap kind and its generated class shape.
The first aligned word owns the tracked element vector; frozen declared fields
and the trailing dictionary reservation follow the native prefix. Generic class
allocation initializes that prefix before publication. Failed construction,
cycle clearing, and terminal retirement use the same list element and class
field ownership routines, including a null prefix during unpublished rollback.

List descriptors require physical list storage. Explicit `list.__getitem__`,
`list.__len__`, `list.__imul__`, and sibling descriptors operate on that storage;
source operators resolve subclass special methods. Unpack, slice, string join,
and bytes/bytearray construction use their existing source-protocol fallback
for class-bearing list receivers. Semantic `list[T]` hints
never certify exact Python class identity or compact element representation.
Native raw list indexing requires the shared constructor-rooted exact-list fact;
flat integer storage continues to require its separate mutation-sensitive fact.
The native generic lane observes Vec, compact-int, and compact-bool layouts from
one shared heap-kind selector for both local reads and loop hoists; inferred
result types cannot select a storage layout.
LLVM and WASM use runtime source-dispatch guards for hinted list length and
membership.

Builtin list identity is fixed even when `list.__new__(list)` installs an explicit
class edge. The shared class attachment authority rejects builtin/immutable
class reassignment, preserving the module-class exception. Compatible heap
list subclasses use the existing sealed-layout and owned-class-edge transfer.
The reserved dictionary word does not grant public state: exact list has
neither an instance dictionary nor weakref support under the slot policy.

Generic shallow/deep copy reconstruction is not implemented by this storage
change. List subclasses are excluded from exact-list copy shortcuts; the
existing generic fallback still returns its input and remains an open contract.
Pickle's exact-list payload shortcut likewise excludes subclasses so available
reducers retain control. Default list-subclass pickle reconstruction and complete
protocol/memo behavior remain open; class-state traversal alone is not evidence
of copy or serialization conformance.

### RuntimeState
`RuntimeState` owns runtime-global state:
- Builtin classes and method table caches.
- Interned names and attribute name caches.
- Module/exception caches and last-exception tracking.
- Hash secret and capability cache.
- Async registries and task metadata maps.
- Context variable defaults, per-thread frames, token ownership, and copied
  context snapshots.
- Stdlib state-machine registries whose handles must not survive runtime
  teardown, including fallback `configparser` parser handles and fallback
  `csv` reader/writer handles, dialect registry, field-size limit, and
  `random.Random` generator handles.
- Runtime extension registries, including `molt-runtime-collections`
  defaultdict factory-handle state. These registries must retain heap values
  they store, return independent owners when exposing them back to Python, and
  release all retained handles during runtime-state clear/drop.
- C-API extension module metadata and per-module state registries.
- Call binding provenance state for heap-backed `CallArgs` builders.

The lifecycle publishes a single ready-state pointer for the fast path:
- `molt_runtime_init()` allocates and initializes the state, then publishes
  the pointer.
- `molt_runtime_shutdown()` revokes the pointer and tears down all state.

### Initialization
- Idempotent initialization (multiple calls return success).
- Finalizing and permanently shut-down runtimes cannot be reinitialized;
  runtime restart is available only to serialized test fixtures.
- Strict ordering: intern base names first, then builtin classes, then caches.
- Initialization, shutdown, persistent-GIL setup, and executable exit run
  their bodies under the shared FFI panic dispatch
  (`molt_runtime_core::with_gil_entry_body!`), so no panic or `resume_unwind`
  payload crosses these C entrypoints. Abort builds keep fail-stop semantics.
  In unwind builds (tests, CI, dev), an invariant panic closes execution
  admission, revokes the ready pointer and the TLS cache, wakes lifecycle
  waiters, records terminal `Failed`, and writes
  `molt runtime lifecycle failed: <panic message>` to stderr. Init and shutdown
  return `0`; executable exit terminates with status `1`.
- A failed lifecycle cannot restart, including through test reset. Its partially
  initialized or retired allocation stays alive because native roots may still
  borrow it; reclaiming unknown partial state would risk dangling pointers.
  This quarantine is failure custody, not a retry or a cleanup success.

### Shutdown
- Requires runtime quiescence (no running tasks/threads).
- Shared teardown acquires shutdown-drain execution custody before its first
  callback-capable operation, or inherits an actual ordinary lifecycle lease or
  existing drain capability. A raw GIL or C-extension context is not that
  capability. It remains held through the final callback-free release tail.
- The boundary includes process-exit profiling, shared mixed cycle collection,
  pending calls, worker/task owner destruction, `atexit`, live stdio flushing,
  class retirement and the C/Molt TLS fixed-point drain. Embedding, process exit,
  and isolate teardown collect at the same first boundary before pending calls;
  isolates do not consume the primary runtime's process-owned pending calls.
- Nested callbacks inherit the owner's capability without reopening ordinary
  admission, creating a new lifecycle lease, or fabricating WASM execution
  depth. Temporarily releasing the GIL to join workers does not transfer this
  thread-local capability to another thread.
- Module retirement preserves the actual `sys` and `builtins` namespaces through
  the ordinary callback-owner and C/Molt thread-state fixed point. Cache aliases
  share namespace lifetime by object identity. After quiescence, `sys` retires
  before `builtins`, with another shared callback drain after each phase.
  Each drain collects cycles released by the preceding owner/TLS pass, then
  drains roots again to catch collector callbacks that resurrect into them.
  Collection ends before callback-free class identity retirement.
  Registered module owners retire with their cache cohort; retired rows cannot
  return a cleared namespace or restart its initializer during shutdown. No
  copied builtin namespace or lookup fallback substitutes for captured identity.
- Drains caches (module/exception, intern tables, method caches).
- Flushes TLS caches.
- Decrefs builtin classes, tuples, and method objects.
- Clears async registries and task metadata.

### Python frame namespace custody

Recursion depth has one thread-local owner in `state::recursion`. Generated
direct-call guards, runtime calls, descriptor binding, and recursive comparisons
charge that same execution stack. A failed entry leaves depth unchanged; Rust
guards discharge through RAII and cannot move to another thread. A mismatched
exit or teardown with a live charge is an invariant failure, never a saturating
decrement or silent reset. Suspended activations retain no execution charge.
The recursion limit is runtime-owned policy shared by that runtime's threads;
an isolated runtime starts with its own default. `sys.setrecursionlimit` checks
the actual calling thread's depth before publishing a change. No process-global
depth, thread-local policy mirror, or cold-path synchronization exists.

Compiled entry (`molt_trace_enter_slot`) is the sole owner of Python frame
creation; callable dispatch never manufactures a second frame. Each code slot
publishes an owned code/globals pair under the runtime execution token. Dynamic
function invocation transfers exact callable code, captured globals and builtins through a scoped,
slot-keyed, single-use handoff. Typed generated calls and runtime dispatch use
the same handoff, independently of their machine return ABI.

Frame entry returns `None`, not its borrowed code identity. The frame stack
owns the transferred code, globals and builtins until exit; disposing an unbound
runtime-call result must not release those owners. Direct lexical entry and
invocation handoff obey this same contract in native and WASM backends.

Frame introspection counts materializable Python frames, not native dispatch
entries. `sys._getframe` publishes the runtime callable directly, with its zero
default owned by the intrinsic manifest. Depth uses the integer-index protocol
and the CPython C-int range; negative depth selects the current frame, while an
exhausted stack raises `ValueError`. Python adapters such as `inspect.currentframe`
skip only their own actual frame through the same selection primitive. Traceback
stack adapters capture their caller before delegating, and retain an explicitly
provided frame unchanged. Code names use the definition's unqualified Python
name, independently of the function's qualified name. Each formatted stack entry
owns the header, source and caret text for one frame; traceback, stack and
exception formatting share that renderer. Allocation and callback failures
propagate; they must not become a successful `None` result.

Exception attributes use ordinary instance lookup and mutation, including Python
hooks and data-descriptor precedence. `molt-obj-model::exception_layout` owns the
field declarations consumed by both runtime descriptors and native C tables.
Each declaring class publishes its namespace once; requested attribute names do
not lazily manufacture a separate exception namespace. Physical field access
validates actual storage and inheritance without consulting `__class__`.

Python `__notes__` lives in the ordinary instance dictionary. The reserved C
`PyBaseExceptionObject.notes` field is independent metadata with distinct missing
and explicit-None states; GC and physical snapshots retain both owners separately.
Materializing lazy args publishes a tuple before returning a borrowed reference;
failure preserves the original error and never exposes an unowned result.
Iterable conversion uses the shared sequence authority, honoring subtype
iteration and target-version length-hint behavior. Physical list mutation retains
its storage semantics even when a subtype overrides iteration.

Exception-group construction, `derive`, `split`, and `subgroup` share one
admission and partition authority across managed and C-owned storage. Matching
uses actual exception classes and inheritance; predicate callbacks follow the
declared Python-version contract. Traversal retains the original child tuple
across callbacks and calls the visible `derive` once for each nonempty result.
Metadata publication uses the existing owner for each representation, including
C-owned results returned by an overridden `derive`. Subgroup traversal does not
allocate or derive a discarded remainder.

Exception text is rendered on observation through the declaring type's real
repr/str slots. The builtin exception schema owns those declarations independently
of physical layout: BaseException owns repr and the generic args str slot, while
KeyError, groups, SyntaxError, ImportError, OSError and the three concrete Unicode
errors own their specialized str slots. Managed and foreign exceptions use the
same ABI slot implementations and physical field access. An explicit base slot
call retains that owner's behavior. Object's repr slot produces the default
representation; object's str slot dynamically calls repr.

Rendering does not cache formatted text or classify Python subclasses by name.
Each callback observes current state in Python's evaluation order, and every
field or argument used across it has an owned reference. In particular, a base
args render retains its original tuple across rebinding; Unicode reason and
encoding callbacks precede reads of the later fields. The owned result and exact
callback error survive temporary-owner cleanup. Diagnostic rendering detaches
and restores the existing raised state through the exception transaction.

The runtime renderer owns either an unchanged callback string or lossless WTF-8
bytes. A root str/repr result keeps the callback's string identity; container,
exception, percent, format-field, pprint and traceback composition preserves its
code points. Public Python string producers use that owned/byte boundary. Rust
String adapters are restricted to host diagnostics and scalar-only serialization.
Callback failures stop further Python formatting and reach the existing raised
state unchanged. Dynamic error messages enter exception allocation as bytes too.
Storage fast paths first honor subclass rendering slots. Explicit `str.__str__`
uses its declaring string slot, while generic str dispatch retains callback
identity. Format fields own each resolved value through nested formatting;
dictionary pairs and collected set/view inputs stay pinned through repr callbacks.
Function, code, native-callable and super names use raw structural name bytes.
Format padding carries an arbitrary Python fill code point through numeric and
text assembly, and integer `c` formatting uses the shared WTF-8 encoder.

String precision selects a borrowed WTF-8 prefix before allocating output;
precision zero does not scan or copy the receiver. Percent conversions retain
owned callback strings while reading that prefix, and the immutable percent
receiver stays pinned across callbacks. Requested numeric precision is admitted
before integer digit rendering. Floating presentations bound Rust's decimal
formatter to the exact binary64 expansion and extend excess precision with
fallible zero padding, preserving the requested Python result without a Rust
precision panic. Allocation failures propagate through the pending exception
authority, including emergency MemoryError precedence.

`object/ops_string_format.rs` owns the lossless code-point field grammar for
native `str.format`, `str.format_map`, and the lazy `_string` parser and
field-name iterators used by `string.Formatter`. Iterator cursors retain the
source string and expose one token or lookup step at a time; later syntax
errors cannot precede earlier callbacks. Decimal field indexes share the
advanced-format parser's Unicode decimal and target `isize` bound. Named fields
do not change positional numbering state, and lookup keys may contain braces.
Native formatting resolves a field before validating conversion, expands only
specs containing opening braces, and enforces its two-level expansion limit.
`string.Formatter` keeps its overridable Python orchestration and recursion
policy while consuming those same grammar projections. Its numbering predicates
inspect the complete field spelling (`== ""` and `isdigit()`) before calling
overridable `get_field`; only `get_field` splits lookup components. These Python
predicates intentionally differ from native formatting's first-component rule.
Both lazy iterator entrypoints validate that their exposed mutable cursor is
within the source and on a WTF-8 code-point boundary before parsing a span;
invalid cursor state raises `SystemError`. The intrinsic manifest declares
iterator return types for both projections.

Literal and dynamic `.format` calls both capture the real callable and arguments
through ordinary call lowering before the runtime parses any fields. The
frontend has no host `string.Formatter` parser, format-token cache, or early
format-syntax exception path. This trades compile-time token expansion for one
runtime semantic authority; it does not imply a measured performance gain.
Native composition borrows pinned immutable source/result strings and uses the
shared fallible `FormatWriter`, including nested spec expansion. With an empty
writer, a nonempty final field retains its callback string's identity and type;
an earlier field followed by another field is copied even when the latter is
empty. A final literal retains the receiver only when its storage span is the
complete nonempty source string. Escaped braces and partial source spans cannot
adopt it. Empty output materializes an exact string. An expanded spec may retain
its callback owner internally, but the outer formatting callback receives an
exact-string projection without redispatching a subclass's `__str__`. The writer
consumes or releases the formatted result before the field value is released;
the expanded spec owner outlives that value. Owned callback outputs transfer
directly to the writer, and temporary-owner release preserves both existing
raised-error channels.
The `_string` projections reuse callable-iterator ownership rather than adding
specialized iterator layouts; their implementation type names are not a
CPython iterator-type identity claim. Execution coverage remains defined by the
verified-subset receipts.

RuntimeHooks ABI 31 exposes callback-free type metadata for the current name,
qualname and selected base. Managed type projection uses those structural fields,
and canonical name mutation updates an existing physical tp_name view before
publication. C heap name/qualname observers retain their separate Unicode fields;
exception repr follows CPython's final tp_name component rule. No foreign getter
shortcut or metaclass attribute lookup supplies a second name authority.

Stored message fields retain their original objects, independently of args
rebinding. The common message word is storage for those typed fields and
preexisting diagnostic metadata, never a rendered-message cache. Missing object
fields use the payload's non-object sentinel; an explicit Python None is a
present value. Descriptors map missing fields according to schema policy, while
ABI capture, commit, rollback, traversal and destruction retain the presence
distinction and each owned edge.

Extracted stack summaries own their captured frame summaries. Formatting reads
those same objects, including subsequent public field edits; it must not reread
a retained live frame or a parallel cached payload. Source text preserves the
distinction between deferred lookup and explicit empty text. Diagnostic expression
anchors use the runtime's Python parser, not a second punctuation classifier;
parsing is observation-time work and does not allocate frame-local dictionaries.
The retired source-line, inferred-column and standalone-caret intrinsics are not
an alternate rendering API. File-backed capture retains normalized line endings
and trailing whitespace for 3.12; 3.13+ strips each captured line before joining
the span. Explicit source retains its supplied whitespace on every target.
Captured frame text and formatted entries retain WTF-8, including lone surrogates.
Dedent shares one byte implementation with the host string adapter. Parser anchors
require valid UTF-8 source; the raw source remains intact when no AST is available.
Diagnostic byte offsets count code-point starts; caret alignment uses
spaces and Unicode East Asian display widths, not byte counts or literal tabs.
Public `unicodedata.east_asian_width` and diagnostics share the complete generated
width tables generated by `tools/gen_unicode_width.py` from every supported
target CPython. The table and public Unicode version are selected by the runtime
target, never the build host. Regeneration requires already-installed target
interpreters and does not download them. Cross-version acceptance must bind this
Unicode authority as well as target-specific AST/formatting behavior.

Live optimized-frame locals are the lazy observation of each synchronous frame's
binding homes, which own its bindings (`builtins/frames/bindings.rs`), with no
eager per-call dictionaries or
recognition of particular introspection-call spellings. The observation contract
differs by target: CPython 3.12 refreshes a cached dictionary, while 3.13 and later
expose a write-through frame-locals proxy and independent `locals()` snapshots.
Observability includes ordering: publish a replacement binding before releasing
the displaced owner can invoke user code; retained observations must remain safe
through callbacks, suspension and frame retirement. Evaluate unobserved calls,
first observation, repeated observation and proxy writes separately, measuring
allocation, synchronization, ownership and optimization costs. A regression in
one eager implementation does not establish the cost of every correct design.
The observation objects' ownership is observable too: a 3.12 cached dictionary
can retain a displaced value until refresh, and a 3.13+ snapshot retains its own
values until released. Proxy writes also differ by target and physical storage:
3.13 releases a replaced binding during the write; 3.14 retains displaced
non-immortal plain-slot values in the frame object's overwritten-local owners
until that frame object is cleared. Writes through actual cells release their
displaced value during the write on both targets. This retention belongs to the
frame, including retained observations after return, not to an arbitrary next
compiled read or to every ordinary source assignment.
For 3.14 plain slots, frame destruction releases current bindings before the
overwritten values, which retire newest first. Explicit `frame.clear()` instead
clears that history before current bindings. Writing the identical object adds
no overwritten-value owner. The clear and destruction entry points must preserve
these distinct observable orders rather than share an incorrectly ordered
teardown sequence.
Refreshing a dictionary can therefore invoke a finalizer that reenters frame
observation; publication and borrow lifetimes must permit that reentrancy.

Each frontend-lowered module initializer alone constructs and publishes its
module code object with its lexical globals dictionary, before entering the
module frame. Executable, host, isolate-bootstrap and import-dispatch wrappers
allocate the code-slot table but never synthesize or replace module code objects.
Backend assembly consumes already-lowered modules, not source paths or a second
frontend-lowering context. This ownership also preserves logical filenames and
removes startup ordering derived from an eager-module set.

The generated operation schema admits code metadata before backend emission:
`code_new` has exactly nine operands, `code_slot_set` exactly two (code, globals),
and `code_slots_init`/`trace_enter_slot` no operands. Slot counts and IDs are
explicit nonnegative integers, never implicit slot zero. The same admission
applies to serialized input, direct backend calls and preserved TIR operations;
runtime checks still own code-object and namespace type validation.

Function objects capture builtins with globals. Globals admit dictionary
subclasses through the existing dictionary-storage authority, retaining the
original object as `__globals__`, not substituting its backing dictionary.
An explicit `__builtins__` entry governs new function creation: modules normalize
to their dictionary, while other supplied values retain their identity, including
custom mappings and nonmappings. A present `None` is not an absent entry; invalid
subscript protocols fail at lookup rather than silently choosing default builtins.
When the entry is absent, the active captured builtins are inherited. Existing
functions are unaffected by replacing that entry. Frame presence is distinct
from a captured unavailable bootstrap namespace: only absence of a frame can
select interpreter defaults. Global lookup, public builtin resolution and the
C eval namespace view preserve this distinction; a missing captured namespace
never reconstructs classes, exceptions, functions or intrinsics by name.
Suspended tasks retain code and both namespace objects in
the existing auxiliary sidecar and restore them on every resume. Live frames,
frame snapshots, and lazy traceback payloads retain their exact namespace
edges. Source filenames and mutable module metadata are diagnostic data, never
namespace lookup authorities. GC traversal and retirement visit the same owned
edges, including aliased globals/builtins edges separately. Generator shortcuts
and binding share one constructor; resumes and suspended code/frame views use
the retained code. There is no function-address-to-latest-code registry.
The same sidecar owns the active await continuation. Scheduler subscriptions
only govern wakeups and may retire before resumption; send/throw/cancellation
and `cr_await` use the retained continuation. GC visits and detaches this edge
with the activation's other owners. Replacement publishes the new edge before
releasing the displaced owner, so finalizer reentry cannot see stale custody.
Compiler closure stores and generator, coroutine-adapter, async-generator,
async context-manager and asyncio payload slots share `object/payload_refs.rs`
for reference publication.
Borrowed self-assignment preserves the sole slot owner; owned self-assignment
consumes its distinct incoming reference. Complete fixed-size I/O payloads
detach every slot into stack storage before releasing any displaced owner,
so callbacks cannot observe partially retired storage or lose a reentrant write.
`ag_await`, like `cr_await`, projects the retained semantic await edge. Public
local-variable spellings and transient scheduler waiter registrations do not
identify the Python object acquired by an await expression.
Manual send/throw carries owned values only in a scoped resume context. A
scheduler root masks any enclosing manual invocation, including a nested event
loop. Delegation stops at the Python iterator adapter; its private Future wait
bridge is closed and unsubscribed before an explicit resumption reaches the
iterator. Poll failures are captured under the polled task's exception owner
before restoring the caller, including a caller with no current task.
Closing an unstarted coroutine follows the selected Python version: 3.12/3.13
retain frame storage until retirement, while 3.14 releases captures and namespaces
during close. The exact `cr_code` remains available after closure.
Started generator and coroutine close return their completion value for Python
3.13 and later; Python 3.12 returns `None`.
Task construction matches the physical target in the pending callable's immutable
code identity; runtime-native tasks do not manufacture Python code ownership.
Frontend code-slot publication uses the constructor's explicit target, never its
type-hint spelling. Generator expressions use the same callable metadata and
generated task constructor as named functions; async functions have no second
frontend-generated constructor body.

Generated task constructors return a failed allocation result before resolving
or storing payloads, retaining captures, registering cancellation, or wrapping
an async generator. The pending allocation exception remains authoritative.
An async-generator wrapper retains its inner task on success; the constructor
releases its temporary task owner after wrapping on either outcome. Failed task
allocation releases acquired namespace references without consuming or leaking
the scoped invocation handoff.
Ordinary `alloc_task`/`call_async` operations skip initialization on failure and
rejoin their existing exception edges; they must not return around frame/RC
cleanup. Native, WASM and LLVM consume `TaskConstructorLayout` for task kind,
payload prefix, completion policy and checked frame-extent validation.
Inferred extents use the same checked sizing authority as explicit extents;
backends do not multiply payload counts before validation. Native internal CFG
joins carry both object and pointer cleanup roots into the continuation, even
when allocation fails or the task was constructed inside a non-entry block.

When a compiled builtins body is admitted, its canonical module transaction
publishes the builtins namespace before a user frame captures its default.
Builtin callable metadata (including defining module and binding policy) comes
from `BuiltinFuncSpec` for both frontend materialization and generated runtime
publication. The Python facade must not reacquire optional providers merely to
patch their metadata: a pure profile can omit filesystem callables without
making builtin namespace initialization itself require filesystem support.
Global reads preserve subclass/custom mapping `__getitem__` and treat only
`KeyError` as a miss. Global stores/deletes and function metadata capture use
the underlying dictionary protocol, not user `__setitem__`/`__delitem__` hooks.
Relative-import package metadata uses this same raw globals backing authority;
admission must accept dictionary subclasses without invoking `__getitem__`.
Captured namespace misses stay authoritative: later module/cache replacement
or globals `__builtins__` mutation cannot redirect an existing activation.
Live, traceback and suspended-task views share one frame-class materializer and
the same interned field slots, including `f_builtins`, `f_back` and `f_lineno`.
Native runtime tasks without captured Python code expose no fabricated Python
frame. A failed locals snapshot preserves its allocation error, never retries
with a success-shaped empty dictionary.
For optimized functions that publish a locals dictionary, the current adapter
reuses it on Python 3.12 and copies it on Python 3.13+; module scope retains
namespace identity. This does not close the canonical-binding and observation
requirements above, particularly aliased calls, refresh timing and write-through
proxies. The runtime target-version authority selects the behavior, not the host
interpreter. Copies use the shared dictionary-copy primitive, including its
ownership and allocation-failure behavior.
Dict, set and frozenset construction share one unpublished backing transaction:
the object owns each admitted buffer immediately, and normal lifecycle teardown
rolls back partial storage. Initial edges are published only after successful
hashing. Raw object allocation and capacity/backing denial record non-allocating
`MemoryError` without replacing an existing exception. Frame/traceback instances
use the same class allocation authority as ordinary instances.
Task execution kind belongs to the code object (direct, generator, coroutine,
or async generator). Generated trampolines alone own callable task allocation
and closure layout. Reconstructing `FunctionType` retains that code-owned kind;
runtime marker truth and cached task flags are not competing dispatch facts.
The packed function metadata tuple contains fourteen fields; field 11 (zero-based)
is the typed execution-kind integer (0 through 3 in the order above), and fields
12 and 13 are ordered free-variable and cell-variable name tuples. Publication
sets the immutable code fact; introspection reads the code policy directly.
Legacy task marker attributes are neither emitted nor consumed. Arbitrary public
attribute mutation cannot rewrite code kind or change sibling functions sharing
that code object. The four-argument metadata initializer ABI is unchanged.
The iterable-coroutine protocol bit shares the immutable code-policy scalar;
`types.coroutine` clones generator code before adding it, leaving siblings and
existing suspended objects unchanged. `co_flags` projects code policy and
signature facts. `inspect.markcoroutinefunction` is a separate public identity
marker and does not change execution kind or make a returned value awaitable.

Code callable identity also owns physical entry provenance (positional, lexical
closure, opaque runtime context), alongside target, trampoline and arity. Code
cloning preserves the complete identity; reconstruction cannot infer provenance
from the public closure tuple or free-variable count. Opaque-context code cannot
be reconstructed or assigned through Python's function-code API. Function
dispatch consumes its validated scalar ABI; it never scans cells on the hot path.

Both metadata transport ABIs decode into one function-metadata initializer.
Code attachment is a prepare/publish/retire transaction. Preparation validates
the complete signature, callable identity, lexical metadata and execution kind
without changing either owner. Publication retains incoming edges and installs
the coherent code/signature state without callbacks or decrefs. Executable
scalars, mutation epoch and binder/cache state become coherent before displaced
owners are released. Metadata initializers repeat preparation after attribute
writes that can reenter; fresh construction preserves epoch zero. Rejected
preparation leaves code identity, owned edges and epoch unchanged and retryable.

Callable constructors transfer one result owner. Native, WASM and LLVM bind that
owner to a named result or release it immediately when discarded; dropping only
the machine value is not an ownership operation. Function-closure extraction is
borrowed instead: a bound result acquires one reference, while a discarded result
acquires and releases none. These contracts include code objects, descriptors,
bound methods, async-generator wrappers and call-argument builders. Generated
WASM call sinks declare the release-import dependency for owned results.
The same sink governs callable dispatch results, including guarded, dynamic,
method and builtin calls. Compiled direct calls consume their semantic return
ABI: discarded object returns release ownership, raw scalar returns need no
boxing or allocation, and void calls have no result to release. Runtime direct
calls use generated boxed-value ABI facts, never an integer carrier or symbol
prefix as an ownership proof.
Indexed statement mutation follows the same authority: generic assignment,
dictionary/integer-list specialization, and indexed deletion return a borrowed
container status, not a new owner. Discarding that status must not release the
container. A bound internal result must first acquire its own reference.
Python `__setitem__`/`__delitem__` dispatch remains distinct: statement dispatch
releases the method's owned result before continuing, while direct method calls
preserve the ordinary Python result contract. Native, LLVM, and WASM consume
these facts from the shared runtime ABI manifest rather than per-backend lists.
Static native calls preserve that exact signature even for closure targets and
void imports. Execution-frame tracing belongs to the callee, not a call-site
switch or value-only pointer dispatcher. After closure transport, argument
arity must match the declared ABI; Python binding uses callable dispatch instead
of casting an incompatible static target.

Attribute APIs transport boxed values as `u64`, including read, write, delete,
descriptor, inline-cache and GPU bridge results. Their error paths therefore use
the boxed exception sentinel; raw signed numeric/status sentinels must never be
reinterpreted as object values. A cache miss remains distinct from an exception
or a successful boxed `None`. Native C declarations preserve all 64 bits on
LLP64 as well as LP64 hosts.

Raw positional call admission is shared by runtime fast helpers and native/WASM
lowering. It requires the actual callable's exact arity, closure shape, direct
execution kind, and binder eligibility. Guarded target mismatch or shape mismatch
routes the original Python arguments to the binder, never to an indirect call
using the expected target's ABI. The actual callable owns closure and defaults;
frontends must not insert lexical defaults or task payloads at Python call sites.
Dynamic-call argument builders are selected only by keyword/starred argument
syntax, not caller overrides, lexical type hints, or bound-method guesses.
Positional object dispatch binds the actual descriptor result, including self.
Both guarded and inline direct calls retain the invocation context handoff.

Teardown detaches slots and invocation handoffs before callback-capable decrefs.

Regression authorities: `builtins/frames/namespace_tests.rs`, suspended-task
tests in `async_rt/poll.rs` and `object/aux_header.rs`, module lookup tests, and
the `globals_callable` differential capsule. These tests define the changed
contract; passing one host lane does not establish a cross-target matrix claim.

### Canonical objects and ordinary owned references

Async exception dispatch matches canonical class identity and inheritance.
The cancellation class published by `asyncio.exceptions` is obtained from the
runtime class authority, independent of mutable `builtins` bindings. Callback
dispatch propagates `SystemExit` and `KeyboardInterrupt` (including subclasses);
an unrelated same-named exception has ordinary callback error semantics.
Required async method lookup preserves descriptor exceptions and releases its
temporary attribute-name owner at every arity. A missing optional method and
a failed descriptor lookup cannot select the same fallback path.

- `CanonicalObjectCache` owns the physical lifetime of fixed empty values,
  interned strings, Missing, NotImplemented and Ellipsis. One publication
  protocol installs the immortal refcount and flag before exposing a fixed
  singleton. Hits remain lock-free; failed initialization publishes nothing.
- Dictionaries, module caches, atomic caches and extension-state slots own
  ordinary references, not permission to make their referents mortal. Their
  teardown uses the same reference-release primitive as ordinary execution.
- Reference-count call sites inline the ordinary ownership transition. Fatal
  diagnostics, tracing, C-ABI view transactions, and terminal destruction have
  shared implementations outside that inline path. Terminal release preserves
  the pre-release header snapshot and the finalizer/weakref resurrection window;
  code-size policy must not omit validation, callbacks, or resource teardown.
- Only the canonical pool's final teardown makes its detached allocations
  mortal, after callback, class, ABI and ordinary root retirement. Arbitrary
  shutdown edges cannot revoke immortal lifetime, regardless of interning.
- Literal string/bytes/bigint caches are bounded ordinary-reference owners.
  Eviction releases only the displaced cache edge; each constructor result
  has independent ownership. Lookup acquires that result while cache custody
  is held, including the shared WASM mutex. Literal caching does not grant
  immortality or suppress accounting for later users.

### Executable Process Exit
- Native executable stubs and backend-generated `molt_main` success exits use
  `molt_runtime_exit(code)` rather than `molt_runtime_shutdown()`.
- `molt_runtime_exit` runs the safe Python-level process-exit subset once:
  worker quiescence, task/exception cleanup, `atexit` callback execution, and
  stdio flushing.
- `molt_runtime_exit` intentionally does not free `RuntimeState` or depend on
  Rust/C TLS destructors; it calls `_exit(code)` after
  Python-level finalization. Full state reclamation remains the explicit
  embedding/C-API `molt_runtime_shutdown()` contract.

### WASM host ownership

- `molt_main` is reusable application startup, not a runtime lifetime owner.
  Finite Node, Wasmtime and request-worker owners resolve the canonical
  execution-enter, execution-leave and runtime-shutdown ABI before guest entry.
- The finite owner begins when runtime instantiation succeeds, before fallible
  application setup. Setup failure, entry failure and normal completion all
  consume that same owner; setup must not bypass canonical runtime teardown.
- Capture startup/callback failures and pending runtime exception diagnostics
  before teardown. Release execution leases before calling shutdown, including
  failure paths. A cleanup failure must not hide the original failure.
- Runtime teardown owns `atexit` execution, live `sys.stdout`/`sys.stderr`
  flushing and release/flush of pinned bootstrap streams. Hosts must not force
  each print to flush or introduce a separate stream-finalization path.
- Backing host services remain available through guest finalization. Host-only
  cleanup then closes their resources, including on runtime-shutdown failure;
  it must not dereference guest handles or recreate services. Host callbacks
  queue responses for delivery under an execution lease, never enter the guest
  independently, and cannot deliver after disposal. Preserve all cleanup errors
  alongside the original application failure.
- Reusable browser embeddings retain their runtime across `run` and exported
  calls, then explicitly dispose it once at the owning application's end.
  Disposed embeddings reject further guest calls; repeated disposal never reruns
  teardown and rethrows any recorded disposal failure, including falsy JavaScript
  thrown values. Full and minimal browser hosts share this disposal authority.
  A listener-removal failure cannot skip later listeners or owned resources.
- Successful execution admission requires shutdown to return raw i64 `1`.
  Raw `0` is allowed only for an unused lifetime that never entered execution;
  it must not conceal refused teardown after an active application ran.
- WASI reactor initialization precedes execution admission when libc or host
  memory binding requires it. It remains inside lifetime ownership: a bootstrap
  failure still finalizes the runtime and closes host resources.
- Wasmtime subprocess and database output readers remain concurrent with guest
  stdin writes so duplex pipes can make progress. Their owner retains cancellation
  and join custody, cancels the whole cohort before a bounded wait, and reports
  incomplete cleanup. Host-only close uses held child handles, not rediscovered
  PIDs, and does not wait for a WebSocket peer to finish a close handshake.
- A Node DB shutdown deadline reports incomplete cleanup; it never proves child
  closure or authorizes terminating the owning Worker. A late child `close`
  still drains the Worker's response and parent ports and reports late errors.
  Finite Node exit paths record an exit status and let owned resources and stdio
  drain; forced process exit must not discard an incompletely closed child.
- Shutdown is an essential linked export in the generated WASM ABI policy,
  not an optional runner capability. Explicit WASI commands retain their own
  `_start`/`proc_exit` semantics and do not acquire a Molt runtime lifetime.

## Implementation
- `molt_runtime_init()` is wired into generated entrypoints; executable exits
  route through `molt_runtime_exit()` for Python-level finalization plus
  hard-exit, while `molt_runtime_shutdown()` remains the explicit embedding
  teardown API.
- `RuntimeState` now owns builtin classes, interned/method caches, module/exception caches,
  hash/capability state, async registries, context variable state, and argv storage
  (no lazy_static globals for those domains).
- Context variable defaults, per-thread frame maps, reset tokens, and copied
  context snapshots live under `RuntimeState.contextvars`; full shutdown and
  process-exit finalization clear that state after `atexit` callbacks. Default-only
  `ContextVar.get()` reads do not allocate per-thread context state.
- Fallback `configparser`, `csv`, and `random` registries plus C-API module
  metadata/state registries live under `RuntimeState` and are cleared during
  shutdown and executable process-exit finalization. `CallArgs` builder
  provenance registries are also runtime-scoped instead of process-global.
  Fallback `itertools` class/function/keyword-marker slots are owned by
  `RuntimeState.itertools` and are cleared through the same shutdown paths, so
  iterator helper objects cannot survive isolate or executable lifecycle
  boundaries as process-global object roots.
  Special descriptor cache slots, including function `__code__`/`__globals__`
  descriptors, are cleared through the same lifecycle path as the rest of
  `RuntimeState.special_cache`.
  Descriptor-cache lookup snapshots retain exposed heap bits before leaving
  TLS cache custody. The cache alone owns the name/version lookup key; operation
  snapshots retain only class and descriptor owners, without copying name
  buffers on cache hits. Eviction cannot change an in-flight snapshot.
  `descriptor_bind` retains the descriptor across
  reentrant `__get__`/property execution so class-dict or cache mutation cannot
  invalidate borrowed descriptor storage mid-bind.
  Fused method dispatch likewise pins the selected function and attribute name
  before instance-dictionary shadow lookup, which may execute key equality.
  Both hit and miss paths revalidate receiver, type and function mutation state
  after that callback. Fused super dispatch supplies raw positional arguments to
  the canonical binder; a cached target never authorizes the already-bound ABI
  after code, defaults or signature replacement.
  Resource tracker factories and current-thread tracker state are reset at
  lifecycle shutdown boundaries so memory/time limits cannot leak into the
  next runtime in an embedding process.
  Their per-runtime handle counters and registries reset with a new runtime
  state, so stale handles cannot address process-lifetime parser/CSV/RNG,
  extension-module, or call-binding state.
- TLS guard drains per-thread caches on thread exit; scheduler/sleep worker threads
  still participate in shutdown cleanup and are joined before teardown completes.
- Native subprocess ownership is runtime-scoped through `RuntimeState.process_registry`.
  Molt-created Unix children enter an owned process group by default, so handle
  drop and runtime teardown terminate the whole child process tree rather than
  only the direct child. The registry drains early in shutdown, closes
  process-owned stream references once, wakes wait futures, and only joins wait
  workers that have finished inside the bounded teardown window. WASM process
  host handles use the same runtime-owned registry surface instead of
  process-static maps.
- Socket side registries are runtime-scoped through `RuntimeState.socket_state`.
  Native fd-to-object mappings, WASM socket metadata, non-Unix peer links, and
  ancillary queues are cleared immediately after worker shutdown and process
  registry drain in both embedding shutdown and executable process-exit
  finalization. Final socket reference release unregisters native fd mappings
  before closing the socket, so dropped unclosed sockets cannot leave stale
  process-lifetime fd-to-pointer entries.
- Signal handler slots, pending-delivery flags, the wakeup fd, and the
  main-thread loop's park route are runtime-scoped through `RuntimeState.signal`.
  SIGINT starts in the `default_int_handler` disposition (Python-visible
  handler, OS default action). The raw C signal handler reaches the active
  runtime-owned atomics only inside an admission window; it records, then
  wakes, and never owns state. Installing Python-level handlers retains callable
  references, and `getsignal()` returns owned callable references. Only the
  lifecycle-owned process runtime can publish the active signal state; handler
  and wakeup-fd APIs cannot republish it. Inactive isolate teardown clears only
  its local state. Successful active-state deactivation authorizes OS-disposition
  reset and tripped-summary retirement. Shutdown waits out the same admitted
  signal/C pending-call notifiers before releasing handlers and the park route.
  Wakeup descriptors are validated as nonblocking before publication. The exact
  uncaught-KeyboardInterrupt exit latch belongs to the active runtime and resets
  only when the next runtime successfully publishes its signal authority.
- VFS bundle loading enforces cumulative load quotas before retaining entry
  contents. Native directory bundles, tar bundles, and injected WASM bundle
  entries share byte, entry-count, per-entry, and path-byte limits with
  explicit environment overrides; oversized configured bundles fail fast
  instead of silently constructing unbounded in-memory file maps.
- Pointer registry is reset on shutdown so NaN-boxed addresses cannot outlive
  runtime teardown; object pointer resolution consults the registry to satisfy
  strict provenance tooling.
- Immediate object-address recycling pools were removed: NaN-boxed pointer
  identity can outlive refcount-zero in generated cleanup edges, so all
  allocator-backed objects now return directly to the allocator on decref.
- The implicit thread-local object nursery was removed from the default
  allocation path: without a global write barrier and function-exit reset
  contract, nursery objects could escape and later drop heap-backed payloads
  while their object headers remained addressable. Scope arenas remain the
  only bulk-reclaimed object storage and are marked explicitly with
  `HEADER_FLAG_ARENA`.
- Remaining: optional allocation registry + pointer registry lock overhead optimization (OPT-0003).

## Allocation Tracking (Phase 2)
Add an optional allocation registry for full teardown validation:
- Debug builds can enable full tracking by default.
- Release builds can opt-in for diagnostics.
- Registry supports leak detection and per-type summaries.

## Module Instances

Exact modules and `ModuleType` subclasses share the class-shaped instance
layout. The native prefix retains module identity; declared slots follow it,
and the shared trailing dictionary owner is the module namespace. Attribute
lookup, slot mutation, weak references, GC traversal and detachment consume
that same representation. A module never has a separate ordinary-instance
dictionary beside its namespace.

Construction prepares native backing, empty fields and the eager namespace
before committing the class edge. The namespace remains locally owned until
that edge admits the shared dictionary slot; the two stores then commit without
allocation or Python callbacks. Failure before commit releases the classless
allocation and prepared backing without invoking a subtype finalizer. Dictionary
stores require an admitted slot and never silently discard an incoming owner.

Python `ModuleType.__new__` allocates an empty namespace for the requested
subtype. `__init__` sets the standard module metadata while preserving unrelated
attributes and bypassing user attribute hooks. Import allocation shares this
initialization, while the loader execution transaction alone owns same-name
identity redirection. Class descriptors and hooks apply to module attribute
access; the default module methods own namespace-level `__getattr__` and
`__dir__` behavior.

Fresh import completion owns parent-attribute publication and retains the
returned child before invoking the parent's setter. A setter can replace or
remove `sys.modules` entries without changing that return identity. Cached
imports, reload, and direct loader execution do not repeat publication.
`AttributeError` from publication becomes `ImportWarning`; other exceptions
propagate while the loaded child remains committed. From/star import reads
honor module subclass hooks and descriptors, with export names and values
retained across callbacks. Implicit repr/str resolve methods on the type.

C-API borrowed namespace getters use the linked dictionary/frame authority in
both header transports. Generic attribute access can create a fresh descriptor
result, so it cannot provide a borrowed result by dropping its returned owner.

## Interpreter sys Namespace

`RuntimeState.interpreter_sys` owns the canonical sys module independently of
the public `sys.modules` mapping, private import cache, and ModuleTable view.
Only first publication by the current canonical initializer can establish this
typed role. A same-named standalone module, public replacement, deletion, or
extension publication cannot establish or replace it. Module dictionaries are
read-only attributes; the retained module owns its actual namespace throughout
callbacks. Public import resolution still observes current `sys.modules` entries.

Internal importlib/runpy/argument/version/unraisable/stdio, print/input, and
path-cache invalidation consumers use this role. Dictionary values remain subject
to normal Python key equality and pending errors, with dictionary and selected
value ownership supplied by the shared instance-attribute lookup. C sys lookup
reads only this dictionary; it never invokes module `__getattr__` or materializes
missing attributes.

The native initializer publishes process facts, import paths and stdio before
the Python body executes. That body shapes metadata and defines supported APIs
before initialization completes. Intrinsics are required runtime bindings;
resolution failures propagate instead of publishing fabricated defaults or
no-op functions. Public deletion and replacement survive subsequent reads and
republication of the retained namespace. Version setup precedes initialization;
only its owning initializer may refresh unfinished version shapes. A completed
sys namespace cannot be retargeted to another Python version.

`PySys_GetObject` preserves the exact incoming C/runtime exception and suppresses
new lookup failures. Targets from Python 3.13 report those failures through the
existing unraisable transaction. RuntimeHooks ABI 28 carries a C-compatible
`SysLookupPolicy` so `PyImport_GetModuleDict` retains its propagating boundary.
The single ABI version constant drives runtime registration and all fixtures;
RuntimeHooks has no C-header mirror. Borrowed views add no persistent result
registry and survive public sys deletion because the interpreter owns the module.

The sys phase of `ModuleRetirement` transfers this owner exactly once, together
with cache and table owners, before releasing callbacks. Ordinary aliases cannot
clear its dictionary early. A terminal sentinel prevents publication after
retirement, and the existing fixed point drains callbacks before builtins retire.

## Cached Handles

Cached handle publication is owned by `molt-runtime-core::cached_handle` across
the runtime and satellite crates. Initialization transfers one reference into
the runtime slot, hits borrow it, and a losing initializer releases its own
candidate exactly once. Python-callable exports retain a separate result owner;
clearing a module binding must never consume the slot's reference. The itertools
class and keyword-sentinel exports follow this same rule in reduced and full
profiles.

## GC/Cycle Collection

Reference counting and `object/gc.rs` share object ownership and finalizer
semantics. Every finalization mode uses this same mixed runtime/native collector
under the shared teardown capability, first before pending callbacks and module
retirement. Unreachable-cycle finalizers can reenter public runtime APIs without
ordinary admission. Dropping the last external owner of a heap type does not
break its ordinary type-to-MRO-to-type cycle; embedding must collect it rather
than requiring callers to clear it manually.

The live callback-owner and C/Molt TLS fixed point also collects cycles released
by module, cache, class-content, and thread-state retirement. Collection begins
each pass, before the pass drains owners: a finalizer can resurrect a root even
when the collector reports zero reclaimed objects. Reclaimed objects count as
progress and require another owner/TLS pass. Owner release later in a pass also
requires another pass, so newly unreachable cycles are collected before advancing
from ordinary modules to `sys`, then `builtins`, and finally callback-free class
identity retirement. Native allocations retire through their existing clear and
deallocation owners; registry reset still requires zero live native identities.

Only a `Completed` collection that retires an original unreachable allocation
identity from the tracked cohort counts as collector progress. The shared
registry's allocation generation is checked after collector pins are released;
address reuse cannot count a replacement as the original survivor. Python's
`gc.collect()` count remains the number classified as collectable, including
retained `DEBUG_SAVEALL` garbage or a native `tp_clear` that leaves edges intact.
That public count cannot keep shutdown's fixed point busy. Reentrant no-op,
resource error, callback error, and unsupported concurrency are explicit
diagnostics and stop further collection attempts for that teardown; they cannot
keep the fixed point busy or claim successful reclamation. Free-threaded cycle
collection still requires the currently unavailable stop-the-world epoch and
remains unsupported. The post-teardown leak gauge reads counters only and cannot
reenter the retired runtime.

This ordering follows CPython v3.12.0: `Python/pylifecycle.c` collects before
module finalization and again within `finalize_modules` after releasing module
owners. It does not move callback-capable collection after core runtime teardown.
The serialized lifecycle regression covers an ordinary dropped `PyType_FromSpec`
result, a type held until actual module retirement, and finalizer custody/order;
the process-exit regression preserves cycle-before-pending-callback order.

Class reference storage distinguishes cycle clearing from terminal retirement.
Cycle clearing detaches the MRO self-cycle, namespace and annotation owners and
invalidates method caches. Names, bases, captured slot declarations and sealed
physical layout stay owned until terminal destruction: an instance in the same
unreachable group may still need its class metadata for traversal and clearing.
GC order must never determine whether an instance's physical fields can be
released. Terminal destruction and the callback-free class retirement tail use
the complete reference family; neither reconstructs layout from a cleared
namespace.

Container tracking is target-version-specific, not host-version-specific.
`heap_lifecycle` owns the shared child predicate: mutable GC-capable children
require tracking even when currently untracked; only immutable exact tuples
can be treated as permanently atomic after demotion. Dictionary writes inspect
only newly published references, before releasing displaced owners. They never
rescan the whole dictionary or demote it. This applies to construction (including
duplicate keys), replacement, deletion, clear, pop, staged publication, and
bounded string-binding transitions.
For Python 3.12/3.13 targets, full collection first determines reachability,
then demotes reachable atomic tuples, then reachable atomic dictionaries, before
weakref callbacks or finalizers. Minor collections do not demote dictionaries.
For Python 3.14 targets, dictionaries remain tracked from allocation to release.
Allocation accounting and initialized-payload publication remain distinct from
current collector membership.

## Safety and Concurrency
- `molt_runtime_shutdown()` must acquire a global runtime lock (GIL or
  equivalent) to block concurrent access while tearing down.
- TLS caches must be drained on all threads or tracked and reclaimed at
  shutdown (scheduler/sleep worker threads now participate in shutdown cleanup).
- WASM host environments must wire lifecycle entrypoints where applicable.

## Verification
- Cold lifecycle tests start without an ordinary lease and prove custody before
  the earliest callback enters a public runtime API. Native raw-GIL nesting must
  not mask missing shutdown custody.
- Test ordinary-lease and nested-drain inheritance, process-exit collection,
  pending/atexit/stdio callbacks, isolate ownership, and late TLS repopulation.
- Active ordinary owners must still refuse embedding shutdown; recursive and
  racing admission must not reopen a finalizing or permanently retired runtime.
- Rebuilt native and WASM consumers prove output, exit and disposal together.
  Runtime unit tests, Miri leak checks and target execution are distinct proof
  cells, not interchangeable claims.

### Canonical class projection retirement

Canonical runtime classes have semantic retirement pins and independently owned
C allocations. A projected MRO tuple owns a C reference back to its type view;
retained tuples, lists and other concrete views can own that tuple or type in
turn. Interpreter shutdown retires this complete incoming projection component
inside the callback-capable owner drain. It discovers owners through the same
physical field inventory used for release, adds a C pin to every member, and
adopts the component into the existing closed publication transaction.

The transaction drains all physical owners while all identities and allocations
remain live, verifies the mirrored reference ledger is empty, then revokes every
identity and frees the allocations. Only afterward does it release the stable
runtime holds. Publication rollback uses this same ordering. Alias release can
run callbacks, so the shutdown fixed point repeats before semantic class metadata
and canonical anchors are detached. Runtime pins never substitute for C pins,
and no decref may reach a freed Type view through a revoked identity.

Ordinary cycle collection detaches the mutable Type mirrors (`tp_mro`, `tp_dict`)
through that same physical owner inventory after their semantic sources become
empty. Its detached resource releases them outside locks; identity remains live
until ordinary RC runs after native clear/deallocation and collector-pin release.
GC never forcibly revokes a Type or pulls in an externally rooted alias. Shutdown
waits for the existing native allocation registry and retained C thread-state
count to empty before its forced projection cohort retirement. Retirement runs
after the combined runtime/C TLS drain returns, never in its cleanup callback:
the callback precedes C error/context/dict release, and preserved errors can own
off-record aliases. Projection release repeats the existing outer owner drain
before module retirement advances. Final-tail assertions reject remaining native
or C thread-state owners before any canonical class metadata is destroyed. Direct extension-held
C pointers have interpreter lifetime and are invalid after successful shutdown.
Managed owning aliases participate in the component, and native owners must
complete their callbacks first. Ordinary rc-zero removal checks zero incoming
mirrors in O(1) under its existing address lock. Discovery and component
drain require O(R + V + E) expected work and O(V + E) temporary storage per
necessary pass, for R class roots, V live managed views and E physical owners.

### Sequence release and mutation publication

Lists and tuples release terminal element owners from last to first. List clear,
zero repetition, and contiguous slice replacement use the same reverse storage
order. Extended-slice deletion releases removed owners in ascending original
index order; extended-slice assignment follows the requested slice order.
These orders apply to both ordinary storage and published C-API projections.
Every mutation publishes the complete new contents before releasing displaced
owners, so reentrant finalizers observe one consistent generation and their new
mutations survive. Indexed deletion compacts surviving ranges only after the
first removed slot. The compaction neither scans nor writes the untouched
prefix: its work is linear in the removed count plus the affected suffix. An
ABI-projected list still stages its complete transaction before compaction.
Removed owners remain separate until publication, and no tail is shifted
repeatedly.
