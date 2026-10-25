# Molt Runtime Spec

## 1. Core Principles
The Molt runtime is a minimal, high-performance library written in Rust. It provides the essential primitives for Python semantics while minimizing overhead.
- Dynamic execution policy: compiled binaries intentionally avoid unrestricted
  `eval`/`exec`, runtime monkeypatching, and unrestricted reflection lanes by
  default. Any future widening must be capability-gated and justified with
  reproducible performance evidence (see
  `docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md`).

## 2. Object Representation: NaN-Boxing
Molt uses 64-bit NaN-boxing for all objects. This allows small primitives to be stored inline without heap allocation.

Numeric presentation shares one runtime formatting owner for floats, complex
components and percent formatting. General notation uses the rounded decimal
exponent; omitted float types retain their distinct decimal-point rule. NaN
payload signs are not observable presentation signs. PEP 682 negative-zero
coercion applies after rounding, and explicit alignment remains independent of
the zero-fill flag. These are shared semantic requirements; verified target and
Python-version coverage remains defined by the verified-subset receipts.

`round` selects the type's `__round__` before interpreting a digit argument;
instance attributes and callable arity do not select this protocol. Omitted or
None digit arguments invoke the method with no arguments. Inherited int/float
descriptors and exact builtin fast paths share the numeric rounding owner.
Integer descriptors use arbitrary-precision `__index__` conversion and the
existing integer-power admission; float descriptors clip through the shared
target-sized index conversion before handling NaN, infinity or extreme counts.
Explicit `int.__round__(value, None)` is rejected on Python 3.12/3.13 targets and
accepted on 3.14, while builtin `round(value, None)` always omits the argument.
Rejected index results and callback exceptions retain the
canonical conversion diagnostics. Signed ties-to-even decimal multiples share
one decision across integer carriers and negative float digit counts.

Integer byte conversion publishes its defaults once in the native callable's
owned default tuple: `int.to_bytes(length=1, byteorder='big', *, signed=False)`
and `int.from_bytes(bytes, byteorder='big', *, signed=False)`. The common codec
binder reads that tuple, rejects positional `signed`, and reuses the shared
Clinic keyword admission. Omission uses the internal missing sentinel for big
endian order; it does not allocate a default string or consult host byte order.
Both operations admit byteorder as Unicode before evaluating signed truth,
then validate the exact case-sensitive contents. Converter failures preserve
the original exception and prevent later conversion callbacks.

`from_bytes` follows `PyObject_Bytes`, including subclass `__bytes__`, buffer and
iterable conversion. Exact immutable bytes decode directly; exact bytearray
storage uses the existing counted export owner. Normalized conversion storage
retires before the requested integer subclass constructor, and the intermediate
integer retires after that borrowed constructor call. This uses the ordinary
runtime conversion and ownership mechanisms in standalone native and WASM
binaries; no host interpreter is involved.

The common integer encoder has an explicit empty-signed-output policy. A Python
3.12 target permits `(-1).to_bytes(0, signed=True)`; Python 3.13/3.14 targets
raise `OverflowError` because an empty output cannot hold its sign bit. Only
that zero-width negative case consults the guest version; ordinary nonempty
conversions perform no version query. The explicitly published five-argument
3.12 `_PyLong_AsByteArray` C ABI and its physical integer-layout projection keep
the 3.12 policy independently of the guest Python version. This distinguishes
guest language semantics from the admitted extension ABI; it adds no Python
3.15 support claim. The existing bigint differential owns Python call semantics,
and the common encoder's byte/status controls exercise both empty-output
policies. Actual verified cells remain defined by their target proof receipts.

