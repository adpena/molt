# libmolt Extension ABI Contract
**Spec ID:** 0217
**Status:** Draft
**Owner:** runtime + tooling
**Goal:** Define the stable ABI boundary and bounded source-compat header model
for C/C++ extensions recompiled against Molt.

---

## 1. Principles
- `libmolt` is a recompile target, not a `libpython` drop-in.
- Stable ABI and source-compat are different promises and must stay separated.
- `MOLT_C_API_VERSION` also gates compiled layouts shared by the C facades.
- Compatibility overlays may grow to unblock real extension builds without
  implying CPython ABI compatibility.
- Private/generated upstream headers are never part of the `libmolt` contract.
- Ecosystem compatibility is about primitives, wiring, and integration. The
  extension ABI exists so upstream package extension sources can be recompiled
  against Molt, linked to Molt runtime symbols, staged through Molt package
  custody, and tree-shaken to the reachable user-program closure. It is not a
  mandate to recreate NumPy, SciPy, pandas, or other package APIs in Molt
  Python.

---

## 2. Contract Tiers

### 2.1 Tier A: Stable ABI
- Canonical header: `include/molt/molt.h`
- Contract:
  - opaque `MoltHandle`-based object model
  - exported `molt_*` runtime symbols
  - `MOLT_C_API_VERSION`
- Stability promise:
  - major-versioned
  - intended to remain small, explicit, and toolable
  - the only header tier that downstream code should treat as ABI-stable

### 2.2 Tier B: CPython Source-Compat Facade
- Canonical entrypoints:
  - `include/Python.h`
  - `include/molt/Python.h`
  - small legacy forwarding headers such as `datetime.h`, `frameobject.h`,
    `pymem.h`, and `structmember.h`
- Contract:
  - source-level compatibility shims for high-value extension code
  - maps `Py*` names onto `molt_*` runtime primitives, helper macros, and
    fail-fast stubs where semantics are still missing
- Stability promise:
  - bounded and documented
  - not a frozen ABI surface
  - additive source coverage may expand without a major bump; incompatible compiled layouts require one

### 2.3 Tier C: Package Header Custody
- Current focus:
  - `numpy/*`, SciPy, pandas, and other package-owned headers come from the
    package's own source/build plan include dirs.
  - Generated package headers and include-only sources are materialized by the
    upstream build system or source-plan custody, not checked into Molt.
- Contract:
  - unblock real-world ecosystem builds without shipping package header clones
  - fail closed when the package build/source plan lacks a required generated
    header, with evidence pointing to the missing package-custody artifact
- Stability promise:
  - package-owned header semantics track the package version being recompiled
  - Molt owns only the libmolt/CPython-ABI C API tier and package-custody wiring

### Runtime hook admission

Runtime hook registration admits the integer ABI prefix before reading any
callback. An incompatible magic, version, or table size returns failure without
reading the table tail; only the exact current layout permits a complete vtable
read. Rejected short or unaligned producer tables do not acquire runtime state.

### Managed iterator protocol

`PyObject_GetIter`, `PyIter_Check`, `PyIter_Next`, and the `None` advancement
branch of `PyIter_Send` project the runtime iteration authority for managed
objects. Physical builtin storage tags never replace live Python class lookup.
Native extension objects continue to use their native iterator slots, including
the sequence fallback. Iterator checks do not bind descriptors. `PyIter_Next`
discards normal completion payloads; `PyIter_Send` transfers their exact value.
Owned result status is separate from the bits, including a valid float +0.0.
Explicit list storage mutation remains separate from iterable materialization:
subtype overrides run for a distinct iterable RHS, not for the destination's
physical storage or the self-assignment snapshot.

Inherited scalar descriptors admit receivers by the canonical class hierarchy
and read their intrinsic payload through the shared numeric authority. Float
`__float__`, `conjugate`, `is_integer`, `as_integer_ratio`, and `hex` therefore
accept managed subclasses without invoking conversion overrides. An unrelated
object with a `__float__` method does not satisfy the float descriptor contract.
Scalar extraction preserves signed zero, NaN, infinity, and subnormal values.
The sealed inherited `ClassFieldKind::Intrinsic(ScalarValueKind)` row owns
tagged scalar storage: constructors select the expected kind, and payload
readers use the same typed row and allocation extent. An MRO scan or a
float-looking ordinary field cannot authorize a scalar read. The shared float
reader covers inline values, native float carriers, and one tagged Float
intrinsic without recursively following objects.

Storage recognition is separate from exact Python type identity and protocol
dispatch. `PyFloat_AsDouble` and explicit float descriptors read subclass
payloads; the float constructor dispatches a subtype's conversion override.
`sum()` admits only exact floats to its float fast phase. Source and operator
unary calls share subtype-first dispatch; range membership/count/index and
decimal percent formatting retain rich-equality and integer-conversion
overrides respectively. These consumers never infer protocol admission from
the physical `TYPE_ID_FLOAT` tag alone.
Mixed int/float subtype arithmetic enters the existing forward/reflected
dispatch authority before reading payloads. Inherited float arithmetic
descriptors normalize owned scalar carriers and invoke the same exact-builtin
operator implementation. This preserves reflected subtype priority and makes
`NotImplemented` terminal after the protocol is exhausted; it neither repeats
an override nor substitutes payload arithmetic for a declined method.

Semantic integer arithmetic and integer-format protocol results use the shared
integer-only projections, `index_i64_integral_bits` and
`index_bigint_integral_bits`. The codegen-tolerant `to_i64`/`to_bigint`
conversions do not authorize integer payload paths: inline floats, native heap
floats, and tagged Float instances all remain in the float family. Decimal,
integer-base, and character percent formatting validate `__int__`/`__index__`
results with the same arbitrary-width integer projection before consuming them.
Floor-division dispatch retains the caller's operator spelling, including `//=`,
when subtype protocols decline.

