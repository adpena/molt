# Generated compiled reference-count transitions (HF-205)

This contract specializes the existing owned-reference transition; it does not
change Python lifetime boundaries, ownership insertion, collector roots or the
terminal-release transaction. The [RC/GC design](rc_gc_redesign.md) owns that
lifetime model. Qualification belongs to the V1 ledger and retained lane
receipts; implementation alone does not qualify a backend or platform cell.

## One fact plane, one generator

`molt-passes::representation_facts` proves physical representations on final TIR
`ValueId`s. Raw integers, bools and floats have no carrier reference count.
`Repr::BoxedImmortal` records a producer-proven immortal singleton or a boxed
inline-safe integer, bool or float; an annotation never establishes it. A local
worklist completes those physical word proofs through boxing, retained aliases
and all-incoming-proven phis in the same representation authority. It does not
widen mathematical scalar forwarding or heap-identity rewrites. Its LIR projection, `Immortal64`, preserves
that proof while carrying the unchanged boxed word. LIR's `inline_i64_values`
is the direct projection of `RawI64Safe`, separate from the shared physical
I64 class that also admits full-width carriers. Boxing consumes that range
proof directly, without allocating a temporary owner or rechecking it at run
time. Hand-authored LIR without a producer proof keeps checked full-width boxing. Native's existing final
SimpleIR projection transports the same fact with its exact body. LLVM consumes
the value-keyed map. No backend re-lifts code or infers immortality from a name.
Native cleanup tokens and explicit retain credits consume this same physical
proof; a proven boxed immediate must not acquire a token that later emits RC.

Carrier and boxed obligations differ: `RawI64FullDeopt` has no carrier owner,
but boxing it can allocate a BigInt. Only an inline-safe integer proof can
elide the boxed obligation. `Never`, a missing fact and an annotated parameter
cannot authorize elision. Mixed heap/immediate phis remain dynamic. Boxing producer-proven None preserves
its one immediate word; other boxed scalar domains keep their physical boundary.
Generic WASM stores boxed transport, so it consumes the boxed obligation even
when the value can use a raw full-i64 carrier in native or LIR. No RC
insertion, cancellation or cross-call coalescing rule changes here (HF-214).

A semantic `None` annotation does not authorize replacing a boxed operand
with literal None. LLVM materialization preserves that word, including frame
home publication; `ConstNone` alone supplies the singleton constant. Replacing
an annotated heap operand would leave retained credits outside its homes and
leak them when the frame exits.

`molt-codegen-abi::inline_refcount::generate_refcount` is the single
builder-parametric IR generator. Its adapters spell instructions and explicit
blocks for Cranelift, LLVM and WASM. Layout offsets, tags and header masks come
from the ABI owner. Sparse heap membership is generated from `heap_kinds.toml`;
the runtime validator consumes that exact projection too. A broad min/max test
would incorrectly admit retired ids 205, 220 and 231.

For dynamic words, the generated decision is:

1. An immediate or null pointer word performs no memory access or runtime call.
2. Atomic mode delegates pointer transitions to the existing runtime. Plain
   mode requires the invocation's complete execution admission, before reading
   any header. WASM also proves that the address fits its target pointer width.
3. Validate the heap kind before accessing flags or count. An immortal flag
   skips all writes. Native flag loads acquire publication of C-view custody;
   refcount loads/stores remain plain under admitted GIL custody.
4. C-API views delegate to the canonical bridge transaction. A retain also
   delegates terminal/deallocating state, zero, sentinel corruption and overflow.
   Its ordinary interval is 1 through `u32::MAX - 2` inclusive.
5. A release stores one subtraction only for counts 2 through `u32::MAX - 1`.
   Count 1 reaches the existing runtime **before** its decrement: that runtime
   owns the sole zero transition, finalizer/revival window, weakref handling,
   bounded child release and physical free. Error paths likewise receive the
   original word and unchanged count. There is no second terminal entry ABI.

The pure `retain_next` and `release_transition` algebra remains authoritative.
The generator's interval specialization is checked against it, including every
terminal/overflow boundary and mirrored/immortal flag combination. LLVM shifts
mask their count before evaluation: an invalid type id cannot create poison
even on an otherwise rejected branch.

