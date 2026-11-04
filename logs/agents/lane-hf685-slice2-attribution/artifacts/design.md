# HF-685 slice 2: construction instruction attribution

## Phase 1: measurement prerequisite (2026-11-04)

The assigned outcome is an exact per-function construction anatomy followed by a
structural correction selected from that evidence. No runtime/compiler correction
is selected yet. Slice 1's five-pair macOS arm64 medians are 16,663 instructions
per plain construction, 28,048 per new-and-update, and 29,010 per dataclass
construction; its noisy spreads do not establish a speedup. Its sampled anatomy
left 41% of plain self samples in other runtime. These are historical diagnostic
measurements, not Linux observations or instruction attribution.

Live source ca1cdefc68c1 already has sealed class layouts, lazy instance dictionaries
and one scoped physical-field traversal (object/class_layout.rs and
object/field_storage.rs; public authority docs/design/foundation/49_object_field_ownership.md).
Do not replace these with another layout, cache, registry or dictionary policy.
The source and slice-1 report were checked before writing this design.

Live molt-gate is x86_64 Linux on the inventoried WSL distribution. Its checkout
is clean and behind origin/main by nine commits; it is a donor, never reset.
The probe found perf but no valgrind. The only phase-1 tracked source change is
adding valgrind to tools/trunk_loop.py:LINUX_SYSTEM_PACKAGES. This feeds the
existing WSL_GUEST_PACKAGES projection and approved missing-package installer.
No installer, host registry, privilege bypass or new proof cell is introduced.
The inventory already carries install_system_packages=true, owner-approved
2026-11-04. Run the canonical provision command after review; retain its receipt
and exact valgrind version. Existing tests/tools/test_trunk_provision.py exercises
the package projection and missing-only install boundary. Run that authority.

## Attribution protocol and interpretation

Use exact source ca1cdefc68c1 in an owned remote scratch checkout/build, without
moving the donor checkout. Record Git identity, compiler/runtime profile, toolchain,
program hashes and executable hashes. Reuse canonical programs from
tools/perf_scoreboard_primitives.py for plain_construction, new_and_update and
dataclass_construction; retain their stdout and compare with CPython 3.12.16.
Collect Callgrind Ir at N=0 and N=K, with matching binary and environment. Normalize
each function's signed difference by actual operations (new_and_update uses N/10).
Use sufficient K to dominate startup, retaining negative differences rather than
clamping them. Collect both exclusive costs and inclusive call-edge costs and
dynamic calls. Preserve unmapped symbols; they cannot support a guessed mechanism.
Inline functions belong to their machine-code owner unless debug attribution
proves otherwise. Inclusive rows overlap and must never be summed as mechanisms.
Check self-cost totals against the profile event summary; inspect recursive edges
before interpreting inclusive totals. Rank the top 15 by exclusive delta, showing
their inclusive delta beside it, plus a separate inclusive ranking as needed to
identify expensive call chains. Partition exclusive costs by actual mechanism:
allocation, binding, init dispatch, attribute store, dict materialization,
refcount, GC tracking, hashing, runtime entry and guards. Preserve loop/control
and unclassified costs rather than forcing them into a runtime category.

Callgrind counts modeled instruction reads, not hardware retired instructions.
It is the requested attribution oracle, not a replacement for the separate
five-pair macOS hardware-counter A/B. Do not compare its ratios with a hardware
ratchet. The Valgrind manual describes exclusive and inclusive costs and warns
that callgrind_annotate does not detect recursive cycles:
https://valgrind.org/docs/manual/cl-manual.html . Its format authority defines
cost rows, compressed identities and inclusive call-edge rows:
https://valgrind.org/docs/manual/cl-format.html . No upstream code is copied.

## Phase 2 gate

Before structural implementation, append the actual three attribution tables,
source-grounded dominant mechanism, chosen correction, invariants, affected
producer/consumer/backend cells, alternatives and independent semantic witnesses.
Have two sequential fresh read-only Codex reviewers read only this file, resolve
their findings here, and commit the reviewed design before structural code.
If dominance belongs to HF-205, HF-198, HF-226 or HF-79, retain that ownership and
limit implementation to the measured HF-685 portion. An unsettled semantic
decision returns to the parent. Do not claim attribution or a fix before evidence.

