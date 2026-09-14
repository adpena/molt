# 49 — Object field ownership

Status: current source contract; consolidation changes require source-bound
native/WASM execution before release acceptance. This replaces the historical
heap-versus-stack split from #86 (`ac73ab954`), not its single-owner invariant.

## One field owner, independent of allocation placement

Every materialized field containing a mortal heap reference owns that reference.
`object/field_storage.rs` owns physical field traversal and the transition from
inline inferred attributes to dictionary backing. Declared slots stay inline.
Runtime traversal, serialization, field access, dictionary publication, and GC
consume this authority; mutable class dictionaries never redefine physical layout.

`object/accessors.rs` retains the incoming value, publishes the new slot, then
releases the displaced value. A callback triggered by release sees the new state.
Dictionary-backed access uses the dictionary's ownership protocol. Initialization
requires a pristine slot; the shared compiler typed-slot planner proves that
condition instead of inferring it from the store's spelling or storage placement.

The compiler has one field-assignment contract: `store` or guarded
`guarded_field_set`. The unchecked `store_init` and `guarded_field_init` wire
operations are retired; frontend construction does not assert a lifetime fact
through a separate spelling. Native initialization specialization consumes the
shared planner's exact-site `FreshInit` proof. WASM and LLVM preserve immediate
inline writes only after receiver, physical-backing and incoming-value admission;
other writes use the same retain/publish/release runtime operation. Standalone
unsafe runtime initialization APIs still require their caller's lifetime proof.

The shared allocation-layout authority distinguishes raw boxed words (initial
`+0.0`, release-neutral) from class fields (initial immortal missing). It owns
their extents and the class dictionary-tail exclusion. MemorySSA retains the
exact-site store plan used for its graph; MemGVN consumes that same plan.
Module-slot promotion uses its region query in both seed and rewritten-loop
validation. Context-free alias/effects queries never infer a neutral old owner.
One physical field region tracks exact allocation identity (when known) and
boxed-word extent, allowing independent fresh fields to retain separate reaching
definitions. Unknown receivers can alias known allocations; class names, including
base/derived views of an inherited slot, never prove disjointness. May-alias CFG
parameters do not acquire an allocation identity. Arbitrary callbacks and
unproved replacing stores still clobber every heap region, across offsets and
classes alike.

Guarded reads, writes, and layout checks accept tagged receivers through
`molt_guarded_field_get`, `molt_guarded_field_set`, and `molt_guard_layout`.
A class hint, including an unchecked return annotation, is not pointer admission.
The runtime checks the receiver before inspecting a header and preserves the
original value for generic attribute lookup or mutation on guard failure. The
compiler must not unbox it into an address first. WASM resolves an address for
inline payload access only after the shared layout check succeeds.

