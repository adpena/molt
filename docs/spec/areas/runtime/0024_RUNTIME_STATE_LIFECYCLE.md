Title: Runtime State Lifecycle and Shutdown
Status: Draft
Owner: runtime
Last Updated: 2026-09-20

## Summary
Molt's runtime uses process-global caches (builtins, interned names, module and
exception caches, capability state, and async registries). These
live for the life of the process and cannot be reclaimed, which blocks Miri
from passing leak checks and makes long-running processes accumulate memory.
This document defines a production-grade lifecycle with explicit init/shutdown,
full teardown of global caches, and a path to auditability.

## Goals
- Provide explicit `molt_runtime_init()` and `molt_runtime_shutdown()`.
- Allow full teardown of all runtime-global caches.
- Distinguish executable process exit from embedding teardown: native
  executables must run Python-level exit hooks and then hard-exit without
  C/Rust allocator or TLS destructor teardown.
- Preserve current fast paths (minimal overhead for steady-state execution).
- Enable Miri leak checks to pass without suppressing leaks.
- Prepare for optional allocation tracking and future GC/cycle collection.

## Non-Goals (Phase 1)
- Replace ref counting with a tracing GC.
- Introduce a full cycle collector.
- Require pervasive API changes in generated code or wasm ABI (unless unavoidable).

## Performance + Concurrency Constraints
- The steady-state runtime must not add new locks or dynamic dispatch in hot paths.
- Initialization/shutdown must be explicit and rare; no hidden work on every call.
- Async/coroutine and channel paths must remain zero-cost at runtime when the
  lifecycle is already initialized.

## Current Leak Sources (Non-Exhaustive)
- Builtin classes (`BuiltinClasses`) and their `__bases__`/`__mro__` tuples.
- Interned names (`INTERN_*`) and method tables (OnceLock values).
- Module cache, exception cache, last-exception tracking.
- Parse arena and TLS caches retain allocations until shutdown/thread exit.
- Capability cache and hash secret storage.
- Async registries (task exception stacks, cancel tokens, per-task maps).

## Proposed Architecture
### RuntimeState
Introduce a `RuntimeState` struct that owns all runtime-global state:
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

Expose a single global pointer (fast path) to the active RuntimeState:
- `molt_runtime_init()` allocates and initializes the state, then publishes
  the pointer.
- `molt_runtime_shutdown()` revokes the pointer and tears down all state.

### Initialization
- Idempotent initialization (multiple calls return success).
- Strict ordering: intern base names first, then builtin classes, then caches.
- Fail fast if initialization fails; do not leave partial global state.

### Shutdown
- Requires runtime quiescence (no running tasks/threads).
- Drains caches (module/exception, intern tables, method caches).
- Flushes TLS caches.
- Decrefs builtin classes, tuples, and method objects.
- Clears async registries and task metadata.

### Python frame namespace custody

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
functions are unaffected by replacing that entry.
Suspended tasks retain code and both namespace objects in
the existing auxiliary sidecar and restore them on every resume. Live frames,
frame snapshots, and lazy traceback payloads retain their exact namespace
edges. Source filenames and mutable module metadata are diagnostic data, never
namespace lookup authorities. GC traversal and retirement visit the same owned
edges, including aliased globals/builtins edges separately. Generator shortcuts
and binding share one constructor; resumes and suspended code/frame views use
the retained code. There is no function-address-to-latest-code registry.
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
`locals()` in optimized function frames reuses the live frame dictionary on
Python 3.12 and copies it on Python 3.13+ (PEP 667); module scope retains namespace
identity. The runtime target-version authority selects the behavior, not the
host interpreter. Copies use the shared dictionary-copy primitive, including
its ownership and allocation-failure behavior.
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

- `CanonicalObjectCache` owns the physical lifetime of fixed empty values,
  interned strings, Missing, NotImplemented and Ellipsis. One publication
  protocol installs the immortal refcount and flag before exposing a fixed
  singleton. Hits remain lock-free; failed initialization publishes nothing.
- Dictionaries, module caches, atomic caches and extension-state slots own
  ordinary references, not permission to make their referents mortal. Their
  teardown uses the same reference-release primitive as ordinary execution.
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

## Implementation Status (2026-04-30)
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
  Descriptor-cache lookup snapshots now retain exposed heap bits before leaving
  TLS cache custody, and `descriptor_bind` retains the descriptor across
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
- Signal handler slots, pending-delivery flags, and wakeup fd state are
  runtime-scoped through `RuntimeState.signal`. The raw C signal handler uses
  only a signal-safe atomic pointer to the active runtime-owned atomics; it
  does not own state. Installing Python-level handlers retains callable
  references, `getsignal()` returns owned callable references, and shutdown
  releases installed handlers after `atexit` while resetting wakeup/pending
  state.
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

## GC/Cycle Collection Guidance
Ref counting remains the primary strategy in Phase 1. A cycle collector or
tracing GC is a separate milestone, because it would touch object layouts,
write barriers, and reachability semantics. This plan explicitly prepares the
groundwork (allocation registry + lifecycle control) to make that evolution
safe and measurable.

## Safety and Concurrency
- `molt_runtime_shutdown()` must acquire a global runtime lock (GIL or
  equivalent) to block concurrent access while tearing down.
- TLS caches must be drained on all threads or tracked and reclaimed at
  shutdown (scheduler/sleep worker threads now participate in shutdown cleanup).
- WASM host environments must wire lifecycle entrypoints where applicable.

## Implementation Plan
1. Create `RuntimeState` and move high-risk globals first (builtin classes,
   interned names, module cache, exception cache).
2. Provide init/shutdown entrypoints and wire in CLI/tests.
3. Migrate TLS caches into runtime-managed registries.
4. Add optional allocation registry and leak reports.
5. Gate all runtime entrypoints on a valid RuntimeState pointer.

## Test Plan
- Unit tests invoke init/shutdown and assert that caches are cleared.
- Miri runs with leak checks must pass (no `alloc` leaks).
- Stress tests exercise init/shutdown in loops (no growth).

## Open Questions
- Should shutdown be required or optional in production binaries?
- Do we need a per-runtime allocator arena for fast teardown?