`index_integral_payload_bits` is the borrowed, validated integer carrier
projection shared by the fixed-width and owned BigInt readers. It reads at most
one sealed Int word and rejects float, nested-object, and malformed payloads
without allocating, retaining, or dispatching protocols.
Numeric formatting uses that projection for admission, integer payloads, and an
omitted presentation code. Classification and floating conversion do not clone
arbitrary-width integers; only actual integer rendering requests an owned
BigInt, while generic integer repr borrows its payload. Integer floating
presentations keep
the original receiver for `__float__` dispatch; integer presentations never call
conversion overrides. Generic `repr`/`str` select the native scalar base from
the shared projection before comparing subtype slots, then render the same
payload only after override handling. Empty format specs retain their `str`
route; explicit nonempty base `__format__` calls retain their payload route.
Physical BigInt/Float tags are not a second formatting admission authority.

### Shared target C data model and linked headers

Both Python header transports consume `include/molt/shared/`: one scalar object
layout, GIL-state enum, and target C data-model authority. `SIZEOF_VOID_P`,
`SIZEOF_INT`, `SIZEOF_LONG`, `SIZEOF_LONG_LONG`, `SIZEOF_SIZE_T`, and `LONG_BIT`
are preprocessing integer constants derived from the target standard headers,
not the build host. Conflicting definitions fail compilation. This distinguishes
Windows LLP64 from Linux/macOS LP64 and wasm32 ILP32 without equating C `long`
with a pointer-sized word.

For the linked CPython-ABI tier, install
`runtime/molt-cpython-abi/include/` and `include/molt/shared/`, and pass both as
include roots. Their relative location is not prescribed. Do not add the
source-compat `include/` root to resolve linked-tier dependencies. For the source
tier, install `include/` with its nested shared directory intact. Neither header
uses a source-checkout-relative path to reach another tier.

Extensions linking a shared runtime/ABI image define
`MOLT_CPYTHON_ABI_SHARED=1`. Both transports use the same `PyAPI_DATA` policy:
Windows imports data through the image's import library; static linking and WASM
retain plain external declarations. This option does not select a different
object model, load libraries, or search for an ambient Python installation.

`runtime/molt-cpython-abi/src/abi_types.rs` remains the Rust `repr(C)` authority.
`tools/gen_cpython_abi_layout.py` projects struct sizes, offsets, and integer
field widths for the target data models; the linked header and runtime C build
compile these assertions using the same target compiler/sysroot as the shims.
`tp_flags` is C `unsigned long`; `tp_version_tag` is C `unsigned int`.

This does not broaden version admission: Molt targets Python 3.12+ semantics,
but the linked header currently declares the CPython 3.12 object layout.
Source-extension admission must match that declared version; successful scalar
layout checks do not establish CPython 3.13/3.14 binary compatibility, package
support, or execution on an unverified target.

---

## 3. Explicit Exclusions
- No binary compatibility with CPython extension wheels.
- No promise that extensions using CPython private structs or direct object
  layout access will compile or run.
- No third-party package headers are shipped by Molt. Private/generated
  third-party headers must be provided through package/source-plan custody.
- No silent fallback to CPython or host Python at runtime.

---

## Value Presence At The Runtime Boundary

Runtime value handles are boxed Python values, not nullable pointers. In
particular, float `+0.0` has handle bits zero. Typed results use their status;
exception snapshots use their presence masks. Neither may infer absence or
failure from a present payload. Inactive snapshot slots must be zero and scalar
exception fields travel in separate native-width lanes.

`ExceptionSnapshot` in `runtime/molt-cpython-abi/src/hooks.rs` owns structural
layout/mask validation and present-edge enumeration for both capture and commit.
The runtime additionally checks field types and the recipient's immutable layout
before publishing the whole state. Capture, C projection, rollback, and commit
preserve each reference occurrence, including aliases, without a second validator
or payload-based ownership test.

Absent physical exception notes metadata and typed object members use one
internal missing marker. Explicit Python `None` remains a present value whose
C projection is `Py_None`; absence projects as `NULL`. Snapshot capture/commit
and collector detachment preserve that distinction and one reference per
present edge, including aliases. The reserved `PyBaseExceptionObject.notes`
field is separate from Python's ordinary `__notes__` dictionary attribute.
There is no synthetic builtin `__notes__` member or synchronization between
these independent stores.

Common and typed Python exception fields are native descriptors published on
their declaring builtin classes. Ordinary MRO lookup and descriptor mutation
own precedence for exception instances, including subclass overrides and
explicit object defaults. `add_note` obtains and publishes `__notes__` through
that same attribute protocol, accepts list subclasses, and rejects a present
non-list value, including `None`. Direct dictionary mutation and attribute
assignment therefore address the same Python notes value.

`ExceptionLayoutRoot::attribute_declarations` supplies the common and typed
declarations to both runtime descriptors and native C member/getset tables.
Runtime callbacks address typed storage by `ExceptionTypedField`; field names
do not select a second layout or mutation authority. Builtin exception creation
publishes its complete namespace once, and later namespace misses cannot
recreate deleted descriptors. Genuine C-owned exception receivers invoke the
declaring shell's physical member/getset through the shared descriptor boundary.
Cause and context accept either runtime exception storage with real ancestry or
native exception storage admitted by the ABI layout authority; assignment and
snapshot commit use the same predicate, and preserve the original object identity.

The ordinary instance dictionary location includes exception storage. Dictionary
materialization and replacement use the generic storage owner, whose exception
publication row refreshes the C view transactionally. `args` conversion uses the
shared tuple materializer. Lazy args publication returns failure explicitly;
failed projection leaves its previous state intact and never returns a retired
tuple. Projection failures preserve pending errors or raise `SystemError`.

Power's optional modulus follows the same rule: the C API's absent argument is
projected to canonical Python `None`; a supplied numeric zero remains a value,
and failed argument conversion remains failure. This does not change the
language's integer-only modular-power contract.

Numeric protocols share the bridge's observed-object classification: a managed
view is committed before observation, foreign identity stays explicit, and a
failed commit terminates dispatch. It must never become a foreign-slot retry or
an omitted argument. Physical numeric projection consumes that classification
and preserves aliased operand identity.

These contracts require behavioral proof at the actual runtime/ABI boundary;
host unit tests alone do not certify native/WASM or package compatibility.

### Extension initialization and callable ownership

