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
from the ABI owner. The runtime header validator consumes exact sparse heap membership generated
from `heap_kinds.toml`. Debug compiled sites delegate pointer transitions to
that validator; release compiled sites consume the owned-pointer lifetime
obligation and the count-word custody encoding detailed in variant E below.

For dynamic words, the generated decision is:

1. An immediate or null pointer word performs no memory access or runtime call.
2. Atomic mode delegates pointer transitions to the existing runtime. Plain
   mode requires the invocation's complete execution admission, before reading
   any header. WASM also proves that the address fits its target pointer width.
3. In release builds, load the count word once. The unsigned interval check
   admits retain counts 1 through 2^30 and release counts 2 through 2^30+1.
   The custody bit places flags and exceptional counts outside that interval.
4. Store one addition or subtraction only inside that interval. Every other
   mortal word delegates unchanged to the canonical runtime transaction,
   including C-API views, deallocation, zero, overflow and terminal release.
5. The immortal sentinel skips all writes. Count 1 reaches the runtime before
   decrement: that authority owns the sole zero transition, finalizer/revival
   window, weakrefs, bounded child release and physical free. There is no second
   terminal entry ABI. Debug builds delegate every non-null pointer transition
   to the runtime before any compiled header access.

The pure `retain_next` and `release_transition` algebra operates on decoded
owners, with the exclusive owner bound from the ABI. The generator's window
specialization refines it; custody, terminal and overflow boundaries retain
unchanged cold dispatch. Header-kind validation is not general stale-address
protection: neither a tag nor a valid kind establishes pointer liveness.

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

Only plain storage reads the snapshot. Debug-compiler (HardenedPlain) and
atomic sites send every pointer word to the runtime and never touch a count
word, so they need no admission. The query has no side effect. Native calls it
once when a compiled count-word access consumes it, and not otherwise. Its
backend tests check that relation on the emitted body, so they hold under both
the dev and the release-fast compiler profile. LLVM and WASM do not test
consumption. LLVM calls it at each task-trampoline entry and before a body's
first RC site. WASM calls it at each op-loop body and task-trampoline entry,
and before a LIR body's first RC site. Where no site reads it, the call costs
time, not safety.

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

## Observed original native tradeoff (2026-11-02)

The November 2026 macOS arm64 worker measurement used the primitive authority's
v26 release workload and exact CPython 3.12.16 image, with five paired samples.
The int-loop slope fell from 567.48 to 421.95 retired instructions per iteration
at the canonical one-million count. A separate zero/ten-million slope on retained
emissions measured 568.20 to 423.67, against CPython's 528.23. All fourteen
per-operation Molt medians decreased, but small changes and startup measurements
remain subject to the recorded noise envelopes. These worker observations are
not release qualification.

The resulting primitive-ratchet calibration advances the methodology lock to
v27 because its verdict limits change. The worker's v26 observations retain
their original comparison class; they are not relabeled as a new release run.

The cost is substantial executable text growth: the canonical loop artifact
grew from 22,757,320 to 49,577,656 bytes. Retained anatomy emissions show native
`__text` growing from 14,972,900 to 41,631,204 bytes. The loop function itself
grew from 1,537 to 4,456 static instructions. Its immediate path bypasses all six
RC calls, while the full natural-loop region still includes their cold blocks
and grows from 169 to 421 instructions. The checks have been duplicated at each
dynamic site; retaining their invariants has a real size price. Integration must
assess this tradeoff explicitly. Text reduction must preserve admission,
exact header eligibility, diagnostics and the unchanged terminal transaction.

## Variant E: runtime custody in the count word (2026-11-04)

This addendum replaces the per-site eligibility sequence above. Bit 31 of the
stored refcount word marks runtime custody; bits 0 through 30 carry mortal
owners. The all-ones immortal sentinel remains distinct: mortal owner counts
must stay below `0x7fffffff`, so adding custody cannot spell immortality.
The pure transition algebra operates on decoded owners; retain rejects a
result at or above `0x7fffffff`, while the sentinel remains a special no-op.
Snapshots expose owners, never the custody bit. Runtime retain, release,
upgrade, revival and C-view hold transitions preserve the stored custody bit.
The ABI identity must distinguish this encoding from the earlier full-width
count representation; cached objects must never mix the two contracts.

`MoltHeader` flag mutators are the sole authority that grants and retires
runtime custody (the immortal sentinel carries it by construction). They enter
custody before publishing IMMORTAL, HAS_ABI_VIEW or DEALLOCATING and reconcile
after publication or retirement. Removing one flag must not clear another
flag's custody. A failed flag CAS settles its speculative custody against the
observed flags. Allocation initializes the count before initializing flags.
These are requirements on every flag mutator, including initialization,
compare/exchange, take, and test-only stores; callers cannot write flags directly.
Immortalization first stores the sentinel under exclusive unpublished custody,
then publishes the flag. Generic flag retirement never clears the sentinel.
Shutdown demotion, after concurrent readers stop, replaces it with one owner
plus custody; retiring the immortal flag subsequently returns it to plain mode.
Terminal detection uses decoded owners, so encoded zero (`0x80000000`) is zero
to the existing release/upgrade/revival transaction. DEALLOCATING publication
stays in that terminal transaction; no generated site decrements to zero.

The single `molt-codegen-abi` generator retains tag/null, pointer-width and
complete invocation-admission checks. A plain admitted site reads one count
word. An unsigned window test admits retain counts 1 through 2^30 and release
counts 2 through 2^30+1. It adds or subtracts and stores only in that interval.
Every other mortal word reaches the unchanged runtime transaction with its
original bits and count. The immortal sentinel performs no write. Atomic
builds continue through the runtime; no biased owner-thread layout exists yet.
The retained variant-E arm64 release-site measurement is 17 inline and five
cold instructions, compared with 44 plus four in the original lowering.
These are observations from the prototype, not a promise for every backend.

