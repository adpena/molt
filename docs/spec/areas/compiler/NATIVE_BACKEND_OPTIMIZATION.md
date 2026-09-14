# Native Backend Optimization

This document maps optimization work to its implementation authorities and
evidence requirements. It is not a support matrix, benchmark result, or separate
architecture specification. `docs/spec/areas/compiler/0100_MOLT_IR.md` owns the
IR architecture; `docs/agent/ORCHESTRATION.md` owns coordination and proof custody.
The historical April 2026 audit is available in Git history. Its monolithic-file
locations, line counts, feature-absence claims, and estimated speedups are not
current evidence.

## Source authority

| Concern | Owning source |
| --- | --- |
| Build stages and frontend invocation | `src/molt/cli/build_pipeline.py`, `src/molt/cli/frontend_pipeline.py` |
| Frontend assembly and Python midend | `src/molt/frontend/__init__.py`, `src/molt/frontend/lowering/midend_pipeline.py` |
| Exact scalar producer provenance | `runtime/molt-passes/src/tir/type_refine/exact.rs` |
| Value-keyed representation and non-heap proofs | `runtime/molt-passes/src/representation_facts.rs` |
| Native name projection and LLVM representation view | `runtime/molt-tir/src/representation_plan.rs` |
| Generated operation semantics and lowering policy | `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py` |
| Operand-dependent operator results and effects | `runtime/molt-ir/src/tir/op_semantics.rs` |
| Frontend immutable literal identity | `src/molt/compiler_analysis/literal_identity.py` |
| Cranelift function lowering and scalar boundaries | `runtime/molt-backend-native/src/native_backend/function_compiler.rs`, `runtime/molt-backend-native/src/native_backend/function_compiler/scalar_carriers.rs` |
| LLVM unary lowering and materialization | `runtime/molt-backend-native/src/llvm_backend/lowering/numeric_ops.rs`, `runtime/molt-backend-native/src/llvm_backend/lowering/value_materialization.rs` |
| WASM representation lowering and result transport | `runtime/molt-tir/src/tir/lower_to_lir.rs`, `runtime/molt-backend-wasm/src/wasm/lir_fast/lir_runtime_ops/call_abi.rs` |
| Call facts, effects, and ownership | `runtime/molt-passes/src/tir/call_facts.rs`, `runtime/molt-passes/src/tir/passes/effects.rs`, `runtime/molt-passes/src/tir/passes/liveness/raw.rs` |
| Native link plan and artifact custody | `src/molt/cli/native_link_plan.py`, `src/molt/cli/native_link_command.py`, `src/molt/cli/native_link_custody.py` |
| Shipping codegen profiles and package overrides | `Cargo.toml` |

The frontend is assembled from visitor/lowering modules; native opcode families
live beneath the function compiler. Historical `cli.py` and backend `lib.rs`
line references must not be used to infer today's dispatch structure or costs.
SimpleIR names remain a native transport projection, not permission to recreate
semantic facts from strings or hints. Change the shared authority and its
consumers together when that projection exposes a correctness or performance
limit.

## Representation and operation boundaries

`extract_exact_scalar_map` owns exact producer provenance, including individual
result slots and executable copy/phi edges. `repr_by_value_for` raises exact
Bool/F64 producers and applies integer range or checked-overflow proofs.
`Repr::default_for` remains the conservative semantic floor: Bool/F64 are boxed
and integers remain BigInt-safe. An annotation, refinement, subclass hint, or
return hint cannot authorize raw storage.

LLVM and WASM/LIR consume the shared value-keyed map. Native `repr_by_name` is
its emitter-capability-filtered projection, not an independent semantic
authority. Native lowering consults one `ScalarRepresentationPlan`:

- `is_raw_int_carrier_name` identifies raw i64 homes. `RawI64Safe` proves the
  inline integer window; `RawI64FullDeopt` describes full-width checked storage,
  not permission for arbitrary unchecked Python arithmetic. Escape boxing must
  preserve full-width values through the overflow-safe boundary.
- `is_bool_unboxed` identifies raw 0/1 homes; other bools remain tagged in their
  main I64 home. `is_float_unboxed` identifies physical F64 homes; other floats
  remain boxed and use explicit extraction at proven float use sites.
- Lane selection, guard satisfaction, non-heap classification, merge rebinding,
  and cleanup use those same facts. Dead F64 homes are scrubbed with F64 zero;
  boxed homes retain their boxed cleanup sentinel. Separate scalar membership
  sets or shadow maps must not become another authority.

Generated integer-bitwise rules cover `&`, `|`, `^`, shifts, and inversion.
Successful operations on exact built-in integer operands produce Python
integers regardless of magnitude; only bool/bool `&`, `|`, and `^` preserve
bool. This semantic result fact neither proves raw storage nor removes
exceptions. Raw shift lowering additionally needs the applicable range proof
and a count in `[0, 63]`. Dynamic operands must retain runtime dispatch and its
exception or warning behavior.