Static native/WASM and the explicitly enabled dynamic bridge loader invoke
`PyInit` inside one runtime-owned initialization transaction. The caller's qualified
module name owns the import identity; a shared library's initializer symbol
uses only its final component. `create_module` receives the actual import spec
and returns the real C module without executing its slots. A normal import
publishes that same object before `Py_mod_exec`; a direct `exec_module` call
preserves the caller's cache and `sys.modules` state. Failure unwinds only the
transaction's own publication and references. Extension-raised exceptions keep
their type and identity; result/error contract violations produce `SystemError`
with the original exception as context. Frontend module bodies must not publish
extensions a second time. Reentrant or repeated loader execution does not replay
slots; direct C calls to `PyModule_ExecDef` may deliberately execute them again.
Multi-phase module state is allocated at first execution, not creation, and is
preserved across direct executions. Single-phase module construction does not
register an interpreter-owned state root; successful import commit owns it.
Slotted definitions cannot enter the single-phase `PyState` registry. Module
construction installs the definition's documentation through the shared module
API in both initialization modes.

`importlib.machinery.ModuleSpec` and extension initialization share one ordinary
runtime-owned class. Bootstrap-free initialization constructs that class, not
a module-shaped surrogate. A supplied spec retains its identity, and its
`parent` owns package metadata rather than a second name-splitting rule.

The runtime scopes and restores the initializer's package-name context even on
failure or nested imports. Only a matching single-phase definition's leaf name
consumes that context. The IR carries an owned module result, never a raw C
pointer across an exception edge. Native addresses and WASM table relocations
both call the same runtime initializer boundary.

Both public header transports share module-definition/C-callable layouts and
route module lifecycle and callable construction to the same linked ABI entry
points. `PyModuleDef_Init` establishes the canonical definition type; matching a
type name or guessing a struct from trailing memory is not initialization.
The WASM ABI generator reads the linked header's local include closure and
fingerprints those dependencies, so moving declarations into shared headers does
not remove their signature authority or leave cached projections stale.
Heap-type factories, readiness and type observers also share one linked ABI
surface and generated stable slot IDs. A source-header spec constructs the same
physical heap type as the linked header: basicsize/itemsize, native slots, GC
edges, selected metaclass and module ownership are admitted by the runtime.
Type dictionaries and name queries return owned references; defining modules
are borrowed. C dictionary keys, values and items return insertion-ordered list
snapshots through the shared retained-snapshot owner; Python dict methods retain
their live-view contract. Abstract mapping queries honor overridden methods,
preserve exact-list results across boxed and specialized list storage, and
materialize non-list results from their first iterator. Length hints are read
from that iterator, not the mapping method's output. A `TypeError` from the
first iterator acquisition names the mapping, method and returned type;
other lookup/iteration/materialization failures retain their original error.
All C dictionary operations admit managed dict subclasses through the runtime's
single backing-storage query. Its not-a-dictionary result is distinct from a
failed lazy backing allocation or invalid storage. Error-reporting operations
preserve the latter failure; `PyDict_GetItem` and its string form suppress new
errors while preserving the caller's raised error. Merge reads backing storage
directly only when the source retains `dict.__iter__`; an overriding subtype
uses its own `keys()` and `__getitem__` protocol. The exported
`_PyDict_GetItem_KnownHash` uses that same dictionary admission and borrowed-result
owner while accepting the caller's signed hash unchanged. It does not run hash
or hashability callbacks; cached-hash lookup retains equality, identity shortcuts
and restart after reentrant mutation. Absence is NULL without an error; lookup
failures retain the original error, as with `PyDict_GetItemWithError`.
Error-reporting string-key wrappers and required-result getters reject null
arguments with SystemError, retaining an already pending exception. Temporary
string-key cleanup preserves the selected error. `PyDict_SetDefaultRef` and
`PyDict_Pop` continue to accept an omitted result sink.

`PyType_GetModuleByDef` searches the complete MRO in
order and matches each defining module's exact `PyModuleDef`, including
multi-phase modules that have no single-phase registry entry. A missing match
raises `TypeError`. Neither header installs Python wrapper attributes as a
second native-slot or module-association authority.

Native embedding must link runtime and ABI into one image. Copying a hook table
between an ABI DLL and a statically linked runtime does not merge their object
identity, type singletons, exception state or finalization authority.
Managed semantic type queries use the runtime's actual class edge, shared with
exception projection. Builtin bindings and heap-class projections retain their
canonical identity; a generic physical carrier is not a second Python class.
The carrier's physical `ob_type` remains an honest layout discriminator, and a
failed class lookup stops dispatch while preserving the original exception.
Managed Python calls use the runtime call authority; semantic class identity
does not authorize invoking a native layout constructor on a managed view.
Concrete C callable layouts retain their C calling convention, and native
extension types retain their native slots. Positional and keyword runtime calls
recognize C wrappers by canonical executable identity and share one convention
dispatcher. External ingress acquires execution custody before the admitted
trampoline; already-admitted positional calls keep the generated native/WASM
transport. Each path owns one recursion/invocation guard. C arguments remain
borrowed: moved operands stay locally owned through the callback and release
in frame order while preserving both exact error channels. Dynamic-load diagnostics describe
the shared initialization transaction, not an inferred raw PyInit return;
the pending C exception retains the actual failure.

Both Python header surfaces use the same exported builtin exception objects.
The shared exception export header owns their declarations and direct-link or
host-symbol addressing. Evaluating a `PyExc_*` name is an address operation:
it does not allocate, inspect or replace a pending exception, or cache an
interpreter-owned class handle. Host lookup returns the object's address,
not the address of a pointer variable. The canonical process-owned shells
retain their addresses while runtime registration and restart rebind their
semantic state. Custom exceptions still use `PyErr_NewException` and the
ordinary class hierarchy. `EnvironmentError`, `IOError`, and the Windows-only
`WindowsError` alias resolve to `OSError`; `ExceptionGroup` remains internal
and is not a public C data symbol.