Required final proof: construction semantics against CPython 3.12 on native,
LLVM and WASM, covering missing init, overridden new, slots, dict identity/update,
vars, pickle and subclasses; five paired base/head hardware instruction slopes
against exact CPython 3.12.16 on all three primitives; head Callgrind tables;
changed tests/direct readers, affected crate checks, static-only, and at most
one canonical precheck in the runner's plain environment. Parent owns landing,
ledger/ratchet and retirement. No reserved path is in this worker's write scope.

## Review record

Phase-1 reviewer: separate `codex exec -s read-only -m gpt-6.1-sol`, high,
given only this path and the implementation-readiness role. It read no other
file. Findings and resolutions (review-bootstrap.md):

1. Exact target: tools/cross_hosts.toml entry molt-gate, SSH adpena@bat00,
   WSL distribution molt-gate. Command from the assigned checkout using its
   selected-image Python: `python -B tools/trunk_loop.py provision --host molt-gate`.
2. Canonical Primitive.program() reads N from `sys.argv[1]`, after compilation.
   Capture `valgrind --tool=callgrind --callgrind-out-file=OUT BINARY 0` and
   the same command with `BINARY K`. Record N, divisor and N/divisor with each
   profile; no recompile or empty-loop subtraction.
3. Use K=10,000 actual operations initially (N=100,000 for new_and_update).
   Check the total normalized slope at 2K: relative difference must be <=1%.
   A failure invalidates the anatomy as steady-state evidence; inspect actual
   changed call paths before adjusting K. Retain all captures and signed rows.

This resolves the prerequisite review. Runtime design remains gated on attribution
and its two independent reviews; no runtime optimization is approved by this phase.

## Phase 2: measured mechanism (2026-11-04T22:03Z)

Baseline source ca1cdefc68c1, Linux x86_64, native Cranelift, logical release,
release compiler and release-output runtime; managed CPython frontend 3.12.15,
Rust 1.99.1, Valgrind 3.22.0. Exact identities, executable hashes, all nine
closed-guard runs and raw profiles are retained in base-attribution-evidence.tar.gz
(SHA256 ce14b0e4cd1c31e150e87db7f46c1f42ace59e549ab9a8cf05cf5c245af531ef).
All nine outputs and program hashes match the independent exact CPython 3.12.16
oracle (attribution-oracles.json). The raw reader checks every exclusive sum
against its profile summary; it also matches Valgrind's independent 820-event
format example. Counts below are signed N=K minus N=0, divided by 10,000 actual
operations. Display rounding does not affect retained raw counts.

Top 15 exclusive functions, with inclusive instruction reads and observed dynamic
calls beside each. `molt_runtime::` is abbreviated `rt::`. Inclusive costs overlap;
they are not a partition and their sum is meaningless. No recursion-based
mechanism conclusion is drawn. The full exclusive table and call-edge costs are
in base-attribution.json.

### plain_construction

Total Ir/op at K: 16012.50910; at 2K: 16014.39555; relative difference 0.01178% (passes <=1%).

| Function | Exclusive Ir/op | Inclusive Ir/op | Calls/op | Mechanism |
| --- | ---: | ---: | ---: | --- |
| `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&usize>` | 1462.03 | 2397.04 | 17.000 | hashing |
| `<core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write` | 934.93 | 934.88 | 16.999 | hashing |
| `rt::object::dec_ref_ptr_terminal_body` | 439.03 | 5171.85 | 2.000 | refcount / retirement |
| `rt::call::bind::arguments::callargs_builder_is_live` | 380.00 | 1085.00 | 5.000 | guards (builder provenance) |
| `rt::call::bind::arguments::callargs_ptr` | 380.00 | 1110.00 | 5.000 | guards (builder provenance) |
| `rt::object::seq_access::pin_tuple` | 356.04 | 356.04 | 4.000 | guards (metadata ownership) |
| `rt::object::object_shape_id` | 344.00 | 504.00 | 18.000 | guards (type / shape) |
| `molt_dec_ref_obj` | 311.00 | 3621.02 | 7.000 | refcount / retirement |
| `rt::call::bind::arguments::require_callargs_ptr` | 279.00 | 2019.00 | 3.000 | guards (builder provenance) |
| `rt::object::alloc_object_zeroed_with_aux_policy` | 275.04 | 708.06 | 2.000 | allocation |
| `core::ptr::drop_glue::<rt::object::seq_access::PinnedTuple>` | 268.03 | 268.03 | 4.000 | refcount / retirement |
| `rt::object::heap_lifecycle::take_best_buffer::<u64>` | 264.04 | 264.04 | 4.001 | allocation / retirement |
| `rt::call::bind::inline_cache::try_call_bind_ic_fast` | 263.97 | 4843.14 | 1.000 | call binding |
| `rt::builtins::frames::namespace::take_invocation_namespace` | 262.00 | 292.00 | 1.000 | call binding (frame) |
| `molt_frame_set_col` | 238.00 | 238.00 | 7.000 | call binding (frame) |