These rules follow pinned CPython's [3.12.13 Clinic argument converters](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/clinic/longobject.c.h),
[3.12.13 integer codec bodies](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/longobject.c),
and the sign-bit validation in [3.13.16](https://github.com/python/cpython/blob/cbc944f4bc59639a444dd971c737788ba2283a91/Objects/longobject.c)
and [3.14.8](https://github.com/python/cpython/blob/8e6e75d9102e39bed2a2b279203a396741180f12/Objects/longobject.c).

Heap objects are referenced directly via tagged 48-bit canonical pointers.
Typed native handles are managed heap objects whose declared owner operations
retain and retire the Rust payload. The payload's Arc or raw Rust allocation
must never itself be tagged as a Python heap pointer. Native-handle declarations
also govern whether Python edges are permitted; an opaque Rust owner cannot hide
untraced Python references. Any separately admitted immediate resource IDs remain
distinct from managed object pointers and follow their own registered custody.

### 2.1 The Bit Scheme (64-bit)
- **NaN Space**: `0x7FF0000000000000` to `0xFFFF000000000000`
- **Pointer (object)**: `QNAN | TAG_PTR` occupies the high bits; the low 48 bits carry an unsigned address. Unboxing masks the tag without sign-extending bit 47. Boxing rejects addresses outside this payload in every profile; decoding also requires the address to fit the target pointer width.
- **Int (64-bit)**: If it fits in the signed 47-bit inline range, stored inline. Otherwise, a heap pointer to a `BigInt`.
- **Float**: Standard IEEE 754 double (non-NaN values).
- **Bool/None**: Specific bit patterns in the NaN space.

```rust
pub enum MoltObject {
    InlineInt(i64),   // 47-bit signed
    Float(f64),
    Boolean(bool),
    None,
    Pointer(u64), // 48-bit pointer payload (strings, bytes, lists, dicts, etc.)
}
```

### 2.2 Native struct sequences and platform records

A native struct sequence has a direct tuple base and one sealed declaration of
its field names, documentation, visible length and constructor role. Public
class attributes describe this layout; changing those attributes cannot change
physical allocation or the collector's reference graph. Only these classes own
the optional class-specification slot; ordinary class allocation is unchanged.
Visible tuple items and hidden native fields retain their own Python edges
through the existing native-instance storage, tracing and retirement owners.
Validation traverses metadata once before publication. Field access and hidden
item bounds use constant work; construction, traversal and materialization scale
with the actual number of fields. The selected class allocation adds one payload
word and may cross an allocator size class; it is not a zero-cost representation.

Original C type names use the existing native-module capture and C-name owner.
Module or qualified-name rebinding does not change that creation name; a
successful heap-type name assignment, including an equal-name assignment,
retires the capture. Static and heap mutability remain governed by the canonical
class semantic policy. The public C struct-sequence factories, physical hidden
item access, constructor, representation and reduction all consume this same
layout. External static type shells use the existing binding transaction,
publish Ready last and restore the shell on preparation failure. These rules
follow pinned [CPython struct sequences](https://github.com/python/cpython/blob/v3.15.0/Objects/structseq.c),
with target-version constructor policy retained for the supported versions.

`pwd` supplies the seven-field `struct_passwd` through this factory on admitted
Unix targets. Its three queries and the native user/group lookup consumers share
one lower platform owner over reentrant NSS lookups. Returned strings are copied
before native scratch storage retires and use the startup filesystem codec at
the Python boundary. Password enumeration serializes the process cursor and
copies native records before releasing it; no Python callback runs under that
cursor lock. Missing entries, failed index conversion and filesystem-encoding
errors retain their distinct protocols. Import availability is a target fact;
query capability admission precedes operand callbacks. Windows and WASI do not
gain a synthetic password database. The source authority is
[CPython's password database provider](https://github.com/python/cpython/blob/v3.15.0/Modules/pwdmodule.c).

Filesystem stat results project the admitted path, descriptor or VFS query's
`PathMetadata`. Signed nanosecond timestamps are the sole time facts. Integral
sequence seconds use floor division, float attributes use the seconds and
fractional remainder, and nanosecond attributes never reconstruct values from
floats. Native optional fields come from that acquisition owner. VFS identity
fields remain explicitly unknown.

Native integer projection follows the selected CPython source and target C ABI:
link counts, block counts, block size, flags and generation use signed `long`.
Device acquisition retains the unsigned `long long` view; the shared projection
preserves native `NODEV` semantics. `st_dev` uses this device convention, while
`st_rdev` uses signed `long` for 3.12 and the device convention from 3.13 onward.
The distinction follows pinned
[3.12 stat conversion](https://github.com/python/cpython/blob/v3.12.13/Modules/posixmodule.c)
and [3.14 stat conversion](https://github.com/python/cpython/blob/v3.14.3/Modules/posixmodule.c).
The platform crate owns native widths; the record publisher owns target-version
selection. Both runtime and path satellites consume the same `PathMetadata`
layout and must be rebuilt together after its physical fields change.

Each acquisition constructs the final native record directly, with no
intermediate tuple or Python reconstruction. The existing runtime class-cache
inventory owns its lazy type, roots and retirement. Public `os.stat_result`
rebinding cannot change the class of records returned by either `os` or
`pathlib`.

Windows ordinary paths keep the existing metadata query. A reparse candidate
is reacquired through one owned metadata handle with the original follow intent;
the initial snapshot is discarded. Descriptor queries borrow their current
handle. The actual attributes and tag come from
[FileAttributeTagInfo](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_attribute_tag_info),
never from a guessed reparse kind. Query failures propagate. Metadata handles
permit read, write and delete sharing and close on success and error. The
reparse slow path adds one handle open/close, a full metadata query and a tag
query; ordinary path work is unchanged, and descriptors add a tag query only
for a reparse record. Windows execution and cost remain unverified, and existing
identity limits do not establish full Windows stat parity.

These representation and ownership contracts require native, WASM, C-ABI and
platform execution receipts for acceptance. Development CPython source/oracle
tests do not establish emitted-binary behavior and do not introduce an
interpreter into compiled applications.

## 3. Memory Management

### 3.1 RC + Incremental GC (Baseline)
- **Reference Counting (RC)**: The primary mechanism. Every heap object has a 32-bit RC in its header.
- **Biased RC**: Objects that are predominantly owned by one thread use biased RC to avoid atomic overhead.
- **Cycle Detection**: The shared collector follows declared heap edges and
  retires unreachable cycles through the same ownership and callback protocols.
  Its scheduling and collection phases belong to the GC design below; native
  owners cannot hide Python edges from that traversal.

Readable and writable buffer acquisition share one scoped descriptor/lease
owner. Exporter callbacks complete before consumers borrow storage addresses;
release happens after consumers finish, with the original error visible. Ordinary
retained memoryviews own their storage or native export until retirement.

Raw stream callbacks have a different lifetime: the caller may mutate or release
the input after `write` returns, as required by
[RawIOBase.write](https://docs.python.org/3.14/library/io.html#io.RawIOBase.write).
A temporary view exposes that storage without copying only during the callback:
readonly for `write`, writable for `readinto`. Its scope expires before the
caller's export is released. Retaining a
Python view or deriving another view cannot prolong that export; access after
scope expiry must fail before reading storage. Releasing one view during the
callback does not expire already-derived views. Scope edges retain only the
existing scope anchor, while actual buffer exports keep their counted protocol.
Native pointers obtained for the callback have that documented call lifetime;
subsequent descriptor release must remain safe. This contract introduces no
second storage registry or eager input copy.

Memoryview context entry returns the same unreleased view; exit ignores
positional exception information and invokes that existing release owner.
Active native exports refuse release atomically, while derived views retain
their independent lifetimes. Repeated exit remains safe, and release callbacks
observe the original view as released before reentry. These native descriptors
follow pinned [CPython memoryview semantics](https://raw.githubusercontent.com/python/cpython/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/memoryobject.c)
and participate in ordinary context-manager dispatch.

Buffered readers and writers share one operation owner, traced raw edge and
scoped callback-view authority, following pinned
[CPython buffered I/O](https://raw.githubusercontent.com/python/cpython/3bb231a6a5dc02b95658877318bf61501a7209e9/Modules/_io/bufferedio.c).
Reader construction separates allocation from initialization; arbitrary Python
raw streams implement the same public protocol as native files. Reader data
lives in the existing bounded buffer and cursor. Large reads use direct output
spans, `read1` and `readinto1` limit raw calls, and `peek` preserves the cursor.
Buffered reads and in-buffer seeks return before locking; actual raw callbacks
use the shared reentry/concurrency owner without holding a payload borrow or
mutex across Python. Exact native bindings avoid callback views and operation
allocation; binding changes enter that same protocol. Position facts stay on
the handle, avoiding a raw position query for known in-buffer seeks.

`RawIOBase.read/readall` use native defaults over virtual methods, following
[CPython's base I/O protocol](https://raw.githubusercontent.com/python/cpython/3bb231a6a5dc02b95658877318bf61501a7209e9/Modules/_io/iobase.c).
Target-version selection governs count validation and the Python 3.15 consumed
bytearray contract. Unsafe historical out-of-storage reads are rejected.
Streaming reader working storage is O(buffer size + requested output); callbacks
add a scoped view per raw call. Byte construction may copy accumulated output
through the existing inline-bytes allocator; no zero-copy output or throughput
claim follows from direct ingress. TextIOWrapper's buffer protocol and Python
raw BufferedRandom support remain separate acceptance obligations. Test,
oracle and receipt machinery runs only on the development side.

### 3.2 Memory Management Roadmap
RC is predictable but adds per-write overhead. We plan to evaluate:
- **Generational tracing GC** for short-lived objects (lower average overhead, better cache locality).
- **Hybrid RC + tracing** (RC for deterministic release of FFI buffers; tracing for graph-heavy Python objects).
- **Region/arena allocation** for compiler-internal short-lived objects.

Determinism constraints: GC triggers use explicit byte/epoch budgets. Finalizers
and release callbacks preserve admitted CPython semantics, including reentry;
cleanup must not expose partially retired ownership or replace the original error.

See `docs/spec/areas/runtime/0009_GC_DESIGN.md` for the concrete hybrid design and targets.

### 3.3 Header Layout (runtime-relevant)
```rust
struct MoltHeader {
    type_id: u32,
    ref_count: u32,
    poll_fn: u64,
    state: i64,
    size: usize,
}
```

## 4. Collections
- **Lists**: `MoltList` - Heap-managed `Vec<MoltObject>` storage with explicit length + capacity (growth supported).
- **Tuples**: Exact immutable inline storage records its length and trailing
  elements in one allocation, with no separate element buffer. Borrowing tuple
  construction retains its elements; consuming construction transfers owned
  elements only on success. Producers retire every prepared element on failure.
  Mixed borrowed and owned inputs must acquire the borrowed references before a
  consuming call. Tuples remain hashable when their elements are hashable.
  Rich comparisons use one declaring-family storage contract shared by
  operator dispatch, native descriptors and C-API adapters. List/tuple
  lexicographic loops and SIMD byte comparisons live in the shared object
  model; managed and native C containers provide their own owned storage
  adapters. Internal truth-valued equality consumers use the same dispatch,
  without a second structural fallback.
  Normal operators preserve subtype/reflected dispatch and arbitrary owned
  comparison results. Explicit base descriptors validate physical receivers
  and bypass only the outer subclass override. Tuple equality visits common
  elements before comparing lengths; list equality may reject unequal lengths
  immediately. List ordering releases the equality operands and reloads the
  current pair after callbacks, including callbacks that mutate either list.
  Identifier classification and sealed layout metadata use callback-free string
  storage equality; they must never dispatch a name subclass's `__eq__`.
  User equality consumers retain values across callbacks and stop on errors.
  Dataclass comparison policy belongs to the generated methods and their captured
  field tuples; instance storage and constructor flags carry no equality policy.
  Union construction compares ordinary types by identity, uses rich equality
  for GenericAlias operands, and reuses the left union when already complete.
- **Generic aliases and unions**: native `__parameters__` uses the ordinary
  optional-attribute protocol: skip bare classes, recognize `__typing_subst__`
  presence, then consume tuple-valued `__parameters__` in first-use identity
  order. Getter errors propagate; failed collection publishes no cache. Targets
  3.14+ additionally descend raw lists/tuples when metadata is absent, preserving
  list-subclass iteration and physical tuple storage. This follows CPython's
  [`_Py_make_parameters`](https://github.com/python/cpython/blob/3.14/Objects/genericaliasobject.c).
  Python `typing` keeps its distinct equality/iterable-metadata policy and checks
  default ordering only for Generic/Protocol declarations, following
  [`typing._collect_type_parameters`](https://github.com/python/cpython/blob/3.14/Lib/typing.py).
  Parameter collection does not evaluate alias values, bounds or defaults.
  Each native alias/union owns one nullable cached-tuple edge; GC traversal and
  retirement include it. A completing outer collection replaces any reentrant
  inner publication. `Py_GenericAlias` uses the same runtime constructor and
  opaque managed C view, requiring initialized runtime authority. The former
  private C allocation/layout is removed.
  Cache load-and-retain requires current PyToken/GilGuard custody; atomic slots
  alone do not qualify free-threaded access. Before removing that guard, this
  owner needs object-lock or retained-slot custody. The cache adds eight bytes
  per native alias/union and one GC edge; warm reads retain one tuple, while
  cold identity deduplication has quadratic worst-case parameter comparisons.
  Native/WASM, target-minor, C-API and resource-failure qualification remain
  separate proof cells; implementation alone does not establish their support.
- **Bytes**: inline `[length, data, NUL]` storage. The trailing NUL is reserved
  and initialized outside the logical length, including for empty values and
  subclasses. C bytes access borrows this same storage. Bytearray owns a stable
  backing buffer; bytearray methods return bytearray objects and bytes methods
  return bytes.
  Bytearray comparison checks buffer capability before acquiring self and then
  the peer. Both exports remain live through comparison and release in that
  order. Buffer acquisition, contiguous-span admission and writable admission
  share one scoped owner; capability checks never acquire an export.
- **Strings**: UTF-8 surrogatepass buffers retain the inline length/data/NUL
  prefix and own one atomic classification byte after the NUL. `StringStorage`
  owns the complete extent and the Unknown/Ascii/NonAscii fact; bytes storage
  and generated length/data offsets are unchanged. Embedded NULs remain content.
  `InlineBytesStorage::set_len` initializes the NUL and resets the string fact
  under exclusive construction or resize authority. Native string fields begin
  after the classification trailer and alignment. Allocation rounding may move
  a string to the next size class despite the one-byte logical increase.
  No-intern and direct-fill constructors do no classification scan. First use
  scans unknown content once; warm reads are constant work. Ordinary allocation
  reuses its existing ASCII check after filling bytes. Concurrent first readers
  may compute the same immutable fact and publish it with Release/Acquire;
  no global classification cache or lock is introduced. Whole-string UTF-8
  consumers share the fact, while sliced/unkeyed byte inputs retain their own
  classification. Unique append and the C-API Unicode construction commit reset
  the fact and retire hash/UTF-8 state; C-API raw writes remain governed by the
  existing open construction transaction, not ordinary immutable mutation.
  `find/split/replace/startswith/endswith/count/join` use ASCII fast paths with
  codepoint indexing for non-ASCII.
  Runtime core's `text_storage` owns code-point iteration and encoding; adjacent
  explicit surrogates remain distinct, as required by
  [CPython's codec contract](https://github.com/python/cpython/blob/v3.12.13/Doc/library/codecs.rst).
  Strided slices seek once through the existing UTF-8 index authority and walk
  one monotonic byte cursor, copying original code-point spans. Traversal plus
  output is linear in input and selected bytes; normalized index storage remains
  proportional to selected elements. Single-character reads copy their span
  without a temporary encoding buffer. Index cache hits still include ASCII
  probes, synchronization and bounded local scans; they do not promise constant
  time. The independent native/WASM differential carrier covers empty, reverse,
  extreme-step and long cached strings, including surrogates and embedded NULs.
- **Dicts**: `MoltDict` - Insertion-ordered key/value pairs plus a deterministic, open-addressing hash table for lookups.
    - Table hashing follows the runtime hash secret and capability policy;
      `PYTHONHASHSEED=0` selects deterministic string and bytes hashes.
    - `dict_keys`/`dict_values`/`dict_items` return view objects backed by the dict (not materialized lists).
    - Iteration tracks the physical cursor, expected live count and remaining count.
    - Hot-path methods are exposed as intrinsics (e.g., `list.count/index`, `tuple.count/index`, `bytes/str.find`).
    - **Tier 0 (Structified)**: Objects of stable classes are lowered to a struct with no `__dict__`. Access is `*(base + offset)`.
    - **Tier 1 (Shape Dict)**: Uses a "Shape" pointer + a value array. If keys match the shape, access is indexed.
- **Ranges**: `MoltRange` - Lazy sequence storing `start/stop/step` inline. `len`, `iter`, and `index` are computed without materializing lists.
- **Slices**: `MoltSlice` - Inline `start/stop/step` object used by indexing and slicing ops.

Dicts, sets and frozensets share sparse typed entries and one probe index. The
payload contains two stable tracked owners, a live count and an occupied-index
count: four native words. Dictionary rows occupy 24 bytes and set rows 16 bytes;
each live row retains its admitted hash. Deletion vacates one row and leaves a
probe tombstone, without moving other rows, allocating or rehashing keys. Ordinary
deletion retains physical extent. Pop may trim trailing vacancies; dictionary
`popitem` selects the last insertion, while set `pop` selects an arbitrary member
and does not promise the same order as iteration.

Insertion compacts in stable order when holes reach the live count or index
admission reaches the load threshold. Both owners reserve before compaction;
the commit performs no Python callbacks or allocations. Failed preparation may
retain spare capacity but preserves semantic bindings and order. Delete-only
workloads retain backing capacity until a later insertion, clear or destruction.
Dictionary cursors traverse physical rows; set cursors traverse probe slots.
Full sparse walks therefore depend on physical extent or index capacity, not
only live count. A frozen set's first hash scans physical rows using retained
hashes; subsequent calls use the existing object state cache without element
callbacks. Exact frozen sets use inline state and native subtypes use the
existing class/state sidecar.

Hash iterators retain their canonical class after exhaustion. Position and
remaining count commit before result allocation or old-edge release. Size
failures remain sticky; forward dictionary keys-count failures detach the target.
The supported CPython patch releases in the version authority do not apply that
remaining-count check to reverse dictionary iterators. Their dictionary length
hint uses unsigned target-width conversion, while set length hints use signed
conversion. Packed cross-module snapshots retain live references in the shared
charged snapshot owner; item tuple construction runs only after the complete
entry observation is retained. RuntimeHooks exposes a physical dictionary
cursor through `dict_next` with no ordinal compatibility lane.

Iterator advance has one authority, `ops_iter::iteration_step`: the runtime's
`tp_iternext` (CPython `PyIter_Next`; `FOR_ITER` calls the slot directly).
Runtime iterator kinds dispatch on their heap type id: sequence and hash
iterators, `enumerate`, `zip`, `map`, `filter`, `reversed`, callable iterators,
generators and glob iterators. A native class, such as an itertools type,
declares its native next once, in its class's native next word, from the same
callback as its `__next__` descriptor. Only the exact class owns that word, and
the class is immutable, so its namespace cannot diverge from the slot. A heap
subclass never inherits the word: its own or inherited `__next__` runs through
the special-method protocol. A step returns an owned item, clean exhaustion
with the StopIteration value, or a pending error. It never builds a
`(value, done)` pair. As in CPython, `enumerate`, `map`, `filter` and non-strict
`zip` keep their input's StopIteration value, and strict `zip` stops advancing
inputs at the first mismatch. `molt_iter_next_unboxed`, `next()` and runtime
consumers read the step directly. `molt_iter_next` projects it onto the pair
ABI and reuses an iterator kind's pair cache. `enumerate` and `zip` refill a
result tuple that only the iterator still owns, as CPython's `enum_next` and
`zip_next` do.

These storage invariants describe implementation. Native, WASM, free-threaded,
backend/profile and performance acceptance require execution of their affected
consumers; source layout and a single green cell do not establish that matrix.

Hash results use the target C ABI `Py_hash_t` width, including 32-bit WASM.
`molt-lang-obj-model::hash_policy` owns the numeric modulus (2**61-1 on 64-bit
targets, 2**31-1 on 32-bit targets), result normalization, and pure hash
primitives shared by runtime objects and C ABI slots. `object/ops_hash.rs`
publishes that policy and its string algorithm metadata to `sys.hash_info`. Integer, finite float, Fraction, and Decimal
hashes share that modulus; complex, tuple-family, frozenset, pointer, and
string/bytes hashes preserve their algorithm's target-word arithmetic.
SipHash retains its 64-bit internal state and 128-bit seed on both targets.
Runtime and C tuples, dataclass, alias, union, and slice consumers share one target-width
accumulator, with the caller selecting its existing finalization policy.

Structured `sys` metadata uses owned publication for tuples, dictionaries,
and namespaces, with full-width object integer conversion for numeric fields.
Allocation failure releases unpublished owned values and preserves the pending
exception. Public recursion and integer-string limits accept `__index__`, check
the C `int` range before changing state, and retain the prior value on failure.

`collections.Counter` is a dict subclass with one physical dictionary storage.
Its compiled methods use ordinary mapping, numeric and iterator protocols;
explicit dict descriptors and inherited views access the same storage. Counts
remain Python values, including arbitrary-precision integers. There is no
Counter handle registry or independent key-equality index. The shared streaming
tally primitive reuses a key's hash only when the receiver's actual get/store
descriptors admit the native dictionary path; overridden methods retain normal
dispatch. Callback exceptions propagate with the already completed mutations.

### Immediate synchronous iterator invocation

Exception ownership remains explicit across iterator callbacks: `molt_raise`
borrows its argument, while `record_exception_owned` transfers an owned error.
Satellite diagnostic raising delegates to the canonical main-runtime exception
constructor and its owned publication path. An empty diagnostic has no arguments;
an explicit Python `StopIteration("")` retains its one empty-string argument.
Canonical diagnostic construction selects failure from its returned pointer;
a preexisting raised exception does not reject successful diagnostic construction.
Explicit class/argument constructors retain their distinct argument semantics and
temporary-owner obligations.

Native pickle validation uses the canonical exception classes exported by its
public facade, while exceptions raised by user callbacks propagate unchanged.
Reduce iterator slots use the canonical iterator-capability predicate, without
calling `__iter__` merely to probe the supplied value.

Builtin argument policy is a closed declaration in `builtin_callables.py`,
projected into the runtime's typed callable metadata. The positional-range policy
uses the existing frame binder's ownership and default filling, with no extra
packing for valid full-arity calls. It rejects keywords before checking the
positional range, after Python has evaluated and expanded every argument.
The contract follows CPython v3.12.15's `_PyArg_CheckPositional` and
`PyArg_UnpackTuple` family ([builtin source](https://raw.githubusercontent.com/python/cpython/e848e4b09ca4bc67c0041c9ce7526dae471d4c36/Python/bltinmodule.c),
[Clinic projection](https://raw.githubusercontent.com/python/cpython/e848e4b09ca4bc67c0041c9ce7526dae471d4c36/Python/clinic/bltinmodule.c.h)).
Named Clinic binding and C function conventions retain their distinct protocols.

The public `iter(object[, sentinel])` callable has one positional-only signature
and a private missing-argument default. Observable calls and direct two-argument
calls select that overload owner; direct unary calls use the same checked
acquisition primitive. Explicit `None` remains a real sentinel. Unary iteration
acquisition and callable/sentinel construction retain their separate protocol
primitives, including callable admission and callback exception custody.

Generic synchronous iterator steps use the shared owned `SpecialIterationStep`
result. The public boxed ABI wraps successful values in its public `(value, done)`
tuple; the unboxed API transfers the value directly and retires exhaustion
payloads before publishing its output. Native iterator pair caches retain their
existing ownership and ABI. All satellite consumers continue through the same
`molt_iter_next_unboxed` entry and `iter_next_owned` ownership projection.

Special `__next__` and sequence-fallback `__getitem__` calls use raw type/MRO
lookup and the existing descriptor invocation authority. Exact functions and
admitted native method descriptors pass the receiver as a direct argument;
arbitrary descriptors still bind and call their actual result. Instance shadows
and custom `__getattribute__` do not participate. The selected callable and
receiver stay pinned across callbacks, class mutation and reentry. Errors from
binding and invocation share the existing iteration exception transaction.
Foreign receiver lookup retains its C-API descriptor contract.

Ordinary native iterator classes declare `__iter__` and `__next__` together in
one method-family table, which drives class lookup and namespace publication.
Both are unary wrapper descriptors whose declaring class owns receiver
admission; `__next__` uses the same result and exception owner as public `next()`.
Generator and asynchronous suspension adapters retain their distinct protocols.
Native iterators preselect the existing inline common class lane before GC
publication. String iterators leave that initialized lane empty until a public
type/descriptor query or live source retirement resolves and pins their class.
Other source-dependent iterator types initialize it eagerly. One resolver owns
query, exhaustion and nonterminal cycle-clear publication; committed terminal
cleanup never resolves a discarded prefix iterator. Thus construction, prefix
advance and drop do not scan an unknown string. An explicit first class query
can cost O(n); complete traversal plus final classification stays O(n). Dropping
an exhausted source cannot change its class or invalidate a bound descriptor. No iterator
payload field or extra class side table is required. String class selection
uses the shared object-owned ASCII fact, so repeated acquisitions do not
rescan immutable content. CPython likewise keeps
[list iterator type identity independent of its source](https://github.com/python/cpython/blob/v3.12.15/Objects/listobject.c#L2997-L3081)
and publishes unary iteration wrappers through its
[slot definitions](https://github.com/python/cpython/blob/v3.12.15/Objects/typeobject.c#L8890-L8893).

This follows CPython's `slot_tp_iternext`/`vectorcall_method` path in
[3.12.15](https://github.com/python/cpython/blob/v3.12.15/Objects/typeobject.c)
and [3.15.1](https://github.com/python/cpython/blob/v3.15.1/Objects/typeobject.c).
For warmed direct-function or admitted native-method calls whose callback
allocates nothing, unboxed successful steps require no runtime heap allocation,
while the boxed API requires its result tuple. Public `next()` also transfers
the raw owned value and introduces no status tuple.
The public exception policy follows CPython's
[`builtin_next`](https://github.com/python/cpython/blob/v3.12.15/Python/bltinmodule.c):
a successful owned item transfers to the caller; a default normalizes only
StopIteration, and absent a default the original pending exception survives.
Its [`zip_next`](https://github.com/python/cpython/blob/v3.12.15/Python/bltinmodule.c)
also pins a uniquely reusable result before source callbacks and publishes each
slot before retiring its old item. These semantic controls govern the shared
native and WASM ownership paths; interpreter oracles run on the development
side and do not provide a runtime fallback.
Descriptor callbacks and foreign binding can still allocate. The owning test
measures both public APIs independently and checks exact result ownership.
This is an allocation contract, not a throughput claim; target execution and
before/after cost measurements remain necessary for acceptance. Async
`__anext__`, send and throw retain their distinct protocols; an await iterator's
existing send(None) path shares this ordinary synchronous step.

Public `next()` without a default preserves the original `StopIteration`
instance, arguments and traceback. The typed exhaustion policy passes through
map, filter, enumerate and non-strict zip; strict zip and tee normalize their
sources. Loop/status consumers normalize exhaustion and retire its payload.
The existing exception scope owns callback unwinding; one normalizer handles
native adapters and Python special methods.

Runtime collectors consume owned items and hold acquired iterators until every
success, early return or error path has retired them. Serial string collection
uses core `iter_next_owned`; RuntimeVtable ABI 10 removes its obsolete boxed-next
slot. Both producer and subscriber use the shared version and size checks.
The unused path iteration bridge and its generated test declarations are retired.

### Iterator partial-result ownership

Map retires partially collected arguments in source order when a later source
fails. Zip instead owns one tuple cache of its arity, initially filled with
`None`. When exclusive and without an exported ABI view, that tuple is pinned
before invoking any source. Each new slot is published, its GC reference flag
and membership are updated, and only then is the displaced item released.
Reentrant iteration therefore allocates an independent result; an aliased or
exported cache remains unchanged. A fresh partial result is released in reverse
slot order, while an exclusive cached partial result survives until replacement
or zip destruction. Strict zip stops at the first established mismatch.

The shared tuple mutation claim is also used by existing fixed pair caches.
C-API tuple initialization keeps its separate authorization to write through an
ABI view; only runtime cache reuse applies the export barrier. Zip's `repr(C)`
payload owns its iterator backing, strict flag and result edge with one extent
and accessor authority on every pointer width. GC traces and clears the cached
result, and destruction releases iterator references before the cache.

This follows the lifetime, publication and retracking contracts of
[CPython 3.12.15 zip_next and map_next](https://github.com/python/cpython/blob/v3.12.15/Python/bltinmodule.c).
The cold cost is one cached tuple per zip. A row requires O(arity) source work
and one O(arity) initial reference count over its slots; slot publication is
O(1), with no per-slot full-tuple scan or additional temporary backing vector.
Cache retention is bounded by one row. Aliased results require the independent
tuples their observable lifetime demands. Existing boxed native iterator status
results remain separate from the zero-temporary generic special-call contract.
These are algorithmic bounds; compiled allocation and timing controls determine
performance acceptance.

### Shared tee storage

Physical shape declarations in `runtime/heap_kinds.toml` explicitly state whether
their payload reserves shared class fields and a dictionary tail. The generated
`ObjectShapeId::has_class_fields` policy controls field access, reset and tracing;
logical class membership cannot turn a native capture, counter or pointer into
instance storage. Task captures and the typed tee cursor/cache block own their
complete payloads and reserve no dictionary tail. Iterator shapes with a reserved
eight-byte tail after their native prefix retain that tail under the common
class lifecycle, even when Python attribute creation is disabled. In particular,
groupby's five owned values, state word and borrowed active-grouper identity
precede its dictionary at byte 56 of the 64-byte payload; the borrowed identity
must never become a dictionary owner. Other native shapes with reserved tails
and the separate native-instance field owner retain their storage contracts.

`itertools.tee` readers own a `(block, offset)` cursor. Each block is a separate
GC-tracked runtime object owning its source iterator, up to 57 cached values,
and its successor. `runtime/heap_kinds.toml` declares both shapes;
`molt-runtime-core::itertools_tee` owns their `repr(C, align(8))` payloads and
allocation extents (16-byte cursor, 480-byte block, excluding runtime headers).
The in-tree and satellite profiles use the same implementation and lifecycle.
There are no raw reader-owner lists, first-reader GC attribution, or secondary
reference-count/identity registries. Clearing a reader detaches only its block.
Clearing a block detaches its own edges before any Python destructor runs.

This follows the linked GC data-object design in CPython
[3.12.15](https://github.com/python/cpython/blob/v3.12.15/Modules/itertoolsmodule.c)
and [3.15.1](https://github.com/python/cpython/blob/v3.15.1/Modules/itertoolsmodule.c),
including the 57-value allocation/retirement tradeoff. Copies and tee-of-tee
share a block at the current offset. Cache misses retain their block and source
across Python callbacks; the shared running state rejects recursive fetches.
Exceptions and exhaustion leave the cursor unchanged and permit a later retry.
Rollover publishes the successor before retiring the prior block, then resets
the offset, preserving CPython's observable value-finalizer callback order.

Terminal destruction transfers a committed-dead block's successor to the
existing decref caller. That caller iterates through the ordinary refcount,
ABI-view, finalizer and physical-free transactions. It stops at an independently
owned successor; releasing that owner later resumes normal retirement. This
bounds stack use independently of lag, including chains with C views. Cyclic
GC still uses the same per-object edge visitor and detach transaction.

Cached reads cost one indexed load and result retain, without allocation or a
scan of readers. A source fetch adds temporary block/iterator owners around one
iterator call. Source dispatch can itself allocate temporary objects; measure
that cost independently through the same iterator protocol. Outside reentrant
retirement, tee cache storage costs one block per 57 fetched values. Lockstep
readers retain at most 57 cached values; with lag
`L`, retained blocks are bounded by `floor(L / 57) + 2`, plus reader cursors.
Retirement work is linear in the number of blocks and values actually freed,
with constant stack use along successor links. No syscalls, process spawns,
serialization, or additional locks are introduced; runtime mutation uses the
existing GIL custody. These are structural cost bounds, not measured speedups.
The owning `itertools_class` tests report total, source and cache allocation
counts/bytes separately, require cached peer reads to allocate nothing, and
verify retention and native/C-view retirement. The existing
`itertools_tee_independence` differential witness covers Python-visible lifetime,
copying, exceptions, callback reentry and cycles. Native/WASM and profile,
free-threaded and performance acceptance require their actual consumer proofs.

## 5. Concurrency: GIL-Like Serialization (Current)
Molt currently serializes runtime mutation with a GIL-like lock. The concurrency
contract is defined in `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`.
- **Thread Safety**:
    - Runtime state and object headers are not thread-safe; `Value` and heap
      objects are not `Send`/`Sync` unless explicitly documented otherwise.
    - Cross-thread sharing of live Python objects is unsupported; serialize or
      freeze data before crossing threads.
- **Async (core runtime)**: Built on a custom poll/scheduler loop in
  `molt-runtime` (no tokio dependency). Python `async/await` lowers to Molt
  futures with explicit poll state.
  Physical coroutine and poll-future admission is owned by
  `async_rt::generators`; await acquisition, native adapters, attribute lookup,
  and the exported native-awaitability query consume those same predicates.
  A poll address alone does not make an ordinary generator, async generator,
  or coroutine iterator wrapper awaitable. Flagged iterable coroutines retain
  their code-owned protocol admission; user awaitables use the class protocol.
  Compiled polls receive a raw payload address; runtime polls receive a tagged
  object word. The WASM ABI manifest owns the runtime poll identities on both
  native and WASM targets. Native compiler producers lower runtime poll symbols
  to their canonical callable keys; compiled polls retain their code addresses.
  Constructors select lifecycle shape from that identity, and polling dispatches
  by its domain without depending on the debug pointer registry.
- **Async (host services)**: `molt-worker` and `molt-db` use tokio/tokio-postgres
  where OS-level I/O is required.

### Decision: Core Async Scheduler vs Host Executor
- **Why**: Deterministic scheduling, capability gating, and WASM/WASI parity are core requirements for compiled binaries.
- **Tradeoff**: We re-implement scheduler + timer plumbing in `molt-runtime`, while leveraging tokio for service crates that need OS I/O.
- **Outcome**: Runtime stays small and portable; host adapters can integrate with tokio without changing core semantics.

## 6. Exception Handling
- **Fast Path**: Most Molt functions return a `MoltResult<T>` which is a specialized `Result` type optimized for register passing.
- **Error Propagation**: The compiler inserts explicit checks: `if (res.is_err()) return res;`.
- **Zero-cost Unwinding**: Used only for `SystemExit` or deep recursions where propagation is too heavy.


### Native subtype storage and constructor ownership

Native tuple, str, bytes, bytearray, set, frozenset, and complex instances retain
those physical payload kinds. `object::native_instance::NativePayload` owns their
payload extent and the aligned field base. Immutable tuple/string/bytes lengths
determine that base; mutable payloads retain fixed backing-pointer prefixes.
Declared slots and the ordinary dictionary tail follow the payload. There is no
stored extension offset or tuple-specific dictionary trailer. Compiler-inferred
attributes on these subtypes use the dictionary; compiled inline field guards
must reject their fixed native shape.

Allocation prepares every native owner before attaching the owned class edge.
The shared publication transaction initializes fields, attaches the class,
enrolls eligible native subtypes in cyclic GC, and publishes initialized storage.
Exact acyclic native values do not acquire GC membership solely because their
physical kind admits subtype cycles. Traversal, cycle clearing, field access,
dictionary access, and class reassignment project through the same field base.
Generic object allocation cannot construct a native payload.

Initial and replacement class-edge publication reject a native physical kind or
field extent that the allocation cannot hold before acquiring class ownership.
This callback-free check shares field projection's sealed layout authority;
constructor initialization and public class-assignment admission remain separate
obligations. Private `UnraisableHookArgs` uses the same static struct-sequence
declaration, allocation, descriptors and constructor as public struct sequences.

Struct-sequence representation and password-lookup/record-replacement errors
compose Python-produced text through the shared `FormatWriter`. Callback strings
retain their WTF-8 bytes, including lone surrogates and embedded NULs; the writer
copies each result before retiring its owner and preserves pending callback or
allocation errors. Descriptor names are borrowed from the sealed declaration.
Native C type-name diagnostics separately obey CPython's byte-precision limits;
the target-version decoder owns the treatment of incomplete trailing UTF-8.
These are
different contracts: Python values never pass through a lossy host-text adapter.
The owning source references are CPython
[struct sequences](https://github.com/python/cpython/blob/v3.12.13/Objects/structseq.c),
[password lookup](https://github.com/python/cpython/blob/v3.12.13/Modules/pwdmodule.c),
and [argument diagnostics](https://github.com/python/cpython/blob/v3.12.13/Python/getargs.c).

Published `__new__` and `__init__` descriptors own the constructor argument and
conversion contracts. Ordinary class calls invoke these phases through one
lifecycle: a foreign `__new__` result skips initialization, and a subtype result
selects initialization from its actual class. Explicit descriptor calls perform
only the requested phase. Constructor input references remain owned across
conversion callbacks.

Fixed native method descriptors with only a receiver ABI slot share the
zero-argument admission contract. Receiver validation precedes keyword and
positional-count diagnostics. Call arguments retain whether dispatch supplied
the receiver: a materialized bound subtype names its actual type, while an
explicit descriptor or method-load call names its declaring owner. Method-form
lookup selects native descriptors through the ordinary attribute transaction;
custom getters, descriptor precedence and instance shadows retain their public
lookup semantics. Keyword and suspending calls consume the loaded receiver into
the existing argument builder before evaluating user arguments. The nullable
method word never enters Python frame storage. Expanded calls retain their
ordinary bound-value protocol. Names come from native callable/type
metadata only when formatting an error; they never select dispatch. Exact
successful calls keep their direct path, and fallback admission borrows the
existing argument span. Extension, variadic, wrapper and named-signature
binders retain their own contracts. This follows pinned CPython's
[method calls](https://raw.githubusercontent.com/python/cpython/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/methodobject.c)
and [descriptor calls](https://raw.githubusercontent.com/python/cpython/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/descrobject.c).

Module functions declare fixed C argument semantics through
`molt_function_set_fixed_builtin(function, positional_count)`, with an explicit
count of zero or one. Promotion validates the executable signature before
publishing native identity and calling-convention metadata atomically. Rejected
arguments share `CFunctionConvention` admission with C-API methods, including
keyword-before-count diagnostics and module-qualified labels from the published
`__qualname__`. Exact successful positional calls retain direct transport.
Public code, default and signature mutation remains sealed; publication
invalidates cached call shapes before retiring displaced owners. Ordinary
builtin promotion retains its existing binding semantics. This follows CPython's
[fixed module calls](https://github.com/python/cpython/blob/v3.12.13/Objects/methodobject.c)
and [function labels](https://github.com/python/cpython/blob/v3.12.13/Objects/object.c).

Native C type readiness and default slot-wrapper publication preserve the same
method pair and receiver ownership. Default-getter admission uses the canonical
wrapper adapter, wrapped slot and actual declaring-owner ancestry; a public
attribute name alone cannot establish that identity. Custom hooks and unrelated
declaring owners keep their ordinary descriptor behavior. The owning rule is
CPython's [method lookup](https://github.com/python/cpython/blob/v3.12.13/Objects/object.c)
and [slot update](https://github.com/python/cpython/blob/v3.12.13/Objects/typeobject.c).
Foreign method-descriptor invocation follows its native type flag; other
descriptors retain their binding protocol. Binding and explicit invocation keep
their distinct receiver validation and diagnostics through the shared owner.

Descriptor receiver errors and C type projections share the physical native
type name, independently of public `__name__` and `__qualname__`. Native
declarations retain their creation module in existing class storage; later
module or qualified-name edits do not change that identity. A successful mutable
heap name replacement switches to the new short C name, as CPython does.

Suspending argument assembly keeps each pending value in one scratch owner until
its scheduled push and reuses one builder slot. Parking visits only newly live
values, so compiler work and frame storage grow linearly with argument count.
One cold exception landing clears parked owners in reverse order and forwards
the pending exception; it creates no runtime exception frame. Explicit exception
successors remain authoritative inside surrounding handlers on every backend.

Native descriptor slots consume the physical payload directly. Source operators
use class special-method lookup for subtype overrides; exact receiver fast paths
cannot bypass those overrides. Inherited sequence concat/repeat slots retain
sequence fallback ordering after reflected numeric methods. Concatenation slots
own rejected-operand diagnostics through `SequenceConcatKind`: str/list/tuple
report their base sequence kind and the rejected logical type; bytes/bytearray
report both logical operand types. Diagnostic type names obey CPython's byte
precision and never select the operation or its dispatch. Generic binary
rejection remains generic even for user classes named after builtin sequences.
In-place bytearray rejection uses the same diagnostic authority; buffer
acquisition errors, reflected callback failures, and allocation/overflow paths
retain their owning protocols. In-place string reuse is restricted to exact
strings so it cannot overwrite subtype field tails.

Normal and in-place numeric calls carry distinct, strictly admitted modes through
RuntimeHooks. In-place methods may return a new object; NotImplemented resumes
the ordinary reflected protocol before sequence fallback. The original operands
remain borrowed and each successful result transfers one owned reference.
`operator.concat`/`iconcat` and `PySequence_InPlace*` deliberately prefer physical
sequence slots. These are separate protocols, not aliases for numeric addition
or multiplication.

Power uses one raw ternary slot-ordering authority for runtime and C operands.
Python-defined ternary power is left-only for semantic targets 3.12 and 3.13;
3.14 also admits reflected power. Python `__ipow__` remains binary even when a
C caller supplies a modulus; a raw C in-place power slot receives all three
arguments. The existing declared runtime target selects this semantic behavior,
independently of the physical C ABI layout. Builtin int, bool, float and complex
C number tables expose their declared operations; immutable in-place slots and
matrix multiplication remain absent. Declaring int descriptors read int storage
of bool/subtype receivers, whereas bool's own bitwise overrides preserve bool
only for two bool operands. Bool inversion warns before the integer operation,
and a warnings-as-errors result terminates the call. Exact builtin numeric
kernels stay separate from subtype callback dispatch. Native, WASM and
concurrency cells require execution and cost qualification of these contracts.
Set in-place slots accept set/frozenset operands and otherwise decline to the
normal reflected protocol. A fallback through dict keys/items views returns a
new set; it must preserve the original set alias. Dict ordinary union accepts
dicts, while dict in-place union uses the shared mapping/pair-iterable update
protocol, including partial updates before an error. Declared keys/items view
slots accept iterables, preserve operand direction, and test membership or
cancel equal items before hashing result tuples. Dict values have no numeric
set protocol. Static C set/frozenset/dict tables and dynamic view slot projections
use these same declaring owners and existing container storage admission.

Managed C-API `PyObject_IsTrue`/`PyObject_Not` and
`PyObject_Size`/`PyObject_Length` enter the same runtime truth and length
protocols. Physical container length hooks cannot decide these inquiries:
subtype special methods, `__index__` conversion, signed pointer-width overflow,
Unicode character counts, and pending exceptions belong to the runtime
protocol. Foreign objects retain their native C slot dispatch.
The bridge commits mutable C projections before invoking a managed inquiry;
failed observation propagates its error without falling back to foreign slots.
Managed C attribute operations share the runtime attribute protocol. Normal
get/set/delete operations invoke user overrides; `PyObject_GenericGetAttr` and
`PyObject_GenericSetAttr` use explicit object operations and bypass those
overrides. Logical-class descriptors, declared slots and dictionary storage
retain their usual precedence, including on dictless native subtypes. An
explicit dictionary in `_PyObject_GenericGetAttrWithDict` replaces only the
instance-dictionary tier. Suppression clears only AttributeError; a suppressed
dictionary comparison failure continues to the non-data class tier, while
descriptor failure ends the lookup. C views never supply an alternate class or
storage authority. The focused `cpython_abi_hooks::generic_attributes_tests`
exercise this contract through real managed C views.
Numeric scalar shortcuts require the canonical builtin class identity; heap
integer and float subtypes use the same logical-class lookup as other managed
instances. Optional normal lookup uses that same normal attribute transaction,
including `__getattribute__` and `__getattr__`, and suppresses only AttributeError.
Storage shape and dictionary availability do not select a different lookup
policy. Implicit numeric, context-manager and class-check protocols use the
shared type-only special-method lookup, including numeric extension bridges.
Explicit object reads and C generic reads retain the initial class
attribute (or initial miss) across instance-dictionary equality callbacks,
including dataclass and replacement-dictionary reads. The selected descriptor
and its original class remain owned until lookup finishes. Descriptor binding
uses the receiver's current logical class after callbacks, and releasing the
snapshot preserves the pending exception. Dictless storage does not introduce
a second MRO or descriptor algorithm. Descriptor-cache keys use lossless
Python string storage bytes, including lone surrogates and embedded NULs.
These contracts are covered by `native_constructor_storage.py` and the focused
`object::native_instance` Rust tests; each target/profile still requires its own
execution evidence before a support claim.

### Class annotation namespace ownership

Class annotation values, generated evaluators and lazy caches are owned only by
the class dictionary. Class allocation, traversal, cycle clearing and terminal
retirement share `ClassReferenceSlot`; there are no private class annotation
slots. The payload has eleven words, retaining epoch at 4, qualname at 7, policy
at 8 and cached size at 9, with slot declarations at 5, physical rows at 6 and
declarations at 10.

Targets 3.12/3.13 store annotations under `__annotations__`. Target 3.14 reads an
explicit `__annotations__` before `__annotations_cache__`, and an explicit
`__annotate__` before `__annotate_func__`. The compiler emits its class evaluator
under `__annotate_func__`, preserving an explicit class-body `__annotate__`.
These are visible namespace entries, matching CPython 3.14; they are not hidden
from `__dict__` or `dir`. Protocol member collection excludes the internal
annotation names just as CPython's `typing._SPECIAL_NAMES` does.

Stored descriptors bind with no instance and the actual class as owner. Lazy
annotation evaluation resolves `__annotate__` through normal attribute lookup,
including metaclass overrides, and owns its callback until it returns. Cache
and evaluator mutations invalidate the type before displaced values retire;
retirement preserves the pending error. Native non-heap types reject annotation
reads before inspecting the namespace. Native dictionary storage offsets do
not publish an instance `__dict__` descriptor: native declaration tables own
that publication.

`type.__annotate__` participates in the existing staged builtin-member version
transaction and its single receipt. Compiled startup version admission and
cohort reset synchronize the descriptor even without version environment
variables. Dataclasses and TypedDict consume the class annotation accessor;
they do not run or cache a second evaluator.

### Function instance dictionary ownership

`object::field_storage` owns dictionary validation, lazy materialization,
first-entry publication and replacement for managed functions, ordinary
instances, dataclasses and dictionary-capable native payloads. The function's
physical dictionary word is a projection of the same instance-slot authority.
Only the sealed class capability admits a public `__dict__` descriptor or
arbitrary public attributes. Declared callable metadata has a separate typed
owner and never reads or writes this user mapping.
No function-only `__dict__` getter or dictionary allocator remains.

Function `__dict__` get/set follows the real class descriptor. `vars`, bound
method forwarding, explicit object/C generic reads, and `functools.wraps` and
`update_wrapper` observe that same live mapping. Replacement accepts a dict,
publishes it before retiring the old owner without changing call metadata;
deletion is rejected, as in CPython's `PyObject_GenericSetDict`. A failed first
insertion does not publish an empty dictionary. Retiring displaced owners
preserves both the C and runtime exception channels. Traversal and cycle clear
visit/detach the function dictionary exactly once through its physical slot.

Native callable module-assignment failures use the shared native-failure
boundary: transfer a C exception, retain a pending runtime exception, or raise
SystemError when a native failure supplies neither. A failed assignment cannot
be reported as success. The owning differential fixture is
`tests/differential/basic/function_metadata_dictionary_isolation.py`; runtime
storage/failure cases live with callable metadata and native dictionary tests.
Implementation source alone establishes no backend/profile acceptance claim.

The fixed `object::function_metadata::FunctionMetadataField` tail owns mutable
function names, documentation, module, defaults, keyword defaults, native
callable declaration fields and pre-publication signature facts. The existing
code object owns immutable published signature facts; publication detaches the
five setup-signature owners and later setup-field replay retains no duplicate. Six mutable CPython function fields use actual class
descriptors. Compiler-private-looking user attribute/dictionary keys are user
data and cannot rewrite execution binding. Internal inspection, intrinsic
reuse and call binding read the typed owner directly.

All physical functions, including generated/thin runtime callables and imported
functions, allocate through the common builder. Its single payload extent is
13 existing words plus 15 typed reference words: 120 additional bytes per
function before allocator rounding. Normal metadata initialization no longer
allocates a metadata dictionary or retains its string-key entries. For a
function that never needs user attributes, the avoided storage is one dict
object (header plus four native words), two tracked vector owners, and the
typed-entry and hash-table backing capacities. Those buffers occupy
`24 * entry_capacity + sizeof(usize) * table_capacity`
bytes before allocator rounding. Shared interned name objects and the metadata
values themselves are not claimed as removed allocations. A later public
dictionary access still allocates its ordinary dictionary; that case saves
metadata entry capacity and key ownership, not the dictionary object. Typed fields are
visited and detached once by the shared function GC/lifecycle handler, and
retirement preserves both error channels. This is a resource-layout tradeoff,
not a measured throughput or allocation improvement; parent acceptance requires
representative allocation/call measurements after functional verification.

### Instance field dictionary diagnostics

`MOLT_DEBUG_FIELD` enables inline field observations and the shared dictionary
storage trace. Optional `MOLT_DEBUG_FIELD_NAME` selects one exact field name
(for example `_coro`) for inline reads/writes, dictionary-backed accesses and
direct dictionary mutations. Instance publication, replacement, reset and
lifecycle clearing include the receiver and dictionary identities; direct
dictionary events correlate through the dictionary identity without a second
ownership registry. A direct clear/swap is selected by the old physical keys.

`field_dictionary` records the operation, receiver/dictionary addresses and
reference counts, supplied name/result, entry count and pending-error state.
`field_dictionary_entry` records matching raw entries, their stored hashes and
the string object's cached state. These observations do not run Python,
recompute hashes, change caches, or retain/release values. No matching entry
line means no matching physical string key at that observation; the trace does
not substitute this scan for ordinary dictionary equality. An empty instance
replacement still emits its publication row. Flags are read once per process;
unset `MOLT_DEBUG_FIELD` disables the entire family. Diagnostic runs are not
performance measurements or correctness acceptance.

Every observation entry has a small inline gate using the same cached enable
flag. Enabled work dispatches to cold, noninlined bodies. With the flag unset,
no diagnostic field iteration, dictionary scan, instance-shape lookup, name
filter initialization or formatting runs. Raw views remain within synchronous
observation under the caller's live-owner custody and GIL; the observer performs
no Python call or ownership mutation. These are source-level properties, not
measured overhead claims.


### Native errno publication and build authority

The native public `errno` module is an ordered projection of the selected C
headers and the target Python version. CPython owns the symbolic names,
conditional Windows Winsock preferences, introduction versions and insertion
order; the target C compiler owns macro presence and integer expression values.
Repeated CPython insertions remain publication events: they can restore a
reverse-map winner after an intervening alias, even when the forward name
already exists. The declaration keeps those events and rejects conflicting
definitions of the same name; raw C symbols remain unique.
The platform build evaluates a fixed-width data record and reads the target
object without linking or executing it. The runtime consumes static rows through
its existing cached dictionary publication, with no per-syscall discovery.

`runtime/molt-runtime-platform/errno_symbols.tsv` is the symbolic publication
source. The shared build-only reader admits native object format, architecture,
byte order, extent, defined data-symbol identity and relocation-free contents.
C assertions additionally witness the effective OS, pointer width and applicable
CRT ABI. Generated target-side const assertions compare portable raw anchors
with the existing libc crate, following its header-versus-Rust testing mechanism.
They never compare a public WSA preference to a CRT value. Absence of a glibc
macro does not independently prove a musl provider;
compiler and SDK identity remain properties of the selected toolchain.

The CLI's existing read-time build ledger and selected toolchain custody own
executable search and SDK identity. An admitted Apple SDK is versioned through
`SDKSettings.json`; captured C resources, semantic flags and deployment controls
feed the existing resource-keyed Cargo intermediate generation. The namespace
preheader is an explicit build-script source dependency. Reuse those owners:
this evaluator adds neither a header watcher nor a whole-SDK scan. Ordinary
bare Cargo retains Cargo's declared-input change-detection guarantees; it does
not claim the CLI's external-resource admission. Qualification also requires the
actual target consumer, not object inspection alone; current acceptance is
recorded in V1-23.

Molt's synthetic wasm32 POSIX host-import errors remain a distinct declaration
in `libc_compat.rs`. Raw WASI C errno and native CRT/Win32 errors retain their
own provenance. A public Windows alias may use Winsock without changing the
corresponding raw CRT value or OSError classification. The zero-argument
`molt_errno_constants` intrinsic and its tuple-of-dictionaries result are unchanged;
its internal collector consumes the runtime's selected Python version.
V1-22 separately tracks retirement of the competing environment version selectors;
this errno migration does not qualify that initialization boundary.

Mechanism references: [CPython errno publication](https://github.com/python/cpython/blob/v3.15.0/Modules/errnomodule.c),
[libc header-versus-Rust controls](https://github.com/rust-lang/libc/blob/main/CONTRIBUTING.md#test-before-you-commit),
[Clang target and sysroot selection](https://clang.llvm.org/docs/CrossCompilation.html),
[Cargo build-script target inputs and dependency tracking](https://doc.rust-lang.org/cargo/reference/build-scripts.html),
and the existing [build early-cutoff contract](../../../design/build_early_cutoff.md).
These define source policy, not release or unexecuted target acceptance.

### Native errno header namespace preparation

The target-C evaluator places its CPython-derived namespace controls
before every system header, including `stdint.h`. CPython 3.12–3.15 includes
`pyconfig.h` before those headers: its explicit `_DARWIN_C_SOURCE`,
`_NETBSD_SOURCE`, and `__BSD_VISIBLE` definitions reopen extensions; the generated
system-extension block additionally enables GNU and NetBSD OpenBSD-compatibility
extensions, while `_BSD_SOURCE` is OpenBSD-only. These controls affect macro
visibility, not the native raw error-number domain or synthetic WASM errors.
Windows CRT/WinSock policy is unchanged.

This preheader is not a general `pyconfig.h` replacement. CPython's additional
POSIX/XOPEN selection depends on the configured target OS/release, and Solaris
`__EXTENSIONS__` depends on a configure compile probe. These are unqualified
portability boundaries, not requirements imposed on the six native release
coordinates from the existing release matrix. Preserve those generic target
paths without claiming feature-profile or runtime qualification. The Darwin
compile-only controls prove actual macOS SDK visibility and header ordering,
not full runtime publication,
all supported Python versions, or musl ABI identity.

Primary authorities: CPython [3.12 configure.ac](https://github.com/python/cpython/blob/v3.12.13/configure.ac),
[3.15 configure.ac](https://github.com/python/cpython/blob/v3.15.0/configure.ac),
[3.15 generated configure](https://github.com/python/cpython/blob/v3.15.0/configure),
and [Python.h header order](https://github.com/python/cpython/blob/v3.15.0/Include/Python.h).

For Linux GNU, preserve the guarded `_GNU_SOURCE` definition, including an
explicit value of zero. glibc tests definedness and derives the effective
POSIX/XOPEN namespace itself. CPython 3.12–3.14 requests POSIX200809/XOPEN700;
3.15 requests POSIX202405/XOPEN800. This does not require another per-version
errno compiler profile on the qualified GNU provider: the selected glibc headers
normalize both requests. Do not substitute one fixed POSIX level for the
provider's decision or extend this reasoning to musl, Android or another libc.
The actual errno integer declarations remain header-owned; the GNU `error_t`
declaration supplies an independent feature-selection witness. Neither a
compile-only cross object nor Zig's selected minimum glibc ABI version proves
execution against that glibc release.

Primary mechanisms: [glibc2.31 feature selection](https://github.com/bminor/glibc/blob/glibc-2.31/include/features.h#L173-L197)
and [glibc2.42 feature selection](https://github.com/bminor/glibc/blob/glibc-2.42/include/features.h),
plus the pinned CPython configure/template sources above. Existing actual-C
controls remain in the platform crate's `errno_build_tests::` proof cell.