Both Python header surfaces declare the same linked exception-state and
attribute APIs. The raised indicator owns the exact exception instance, class
and traceback; handled-exception state remains distinct. Raised-instance
transfer steals or returns the existing owned reference without reconstructing
the exception. Transport must not add a raise-site traceback or replace an
existing context. Native and managed exceptions use the same owned field
projections; physical layout determines only how the shared field schema is
stored. Typed-field batches convert and retain all inputs before publication,
then release replaced owners after the complete new state is visible. Replacing
a raised slot with the same exception still releases the old slot's reference.
Public raised-error transfer gives the C indicator precedence
when both channels are populated. Temporary-owner cleanup instead detaches and
restores both exact channels independently, including allocation-free emergency
runtime errors; it does not project C views or materialize lazy tracebacks.
The synchronous cleanup hook keeps ownership on the caller and runtime stacks,
drains cleanup errors, and restores both channels before resuming a Rust panic.
Printing and unraisable reporting consume the same selected raised error as
fetching it. Native finalizers, clear callbacks and type watchers detach both
outer channels before executing or reporting callback failures. Cold C-view
publication uses the same preservation boundary; cached views retain their
direct path. Builtin exception rendering slots inherit from the shared schema
before namespace readiness so readiness failures can themselves be formatted.
Delayed native and test snapshots use the same runtime raised-state authority.
Native callback completion detects either raised channel without requiring C
error projection. Operand publication continues after a failure while preserving
the exact first error, including emergency runtime state. Non-consuming C error
queries borrow the actual pending class without consuming or normalizing the
instance or materializing its traceback. An emergency MemoryError maps directly
to the canonical C type even before runtime class bootstrap. Consuming instance
queries still require a materializable exception instance; an emergency without
one remains pending instead of becoming an unrelated synthetic exception.
For a genuinely native pending exception, the instance owns its exact native
class; querying that class does not allocate a runtime class wrapper.
`PyErr_SetObject` and `PyErr_NormalizeException` use the generic subclass
protocol when deciding whether to retain an instance. `PyErr_Restore` requires
the exact class. These rules remain distinct from callback-free exception
handler matching and physical receiver admission. All normalization entries
share bounded recursion and preserve constructor failures; unavailable bootstrap
allocation cannot turn an arbitrary input into a normalized exception.
First C exposure of a heap exception class still requires fallible bridge
storage. If that publication fails, the original runtime exception remains
pending but its C type query can return NULL; universal allocation-free error
observation is not yet established.
Emergency and already-bound classes do not need that first publication.
Physical exception fields and `PyCFunctionObject.m_module` own ordinary C
references, including direct `Py_XSETREF` replacements and same-pointer stores.
Their current pointers are independently traversed by shared GC and released
once on refresh, clear or teardown. Cycle clearing first detaches runtime
payload slots, then publishes NULL in ordinary physical C fields and defers
their decrements through one fixed-field retirement resource. This includes a
callable's `m_module`, so self-module cycles can retire before terminal view
destruction. Bound receiver and defining-class mirrors retain their existing
terminal ownership order. Only private mirrors already represented by
runtime edges enter the projection-reference ledger; publicly writable fields
cannot depend on a ledger update that direct C writes do not perform.
Physical traversal and native `tp_traverse` share one edge classifier. Both
managed and native children retain every owned edge occurrence. The collector
discounts private mirrored references for either target representation; public
GC introspection also retains inline values that cannot form cycles.
Builtin list, tuple, dict, set, frozenset, module, traceback, exception and type
GC slots project that same runtime inventory. Public traversal snapshots and
pins every edge occurrence before invoking C visitors outside storage locks;
the first nonzero visitor result is returned unchanged. Immutable frozenset
and traceback storage does not acquire a mutable clear policy. Readying these
types does not admit unsupported raw builtin carriers: native allocation rejects
inherited managed traversal or clear slots even when a subtype overrides the
other slot.

Module definitions contribute typed `m_traverse`, `m_clear` and `m_free`
callbacks at ABI registration; opaque runtime definition identities are never
dereferenced. Module-state edges join the mixed graph through the same physical
edge classifier. Local capacity and tracking visits remain callback-free.
Nonterminal callbacks pin their module owner and lease metadata outside the
registry lock. Collection pins its complete candidate cohort before traversal
and discounts those existing GC pins only for reachability; introspection uses
ordinary temporary owners. Public referent inspection snapshots its argument
sequence and retains yielded values before extension code can reenter.

Module clear invokes `m_clear` before changing the namespace and preserves a
nonzero callback status and the unchanged namespace. Successful clear reserves
against post-callback storage, publishes emptied local fields, then releases
detached owners. Terminal `m_free` is claimed once before callback entry, after
the resurrection verdict and before the canonical C view or state is retired;
`PyModule_GetState` and `PyModule_GetDef` remain available during that callback.
Callback failures have their own collector/introspection status rather than
being classified as allocation failures. Automatic collection isolates and
reports callback errors while restoring the caller's pending error channels.
Terminal `m_free` errors use formatted unraisable reporting with `object=None`;
the callback's borrowed module view must never become a Python owner after
death has been committed. State and definition queries remain borrowed during
the callback.

The process ABI bootstrap owns builtin C storage and descriptor slots exactly
once. Type factories and extension loading enter that bootstrap; neither may
reset live builtin shells, their flags or their runtime-owned dictionaries.
Runtime retirement and rebinding remain separate from process storage setup.

Class constructors declare semantic origin (heap/static), namespace immutability,
and subclass admission independently of their instance shapes, dictionary/weak-
reference support, and physical slots. These facts occupy the existing class
policy/declaration words. Static origin is exact-class state, never inherited.
The shared construction transaction finishes namespace and layout, applies the
semantic policy, then publishes the atomic cache identity. Builtin-bank membership
governs anchor custody; it is not a mutation, annotation, or subclassing policy.

The source baseline is CPython v3.12.0, v3.13.0, and v3.14.0: static native types
become immutable during readiness; heap specs independently select immutability
and BASETYPE. Mappingproxy, method, cell, capsule, and frame-locals proxy are static
non-basetypes; SimpleNamespace is a static basetype. Partial is a heap immutable
basetype; comparison keys, LRU wrappers, and operator getters/callers are heap
immutable non-basetypes. ExceptionGroup and the public io base wrappers are
mutable heap basetypes. The public io ABCs inherit separate immutable native
`_io._IOBase`, `_RawIOBase`, `_BufferedIOBase`, and `_TextIOBase` classes;
concrete I/O classes register virtually with the public ABCs. Public namespace
mutations cannot reach concrete native classes through their physical MRO.
The native bases, concrete I/O implementations, and internal Molt-only file
class are immutable heap basetypes. Other builtin constructors publish their native initializer's BASETYPE
fact before publication.