Descriptor binding also preserves the exact tagged receiver. The canonical
`builtins/attr.rs` boundary accepts `Option<u64>`: absent means class access,
while `Some(None bits)` is a real Python receiver. Scalars and heap objects use
the same function, classmethod, staticmethod, property and `__get__` protocol;
the scalar-specific binder is retired. Raw MRO lookup results are borrowed.
Binding pins the descriptor and its arbitrary `__get__` callable across callbacks;
consumers must not release a borrowed class-dictionary entry. Immediate invocation
uses `descriptor_call1`, sharing function receiver policy without allocating a
transient bound method. Binding errors remain the original exception, not a
secondary call of an error sentinel. One typed error policy distinguishes an
attribute lookup miss from required invocation. Required special methods retain
binding exceptions. Optional numeric slots preserve them on CPython 3.12/3.13,
but treat binding `AttributeError` as absent on 3.14+. Rich comparison suppresses
all binding exceptions on 3.12/3.13 and only `AttributeError` on 3.14+. The shared
`DescriptorCallPolicy` uses the runtime target-version authority; no caller
classifies exceptions after invocation. Exceptions from the called method body
always propagate, independently of this binding-only policy.
The version boundary follows CPython's
[3.13 slot lookup/comparison implementation](https://github.com/python/cpython/blob/v3.13.11/Objects/typeobject.c#L9709)
and [3.14 lookup_method_ex](https://github.com/python/cpython/blob/v3.14.3/Objects/typeobject.c#L2835),
and is exercised by the binding-versus-body differential cases.

Descriptor hooks are resolved on the descriptor's type, including the metaclass
of a class-valued descriptor, never on its own namespace. The shared hook policy
preserves CPython's intentional asymmetry: `__get__` is called raw with explicit
descriptor/instance/owner arguments; `__set__` and `__delete__` bind their hook
before invocation. Property accessors accept general callables. One typed
mutation outcome drives object, dataclass and metaclass consumers, owns callback
lifetimes and discards ignored owned returns exactly once. Missing custom hooks
raise `AttributeError("__set__")` or `AttributeError("__delete__")`; property
diagnostics retain their contextual attribute/owner information.

Class lookup honors metaclass overrides and data descriptors before local class
entries. Local and inherited entries use the shared descriptor binder. Object,
classed-object and metaclass custom lookup share one invocation/fallback
transaction: it captures `__getattr__` before calling `__getattribute__`, pins
both callbacks and their receiver/owner, and consumes the exception stack once.
Literal hook-name attributes do not bypass user lookup overrides.
Custom mutation hooks and generic/metaclass `__call__` resolution use invocation
binding as well: a descriptor-raised `AttributeError` cannot become a default
write/delete or an incidental not-callable `TypeError`.

Exact builtin staticmethod objects use one owned call-target resolver for fixed
arity calls, the argument builder, and arity inspection. Nested wrappers are
flattened iteratively, retaining each successor before releasing its predecessor;
the existing recursion budget stays charged through target invocation. Their
callability does not depend on whether the wrapped object is callable, while
classmethod is not transparently callable. This is not a descriptor-only unwrap.
Inherited staticmethod forwarding, wrapper reinitialization, and truthful
`staticmethod.__call__` publication still require a coherent descriptor-wrapper
constructor/payload/class-dictionary authority; ordinary subclass `__call__`
lookup alone does not establish those capabilities.

Binary dispatch may compare initial method identities to decide reflected-method
priority, but it re-resolves the receiver's current class and method immediately
before each invocation. No borrowed namespace value survives an earlier operand's
callback. This preserves method replacement/deletion and `__class__` mutation,
as exercised by `tests/differential/basic/descriptor_dispatch_mutation.py`.

These receiver/ownership changes do not establish numeric getset support.
Numeric properties still require truthful descriptor types, native class-dict
publication, data-descriptor precedence, and numeric-subclass backing, including
complex subclasses. Source tests in `builtins/attr/descriptor_tests.rs` and the
existing scalar public-entrypoint regressions require runtime execution before
any native/WASM compatibility claim.
Truthful member/getset publication also owns named slot shadowing: current
generic/dataclass SET paths can still reach physical declared-slot storage before
a rebound data descriptor, and dataclass SET/DELETE retain different frozen-check
ordering. The hook invocation consolidation does not close those representation
and precedence gaps.

`object/heap_lifecycle.rs` owns traversal and detachment of materialized edges,
including inline fields, dictionary backing, and the common class edge. It clears
the owning location before releasing detached values. Heap, arena, and frame
objects all use this protocol. `HEADER_FLAG_SCOPED` changes backing reclamation,
not object RC, class ownership, cycle-GC enrollment, or terminal edge cleanup.
The compiler must never emit a second field-release walk for such an object.

Complete scalar replacement is different: SROA proves the entire allocation and
its unobserved, callback-free uses removable and erases them atomically. There is
then no materialized field owner. Nonescape, a frame candidate, or absent
`defines_del` metadata alone cannot authorize this rewrite.

## Representation and publication

- Class-definition success atomically replaces its in-progress policy with the
  finished policy; failure clears only the attempt bit and preserves other
  updates. Both paths use the native/WASM typed auxiliary-word authority.
- Internal class allocation takes a native `usize` byte extent. The external
  ABI converts its raw `u64` extent at entry; neither size nor field offsets are
  boxed Python integers. Runtime constructors and tests share this typed boundary.
- Inline text/bytes allocation accepts only `InlineBytesKind::String` or `Bytes`.
  Bytearray uses its vector-backed constructor; a raw type ID cannot select an
  incompatible payload through the inline allocator. Extent arithmetic is checked.
- Declared class fields begin as the immortal missing singleton, not zero bits.
  `+0.0` and `-0.0` are real values, preserved through backing transitions.
- Field values are boxed. Raw scalar backend shortcuts require the shared carrier
  and pristine-storage proofs; they cannot overwrite an existing heap owner.
- Dictionary materialization transfers inferred attributes once, clears their
  inline slots to missing, and publishes one authoritative dictionary. Declared
  slots retain their own storage and are not copied into that dictionary.
- `HEADER_FLAG_HAS_PTRS` is traversal/tracking metadata, not release custody.
  Immortal missing requires no retain and does not itself enroll a field owner.
- Class-bound frame construction requires a sealed, immutable, nonfinalizing MRO
  for the storage lifetime. Runtime admission checks before touching caller
  storage; other classes use the owned heap constructor. Published class changes
  preserve both physical compatibility and this lifetime requirement.

Tuple subclasses allocate fresh tuple storage after sealed ancestry/prefix admission.
Conversion uses normal special-method lookup, including inherited builtin sequence
slots and user overrides; explicit base slots admit tuple receivers before using the shared physical-storage
operations. Tuple method caches belong to the runtime and retire during shutdown,
not process-global atomics. The tuple-subclass differential corpus exercises both
inherited and overridden dispatch; class-attachment tests also reinitialize the runtime.

## Compiler and proof boundaries

[Design 20](20_rc-ownership-drop-insertion.md) owns object-result and Python-local
lifetimes. The escape analysis supplies capture facts but does not promote class
allocations. The unproved compiler class-frame opcode is retired; the standalone
unsafe runtime API requires callers to prove storage lifetime separately.

Regression consumers include `object/accessors_tests.rs`, class-attachment tests,
typed-slot/DSE/SROA tests, and `tests/differential/basic/typed_field_ownership.py`.
Source inspection or a host unit test does not close native/WASM execution cells.

## Dataclass backing and failure atomicity

Dataclass index operations are projections of the same field accessors, not a
second storage authority. Descriptor-owned exact field keys and declared-slot
classification are prepared before the class edge is attached. The declaration
projection is immutable after publication and participates in GC/terminal edge
custody. Ordinary vector fields retire into the managed dictionary on exposure;
a missing dictionary entry never revives its retired vector value.

Named lookup and mutation preserve dynamic hooks and descriptors, then use that
same backing resolution. Equality and hash pin only their participating fields;
repr pins each value immediately before its callback, preserving sequential field
read semantics. Pickle's default state delegates to object getstate; construction
reset uses the sealed field traversal and publishes the entire empty state before
releasing displaced values.

Saved slots snapshot names in MRO declaration order and use the shared normal
attribute lookup for each name. Duplicate declarations remain repeated observable
reads; custom getters, descriptors and class-level shadows retain their semantics.
Only AttributeError means absent. GC and reset still traverse every distinct
physical owner. Dataclass descriptor-only slot names follow in descriptor order.

All fallible dataclass metadata preparation occurs while the class edge is absent.
Class attachment and GC publication are the final callback-free commit. Failure
therefore cannot invoke a user finalizer on a partially initialized unpublished
payload. Regression coverage is in object/ops_slice_tests.rs; cross-target receipt
closure remains required before a native/WASM conformance claim.