Opcode-wide facts for overloaded operations are conservative. Exact admissible
operands may recover purity through the shared operation-instance authority;
annotations and transport spellings are not evidence. Frontend CSE/LICM and Rust
DCE/GVN/exception consumers must preserve callbacks and invalidate heap reads
across possible mutation. A pure result calculation is not necessarily no-throw:
mixed unbounded-int/float conversion, division, powers and shifts retain their
semantic exceptions. Bool inversion also retains observable warning behavior.
Bytes/text and bytes/integer equality retain option-dependent BytesWarning
effects unless a target policy proves them disabled. The registry projects these
same effects into Python and Rust. Purity does not prove commutativity: the
generated reorder domain admits numeric addition, not string/bytes concatenation.

SCCP value facts must be recursively immutable. Rust uses shared immutable tuple
storage and never snapshots mutable lists or dictionaries. Frontend joins,
convergence comparisons, static truth, and constant CSE use typed literal keys:
`1`, `True`, `1.0`, signed zero, and distinct NaN payloads cannot be conflated by
host Python equality. Equal literal values do not prove Python object identity.

Unary lowering must honor both operand and result carriers. Negating
`i64::MIN`, including the negative arm of `abs`, needs a BigInt; deferred boxing
cannot repair an already wrapped machine result. Cranelift Neg/Abs require an
inline-safe result for their unchecked raw lane, and Pos/Invert require a raw
output home before directly storing raw bits. Other numeric results cross
`def_var_from_numeric_result`, which distinguishes physical F64 from boxed I64
transport. LLVM uses boxed materialization for unary runtime arguments. WASM
Neg consumes the generated overflow-dispatch policy from LIR lowering; unary
runtime results use the shared result sink instead of entering raw locals as
boxed bits.

Unary plus is identity only for exact numeric carriers: Bool must become int,
and dynamic values must dispatch `__pos__`. Preserve the Bool tag for inversion
and its runtime warning behavior. Float negation must preserve IEEE sign-bit
semantics, including signed zero; subtraction from zero is not an equivalent
replacement.

## Ownership and call provenance

Liveness projects the shared carrier authority and exact None provenance through
`non_heap_values_for`; it does not reinterpret annotations as permission to drop
reference-counting obligations. A float subclass may be a heap object even when
its annotation looks scalar. Escape analysis, field-store optimization, RC
elision, and cleanup must preserve that distinction across native and WASM
consumers.

Call facts distinguish proven direct targets from opaque calls using typed
operation provenance and module membership. Builtin-looking names, absent
exception handlers, and annotations are not target, no-throw, no-allocation, or
raw-return proofs. Unknown facts remain conservative. Inlining eligibility must
come from the inliner's own decision authority, and a call-fact table must
invalidate with the operations and CFG on which it depends. Diagnostics must
report the conservative floor or the actual value-keyed proof, not a stronger
claim reconstructed from transport hints.

## Evidence before optimization claims

Separate frontend time, analysis/lowering time, codegen, linking, execution,
peak memory, and final binary size. A change can improve one while worsening
another. Record baseline and candidate compiler revisions and artifact identities;
hold the workload corpus, toolchain, target, and profile fixed unless one is the
variable under study. Distinguish cold, warm, and edit/rebuild runs. Record
cache hits, invalidations, commands, failures, and evidence locations using the
existing project receipt and benchmark mechanisms. Timing telemetry must not
change deterministic optimization decisions.

`tools/frontend_hot_pass_profile.py` profiles frontend lowering over a
deterministic corpus. Native implementation regressions belong with their
consumers, including
`runtime/molt-backend-native/src/native_backend/function_compiler/tests/scalar_carriers.rs`;
the WASM counterpart is
`runtime/molt-backend-wasm/src/wasm/lir_fast/tests/arithmetic/runtime_helpers.rs`.
IR shape and verifier tests establish lowering invariants, not CPython semantic
parity or a runtime speedup. Those claims require replayable differential and
performance receipts for the applicable Python-version, backend, OS, and
architecture cells. No local test establishes unexecuted matrix cells.

Read shipping settings from `Cargo.toml`, including package overrides, rather
than copying a profile here. Linker flags, dead-section removal, selected
archives, and symbol custody must be established from the actual target link
plan and output. Do not infer binary composition from a historical hello-world
estimate or promise cross-boundary LTO benefits without a supported toolchain
and a measured consumer.

## Profile-driven hypotheses

These are investigation directions, not confirmed missing features or ranked
speedup estimates:

- If frontend profiles show repeated parsing, analysis, or serialization,
  identify the duplicated work and its invalidation boundary before adding a
  cache, daemon, or alternate transport.
- If codegen dominates, measure per-function lowering, import/signature reuse,
  and rebuild topology before moving modules or changing crate boundaries.
  File decomposition alone does not prove reduced compiler work.
- If runtime dispatch, boxing, or RC dominates a workload, trace the missing
  exact fact through calls, joins, escape boundaries, and backend consumers.
  Extend the existing proof authority rather than introducing a local hint.
- If allocation, field access, or container operations dominate, evaluate the
  existing escape, storage, and alias facts against that workload before
  claiming a missing specialization or adding speculative dispatch.
- If link time or binary size dominates, measure reachable sections, archive
  selection, code duplication, and profile tradeoffs before changing inlining,
  LTO, runtime packaging, or target-specific linker policy.

Prioritize demonstrated frontier gain against critical-path cost. Preserve
semantic and custody invariants while measuring; record incomplete or failed
proofs explicitly rather than promoting an optimization hypothesis to support.