The canonical exception schema owns each class's origin and module. Asyncio's
CancelledError and I/O's UnsupportedOperation are mutable heap classes. The
`_io` and `io` facades share the runtime-owned UnsupportedOperation identity;
`io` publishes its public module metadata. Exact class declarations preserve
schema identity across renames, without classifying exceptions by mutable
names. Class-level lookup follows the same namespace and MRO as instance lookup.
Bank-owned constructor, comparison, and iteration shortcuts require immutable
namespaces; mutable bank classes use ordinary descriptor dispatch.
Errno promotion requires the exact canonical OSError class; subclasses cannot
opt into promotion through a rename. Implicit unhashability from an own
`__eq__` declaration is published once during namespace construction. Later
`__eq__` mutation does not disable an inherited hash implementation.

Annotation reads and deferred evaluator admission require HEAPTYPE, not mutable
namespaces. Immutable heap classes can materialize/read annotations but reject
assignment/deletion; static classes reject annotation access. Structural C metadata
discriminator 6 projects HEAPTYPE, IMMUTABLETYPE, and BASETYPE together. Managed C
views use compact PyTypeObject storage for static types and real PyHeapTypeObject
storage for heap types. Heap name/qualname fields mirror runtime owners through
the existing projection ledger, are updated with the semantic name setters, and
retire with the existing type-view root inventory. tp_name's CString stays owned
by the managed view across mutation. The process-owned internal ExceptionGroup
shell has full heap extent; its ht_* and allocated name owners retire and rebuild
through the existing static-shell lifecycle.
Mapping proxies, frame-locals proxies, methods, simple namespaces, capsules,
cells, partials, comparison keys, LRU wrappers and operator getter/caller types
are immutable classes. This does not make their instances immutable.
`DynamicClassAttribute`, `ModuleSpec`, compiled-loader classes, `CacheInfo` and
the Python-style LRU decorator factory remain mutable classes; native physical
storage is not an immutability declaration.

The `types`, `functools` and `operator` cache families use the same callback
drain and canonical-class root selection. Callback-bearing and mutable owners
detach before releasing references; immutable class anchors remain published
through the existing runtime-class retirement fixed point. That transaction
pins their cohort, closes weak references, clears namespaces, validates and
detaches class identities, then releases cache anchors. Static C bindings such
as `PyDictProxy_Type` enter that same cohort. Retirement never filters a bad C
binding or admits arbitrary immutable heap classes to hide a publication error.

Both C headers share physical tuple/list prefixes and one linked container and
call API. Tuple, list, dict, set, mapping, slice, argument builders and call
packers retain physical heap types, modules and exception objects through the
existing edge-custody bridge; they never serialize a foreign `PyObject *` as
zero managed value bits. Mutable container writes and borrowed/new references
follow the canonical ABI owner. Consuming setters retire the supplied reference
on failure as well as success; builders detach consumed items before cleanup
and preserve the selected exception across extension destructors. `N` units
remaining after a failed variadic build are consumed once. The dual-header
type-factory fixture exercises these element identities, calls and failure
owners before its physical base and module-MRO checks. Raw `GET_ITEM` macros
observe physical slots; unchecked `SET_ITEM` stores keep displaced references
and can restore a slot to NULL through the same publication transaction.

Argument parsing has one format grammar, physical input custody and cleanup
ledger. Keyword values are read at each conversion point so converter callbacks
can change later arguments; cleanup-supported converters, exported buffers and
allocated encodings retire when any later conversion fails. Positional-only
parsers check arity before invoking converters. Both set constructors use a
tagged omitted/present input, so NULL is distinct from Python None and float
zero. Dictionary pop and deletion share one owned removal result from the
runtime lookup; no lookup-then-delete path re-hashes extension keys.

Spec-built native heap types allocate through the selected metaclass's
`tp_alloc` and retire through the matching `tp_free`. Class, MRO, base, module,
name and slot owners participate in traversal. Base inputs normalize to one
owned tuple before metaclass and physical-layout selection. Member declarations
are copied into metaclass-owned trailing storage, and relative data and native
dictionary offsets share the checked allocation layout authority. Failed
construction cuts partial self cycles while preserving the original exception.
Default heap-subtype lifecycle owns finalization, member/dictionary edges and
the instance's class reference before delegating to a builtin or extension base.
Base payload destruction shares collector retirement and the actual type's free
slot; bridge numeric carriers retain their separate proven allocation domain.
GenericAlloc publishes tracked GC objects, while raw GC allocation remains
untracked until construction publishes it. GC finalizers use the collector's once-only finalized
state; non-GC resurrection permits finalization on the next terminal attempt.
Spec-type allocation has no Rust `Box` allocation or intentional-leak lane.

Physical type storage is independent of mutable `tp_flags`. Managed C views use
their allocation enum; the ExceptionGroup process shell uses its exact typed
storage declaration; native type allocations carry their immutable minimum
extent in the existing generation-owned type lifecycle record. Generic allocation
records its actual checked byte count. A spec factory captures the allocating
metaclass, selected allocator, member offset and requested extent before calling
`tp_alloc`, and admits its returned storage under that allocator's CPython
contract. Correct custom allocators, including ones using raw C allocation,
remain supported; returning less than the requested extent violates the C
allocator contract. Later metaclass or public flag changes cannot enlarge the
recorded extent. Free and successful realloc revoke the storage generation.

Readiness of an unknown static extension type may temporarily expose HEAPTYPE
(as Cython does); it inherits protocol table pointers and never manufactures an
inline heap tail. All heap-tail readers, metadata publication and physical owner
retirement consume the same storage admission. Heap name ownership survives a
cleared HEAPTYPE bit. Bridge hierarchy, dictionary and name projection stage their
complete fixed owner inventory before committing to the same retained identity;
fallible preparation and displaced-owner release occur outside bridge locks.
The inherited canonical type GC slot retains CPython's public HEAPTYPE result;
internal foreign-object custody uses that same physical storage admission.
Custom heap-type allocations enter the existing collector untracked before
readiness can publish self/MRO/descriptor edges, and are tracked only after
construction completes. Rejection before payload admission uses header-only
terminal cleanup. Arbitrary extension frees must observe both the storage
generation and collector identity revoked before returning the allocation.