### new_and_update

Total Ir/op at K: 27740.20570; at 2K: 27718.02650; relative difference 0.08002% (passes <=1%).

| Function | Exclusive Ir/op | Inclusive Ir/op | Calls/op | Mechanism |
| --- | ---: | ---: | ---: | --- |
| `rt::builtins::attributes::get_attr_object_ic_with_mode` | 887.99 | 14060.55 | 4.000 | guards (attribute resolution) |
| `rt::builtins::attr::cached_class_namespace_mro` | 780.28 | 1435.81 | 10.000 | guards (attribute resolution) |
| `<rt::builtins::attr::class_cache::ClassLookupCache>::lookup` | 780.13 | 780.13 | 12.003 | guards (attribute resolution) |
| `rt::builtins::attr::bind_descriptor` | 757.00 | 3621.81 | 2.000 | call binding (descriptor) |
| `rt::builtins::type_ops::type_of_bits` | 657.00 | 1500.00 | 12.000 | guards (type / shape) |
| `rt::builtins::attributes::class_lookup::attribute_lookup_transaction::<rt::builtins::attributes::attr_lookup_ptr_with_mode::{closure#0}>::{closure#0}` | 642.00 | 6189.32 | 2.000 | guards (attribute resolution) |
| `rt::builtins::attr::object_attr_lookup_inner` | 581.00 | 7085.50 | 3.000 | guards (attribute resolution) |
| `molt_dec_ref_obj` | 538.99 | 4507.30 | 12.000 | refcount / retirement |
| `rt::builtins::classes::builtin_classes` | 525.13 | 524.58 | 21.005 | runtime entry / singleton access |
| `rt::builtins::attributes::class_lookup::type_attr_lookup_ptr_inner` | 452.04 | 2199.52 | 1.000 | guards (attribute resolution) |
| `rt::call::bind::inline_cache::load_method` | 430.00 | 7033.51 | 2.000 | call binding |
| `rt::object::object_shape_id` | 396.00 | 616.00 | 22.000 | guards (type / shape) |
| `rt::object::builders::canonical_special_singleton_bits` | 384.00 | 384.00 | 12.000 | runtime entry / singleton access |
| `rt::builtins::attr::object_attr_lookup_inner::{closure#0}` | 368.00 | 4694.76 | 3.000 | guards (attribute resolution) |
| `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&usize>` | 344.30 | 564.49 | 4.002 | hashing |

### dataclass_construction

Total Ir/op at K: 29268.72200; at 2K: 29288.79015; relative difference 0.06852% (passes <=1%).

| Function | Exclusive Ir/op | Inclusive Ir/op | Calls/op | Mechanism |
| --- | ---: | ---: | ---: | --- |
| `rt::builtins::attr::bind_descriptor` | 857.00 | 3562.94 | 2.000 | call binding (descriptor) |
| `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&usize>` | 766.09 | 1229.14 | 9.000 | hashing |
| `rt::object::dec_ref_ptr_terminal_body` | 761.98 | 8914.29 | 3.000 | refcount / retirement |
| `rt::object::alloc_object_zeroed_with_aux_policy` | 554.01 | 1963.63 | 4.000 | allocation |
| `<rt::object::heap_lifecycle::DetachedEdgeSink>::release_all` | 553.00 | 2522.95 | 3.000 | allocation / retirement |
| `rt::object::heap_lifecycle::take_best_buffer::<u64>` | 521.01 | 521.01 | 8.000 | allocation / retirement |
| `rt::object::gc::gc_untrack` | 514.71 | 642.35 | 4.000 | GC tracking |
| `rt::builtins::attr::cached_class_namespace_mro` | 468.17 | 860.92 | 6.000 | guards (attribute resolution) |
| `molt_dec_ref_obj` | 459.00 | 5344.99 | 9.000 | refcount / retirement |
| `molt_lang_obj_model::release_ptr` | 448.01 | 1793.49 | 4.000 | refcount / retirement |
| `rt::object::seq_access::pin_tuple` | 445.24 | 445.24 | 5.003 | guards (metadata ownership) |
| `rt::object::gc::gc_admit_membership` | 437.27 | 534.31 | 3.000 | GC tracking |
| `rt::object::object_shape_id` | 432.00 | 672.00 | 24.000 | guards (type / shape) |
| `rt::object::instance_dict_bits_ptr` | 410.00 | 1010.00 | 10.000 | dict materialization admission |
| `<std::hash::random::DefaultHasher as core::hash::Hasher>::write` | 408.04 | 408.04 | 8.001 | hashing |