Release builds drop the per-site sparse type-id stale-pointer check. Valid
owned pointers are a compiler/runtime lifetime obligation; a tag check cannot
prove that an arbitrary pointer is live. Debug compilers (`debug_assertions`) select the generated HardenedPlain mode:
dynamic pointer transitions call the existing runtime, whose exact heap-kind
validator runs before count access and terminates on a rejected kind. It does
not call back into generated code or repeat a rejected transaction. This is
header-kind validation, not general stale-address protection: reading an
unmapped header can fault and a reused live allocation can pass the check.
The generated algorithm owns this profile distinction; adapters only spell
its instructions.

A flag/count publication can temporarily disagree. Under the GIL, no admitted
plain transition can interleave with its flag mutator. In today's atomic build,
compiled transitions always call the runtime and runtime count CAS operations
preserve custody; eligibility does not rely on observing a simultaneous pair.
When clearing custody, reconciliation reloads synchronized flags afterward
and restores custody if any requiring flag is present. Thus a stale failed-CAS
observation cannot leave a completed publisher without custody. A publisher
also restores the bit after its flag CAS. There can still be transient disagreement;
atomic runtime operations do not consult the custody bit for eligibility. Their
existing flag CAS, owner-count CAS, exclusive immortalization, and bridge
transaction remain the linearization authorities; no ordering decision moves
to this bit. The reconcile/recheck sequence is not an atomic two-word state transition and
must not be used to justify a future lock-free biased plain path. Biased RC
requires one CAS over a coherent eligibility/ownership state (or an equivalent
serialized transition) before a thread may take that path. That future design
must close the flag/bit window; this change neither implements nor qualifies it.

HF-720 now owns None-word materialization and guarded invocation-credit
retirement. The merged LLVM code consumes those implementations once; RC
inlining does not create a second frame lifecycle. Mutation controls must
separately distinguish carrier and boxed RC obligations, including full-width
integers whose carrier is raw but whose boxing can allocate. A BoxVal result
receives Immortal64 only from its own BoxedImmortal producer proof.

The focused acceptance compares native, LLVM and WASM lifetime witnesses;
exercises custody, immortal, terminal, overflow and revival transitions; and
measures loop, id, memo_get, getattr_miss, recursion and dataclass_construction
against exact CPython 3.12.16 with native binary sizes. HF-733, borrow inference
and closed-world dead-code removal own further size reduction.

## Variant E development measurements (2026-11-04)

The paired experiment uses the same merged source and runtime encoding. Its
control sends dynamic heap RC to the runtime while retaining producer elision;
it isolates heap inlining, rather than recreating the original pre-HF-205 tree.
Both arms use native Cranelift, release compiler and guest profiles, the
release-output runtime, and exact CPython 3.12.16. Five alternating N=0/N pairs
per arm retain raw `/usr/bin/time -l` counters and executable identities.

| Primitive | Runtime-call control instr/op | E instr/op | CPython instr/op | Control binary bytes | E binary bytes |
|---|---:|---:|---:|---:|---:|
| loop | 223.20 | 223.39 | 518.59 | 27,269,448 | 35,773,240 |
| id | 723.86 | 650.31 | 1,184.35 | 27,285,288 | 35,789,080 |
| memo_get | 2,845.05 | 2,279.69 | 1,569.62 | 27,268,592 | 35,755,872 |
| getattr_miss | 2,644.37 | 2,446.90 | 1,032.86 | 27,252,128 | 35,755,920 |
| recursion | 397,509.74 | 380,745.47 | 56,348.68 | 27,268,800 | 35,772,592 |
| dataclass_construction | 32,410.64 | 28,559.25 | 1,965.05 | 29,236,792 | 38,846,888 |

N is 1,000,000 except dataclass construction 100,000. Recursion normalizes
by 10,000 depth-100 traversals; other rows normalize per iteration. Median
instruction changes are +0.08%, -10.16%, -19.87%, -7.47%, -4.22% and -11.88%
in table order. E binaries are larger in every row. No size ratchet is reset.

These are development observations, not release or ratchet qualification.
E pair spreads range 0.70% to 13.89%; loop, id and dataclass measurements are
especially noisy. Small deltas must not be interpreted as gains, and the
dataclass distributions also limit the strength of its median comparison.
Positive-N wall medians are coarse (0.02 to 0.15s); E peak RSS medians range
16,678,912 to 28,737,536 bytes, versus control 16,039,936 to 24,903,680 bytes.
These are process RSS observations, not allocation or live-heap measurements.
No link-time improvement is claimed.

The retained E loop body has 3,081 static instructions. Its smallest natural
loop union has 304 instructions and six cold RC sites (two retain, four
release). On inline-integer bindings, all six pointer-tag branches bypass
the count loads and cold calls: zero RC calls per normal immediate iteration.
The natural-loop union still includes rejected heap/terminal paths. The normal
path retains iterator work; generic arithmetic and pending-work service remain
cold alternatives owned by their respective ledger families.

The separately requested N=0/10,000,000 loop slope reuses those exact binaries
without rebuilding. Five paired medians are 222.05758 control, 223.08928 E and
520.33324 CPython instructions/iteration. E/control is 1.00465; E/CPython
is 0.42874. Pair spreads are 0.84% control, 1.06% E and
0.86% CPython. The small control/E difference lies within this
noise envelope; heap inlining does not establish an immediate-loop gain.