Subclass registration order belongs to the same generation-owned registry.
Retirement removes membership and bounds ordered tombstones by the live cohort,
with geometric compaction and storage reclamation; an empty cohort is removed.
This does not depend on a later `PyType_Modified` call. Invalidation consumes that
membership in registration order rather than reconstructing a second live set.
Its queued work retains allocation generations and revalidates them at expansion
and invalidation; a callback-retired address cannot be re-admitted by traversal.
Both traversal phases require a current generation and nonzero reference count
under the runtime execution token. Invalidation and type lookup additionally
require checked retention through the canonical runtime lifetime authority;
a positive internal teardown pin does not admit a terminal managed object.
Refused private type lookup returns no attribute; direct metatype attribute
slots instead raise `SystemError`, preserving the slot's NULL-with-error contract.
A type already in deallocation is skipped
without revoking its registration, preserving legitimate resurrection; live
subclasses retain their bases. Watched types retain an owner across callbacks
and unraisable reporting, outside the registry lock. Each watched callback slot is resolved immediately before
dispatch, so earlier callbacks can clear or replace later slots. Reallocation
revokes an old address before calling the
allocator, including the shared raw/object allocator entries; initialized types
are not valid realloc inputs.

Type-namespace lookup hits and misses without an error reassign invalidated
version tags. Failed lookups preserve their exception and leave the tag invalid;
this error propagation is a deliberate Molt divergence from CPython's private
lookup, whose consumers clear such errors. Best-effort tag admission preserves
both exception channels even when a physical base tuple is malformed.
Watcher registration does not fail solely because a version tag cannot be
assigned. Invalid or unset watcher IDs and non-type watch/unwatch arguments
raise `ValueError`; watcher-table exhaustion raises `RuntimeError`, matching
the pinned CPython contract. Watcher callbacks belong to one runtime lifetime;
bootstrap clears the prior lifetime's slots without reusing allocation or
version generations.

These are lifecycle requirements, not an additional support or performance
claim; target acceptance remains tied to retained consumer proof.

Managed C views derive immortality from the canonical runtime refcount, mapped
to the selected CPython ABI encoding during publication. Physical allocators
preserve that lifetime; empty storage never makes a heap subclass immortal.
Owned and borrowed crossings and C-function publication share this rule.
Explicit C immortality retains the stable runtime hold as a process-lifetime
root. Runtime-owner and finalizer transitions do not perform arithmetic on the
immortal C encoding; counted references retain checked bias arithmetic. C API
refcount promotion absorbs private mirrored reference counts into that root.
Canonical runtime singletons retain their existing shutdown-only retirement.
Public ownership queries count runtime and direct C owners, discounting only
the bridge hold and private mirrors of runtime edges. Unique ownership alone
does not establish temporary operand-stack provenance. Deferred refcounting
and temporary-reference optimizations explicitly decline when that capability
is unavailable. `PyUnstable_SetImmortal` returns success only for a uniquely
owned non-Unicode object and removes its cyclic-GC membership. Subsequent
mutation cannot re-enroll a managed immortal root.
Both C headers route canonical-object GC controls through the linked ABI and
the runtime's membership and finalization authorities. Explicit Track/UnTrack
changes membership without recounting allocations; new native GC allocations
remain untracked until their fields are initialized and Track is called.
Runtime allocation accounting is a lifetime claim in the existing object header,
independent of current membership and payload eligibility. Explicit untracking,
container demotion and cycle clearing preserve that claim; terminal retirement
consumes it once. A foreign wrapper remains enrolled until retirement even when
clearing has detached its native pointer. Retirement never rechecks cleared
native pointers or class storage to decide whether an existing identity exists.
Compact integer and boolean lists acquire collector membership and that same
one-shot allocation claim when their physical storage becomes a generic list,
before reference-bearing mutation or C view publication. Explicit tracking uses
the same admission boundary; every admitted identity can retire independently of
the constructor or projection that exposed it.
Private source-header buffer objects retain their distinct registered layout;
they cannot be cast to canonical `PyObject` storage or silently claim GC support.
Bridge resolution rejects these registered pointers before Foreign protocol
dispatch or wrapper publication, using the existing runtime membership owner.
The source header still owns their representation-only refcount and `Py_TYPE`;
their explicit `molt_c_heap_*` buffer lease API supplies no Python class or slot
table. Both Python.h facades use the canonical `Py_buffer` prefix and linked
buffer/memoryview functions. Private registered storage must not be passed as
a `PyObject` exporter; genuine native exporters use `bf_getbuffer` and
`bf_releasebuffer`, with metadata owned by that exporter or the linked lease.
Native allocation failures set an exception at their origin, including size
overflow, unsupported GC layout and failed collector-identity publication.
Cleanup preserves that exception and releases temporary heap-type ownership.
Both headers also share the linked allocation and initialization functions:
raw allocation honors the type's physical size, variable initialization sets
its real signed size, heap instances retain their type, and `GenericNew`
dispatches the type's allocator. These primitives never invoke a Python
constructor as an allocation substitute. Size getters and setters access the
shared variable-object layout and evaluate their operands once.
Native descriptor, slot-wrapper and callable completion share the operand
synchronization boundary. Direct operands remain owned across reentry and are
committed before runtime observation, including partial writes on callback
failure. The original callback exception takes precedence over a synchronization
failure; a synchronization failure after callback success remains an error.
Ordinary attribute access and explicit generic access share
the runtime lookup kernel with an explicit policy; generic access bypasses user
get/set overrides while retaining descriptor and dictionary semantics.

