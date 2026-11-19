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
| Frontend assembly and serialization | `src/molt/frontend/__init__.py`, `src/molt/frontend/lowering/serialization.py` |
| Exact scalar producer provenance | `runtime/molt-passes/src/tir/type_refine/exact.rs` |
| Value-keyed representation and non-heap proofs | `runtime/molt-passes/src/representation_facts.rs` |
| Native value representation and linkage carriers | `runtime/molt-tir/src/representation_plan.rs` |
| Final native body/fact ownership, reachability and partitions | `runtime/molt-backend-native/src/native_backend/simple_backend/program_pipeline.rs` |
| Shared final typed transformations and ownership planning | `runtime/molt-passes/src/tir/drop_phase.rs`, `runtime/molt-passes/src/tir/passes/deforestation/split_field.rs` |
| Generated operation semantics and lowering policy | `runtime/molt-ir/src/tir/op_kinds.toml`, `tools/gen_op_kinds.py` |
| Operand-dependent operator results and effects | `runtime/molt-ir/src/tir/op_semantics.rs` |
| Frontend immutable literal identity | `src/molt/compiler_analysis/literal_identity.py` |
| Whole-function Cranelift lowering and scalar boundaries | `runtime/molt-backend-native/src/codegen_ssa/walk.rs`, `runtime/molt-backend-native/src/codegen_ssa/scalar.rs`, `runtime/molt-backend-native/src/native_backend/codegen_builder.rs` |
| Shared numeric lowering and LLVM instruction selection | `runtime/molt-backend-native/src/codegen_ssa/numeric.rs`, `runtime/molt-backend-native/src/llvm_backend/lowering/codegen_builder.rs` |
| wasm32 lowering (LLVM's wasm32 target) | `runtime/molt-backend-native/src/llvm_backend/wasm_entry.rs`, `runtime/molt-backend-native/src/codegen_target.rs` |
| Call facts, effects, and ownership | `runtime/molt-passes/src/tir/call_facts.rs`, `runtime/molt-passes/src/tir/passes/effects.rs`, `runtime/molt-passes/src/tir/passes/liveness/raw.rs` |
| Native link plan and artifact custody | `src/molt/cli/native_link_plan.py`, `src/molt/cli/native_link_command.py`, `src/molt/cli/native_link_custody.py` |
| Shipping codegen profiles and package overrides | `Cargo.toml` |

Cranelift AOT and JIT consume a `NativeFinalFunction` containing the terminal
TIR body and its value-keyed carrier plan. The module owns the linkage rows for
local and external declarations. Partitioning transforms the declarations and
bodies together. The object compiler discards source operations after ingest
and emission consumes one local body at a time.

## Representation and operation boundaries

The walker creates a physical entry, converts its linkage parameters into
their TIR carriers, then visits executable blocks in reverse postorder. Block
arguments transport SSA values. Cranelift seals its blocks when all predecessor
edges have been emitted.

`extract_exact_scalar_map` owns exact producer provenance, including individual
result slots and executable copy/phi edges. `repr_by_value_for` raises exact
Bool/F64 producers and applies integer range or checked-overflow proofs.
`Repr::default_for` remains the conservative semantic floor: Bool/F64 are boxed
and integers remain BigInt-safe. An annotation, refinement, subclass hint, or
return hint cannot authorize raw storage.

Cranelift and LLVM read `ProducerCarrierPlan` by SSA value ID.
`is_inline_safe_int` proves the inline integer window and `is_full_deopt_int`
allows full-width checked storage with overflow-safe boxing. Refcount lowering
uses both the held carrier and its value proof. WASM/LIR draws representation
facts from the same value-keyed analysis.

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
For generated heap **code-literal** producers, GVN's Rust payload keys and
scoped dominance permit reuse of the first materialization; this is a literal
admission rule, not an equality proof for arbitrary runtime objects.
`ownership_lattice_min::Replacements` preserves an independent reference when
either literal binds a Python local; two unbound literals share the terminal
drop root. The first constructor and its exception edge remain.

Unary lowering must honor both operand and result carriers. Negating
`i64::MIN`, including the negative arm of `abs`, needs a BigInt. The shared
lowering admits raw arithmetic only where its result proof permits the
operation. The runtime handles the remaining Python cases.

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
`runtime/molt-backend-native/src/codegen_ssa/differential.rs`.
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
