# Import bedrock: single module dictionary cutover

Status: source implementation; native/WASM runtime qualification remains pending.
Owner: module initialization, publication, import and runtime lifecycle.
Registry schema 5 (`MOLTMOD5`) carries explicit file-location and stdlib-provider
provenance in the existing flags byte; its native/LLVM/WASM projections share
that authority. `ModuleSourceRole` is projected by the canonical metadata
constructor from resolved compiler source paths and the actual stdlib root.
The superseded generated import dispatcher is removed; public imports use the
canonical import transaction directly. Required metadata/worker/cache/registry
inputs carry the role unchanged. A module name, path spelling or optimization
hint never grants a provider/bootstrap role.
The 56-byte header and 48-byte rows also carry ordered namespace portions in a
separate table. Schema/magic admission
rejects incompatible images instead of treating a missing fact as false.

`ModuleTable` owns one ordinary Python dictionary from the first module
publication, before `sys` exists. Bootstrap exposes that same object as
`sys.modules`; it does not copy entries or replay another store. This follows
[CPython 3.12.15 Python/import.c, `_PyImport_InitModules` and `_PyImport_GetModules`](https://github.com/python/cpython/blob/v3.12.15/Python/import.c#L168-L196).
The earlier table-backed dict-view proposal is superseded: ordinary dictionary
primitives already own insertion order, hashing/equality callbacks, arbitrary
keys/values, iteration, C views and GC edges. Adding another dict representation
would duplicate those obligations without demonstrated benefit.

## Storage and lifetime contract

- `module_dictionary` borrows the interpreter-owned dictionary. Failed allocation
  does not publish a sentinel as a successful generation. Registry metadata is
  initialized separately, so embedding/bootstrap publication needs no registry.
- `module_lookup` uses ordinary callback-safe dictionary lookup and returns one
  owned value. No backing borrow crosses a callback. Arbitrary public values pass
  through imports; `None` halts an import with `ModuleNotFoundError`; absence
  enters admitted initialization or reports a genuine miss. Import failures
  use the same structured import-exception constructor as `IMPORT_FROM`:
  `ModuleNotFoundError.name` owns the admitted missing name (or the provider's
  missing dependency), independently of message rendering; `path` and
  `name_from` default to `None`. Metadata is pinned through allocation and
  published transactionally before the error is raised. This adds temporary
  reference holds and, for host-only diagnostic names, one Python string
  allocation on the error path; successful cached imports do no extra work.
  Cache-only C `PyImport_GetModule` misses retain their distinct null/no-error
  contract. A malformed compiled private body which returns no namespace is
  an internal execution-producer failure, separate from public lookup absence.
- The immutable generated `ModuleRegistry` owns compiled names and initializer
  addresses. Dense metadata owns initialization state and temporary original-spec
  and initializer namespace references. One lazy transient per-name execution
  lock authority coordinates registered and arbitrary names; it owns synchronization
  records only, with no module, spec or other Python-object edge. `Ready` and `Replaced` rows
  retain no permanent object reference. There is no module-object Rust HashMap,
  adoption fallback, bootstrap replay or completed-slot shadow store.
- Public literal/dynamic/fromlist/isolate imports share the existing dictionary
  observation and the runtime-owned ordered finder core. The `_frozen_importlib`
  and `importlib.util._find_spec` facades share that core; the public
  `util.find_spec` parent/cache helper remains independently rebindable. A compiled row
  is a candidate body, not a proof that user `sys.meta_path` finders may be skipped.
  The compiled provider applies its role-qualified availability fact when
  consulted. The shared actual body dispatcher enforces provider absence and
  execution dependency failures after selection, including explicit source
  loaders and aliases which execute that admitted catalog body by identity.
  The synthetic `__main__` row is the fixed compiler-selected program entry
  body, with a direct source-execution role rather than an advertised stdlib
  provider. Its named entry row retains the actual provider flag. No arbitrary
  named body can acquire this capability: only the backend's entry constructor
  binds the synthetic row to the one entry initializer. Explicit file execution
  remains distinct from named module availability.
  FileFinder declines a potentially unavailable stdlib provider only after
  actual candidate admission and a positive physical target-path match to that
  row's catalog origin. Target filesystem canonicalization owns aliases and path
  syntax; an unavailable historical build coordinate proves no match. This is
  catalog-origin identity, not a claim about immutable runtime file contents.
  No compiler root is serialized or treated as a runtime filesystem capability.
  Only these cold availability candidates pay up to two path resolutions;
  standalone/relocated compiled-finder admission performs no filesystem work. Explicit internal catalog
  execution remains limited to startup/provider/import-support bodies. Python
  functions defined by support modules use ordinary public imports. The backend
  no longer rewrites literal public transactions into numeric initialization;
  there is no name-specific finder-priority exception or post-failure fallback.
  Warm cache/spec-probe work is unchanged. A cold miss now dispatches the finder
  and admitted spec transaction directly; the removed speculative catalog attempt
  saved finder work only by assuming an unproven binding. Cost qualification uses
  the existing native/WASM public cases and cached-import/allocator probes.
- Successful ordinary initialization returns the final public value, including
  a replacement or `None`; deletion raises `KeyError` without resurrection. The
  existing dictionary pop/reinsert operations move the completed entry after
  nested imports, following [CPython 3.12.15 `_load_unlocked`](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py#L911-L952).
  The initializer and original spec stay pinned through callbacks; the final
  value has its own owner. Publication, failure cleanup, pop and reinsertion read
  the original spec's current `name` at their separate observable cutpoints.
  Mutable metadata never chooses the execution lock or initializer identity. This completion step is cold and adds one ordinary pop and reinsertion;
  it does not run on a cached import. Public importlib loader reads of the
  `sys.modules` attribute remain distinct from interpreter dictionary C access.
- Recursive/foreign imports use the initializer transaction and the single
  all-name execution lock/wait graph. A visible recursive partial is returned from the dictionary. Deletion
  during initialization admits a fresh same-thread initializer; the existing
  execution snapshot temporarily retains and restores outer execution custody
  without restoring public membership. Explicit runpy/loader execution owns
  its namespace in its execution context and never reserves/restores an ordinary
  import row. Its tagged cold identity enters the same typed body dispatcher;
  ordinary imports performed by that body still read public membership.
  Name-wide suppression and the reserved-row state are removed. Its optional
  `run_name` swap remains a distinct public execution operation.
- Raising inside a compiled initializer or callback only publishes the pending
  exception. Its lexical frame owns context exits/unwind-to-depth; the explicit
  task-terminal scheduler boundary retains whole-task cleanup. A nested failure
  cannot consume a caller's active context manager before its handler runs.
  Raw native/WASM body dispatch preserves the same scalar handler-stack baseline
  as runtime callable dispatch (one cold TLS read/restore, no context snapshot or
  warm-import work). A callee's redundant exception-handler pop at its baseline
  cannot consume the caller's handler; after baseline exit the caller retains
  its ordinary pop and pending-error ownership.
- `PyImport_GetModuleDict` borrows the interpreter dictionary through the
  required `RuntimeHooks.import_module_dict_borrowed` field (ABI 74), including
  before sys bootstrap. Public `sys.modules` attribute rebinding/deletion does
  not retarget C-API or compiled imports; `PySys_GetObject` still reads the
  current sys attribute with its existing exception policy.
- The native compiled loader now enters that same explicit body execution;
  its displaced-cache/import/dictionary-copy lane is removed. Fresh spec loads
  preseed the exact target and join the existing registered ordinary initializer
  owner before its body. The same all-name execution lease serializes compiled
  initialization, arbitrary fresh spec loads and existing-target `_exec`; the
  row/spec/snapshot owner retains only registered initializer namespace custody.
  Direct `exec_module` owns neither an import row nor a name execution lease.
  As in CPython, `exec_module` sets no module metadata: the file, zip,
  extension, sourceless and compiled loaders only run the body, and
  [`module_from_spec`](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py#L806)
  is the one `_init_module_attrs` site. A compiled body executed into a
  loader's module keeps each prologue value (`__file__`, `__package__`,
  `__path__`, `__spec__`) the module already has and stores none it lacks;
  spec admission ends that prologue window, so later stores are the body's own.
  One divergence remains open: a module without a `str` `__name__` gets one from
  its spec or loader, where CPython's file loaders raise (`_check_name`).
  Typed initializer admission owns once-only execution, so publication installs
  the exact supplied namespace instead of retaining a different first winner.
- The compiled Python importlib facade owns the complete outer reload protocol:
  initial public identity admission, the transient `_RELOADING` namespace dict,
  parent/finder discovery, `module.__spec__` assignment, named errors and final
  lookup under the outer reload name. Reentry returns the retained original
  reload target only after the caller's initial membership guard. A `finally`
  removes the marker on every exit. The three-operand reload intrinsic is only
  `_exec(module, spec, spec.name)`; its Exact target is rechecked under the same
  all-name lock after discovery and cannot become a fresh load or replacement.
  Current-target loader shims select their target at that boundary. `_exec`
  keeps metadata, loader callbacks and final pop/reinsert in one finally scope,
  without changing `_initializing`. Presence probes and final callable reads
  remain distinct descriptor observations. The same rule governs
  `module_from_spec` create/exec presence and `spec_from_loader` package probing:
  required spec.loader reads cannot become optional, present non-callables reach
  the call error, and actual callbacks are read after the presence probe.
  Package probing suppresses only ImportError and passes its owned result to the
  actual spec constructor, preserving constructor truth errors and timing.
- Creation and `_exec` share one spec-to-module metadata phase. Creation keeps
  factory-provided name/loader/package/path values, while `_exec` explicitly
  overrides them. Spec is assigned separately; package path shares the actual
  locations object; `has_location` governs file/cached. Only assignment-scoped
  AttributeError is ignored. An implicit namespace reuses the existing Python
  NamespaceLoader class through `__new__`, retaining exact `_path`, publishing
  `spec.loader`, and assigning file None under its separate error policy.
  The existing class has one `_path` owner; implicit construction keeps the
  supplied locations identity, while its public constructor retains its
  existing path conversion. `importlib.machinery` ports CPython 3.12
  [`_NamespacePath`](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap_external.py#L1325)
  and `NamespaceLoader(name, path, path_finder)`: `PathFinder.find_spec` returns
  a namespace spec with origin None whose locations are
  `_NamespacePath(name, portions, PathFinder._get_spec)`, which recomputes its
  portions when the parent path changes or `invalidate_caches` advances its
  epoch. Compiled namespace packages carry those ordered portions in a typed
  registry row, without a generated source stub or initializer. The compiled
  finder uses the same `_NamespacePath` class and a loader-less spec with
  origin `None`; module-from-spec binds `NamespaceLoader` and shares its path
  object with the spec. Namespace identity is never derived from location flags.
  `ModuleSpec` starts with `has_location=False` even for a descriptive origin.
  File-location factories and compiler source SSA explicitly set the fact;
  catalog entries require it from the producer, encode flag `0x08`, and include
  it in the registry digest. Generated native and runtime-builtin entries do
  not infer file provenance from their kind or origin spelling. Alias metadata
  follows its actual terminal provider. No runtime filesystem probe is added.
  Molt's file-backed sys/builtins source facades carry that same file fact in
  both compiler and catalog metadata. This describes their compiled source
  implementation; it is not parity with CPython's builtin sys/builtins origins.
- ABI 74 also requires `RuntimeHooks.import_get_module`: `PyImport_GetModule`
  is a cache-only owned lookup, with no import-on-miss or dictionary fallback in
  the ABI layer. `None` is a successful cached value for this C API. Public
  spec getter/truth callbacks stay observable on warm imports. Python importlib
  propagates probe errors; C import and GetModule clear them for target 3.12 and
  propagate them from 3.13, following the corresponding `Python/import.c`.
  Primary references follow the latest patch of each supported minor, as pinned
  by the release authority. Latest 3.13.16 and later revalidate dictionary identity
  after the public probe, including a false flag, following
  [3.13.16 `Python/import.c`](https://github.com/python/cpython/blob/v3.13.16/Python/import.c)
  and [`_bootstrap.py`](https://github.com/python/cpython/blob/v3.13.16/Lib/importlib/_bootstrap.py).
  Loading imports retry through existing bounded import admission; GetModule
  returns Missing without loading. Older 3.13.12 and 3.14.3 retained false-path
  observations and remain historical regression references, not latest-patch
  qualification oracles. Source policy alone does not establish any
  target/version release-acceptance cell.
  Python's busy path first acquires `_ModuleLockManager`, retains that scoped
  ownership through its public dictionary observation, then performs the separate
  `_lock_unlock_module` wait. A cycle at the first admission raises the single
  `_frozen_importlib._DeadlockError` subclass of RuntimeError; the C import and
  GetModule `_lock_unlock_module` boundary accepts a partial module on that cycle.
  This distinction follows the latest pinned
  [3.13.16 Python import scope](https://github.com/python/cpython/blob/v3.13.16/Lib/importlib/_bootstrap.py)
  and [C initialization scope](https://github.com/python/cpython/blob/v3.13.16/Python/import.c).
  The exception's existing schema/cache owns its class and teardown; frozen
  payload and bootstrap bindings project that same identity, without exporting
  it in builtins. Discovery and execution admissions propagate the same class.
  Explicit schema publication also excludes CancelledError and UnsupportedOperation
  from builtins, while their existing asyncio/_io class lookup producers and
  runtime raise identities remain unchanged. Class origin and public namespace
  membership are separate declared facts; the frontend drift projection reads
  the same explicit binding declaration.
- The compiler supplies its exact spec SSA value through
  `molt_module_initialization_spec`; ordinary `__spec__` assignment cannot claim
  that producer role. The original spec owns True/body/False lifetime even when
  the body replaces module metadata. It is retained only in the lazy active-init
  record and moved with existing nested execution snapshots. Foreign waiting
  follows the owned public value's actual spec truth, with exact namespace
  identity covering only the pre-metadata admission phase. A completed nested
  replacement, arbitrary value or None cannot inherit another namespace's wait.
- Ordinary body failure deletes the current name and restores its original error
  after any cleanup KeyError, including one raised by user key equality. Other
  cleanup errors replace and implicitly chain the body error. Exact extension
  rollback preserves its incoming error instead. Both policies use the existing
  raised-owner transfer and the ABI's single cycle-safe context-field algorithm;
  spec-finally setter errors use that same chaining authority.
- `InterpreterSysNamespace` retains the interpreter's actual sys role.
  `InterpreterBuiltinsNamespace` retains the actual builtins dictionary, as
  CPython's interpreter builtins role does. Public module deletion/replacement
  cannot retarget either role. Active frame builtins remain authoritative for
  `PyEval_GetBuiltins`; the no-frame result borrows the interpreter role.
- Publication and removal defer displaced edges until dependent initialization
  metadata is coherent. Ordinary import-body failure deletes the current public
  name, including a nested replacement or `None`, following `_load_unlocked`.
  Extension construction rollback removes only its own admitted publication;
  explicit loader/runpy execution keeps its separate public-cache policy.
  Typed Extension publication and Some rollback cannot inherit an enclosing
  private compiler body's CompilerNamespace/None policy.
  Expected-value removal resolves callback-safe key equality once, compares the
  final row value and detaches without a second equality callback. Quiescent
  metadata owns no membership fact and performs no post-removal lookup.
- Shutdown detaches dictionary entries and transient transaction slots before
  namespace finalizers. Exact stored-key hashes retire entries without replaying
  user hash/equality. Core cohort capture inspects actual stored Unicode-name
  rows and independently pins the interpreter roles; it cannot invoke key
  equality or import machinery. Ordinary, sys and builtins cohorts reach a callback fixed
  point in that order. Semantic roles and the dictionary root retire once.

### Finder bootstrap and callback custody

The interpreter publishes the native catalog finder with its initial `sys.meta_path`
list. A typed pending-default record temporarily owns that exact finder list and
initial hook list, plus their existing backing mutation epochs. Machinery's actual
admitted stdlib body submits its defining globals to finish this generation;
arbitrary public cache objects cannot supply its classes. An ordinary missing
machinery publication uses the ordinary initializer. An existing cache replacement
remains authoritative public membership while the same body dispatcher executes
bootstrap support privately. No copied namespace or second module store is adopted.
`FileFinder` likewise retains its defining globals rather than rediscovering a
class provider through `sys.modules`. Custom list replacement, `None`, in-place
clear/restore and custom hooks retain their public policy; conditional list mutation
checks the captured epoch under the canonical backing commit lock. Completion,
cancellation, failure and shutdown detach the pending owners before releasing them.
Both platform retirement dispositions are published before Python finalizers run.

The ordered core follows pinned
[3.12.15](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py#L1144)
and [3.13.16](https://github.com/python/cpython/blob/v3.13.16/Lib/importlib/_bootstrap.py#L1144),
[3.14.8](https://github.com/python/cpython/blob/v3.14.8/Lib/importlib/_bootstrap.py#L1147)
and [3.15.0](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap.py#L1103).
3.12 traverses the live iterable; 3.13+ copies it before truth/warning and reload
membership observations. The same interpreter-owned physical reentrant lock backs
`_imp` and each finder attribute/call scope. It is released before post-callback
public `sys.modules` and `__spec__` reconciliation. Descriptor/call errors release
it; manual release of an unowned lock raises `RuntimeError`. Native waits release
the GIL through the existing RLock authority; single-threaded WASM uses its
existing reentrant backing. No state/list mutex spans a Python callback.
An empty path calls the actual public `_warnings.warn` attribute, including
rebinding, noncallable values and callback errors. Acquisition reuses the existing
support loader. The compiler carries `_warnings` as a runtime-import support root
(`_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES`), and the `_warnings` body bootstraps its
own imports as the `importlib` bodies do, so the warning loads with an empty
`sys.meta_path`. Nothing loads `_warnings` at startup. `_warnings` is the
provider and `warnings` its consumer, as in CPython 3.12: `Python/_warnings.c`
defines `filters`, `_defaultaction`, `_onceregistry`, `_filters_mutated`, `warn`
and `warn_explicit` and never imports `warnings`, and
[`Lib/warnings.py`](https://github.com/python/cpython/blob/v3.12.15/Lib/warnings.py#L566)
imports them from `_warnings`. Molt's `_warnings` binds the runtime engine's
objects (`builtins/warnings_ext.rs`) and defines `warn` and `warn_explicit` over
the engine and its location authority, so an empty-path warning does not load
`warnings`. Until `warnings` is loaded, the engine applies its own filters and
writes to the `sys.stderr` object, as CPython's `get_warnings_attr(..., 0)` and
`show_warning` do.

Completed cache hits add no finder, bootstrap, lock-map or filesystem work. A cold
finder traversal owns its three arguments, iterator and current observations;
3.13+ also allocates an O(number-of-finders) list copy. Each finder adds one
uncontended reentrant acquire/release, with no wait syscall unless contended.
Initial support is finite compiler-admitted source, selected by the existing
reached import-demand closure, not every microbinary. Pending bootstrap owns two
list references and their existing epochs only until a terminal disposition;
conditional append preparation copies those small initial lists once. These are
source-level costs, not measured runtime acceptance. The canonical caller-identity
catalog marks public low-level import intrinsics with the same runtime-import
protocol demand fact, independently of nullary target properties; unused, dead or
rebound callable names cannot introduce that source capability.

## Preserved external boundaries

The `module_cache_get/set/del` operation/intrinsic names still have real
producers and public/backend/C-API consumers. They retain their semantic lookup,
publication and removal boundaries while consuming the sole dictionary; they
are not obsolete storage identities and are not retired by this change.
Trusted extension phase/snapshot behavior, parent attribute publication,
recursive imports and public dictionary mutations reuse their existing owners.
Native and WASM enter the same dictionary/state implementation; WASM still
projects `ModuleId` through its generated dispatcher.

## Cost and verification boundary

The removed lane allocated Rust name keys, acquired a module-cache mutex,
retained duplicate object edges and replayed the full map during sys bootstrap.
The replacement retains one dictionary root and uses the existing dictionary
hash/probe on warm imports, plus the existing typed initialization admission.
Normal cached compiled imports already performed this ordinary dict lookup in
baseline `b14f5a8ab`; they did not bypass it through a READY-slot load. The change
removes sys-attribute lookup and dictionary-root retain/release there, plus
completed-slot projection work. Pre-sys bootstrap now uses the same dictionary
instead of the private-map/slot path; that changed cold cost must be measured.
Publication acquires one dictionary edge and temporary transaction ownership;
completed rows release that temporary owner. Uncontended publication adds no
forced syscall, process spawn, serialization, dict-layout branch, journal or
second module store.
The execution-lock table replaces the dense owner array and per-thread blocking
map; it is lazy and retains names/records only for admitted owners and waiters.
Completed false-flag public hits never consult this table. Uncontended cold
admission uses two short manager-lock sections (record pin, then ownership),
with a third on release; a new active name allocates a shared name and record.
Registered initialization's ownership checks also use that same manager and
name hash while its transient state is active. Removing the dense owner array
saves eight bytes per catalog row. The interpreter retains the fixed lazy-manager
slot, and the active records have platform-dependent Condvar/layout costs that
must be bound to the actual artifact's layout receipt. Reentrant execution reuses that record. Each
record contains an owner/depth, waiter count and Condvar. Contention releases the
GIL before acquiring the wait mutex and parks on the owner predicate under that
same manager mutex; no periodic timeout, sleep polling or repeated GIL entry is
used. Final owner release notifies only actual waiters; terminal retirement wakes
all admitted records. The wait graph uses an allocation-free linear cycle walk.
Python's busy path has the two scoped acquisitions required by its source
protocol; C busy paths have one.
A false-flag hit has neither. The internal deadlock class is materialized once
through the existing exception cache on its first raise or frozen-payload request;
it creates no synchronization record or completed-name retention.
No manager lock spans Python callbacks or GIL reacquisition. Every admission,
including rejected/cycle/error waiters, removes its exact wait edge and can retire
its ptr-equal record, preventing map-only or Weak tombstone retention. Hash-table
bucket capacity is bounded by peak simultaneous admission; completed names and
Python objects are not retained. `_RELOADING` adds one transient Python dictionary
edge per active public reload and removes the replaced Rust outer protocol;
normal cached imports do not touch it.
The original-spec array adds eight bytes per catalog row only when the existing
initialization record is first allocated, with cold atomic capture/release and
no completed-row owner. One runtime-scoped TLS identity is set/restored per
initializer; cached imports do not consult it. Warm public imports now perform
the observable spec/flag attribute and truth protocol; no immutable-binding
proof currently permits skipping it. The optional attribute helper enters the
existing typed attribute core directly, removing four redundant runtime entries
per two-lookup probe. Cold metadata and initializing-flag setters also use the
existing descriptor mutation core under their held execution token, avoiding
exported-setter entries while preserving descriptor callbacks and errors.
Target 3.15 also follows the shared metadata change in
[CPython `_init_module_attrs`](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap.py)
and [`runpy._run_code`](https://github.com/python/cpython/blob/v3.15.0/Lib/runpy.py):
module-from-spec and `_exec` skip the entire module/spec cached predicate and
projection; script execution skips `__cached__ = None`; runpy still reads
`spec.cached` but does not publish it. Existing factory/init-globals cached values
remain unchanged. The implementation and reference controls do not substitute
for actual 3.15 target qualification.
Identity revalidation for target 3.13 and later adds a dictionary
lookup after that probe. These costs require actual cached-import measurements;
source-level removal of mirrored storage is not a no-regression claim.
Shutdown performs a linear entry census and exact-key deletion, with bounded
per-pass vectors and no dictionary reinsertion. Actual before/after latency,
allocation and resident-memory qualification must use the same source/profile
and workload; no timing win is inferred from source counts.

The existing registry schema/digest/projection and initialization-owner Python
checks remain authoritative. Runtime controls cover bootstrap dictionary
identity, one owned lookup, absence of completed-slot ownership, public
replacement/deletion/None/arbitrary values, recursive and foreign initialization,
failed extension initialization, loader restoration, parent callbacks and ordered
namespace shutdown. The C-API controls cover pre-sys embedding and mutable sys attribute isolation.
The no-frame builtins controls destroy/unpublish the module, including from a
colliding key equality callback, while retaining the exact interpreter dict.
Rollback controls distinguish a callback replacement from the owned old value,
reject a second equality lookup and check replacement refcounts across retries.
The consolidated Rust graph and actual native/WASM consumers qualify behavior;
source parsing and Python structural checks alone do not close HF-276.