Descriptors found by native lookup use the same semantic boundary: managed
values bind and mutate through the runtime's live descriptor protocol; genuinely
native descriptors invoke their declaring C slots. Physical carrier slots do
not determine the protocol of a managed descriptor. Probe failure is distinct
from absence. Optional receiver/owner operands and deletion have explicit
presence, so C NULL never aliases Python `None` or a numeric value. Explicit
declaring-slot calls retain their exact callback and do not redispatch it.

Both C headers share all twenty linked synchronous object observation and
conversion entry points: type/class inquiry, hash/callability, truth/comparison,
str/repr, length/size, bytes, format and dir. Native slots, descriptor callbacks,
owned results and pending errors use those same owners. Bytes conversion shares
the runtime byte constructor without integer-count semantics; dir uses the
runtime special-method/materialization/sort authority. Format failures never
fall through to another protocol. `PyObject_Dir(NULL)` remains an explicit
unsupported caller-frame operation; asynchronous iteration is a separate API
frontier and is not covered by this synchronous surface.
Structural `PyObject_TypeCheck`/`PyType_IsSubtype` use actual type identity and
the complete MRO; `PyType_CheckExact` remains an identity macro. These checks
do not invoke `__class__`, `__mro__`, equality, or metaclass query overrides.
`PyObject_IsInstance` and `PyObject_IsSubclass` retain the separate Python
class-info protocol: tuples and unions short circuit in order, exact actual-type
instance matches precede metaclass hooks, and lookup/callback failures retain their
original exception. Successful inquiry values survive native callback error
validation; action-status
normalization must not turn a true class query into a false result.
The default type checks use physical MROs for real type pairs;
abstract class objects participate through observable tuple-valued __bases__ and
instance __class__ lookup. Derived admission precedes target admission, missing
attributes and malformed bases remain distinct from callback failures, and
ancestry compares identity without equality or query-hook recursion. Single-base
chains traverse iteratively; branching consumes the shared recursion budget.
Managed and native tuples share ordered traversal with owned elements and no
sequence overrides. These public rules never admit unsafe layouts or exception
classes: their physical checks remain callback-free. Hash and callability use
special-method/slot authority rather than ordinary attribute presence.
Containment on managed C views uses the runtime's canonical Python membership
protocol, including class overrides, hash admission, byte substrings and range
arithmetic. Native C objects retain `sq_contains` and the ordinary iterator
fallback. Count/index share one owned iterator search for every representation;
no storage-tag classifier or sampled-length exact-list search governs them.
Equality callbacks can clear, shrink or grow a list: each step owns its item,
observes live traversal, and preserves the original exception through cleanup.
Runtime list index retains its requested stop rather than clamping it to the
pre-callback size, and both C header transports call these same linked APIs.

Truth inquiries on native objects retain their physical slot authority across
runtime wrapper transport: `nb_bool` precedes mapping and sequence length slots.
Metaclass hook and comparison results use that same inquiry; a failing native
truth slot preserves its original exception instead of producing a boolean.
`PyObject_RichCompare` returns the owned comparison result unchanged, including
non-booleans; `PyObject_RichCompareBool` alone folds truth and applies its
documented equality/inequality identity shortcut. Reflected subtype priority,
NotImplemented fallback, truth errors and operand/result ownership belong to
the linked protocol. Header-local dunder calls and class/MRO reconstruction
are removed, including their unused type-classification helpers.

Native class readiness establishes the inherited metaclass, direct bases and C3
method resolution order before publishing declaration descriptors. Managed class
views project the runtime's original bases and MRO through the bridge's existing
ownership ledger. Both representations use the shared C3 and solid-layout-owner
selection policies; neither derives semantic inheritance from names or a primary
base chain. Native slot inheritance follows the completed MRO and preserves the
coupled comparison/hash, GC/free and vectorcall rules.

A runtime binding alone does not make a native shell a managed projection.
Managed projection ownership requires the exact physical view identity. Native
aliases retain ordinary C roots and complete native declaration readiness while
preserving the runtime's canonical bases, MRO and shared dictionary. Bootstrap
READY flags do not certify completed namespace publication.

Recursive physical projections share publication custody. A type and its MRO
tuple cannot become independently ready while either still depends on the
other's construction skeleton. Failure clears their owned edges while the
allocations remain pinned, then retires both identity directions and runtime
holds. Independent completed projections, including error objects, survive an
unrelated construction failure. Opaque C-function fields are retained by
identity; publication must not semantically read or commit their contents.

A native method table publishes method or classmethod descriptors, or a
staticmethod constructed by the canonical runtime class. Existing namespace
entries win unless the declaration requests `METH_COEXIST`. Inherited methods,
members, getsets and slot wrappers resolve through the MRO; readiness does not
copy declaration tables or fabricate a metaclass call slot for unresolved types.
These are implementation contracts; the declared native/WASM/version/profile
matrix still requires source-bound execution evidence.

Managed `str` and `repr` also use the runtime protocol through owned-result
hooks, preserving subclass behavior, result identity and exceptions without
an ABI-local scalar/string formatter. Native objects retain native slots.

Unicode C exports use the bridge object's stable, terminated 1/2/4-byte
codepoint projection. Raw codepoint access does not request strict UTF-8.
Construction commits through the canonical runtime string boundary before a
runtime consumer observes it; encoding uses the runtime codec and error policy.
Internal Python text retains surrogate codepoints and embedded NUL. Strict
UTF-8 is required only at specified boundaries, including class `__name__`,
codec names and traceback source-column translation. Failed admission preserves
the original structured exception and does not publish partial metadata.

The current writable projection admits unique open `PyUnicode_New`
constructions. Writing an existing unique, unhashed exact string remains an
implementation and release gap; this is not a reduced Unicode compatibility
promise. Native and linked-WASM execution must qualify these paths separately.

`builtin_function_or_method` and its `builtin_method` subclass have canonical
ABI type bindings. Runtime-defined builtins carry real vectorcall storage;
they do not fabricate a native `PyMethodDef` or receiver. Semantic type checks
and vectorcall work across both representations. `PyCFunction_Check` and raw
`PyCFunction_Get*` implementation extraction admit concrete C-defined callables
only; requesting raw C metadata from a runtime-defined builtin raises `TypeError`.
That extraction is not part of the verified runtime-builtin surface.