Each emitted RC site has one cold call. Native and WASM spell its blocks
directly. LLVM generates internal `alwaysinline` bodies with two arguments
(word and invocation admission); its caller's TIR/phi CFG is unchanged. The
machine-emission check must establish that the selected LLVM pipeline actually
inlines them. A source-level `alwaysinline` attribute alone is insufficient.
LLVM creates its entry block first and the other TIR blocks in `BlockId` order.
Hash-map storage order cannot change an immutable stdlib constituent's object
bytes. The regression emits and compares objects at O0, O2 and O3 and checks
that dynamic cold transitions survive without residual RC wrapper symbols.

## Execution admission and diagnostics

`molt_refcount_admission` borrows the current execution authority for one
compiled invocation. It grants plain access only with the GIL, a C-extension
execution context and ordinary lifecycle lease or shutdown-drain custody, with
RC transition diagnostics disabled. It cannot acquire any of those capabilities.
A bare GIL token yields runtime mode. The invoker keeps its execution boundary
until compiled code returns; a balanced C-API GIL suspension restores that
boundary before returning to compiled code. Generator suspension ends an
invocation, and resumption obtains a fresh admission.

Each backend also tracks whether its local execution
policy actually entered a frame; every return, including an early error or
suspension, closes only that frame. The credit is cleared before frame-owned
values can run finalizers. LLVM consumes this same guarded epilogue contract
as native and WASM rather than relying on source `trace_exit` operations that
are absent on generated suspension returns.

The snapshot is an immutable SSA value/local for that invocation, not shared
mutable admission state or an object/name cache. Nested calls cannot replace
their caller's admission. Diagnostic runs retain the existing runtime tracing
and error paths; compiled code cannot silently bypass them.

Free-threaded mode keeps the existing atomic Release/Acquire protocol, including
the terminal acquire fence. [The existing decision](../../agent/FREE_THREADING_READINESS.md#51-refcount-model-keep-atomics-adopt-biased-rc-only-if-measured)
defers biased RC until measured cross-thread contention justifies a header and
lifecycle change. There is no owner-thread field to test today. This work does
not invent an identity, apply plain arithmetic to atomic storage, or claim a
free-threaded WASM cell; that target remains explicitly refused by the ABI.

## Prior art and alternatives

[CPython 3.12's object header](https://github.com/python/cpython/blob/v3.12.0/Include/object.h)
and [PEP 683](https://peps.python.org/pep-0683/) motivate inline ordinary updates,
immortal bypass and cold destruction. [PEP 703](https://peps.python.org/pep-0703/)
requires actual thread ownership and shared-count states for biased RC; borrowing
only its owner-thread branch would not preserve Molt's current atomic contract.

[mypyc's tagged representation](https://github.com/python/mypy/blob/master/mypyc/doc/dev-intro.md)
and [RC transform](https://github.com/python/mypy/wiki/Reference-Counting-Transform)
separate unboxed values from owned object references. [Lean's runtime](https://github.com/leanprover/lean4/blob/v4.24.0/src/include/lean/lean.h)
and [Perceus](https://www.microsoft.com/en-us/research/publication/perceus-garbage-free-reference-counting-with-reuse-2/)
motivate scalar specialization and explicit control flow; their functional
ownership guarantees do not replace Python finalizer ordering.
[Swift's ARC design](https://github.com/swiftlang/swift/blob/main/docs/SIL/ARCOptimization.md)
similarly distinguishes trivial values and warns that moving releases across
effects can change lifetimes. No retain/release motion is introduced here.

Rejected alternatives: unchecked header arithmetic; a GIL-only permission;
duplicate backend transition algorithms; decrementing to zero before calling the
existing decrement entry; a separate deallocator ABI; diagnostic suppression;
an unconditional atomic view of GIL-confined cell storage; and name-based scalar
or immortal classification. Each would lose an existing invariant or add a
second authority.

## Required evidence

The changed obligations are checked transition refinement, no immediate header
access, no immortal writes, original-state cold dispatch, real execution
admission, mixed representation/finalizer lifetime, exception edges and generator
suspension. Native and WASM differential execution, actual LLVM emission,
default/free-threaded runtime controls, generated synchronization and the plain
runner precheck remain distinct evidence cells. Performance uses the exact
primitive scoreboard identity, loop anatomy, 0 versus ten-million-iteration
instruction slopes, and binary size. No performance or release verdict follows
from type checking or the symbolic generator oracle alone.