### Attribution conclusion and ownership

Plain construction's largest exclusive function is pointer hashing: hash_one and
its SipHash callee consume 2,396.96 Ir/op (14.97% of the 16,012.51 slope), with
17 hashes/op. The raw call edges attribute 705 Ir/op each to builder_is_live and
callargs_ptr, 423 to require_callargs_ptr's storage-set check, 282 to retirement,
and 141 each to the two registry insertions: essentially all steady-state
usize hashing comes from builder provenance. These inclusive hash costs include
the hash writer; do not add them again to the exclusive partition.

Plain's builder_is_live/callargs_ptr/require_callargs_ptr exclusive bodies add
1,039 Ir/op, separate from that hashing. Allocation itself is not the dominant
exclusive function. New-and-update instead spends its largest exclusive family
in attribute/descriptor resolution. Dataclass has 9 usize hashes/op plus generic
construction/descriptor/retirement work. Its missing direct init path overlaps
HF-79/HF-226; RC and execution-entry work stay HF-205/HF-198-owned. This slice
removes the measured redundant builder construction mechanism; it does not claim
to eliminate the remaining namespace or dataclass dispatch gap or close HF-685.

### Chosen correction: the builder object owns its typed value inline

CallArgs currently has three representations of one lifetime: a managed object
containing an unused storage-pointer projection, an external Box<CallArgs>, a
builder-address HashMap plus a second storage-address HashSet. A push checks the
builder map twice and the storage set once; consumption/retirement repeats them.
The registry was introduced by 4adb1e19e8 (2026-04-08 debug scaffold) and scoped
to each runtime by 7cb50974c2. Its useful purpose was diagnosis of corrupted/stale
internal pointers, not a Python-visible namespace or constructor semantic.
Every relevant builder ABI already documents that the caller supplies a live
owned builder; even the existing consuming entry installs a pointer-drop guard
before registry validation. The public C-API borrows argument spans and never
accepts this private builder representation.

Replace that system with a single inline CallArgs Rust value in its managed
TYPE_ID_CALLARGS allocation. Rust sizeof/alignment owns its physical layout.
callargs_ptr is an unsafe projection of the caller-owned live builder, with a
live-object kind check; it does not claim to validate freed/forged raw addresses.
Delete both registries, CallArgsPtr, runtime-state mutex field/init/imports,
alloc/free note helpers, liveness probes and dead pointer-payload projection.
No table, slot cache, fallback, generation map or compatibility lane replaces them.
Retain the documented ABI precondition; do not expose builders to Python or add
an unchecked fast path beside a checked slow path. Null/pending-error handling
and live wrong-kind TypeError stay explicit. Tests must not dereference a
consumed/freed builder merely to ask whether it is live.

Prior art is CPython's CALL value-stack custody and compact frame payloads
(Python/bytecodes.c and Include/internal/pycore_frame.h at v3.12.0): compiler-owned
arguments have a proven lifetime, rather than consulting a pointer registry at
each push. This is a storage/custody adaptation, not copied code. Molt keeps its
existing argument-release policy (CALL versus CALL_FUNCTION_EX and target minor),
which CPython remains the differential oracle for. The existing sealed instance
layout and lazy dictionary policy remain the single object-field authority.

### Invariants and complete consumer migration

* Construct CallArgs and reserve its positional capacity before publication;
  allocate the combined header+CallArgs extent unpublished, ptr::write the
  complete Rust value, then finish construction while retaining the linear-unpublished heap policy
  (no Python/collector publication). A failed allocation releases the prepared keyword owner and Vec once.
  No visitor may inspect a zero-initialized, invalid Rust Vec. CallArgs alignment
  must fit the allocator's admitted payload alignment on both 64-bit and wasm32.
* runtime/heap_kinds.toml changes CALLARGS from boxed_bits to inline_rust. Retain
  dynamic edges, cycle=never, weakref deny and linear_unpublished: builders are
  compiler-owned non-Python linear objects. Regenerate all heap-kind projections
  through tools/generators.py write. No kind id or runtime import signature changes.