Module method tables use the same `PyCFunction` construction and managed-view
lifetime as directly constructed C callables. Runtime container ownership must
retain the concrete C layout and its receiver/class/module edges; dropping a
temporary C reference cannot turn it into an opaque object or leave dangling
member pointers. The physical `m_module` member is the sole `__module__`
authority for managed C functions; runtime attribute reads, assignment,
deletion and cycle GC consume that same owned edge. Ordinary Python function
metadata retains its runtime protocol, not a second name registry.

Attribute mutation has an explicit deletion flag at the internal hook boundary;
the C API's NULL deletion sentinel never aliases the Python value `0.0`.
Successful runtime mutation returns Python `None`, while its C adapter returns
status zero. Foreign-call failures transfer the original C exception after
error-preserving temporary-owner cleanup. `PyDict_SetDefaultRef` permits a NULL
result sink without acquiring an unused result reference; this source API
contract does not expand the declared CPython binary-layout version.

---

## 4. Tooling Contract
- `molt extension build` must record the targeted header contract in
  `extension_manifest.json`.
- `molt extension scan` must evaluate support against an explicit, curated list
  of contract headers rather than an unbounded recursive header crawl.
- Public overlay growth must stay compile-validated with representative source
  probes; adding a header to the contract does not imply runtime/ABI parity.
- C-API scan green is only the first gate. A package support claim must also
  prove source compilation, object link, package-native artifact staging,
  import execution, module-state lifecycle, deterministic runtime behavior, and
  binary closure for the claimed reachable path.
- Build/link tooling must model object closure explicitly: compile and link only
  extension objects, symbols, data tables, generated C/Cython outputs, and
  runtime features proven reachable from the user's entry program and admitted
  package imports. Whole-package linking is not an acceptable substitute for
  missing reachability facts.
- Tooling must report the distinction between:
  - stable ABI headers
  - source-compat headers
  - excluded private/generated headers

---

## 5. Practical Scope
- The goal is not “compile all Python extensions” in the CPython sense.
- The goal is:
  - compile extensions that can be recompiled against `libmolt`
  - preserve a narrow stable ABI core
  - add bounded source-compat overlays for high-value ecosystems
  - make high-value ecosystems green by improving shared ABI/import/storage
    primitives, not by cloning their Python APIs locally
  - reject private/generated upstream build dependencies unless Molt chooses to
    ship an explicit compatibility overlay for them

This means:
- simple or Limited-API-style extensions should converge on `molt/molt.h` plus
  a small facade set
- high-value ecosystems such as NumPy may require additional source-compat
  overlays
- extensions that fundamentally require CPython internals remain out of scope
  for `libmolt` and belong, if anywhere, in the explicit bridge policy lane

---

## 6. Relationship To Other Specs
- C-API v0 surface: `docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md`
- C-API symbol coverage: `docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md`
- CPython bridge policy: `docs/spec/areas/compat/contracts/cpython_bridge_policy.md`

## Canonical membership and sequence search

Python membership and managed `PySequence_Contains` share runtime special lookup,
element equality, byte-buffer admission and set lookup. Memoryviews search typed
elements through the ordinary iterator; they do not perform substring searches.
Bytes and bytearrays share index conversion followed by the `PyBUF_SIMPLE` buffer
protocol, including its specified replacement of failed index conversion errors.
Python set membership converts a mutable-set needle after TypeError into a temporary
frozenset. `PySet_Contains` uses the same storage lookup with exact-key admission.
`PySet_Discard` uses the canonical deletion owner directly, hashes the key once,
and distinguishes absent keys from errors using the pending exception. Both C APIs
continue to reject unhashable keys. Other exceptions retain their original identity.
Byte membership uses the shared supported buffer authority; managed PEP 688
`__buffer__` admission remains unimplemented there.

List and tuple search bounds use target-width saturating `__index__` conversion,
ordered start before stop and stopped at the first exception. Negative list bounds
normalize after both callbacks; positive bounds remain live across element equality.
C sequence count/index and native containment fallback retain owned iterator/items.

## Canonical slice storage

Python slice construction and linked `PySlice_New` create the same runtime
three-field object. The C bridge publishes a real `PySliceObject` prefix with
retained start, stop, and step projections; exact C constructor arguments keep
their physical identity. NULL constructor fields become Python None. Slice
construction does not invoke `__index__` or normalize bounds. Subscription and
the documented `PySlice_*` normalization APIs retain their own conversion and
error contracts, including the legacy `PySlice_GetIndices` distinction.

Slice projections participate in the existing recursive publication and mirrored
reference ledger. The runtime owns the three semantic GC edges, and projection
retirement releases its mirrors with the selected exception preserved. No
ABI-owned slice allocation, foreign-slice adaptation registry, or container-local
key conversion exists. RuntimeHooks version 51 includes the Python containment query, typed dictionary backing and supplied-hash lookup queries, the exact slice
constructor and borrowed-field observer, and admits `PyNumber_Index` through
the existing typed numeric dispatch. The separate serial RuntimeVtable is unchanged.

Type-only special-method presence and descriptor binding share one retained MRO
lookup. Presence never executes a descriptor. Managed index providers follow
the runtime's integer protocol; native providers retain their `nb_index` slot
contract. Native wrappers use the same raw type lookup for index, float,
iteration, subscription, descriptor, and async protocol admission.

## Compiled buffer layout admission

C-API major 5 removes the private public-header `Py_buffer` tail. Both headers
now use the CPython prefix and one linked implementation. Extensions compiled
with earlier headers must be rebuilt: an old inline `PyBuffer_FillInfo` can write
beyond current caller storage. Build, audit, package/link admission, seal, and
runtime loading require the current layout major. Sealing never changes a
compiled artifact's version declaration. `include/molt/molt.h` is the version
authority; missing or malformed headers fail admission.

Memoryviews created by Python or either C facade are runtime MemoryView objects.
Their BridgeEntry owns a stable C descriptor projection. Native export leases
are shared runtime resources, visible as mixed GC edges and retired after view
invalidation. Clone, slice, cast and read-only views retain the same transaction.
FromBuffer copies borrowed descriptor values with a null base and leaves raw
storage lifetime to its caller. Indirect/suboffset descriptors remain rejected.