* Every consumer projects the same inline value: ordinary/expanded creation,
  positional/keyword pushes, star/kwstar expansion, method receiver insertion,
  CallArguments::from_builder, clone/snapshot GPU helpers, GC edge visitation,
  terminal capacity/detachment, tracing and tests. Native, LLVM and WASM already
  carry opaque object handles; inspect their lowering and execute their consumers.
  Rust/Luau backends use their own source representation and require no ABI change.
* Consumption still moves uniquely-owned edges into CallArguments or retains
  copies for shared builders. Reentrant callbacks keep the caller's owning builder
  alive, and no new Rust reference spans a callback. Terminal detachment publishes
  empty keyword/positional storage before the object-wide sink releases any edge.
  Move the old Vec into a local owner and drop it once after staging edges; leave
  a valid empty inline CallArgs until allocation deallocation. Do not Box::from_raw
  or drop the inline value twice. Existing pending exception and release ordering
  are preserved, including constructor child arguments, shared builders and CAPI.
* The registry-introspection test is replaced by a behavioral owner/release
  witness; pending-error tests retain refcount/finalizer assertions and stop
  querying freed addresses. Add a live wrong-kind negative control, failure
  atomicity and reentrant argument-finalizer checks where existing tests do not
  cover them. This is not a general trusted-pointer API policy change.

Affected source owners: call/bind/arguments.rs, call/bind.rs,
state/runtime_state.rs, object/heap_lifecycle.rs, object/builders.rs (trace),
arguments_binding_tests.rs and frame_binding_binding_tests.rs. Aperture extensions:
runtime/heap_kinds.toml and its generated runtime/core/codegen/Python projections;
existing owning public call-custody contract in SIMPLE_IR_JSON_SCHEMA.md, not a
new parallel design authority. No reserved path changes.

### Proof and decision rule

Use existing call-argument semantic tests (pending failure, moved/shared ownership,
expanded mappings, target-version cleanup) and the existing semantic runtime gate.
Add/extend CPython differential witnesses for successful/raising constructors,
no init, custom/inherited/foreign new, slots, dict/vars identity/update and pickle,
plus argument-evaluation failures and finalizers reentering calls. Execute native,
LLVM and WASM; exceptions/output must agree, never use expect-fail markers.
Inspect affected direct readers, regenerate/check, and run the static-only gate.
Run the canonical precheck at most once in the plain runner environment; retain
its actual outcome and do not repeat a budget refusal.

Use one five-pair base/head native macOS hardware instruction A/B against exact
CPython 3.12.16, canonical N and startup differencing. Record instructions, elapsed
and peak RSS plus sizes; retain every sample and existing per-axis noise bounds.
The mechanism prediction is zero builder-registry hash calls and no external
CallArgs Box allocation, verified by head Callgrind and source-bound allocation
observations. Require a lower plain-construction instruction median with
separation larger than the observed paired spread, and no material namespace or
dataclass regression; if noise defeats qualification, report inconclusive instead
of rerunning for a favorable sample. No full-board or release-speed claim.

### Structural review record

Two sequential fresh read-only reviewers pending; structural source edits remain
blocked until findings are resolved in this file and this design is committed.

### Architecture review resolutions (round 1)

Reviewer: separate read-only Codex gpt-6.1-sol/high, only this document, findings
retained verbatim in review-architecture.md. It found no demonstrated mechanism
correctness flaw and requested the following evidence/decision clarifications.

1. Concrete producer/consumer lifetime audit, observed before implementation:

| Boundary | Live producer / owner | Callback and retirement rule |
| --- | --- | --- |
| native fc/callargs.rs CallargsNew | calls form.runtime_constructor; bind_owned_runtime_result records the returned owning object value | Pushes borrow it; compiler scratch/drop custody owns it through argument evaluation. No payload load in codegen. |
| LLVM preserved_ops/callable_ops.rs CallargsNew | same typed call form; bind_owned_runtime_result | Same opaque i64 result and consuming runtime ABI; no payload offset embedded. |
| WASM aggregate_ops/callargs_ops.rs CallargsNew | selects generated CallargsNew/NewExpanded imports and store_runtime_result | Managed runtime owns the payload; locals hold opaque i64 handles, including across suspension. |
| molt_callargs_push_pos/push_kw/expand_star/expand_kwstar | documented unsafe precondition: live builder from new, borrowed for whole entry | Caller retains its owning reference. expand_star keeps only raw args_ptr across OwnedIterator::new, length_hint and next callbacks; each actual Vec access is a short expression after the callback. kwstar pins the dictionary owner in the still-owned builder. |
| CallArguments::from_builder | consuming entry holds PtrDropGuard before projection | Unique builder moves Vec/keyword owner without callbacks; shared builder retains copies while both owners remain live. No user body starts with the Rust builder reference alive. |
| molt_call_bind, inline_cache.rs owned/indirect entries | PtrDropGuard installed before pending-error check | Pending failure drops guard once with unchanged original exception. Success completes T1, drops the now-empty builder, then dispatches CallArguments. No use of builder_ptr after guard drop. |
| GPU clone/snapshot helpers | unsafe documented borrowed builder; GPU bridge caller retains its builder input | clone returns a newly-owned builder and no payload reference; snapshot copies Vec words while the caller-owned builder retains values. The subsequent bind runs while that outer owner remains live. Existing cfg(test) controls exercise clone/snapshot without claiming GPU execution. |
| heap lifecycle visitation and terminal detach | managed allocation remains owned through header retirement | Visitation projects inline fields under existing token custody. Terminal capacity counts them before mutation; detach empties both fields before the object-wide sink releases finalizers. Rust Vec storage is moved to a local owner, not dropped in place then read again. |
| PtrDropGuard trace | guard owns live allocation before its call to molt_dec_ref | Trace reads before decrement. All registry-based post-free observations are deleted, not translated into header reads. |

The supplied internal API already excludes forged/freed raw addresses; this does
not add that exclusion. Runtime discovery found no external field consumer of
CallArgs storage beyond these paths. Existing builder map is not sufficient to
make arbitrary-pointer calls safe: consuming entries install/release a raw object
guard even on validation failure. A still-live wrong-kind object remains a safe
negative control at require_callargs_ptr. No memory-safety claim is based on a
source spelling, a class name or a runtime cache hit.

2. Dominance is narrowed explicitly: the largest exclusive *function* is usize
hashing, not a proven largest exhaustive semantic family. The exactly attributed
removable subset is 2,396.96 hash Ir + 1,039 guard-body Ir = 3,435.96 Ir/op,
21.46% of plain construction, before its redundant Box allocation/free and
mutex/context bodies. That measured portion justifies the selected structural
correction without claiming RC, generic namespaces or dataclass dispatch is fixed.
The full raw exclusive rows conserve the total; residual mechanisms remain
visible and will be reported. This is a scoped slice, not HF-685 row closure.

3. Executable acceptance rule, fixed before A/B: for each of five pairs let
r_i=head_instruction_slope_i/base_instruction_slope_i, R=median(r_i), and
S=max(abs(r_i/R-1)). Report R and S. Plain qualifies only if R<1 and
max(all five head slopes)<min(all five base slopes), with every source/output/
guard admitted. New-and-update or dataclass is a material regression if its
head median slope exceeds its base median by more than that primitive's existing
noise_bound from the unchanged primitive ratchet. Also apply the existing
per-runtime paired Molt/CPython spread gate; exceeding it means inconclusive,
never permission to discard/rerun samples. Retain startup, RSS, wall time and
binary sizes independently; a >=5% median RSS or binary growth requires explicit
investigation before accepting the slice. No favorable result substitutes for
semantic or static gates.

Additional audit resolution: the inline CallArgs payload is counted by the ordinary
object-allocation bytes authority. ALLOC_BYTES_CALLARGS then accounts only for
external positional Vec capacity, removing its former Box-size term to avoid
counting inline bytes twice. Existing later argument-vector reservations retain
their own accounting. This does not change ALLOC_CALLARGS_COUNT semantics.

Round-1 implementation-readiness review pending. Structural implementation remains
blocked until that review is resolved and the complete design is committed.

### Completed structural review gate

Architecture reviewer completed first (review-architecture.md); its two evidence
blockers and decision-rule request are answered above. A separate fresh read-only
Codex gpt-6.1-sol/high implementation-readiness reviewer then read only this file
(review-readiness.md) and reported no concrete phase-2 blocker. It accepted the
custody, callback, initialization/retirement, accounting, scoped attribution and
fixed acceptance-rule resolutions. Both reviews ran sequentially, without a
session fork, source inspection or proof execution. This records round 1 complete.
The only remaining prerequisite is committing this reviewed design before code.
