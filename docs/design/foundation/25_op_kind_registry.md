<!-- Design doc (task #57). Audit anchors refreshed against the current worktree 2026-06-13. The op-kind registry source is live; this doc tracks the remaining dangerous-cell burndown. -->

# Op-Kind Single-Source-of-Truth Registry

**Status:** Registry source + generated sync are live; dangerous-cell burndown remains active. The machine-generated enumeration lives in `tools/audit_op_kinds.py` + `tools/op_kinds_baseline.json`.

**Bug class killed:** cross-component op-"kind"-string drift — molt's most prolific silent-miscompile family (5 proven instances; see "Motivation").

The gap counts and async-op examples in §4 below are a June 2026 audit snapshot,
not the current operation inventory. Source calls now dispatch through live
callable bindings: the obsolete spelling-specialized channel, spawn,
cancellation, future/promise, and thread-submit SimpleIR lanes (including
channel-specific yield opcodes) were retired. `block_on` and `call_async`
remain live state-machine operations; runtime intrinsic providers remain
separate from retired compiler operation spellings.

---

## Fail-closed operation admission

The registry owns both the frontend MoltOp vocabulary and the SimpleIR wire
vocabulary. `tools/op_kinds/registration.py` projects existing mapper, control,
custody, field-role, and runtime-semantic declarations into generated membership tables.
`simpleir_preserved_kinds` registers wire carriers without granting new effect,
ownership, or target-support facts. `frontend_lowering_kind` records frontend
spellings whose serializer uses a different registered wire spelling.
`simpleir_backend_private_kinds` records target-owned operations (including
Luau exception-capture markers and string operations) that shared SSA must
re-lift. These registrations grant no effect, ownership or cross-target support
facts. Full-program admission rejects them at every public transport and checked
backend entrypoint, before preprocessing can discard operations. Function-level
shape validation and SSA admit them only for internal lowering and re-lifting;
frontend serialization also rejects them. The generated private-owner projection
and shared `validate_simple_ir_op_shapes` boundary enforce this distinction.

Frontend serialization validates all input kinds before optimization can discard
them, rejects registered operations without a serializer, and validates emitted
wire kinds. SimpleIR admission validates every function body before target
selection, pruning, or lowering, through the shared contract used by JSON,
NDJSON, MessagePack, and binary document readers. Direct SSA lifting shares the
registered-kind validator while allowing the internal private vocabulary.
Unregistered operations report the kind and function instead of
being dropped or acquiring the preserved `Copy` carrier. Registered preserved
operations retain their existing lowering and target-specific admission.

`tests/test_frontend_op_kind_registry.py` scans every literal `MoltOp(kind=...)`
in the frontend and the serializer's complete resolved wire vocabulary against
the generated tables. Generator freshness tests keep the Python and Rust
projections synchronized.

## Predicate result, effect, and representation authority

The six rich comparisons are callback-capable, throwing, arbitrary-owned-value
operations at opcode scope. `predicate_semantics` selects equality, ordering,
truth conversion, or containment;
`comparison_scalar_domains` generates the identical exact-primitive pair policy
for the Python frontend and shared native/WASM Rust pipeline. Exact scalar pairs
produce bool; equality is total except for the warning-sensitive pairs declared
in `comparison_warning_pairs`, whose warning callbacks and exceptions remain
observable. Ordering is nothrow only within an ordering-enabled domain. Unknown
operands remain impure/DynBox.

Frontend memory effects and arbitrary-heap access are distinct projections.
The existing `frontend_effect_kind` rows may state a validated boolean
`may_access_arbitrary_heap` when the pre-serialization operation is narrower
than its mapped opcode. Thus a slot load or retain-only binding alias can
preserve receiver-class facts without pretending that all preserved `Copy`
operations are pure. Unknown operations remain invalidating; slot stores that
can release displaced owners remain arbitrary-heap boundaries.

Annotations, return hints, and subclass-accepting guards are not exactness proofs.
Frontend queries follow intrinsic defining operations; TIR's
`type_refine::extract_exact_scalar_map` propagates intrinsic producers, existing
scalar transfer rules, and SSA edges without importing those hints. The separate
`exact_scalar_result_type` fact preserves heap BigInt semantic exactness without
changing its boxed carrier. Refinement, DCE, GVN, LICM, exception elimination, and
representation floors consume the shared `op_semantics` facts. No rich comparison may
declare an operand-independent bool result or an unconditional bool projection.
SROA's reference-count-neutral stores consume the same exact-producer map and,
for integers, an inline-range proof. Its former constant-immediate table and
annotation/range-only fallback are retired; arbitrary Python arguments cannot
lose reference-counting stores merely because they carry scalar hints.
Truth and containment produce exact bool values on success without licensing
callback elimination: unknown truth operands and containment remain impure.
The manifest's `operand_arity` and `result_arity` share one operation-shape
admission across verification, effects, intrinsic result slots, guard hints and
scalar provenance. Fixed operands, variadic builders and key/value pairs are
explicit; a malformed producer cannot mint exactness or fall back to a pure
opcode effect. Double logical negation
can collapse to its input only when that input is an exact bool.

Initial lifting and refinement project scalar function return contracts from
the assembled exact-producer map, never annotation-derived arithmetic facts.
Generic `Pow` stays DynBox even with exact numeric operands: negative exponents
can change integer results to float, and fractional powers can produce complex.

At runtime, `object/ops_compare.rs` owns class-slot lookup, strict-subclass
reflected priority, NotImplemented fallback, and result custody. Equality values
and list/tuple ordering preserve arbitrary result identity. Truth boundaries
(container equality, sorting, C API bool requests) consume and release the owned
result and propagate truth-conversion errors. `object.__ne__` delegates to only
its receiver's equality slot; the common dispatcher owns reflection.

## Owned literal payload authority

`literal_payload_opcodes` records scalar and owned carrier shapes in the opcode
registry. The generator emits `LiteralPayloadKind`, the shared
`OwnedLiteralPayloadKind`, and their exhaustive opcode projections. IR admission
selects carrier validation through those facts, including canonical kind aliases.
String bytes remain lossless for surrogate code points, embedded NULs and empty
payloads; absent or conflicting carriers remain errors.

WASM derives its literal policy from that registry through
`molt.opcode_literal_payloads`. Its manifest owns materializer imports, scalar
seeds and backend lowering choices; authored `literal_payload` rows are rejected.
Materialization, scratch ownership and frame anchors use the shared owned enum;
absence is `Option::None`, so allocating owned scratch cannot accept a missing
payload shape. The registry and projection leaf are direct WASM generator inputs.

Source audits and generator module readers share `molt.rust_source_scan` lexical
offsets and Rust module declarations. Test-only file ownership excludes independent
oracles from production semantic debt, while a file also owned by production is
still audited. Test attributes mask exactly their item; later production siblings
remain visible. Guarded wildcard arms do not replace the unconditional match
default, and region boundaries ignore braces in comments and literals. The
structural metric baselines remain unchanged by this authority migration.

## Source target admission and structural debt

`AdmittedTargetProgram` borrows the validated input and owns the same function
representation plans produced by shared target admission. Rust's checked entry
creates this witness before its private source assembler can run; checked
publication still refuses accumulated lowering failures. The backend's internal
source fixtures retain their test-only path without granting production support.

`tools/op_kinds/runtime_requirements.py` composes registered wire kinds, minimum
runtime requirements, target profiles and numeric roles for both generated Rust
and Python consumers. The structural audit can exclude a literal dispatch domain
only when the actual source admission, immutable input, generated projections and
private checked-publication boundary establish that the domain cannot arrive.
Refusal-only handlers for those domains are deleted from the Rust backend.

The source audit does not infer a complete support promise from these facts.
Payload-dependent numeric/string domains, unknown spellings, wildcard dispatch,
and unresolved receiver/control flow remain obligations. Rust strings now consume
the shared byte/text literal carrier losslessly when UTF-8 is representable;
surrogatepass strings still expose the missing Python string representation.

## 1. Motivation — the bug class (5 proven instances)

A `MoltOp` produced by the frontend visitors is serialized to a JSON op whose `"kind"` string is the **wire contract** between the Python frontend and the Rust backend. Five independent components must agree on that vocabulary, and each keeps its **own private copy** of the table:

1. **Frontend emitter** — `src/molt/frontend/lowering/serialization.py` plus the extracted `serialization_*_ops.py` handler modules. `map_ops_to_json` dispatches to the handlers, which emit the JSON `"kind"` string (lowercase). **This is the authoritative wire vocabulary** (see §3).
2. **TIR SSA mapper and reverse backend spelling** — `kind_to_opcode` in `runtime/molt-ir/src/tir/ssa.rs:1902`, backed by `op_kinds_generated.rs:20`, maps a kind string → `OpCode`. `opcode_canonical_kind_table` is the generated reverse authority for backend op-name tails such as MLIR opaque lowering. Unknown input kinds deliberately fall back to `OpCode::Copy`, stashing the spelling in `_original_kind`, as the runtime backstop behind the generated registry.
3. **LLVM lowering** — `lower_preserved_simpleir_op` (`runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops.rs`) dispatches through handler-owned `HANDLED_KINDS` slices in `preserved_ops/{direct_ops,callable_ops,container_ops}.rs`, plus the ABI-exact `molt_<kind>` runtime fallback `try_lower_preserved_runtime_call` (`lowering/runtime_helpers.rs`), guarded by a **terminal fail-loud** state in `lowering/op_dispatch.rs`.
4. **RC/alias classifier** — `classify_copy_kind` / `copy_kind_mints_owned_value` / `copy_kind_is_explicit_no_heap_move` in `runtime/molt-passes/src/tir/passes/alias_analysis.rs:535/496/645`. **Its `_ => CopyLowering::TransparentAlias` default (alias_analysis.rs:564) is the UAF-escalation precondition.**
5. **Native + WASM SimpleIR dispatch** — extracted native `function_compiler/fc/*` handler slices plus the WASM facade/child modules, reached via the `lower_to_simple` `_original_kind` restoration (`runtime/molt-passes/src/tir/lower_to_simple/op_lowering.rs`).

The proven failures:

- **(#1) `matches!`-oracle default-false** (the ModuleImportFrom lesson): an opcode added to the system but missing from a `matches!`-based effect oracle (`opcode_may_throw` / `is_side_effecting`) defaults to "no effect" → SCCP/LICM eliminate a side-effecting op.
- **(#2) `STATIC_TYPE_COUNT` stale base**: a hand-maintained count drifted from the enum it counted.
- **(#3) intrinsic resolver name ≠ symbol** (asyncio P0): the resolver keyed by name while the runtime queried by symbol.
- **(#4) inliner `__molt_closure__` literal duplication** (task #44): a string literal copied into two files drifted; fixed by a shared const.
- **(#5) kind-string drift** (this task): `serialization.py:633` emits `"floordiv"`; `ssa.rs:1798` recognized only `"floor_div"` → silent lift to `Copy{_original_kind}`; `matmul` (serialization.py:736) had **no mapper entry at all**. On stale bases this escalated to UAF under drop insertion.

The structural cause is identical in every case: **N copies of one table, no compiler-enforced agreement.** The fix is ONE table that generates all N copies + a CI sync test that turns drift into a build error.

---

## 2. The phase-1 enumeration (machine-generated)

`tools/audit_op_kinds.py` extracts each component's table **directly from source** (never hand-copied) and prints the drift matrix. Extraction methods:

- **Frontend (Python):** `ast`-based over `serialization.py` and extracted `serialization_*_ops.py` handler modules. Constant `"kind": "literal"` dict-value literals plus computed sites are resolved structurally:
  - `serialization.py:635` `op.kind.lower()` under `op.kind in ("ADD","SUB","MUL")` → `{add,sub,mul}`.
  - `serialization.py:647` `op.kind.lower()` under `("INPLACE_ADD","INPLACE_SUB","INPLACE_MUL")` → `{inplace_add,inplace_sub,inplace_mul}`.
  - `serialization.py:2419` `{"BOX":"box","UNBOX":"unbox","CAST":"cast","WIDEN":"widen"}[op.kind]` → `{box,unbox,cast,widen}`.
  - `serialization.py:4329` bare `op.kind` under `("gpu_thread_id",…,"gpu_barrier")` → the 5 gpu kinds.
  Resolution walks the AST parent chain to the enclosing `if op.kind == …`/`in (…)` guard, then interprets the local assignment (`.lower()` transform or dict-subscript). **An unresolved computed site is a hard error** (the extractor cannot prove the wire vocabulary).
  **Total: 431 emitted JSON kinds** (416 literals + 15 spellings from the 4 computed sites).
- **Rust `match` arms** (`kind_to_opcode`, `classify_copy_kind`, and handler drift checks): a line-anchored brace/comment-aware state machine. It locates `fn NAME`, finds `match X {`, brace-matches the body, then collects the string literals of every **top-level** arm pattern (left of `=>`), skipping `//`+`/* */` comments and `"strings"`, and skipping each arm body whether `{}`-block or comma-terminated. **Validated against floordiv/floor_div/matmul + `index` (a `{}`-block arm following another `{}`-block arm).** Failure modes (each absent in the parsed functions, asserted/documented): a `=>` inside a pattern literal (impossible — kinds are identifiers); raw strings `r"…"` in a pattern (asserted absent via `(?<![A-Za-z0-9_])r#*"`); macro-generated arms (none); nested `match` in a body (handled by the balanced-brace skip).
- **LLVM preserved-op handler slices** (`preserved_ops/{direct_ops,callable_ops,container_ops}.rs`): dedicated LLVM coverage is extracted from the local `HANDLED_KINDS` slices that `preserved_ops/op_family.rs` routes. The audit separately compares each slice to its adjacent `match kind` body and reports `llvm_preserved_handler_routing_drift` as a dangerous cell if they diverge.
- **`matches!(…)` arms** (`copy_kind_mints_owned_value`, `copy_kind_is_inert_marker`, `copy_kind_is_explicit_transparent_alias`, `copy_kind_is_explicit_no_heap_move`): balanced-paren extraction of the macro body's literals + `.starts_with("PREFIX")` prefix rules.
- **Runtime ABI surface:** generic direct/preserved eligibility comes from normalized boxed-call contracts. Runtime export scanning verifies machine shapes; it never upgrades raw i64 carriers into object values. Fixed/custom declarations and residual raw/mixed helpers remain separately audited.
- **Structural/pre-SSA consumed kinds** (not routed through `kind_to_opcode`): **owned** by `[[simpleir_control_kind]]`, generating `tir::is_structural`, CFG block-boundary helpers, and `lower_from_simple` pre-SSA membership. (Drift-proof: a new structural/pre-SSA/SSA-only kind must be classified in the registry before generated consumers or the audit accept it.)
- **Native/WASM SimpleIR arm presence** (advisory): native coverage now unions the extracted `function_compiler/fc/*::HANDLED_KINDS` op-family authorities (plus inline dispatch slices) with the legacy textual `function_compiler.rs` arm scan, so decomposition does not hide real native handlers from the audit. WASM still uses a textual scan for arm-shaped `"a" | "b" … =>` tokens (every OR-alternative captured). **Advisory only** — textual scans can over-/under-count (guards, bindings, unrelated helper arms); never a sole basis for a disposition.

### Source table sizes (current worktree, 2026-06-13)

| table | size |
|---|---|
| frontend emitted JSON kinds | **431** (416 const literals + 4 computed sites resolving to 15 spellings) |
| `ssa.rs kind_to_opcode` arms | 150 |
| LLVM preserved-op handler slices | 157 |
| classifier OwnedValue allow-list | 48 (+ `vec_*` prefix) |
| classifier InertMarker arms | 13 |
| classifier transparent-alias set | 207 |
| classifier no-heap-move (alias) set | 7 |
| structural/pre-SSA consumed kinds | 23 |
| runtime `molt_*` extern exports | 3531 |

---

## 3. The authoritative-layer decision: serialization JSON kind, NOT MoltOp vocabulary

There are two candidate "kind" vocabularies:

- The **`MoltOp.kind`** vocabulary — UPPERCASE (`"FLOORDIV"`, `"MATMUL"`, …), created at ~1777 `MoltOp(kind=…)` sites across `src/molt/frontend/visitors/`.
- The **JSON `"kind"`** vocabulary — lowercase, emitted by `map_ops_to_json` (`serialization.py:396`).

**Decision: the JSON `"kind"` string is the single source of truth for the cross-component contract.** Rationale:

1. The `MoltOp.kind` vocabulary is **fully internal to the frontend** — it is consumed in its entirety by `map_ops_to_json` and never crosses the process boundary. Every backend component (ssa.rs, lowering.rs, alias_analysis.rs, function_compiler.rs, wasm.rs) keys on the **JSON kind**.
2. `map_ops_to_json` is already a **translation boundary** (uppercase MoltOp → lowercase JSON, with folds/fusions). Several MoltOp kinds map to a different JSON kind (`BOX`→`box`, `INPLACE_ADD`→`inplace_add`) and some MoltOp kinds produce *no* JSON op (folded) or a *different* JSON op (e.g. `ADD`→`const_bigint` on overflow fold). Making the upstream MoltOp vocabulary "authoritative" would not capture what actually reaches the backend.
3. The proven bug (#5) was a **JSON-kind** drift (`"floordiv"` vs `"floor_div"`), not a MoltOp drift.

Phase 2's table is therefore **keyed by the emitted JSON kind**. (The MoltOp→JSON translation in `map_ops_to_json` remains the frontend's business; the registry constrains its *output* vocabulary, not its internal enum.)

---

## 4. The drift matrix — dangerous-cell findings

The audit records drift observations and bug preconditions. Each finding is limited by its source authority: frontend dormancy does not establish whole-pipeline deadness, and an owned boxed return does not establish heap allocation. Emitted-but-unmapped is intentional for preserved `Copy{_original_kind}` operations with valid lowering.

| category | count | meaning |
|---|---|---|
| `llvm_coverage_gap` | **0** | emitted + unmapped + NOT llvm-covered (no arm or ABI-exact runtime fallback) → **LLVM build-fails loud** (fail-loud guard). **EMPTY.** |
| `ownedvalue_llvm_gap` | **0** | OwnedValue + not llvm-covered → the UAF/double-free precondition. **EMPTY = the LLVM fatal contract holds.** |
| `classifier_silent_fallthrough` | **0** | emitted + unmapped + classifier fell to `_ => TransparentAlias` (no explicit class) + is a real runtime op (`molt_<kind>` exists). **EMPTY = known transparent-alias decisions are table-visible.** |
| `simpleir_lane_gap` | **0** | emitted + unmapped + no native AND no wasm arm AND no symbol → nothing can lower it on the SimpleIR lanes. **EMPTY.** |
| `mapped_never_emitted` | **45** | a mapper arm the frontend never emits — mostly round-trip or explicit alias spellings (benign); `floor_div` is now an explicit alias of canonical `floordiv`. |
| `ownedvalue_never_emitted` | **0** | a OwnedValue spelling absent from frontend serialization; internal producers may still require it. |
| `llvm_boxed_runtime_abi_mismatch` | **0** | normalized object-value/void call contracts whose parameter or result shape disagrees with runtime exports. Dedicated raw machine declarations do not authorize generic calls. |

### 4.1 Disposition of every dangerous category

**`ownedvalue_llvm_gap = 0` and `simpleir_lane_gap = 0` are the headline:** on current main there is **NO silent miscompile and NO UAF from kind drift.** The original floordiv-class *silent* miscompile (operand-0 passthrough on LLVM) was already closed by a dedicated LLVM `"floordiv"` arm (lowering.rs:10325) and the universal LLVM fail-loud gate (lowering.rs:2410). Every remaining gap is either fail-loud (a build error) or leak-safe (a non-UAF reference leak).

**`llvm_coverage_gap` (26) — LATENT, fail-loud.** All 26 have native+wasm coverage; they fail-loud on LLVM only. Breakdown:
- **18 async/concurrency runtime ops** (`block_on`, `spawn`, `call_async`, `cancel_token_*` (8), `cancelled`, `cancel_current`, `chan_drop`, `future_cancel{,_clear,_msg}`, `promise_set_{result,exception}`, `task_register_token_owned`, `thread_submit`). These have runtime functions under **different spellings** (e.g. `spawn`→`molt_thread_spawn`, not `molt_spawn`), so the LLVM `molt_<kind>` probe misses them. *Disposition: latent LLVM gap* — the asyncio runtime surface is less mature on the LLVM lane; an async-heavy program targeting LLVM would hit a build error (not a miscompile). Repro sketch: `asyncio.run(main())` with a `create_task`/cancel path, `--target llvm`.
- **3 repr-identity ops** (`cast`, `widen`, `copy_var`). On NaN-boxed values these are **identities**; native/wasm lower them as operand-0 passthrough (`"box"|"unbox"|"cast"|"widen" => op` at function_compiler.rs:1490; wasm.rs:12511). On LLVM they carry `_original_kind` (set), so they hit the fatal gate. *Disposition: latent LLVM gap with a trivial fix* — add an identity arm to `lower_preserved_simpleir_op` returning operand 0. `copy_var` is emitted by the string-split-field fusion (`serialization.py:267`), so the trigger is narrow. Repro sketch: a program whose only `.split()[i]` consumers fuse, `--target llvm`.
- **2 loop-IV ops** (`loop_index_start`, `loop_index_next`). Consumed specially by `lower_from_simple.rs:201/278` (folded into a counted-loop IV) — they should never reach `kind_to_opcode`'s Copy fallback on the lift. *Disposition: benign* (structural-IV machinery; the audit flags them only because they are not in the CFG leader/terminator helpers — they could be added to the derived structural set in phase 2).
- **1 other** (`object_set_class`). It has native+wasm coverage, and shares `class_apply_set_name`'s native arm, but no LLVM arm. *Disposition: latent LLVM gap* — `obj.__class__ = C` on the LLVM lane fails loud. Repro sketch: `obj.__class__ = C`, `--target llvm`.

Closed in the current audit: the repr-identity ops (`cast`, `widen`, `copy_var`) now have explicit LLVM identity arms that bind result values to operand 0, matching the native/WASM NaN-box passthrough contract without weakening the terminal fail-loud guard. The loop-IV helpers (`loop_index_start`, `loop_index_next`) are also closed as LLVM-gap false positives: `[[simpleir_control_kind]]` marks them as `pre_ssa_rewritten`, and the audit derives that pre-SSA consumed set directly from the registry. Runtime fallback coverage now derives from normalized boxed-call semantics, with export shape checked separately. Async/cancellation/channel/thread operations and void side effects (`print_newline`, `spawn`) share that generated authority. An i64 carrier alone cannot authorize the generic path. The pointer-ABI operation `object_set_class` has dedicated LLVM lowering to `molt_object_set_class`. Field assignment has one `store`/`guarded_field_set` contract; unchecked initializer spellings and their duplicate LLVM arms are retired. `call_async` is closed by reusing the LLVM task-frame allocation authority already used by `AllocTask`, plus the native-compatible `molt_async_sleep` constructor special case.

**`llvm_boxed_runtime_abi_mismatch = 0` — required invariant.** Both ABI audits consume `runtime_boxed_call_specs` from the canonical manifest normalizer. Value and void calls share one semantic authority; a missing export or incompatible shape is a finding before frontend emission. The old handwritten void-only table and audit schema are retired.

**`classifier_silent_fallthrough = 0` — CLOSED.** The 207 table-visible transparent-alias decisions now live in `classifier_transparent_alias`, a generated table distinct from `classifier_no_heap_move`. This preserves the same leak-safe drop-insertion behavior (`TransparentAlias`, never `OwnedValue`) while making each known decision explicit: a future ownership promotion must move the kind out of the transparent-alias table and into `classifier_owned_value` with matching backend evidence, rather than hiding behind the `_ => TransparentAlias` default.

**`mapped_never_emitted` — frontend-only mapper dormancy.** The module phase re-lifts post-pipeline SimpleIR on every build, so `kind_to_opcode` MUST recognize generated round-trip spellings even when the *frontend* never emits them. Verified round-trip outputs (benign): `build_list`, `get_attr`, `set_attr`, `for_iter`, `yield`, `yield_from`, `checked_add`, `checked_mul`, `exception_pending`, `iter_next_unboxed`, … The prior `floordiv`/`floor_div` schism is closed in the live registry: canonical spelling is frontend `floordiv`, `floor_div` remains a table-visible alias, and `lower_to_simple` emits `floordiv` so round-trip output no longer recreates the old split. The remaining entries are alias arms such as `load_attr`/`store_attr`/`get_iter`/`const_int`/`call_function` plus generated round-trip vocabulary; they are benign as long as the alias set stays explicit and generated.

D5 and D6 measure absence from the Python serialization handlers. They do not
enumerate typed pass producers. For example, terminal ownership in
`drop_insertion/activation.rs` constructs `StateSet`, `IsPending`, and
`TaskWait`. Their typed shapes and mapper spellings belong to `op_kinds.toml`;
`lower_to_simple/op_lowering.rs` serializes them through
`opcode_canonical_kind_table`. Their `state_set`, `is_pending`, and
`task_wait` spellings therefore remain valid frontend-dormancy observations,
not dead operations or missing mappings. Neither a backend handler nor a
runtime callable by itself proves that a producer emits an operation.

The human report and baseline diagnostics share category-specific guidance.
This reporting distinction does not exempt names or change the exact baseline
gate: new and stale category members still fail until their source changes are
reviewed. Counts of D5/D6 observations are not counts of unreachable operations.

---

## 5. Phase-2 mechanism (the recommendation, 5 lines)

Current schema note: `op_kinds.toml` now also owns `result_arity`
(`zero`, `one`, `two`, or `variable`) and generates
`opcode_fixed_result_count_table`, so TIR verification consumes the registry
instead of maintaining a parallel opcode-to-result-count match. The generator
rejects `variable` unless the opcode is on the audited context-dependent
whitelist, so fixed-result opcodes cannot quietly escape verifier coverage.
The same table owns opcode-intrinsic result types through
`operand_independent_result_types`, generating result-indexed
`opcode_operand_independent_result_type_table` and
`opcode_operand_independent_result_tir_type`. Operand-dependent producers
(`Div`, shifts, arithmetic, `and`/`or`, indexing, iterators, calls, and tuple
builders) deliberately stay absent so `type_refine.rs` proves them only from
operand/attr facts. Checked arithmetic declares `[i64, bool]`; unboxed iteration
declares `[operand, bool]`, preserving the dependent payload and exact status
separately. Exception-pending reads are exact Boolean producers but remain
impure mutable-state observations. No result fact licenses code motion.
The exact and proven maps consume each intrinsic scalar result slot before and
after type refinement: an unknown iterator still produces a proven Boolean
status, not a proven element or a nonthrowing/pure iteration operation. The
shared result-slot regression family owns this distinction across producers.
`block_versioning.rs`, `fast_math.rs`, and `strength_reduction.rs` consume the generated intrinsic
table instead of private opcode matches; `gvn.rs` also consumes it as part of
value-key/type gating. Branchless counting consumes the shared exact-scalar map
and lazy value-range proof, requiring a nonallocating inline counter increment
and exclusive CFG arm predecessors instead of maintaining its own type map.
GVN numbering eligibility is also table-owned as a role lattice:
`gvn_always_numberable_opcodes`, `gvn_type_gated_numberable_opcodes`, and
`gvn_value_keyed_constant_opcodes` plus `gvn_numberable_attr_key_opcodes` generate
`opcode_gvn_numbering_role_table` plus
`opcode_gvn_value_key_spec_table`, keeping unconditional CSE, primitive-gated
CSE, same-block constant payload keys, and attr-sensitive numbered ops separate.
`ConstBigInt` is value-keyed by its exact decimal `s_value` payload, but still
does not seed type-refine guard proof because its result type is `DynBox`. Type-refine's
proven-map literal seeds are separate generated facts
(`proven_result_type_seed_opcodes` feeds
`opcode_is_proven_result_type_seed_table`) so payload identity and guard-proof
seeding cannot accidentally share a private pass-local opcode list.
Type-refine result-type membership is generated as two rule lattices:
`type_refine_attr_result_type_rules` feeds
`opcode_type_refine_attr_result_type_rule_table` for attr-derived class, call,
guard, and Copy-original-kind facts, while `type_refine_operand_type_rules`
feeds `opcode_type_refine_operand_type_rule_table` for operand-dependent
arithmetic, boolean, bitwise, iterator, indexing, tuple, Copy, BoxVal, and
UnboxVal rules. `type_refine.rs` owns only the rule semantics and live
operand/attr parsing, not private opcode membership.
Call graph and CallFacts share `FunctionCallSites`, an exact-site projection of
the mandatory `may_call_python` opcode effect, exact scalar primitive matrix,
GPU source provenance, and fixed-slot load/store lifetime facts. Callback
capability is independent of throwing and global-memory effects. Unknown
preserved Copy operations fail closed; actual value identity and independent
`callback_free_copy_kinds` contracts provide explicit exemptions. Trace entry
and exit preserve non-owning custody but are effectful, so alias analysis,
overflow peeling, and exception capture cannot ignore their callbacks. Direct targets retain
the existing source-role authority. Native leaf classification runs after
lifetime finalization so explicit release callbacks also prevent guard elision.
`async_work_poll_after_kinds` is solely the preserved call-return polling
protocol; it does not classify Python call edges.
SCCP constant folding now follows the same shape:
`sccp_constant_seed_rules` feeds `opcode_sccp_constant_seed_rule_table` for
constant constructors the lattice can seed from attrs, and
`sccp_constant_eval_rules` feeds `opcode_sccp_constant_eval_rule_table` for
foldable arithmetic, comparison, unary, list/dict, and tuple-as-list rules.
`sccp.rs` owns attr parsing, overflow refusal, Python division/mod/pow behavior,
compound-size caps, and concrete fold semantics, not opcode membership.
Value-range integer reasoning is also table-owned:
`value_range_transfer_rules` feeds `opcode_value_range_transfer_rule_table` for
modeled interval transfer functions, `value_range_const_fold_rules` feeds
`opcode_value_range_const_fold_rule_table` for checked integer constant folding
used by constant-mask and container-length derivation, and
`value_range_cond_narrow_rules` feeds
`opcode_value_range_cond_narrow_rule_table` for loop-guard true-edge upper-bound
narrowing. `value_range_container_length_rules` feeds
`opcode_value_range_container_length_rule_table` for fixed literal builders,
list-repeat candidates, and `len(...)` calls. `value_range.rs` owns the interval
formulas, saturation, Python shift/mod semantics, CFG polarity checks,
symbolic-len recording, builtin-name and operand-shape validation, copy
resolution, and raw-lane soundness boundary; it does not carry private
opcode-membership lists.
Range loop devirtualization pattern membership is generated too:
`range_devirt_roles` feeds `opcode_range_devirt_role_table` for the
CallBuiltin/GetIter/IterNextUnboxed roles in the `range(...)` iterator pattern.
`range_devirt.rs` owns builtin-name checks, operand/result shape, loop-header
role, dominance, and CFG validation; the registry owns only the closed
opcode-role lattice.
Polyhedral loop classification is generated too:
`polyhedral_loop_header_opcodes` feeds
`opcode_is_polyhedral_loop_header_table` for loops that may receive tiling
annotations, while `polyhedral_affine_body_opcodes` feeds
`opcode_is_polyhedral_affine_body_table` for the opcode-only affine body
allowlist. `LoopForest` owns loop headers and body block sets; `polyhedral.rs`
owns tiling annotation and live Copy refinement, not private loop traversal or
affine-body opcode lists.
Vectorization opcode classification is generated too:
`vectorize_opcode_facts` feeds `opcode_vectorize_facts_table`, so
`vectorize.rs` owns accumulator recognition, live Copy refinement, min/max
pattern validation, lane typing, and hint emission while body eligibility,
annotation targets, and reduction-family membership live in the registry.
`LoopForest` owns loop headers and bodies.
SSA attr transport is generated too:
`ssa_s_value_attr_keys` feeds `opcode_ssa_s_value_attr_key_table`, and
`ssa_original_kind_preserving_kinds` feeds
`simpleir_kind_preserves_original_kind_for_ssa`. `ssa.rs` owns live value
resolution and attr insertion while the registry owns string payload key routing
and mapped `_original_kind` preservation spellings.
Representation-aware LIR verifier dispatch is generated as a rule lattice:
`lir_verify_rules` feeds `opcode_lir_verify_rule_table`, so `verify_lir.rs`
owns BoxVal/UnboxVal/arithmetic/truthy-materialization invariant checks and
diagnostics without carrying a private opcode dispatch list.
The TIR pass fuzzer's fixed-shape opcode palette is registry-owned too:
`fuzz_tir_opcode_shapes` generates `FUZZ_TIR_OPCODE_SHAPES`,
`opcode_fuzz_tir_operand_count_table`, and
`opcode_fuzz_tir_attr_payload_rule_table` for
`runtime/molt-backend/fuzz/fuzz_tir_passes.rs`. That fact is deliberately
tooling-only operand and synthetic-attr generation shape; result counts still
come from `opcode_fixed_result_count_table`, and variable-result opcodes such as
`Copy` are rejected from the fixed-shape palette.
Drop-insertion suspension retain points are generated as a distinct ownership
fact: `drop_insertion_suspension_point_opcodes` feeds
`opcode_is_drop_insertion_suspension_point_table`, so `drop_insertion.rs`
retains live owned values across StateYield/channel/high-level yield ops without
using the broader state-machine legality set as a proxy.
`drop_insertion_return_deferral_barrier_opcodes` separately feeds
`opcode_is_drop_insertion_return_deferral_barrier_table`, keeping explicit
IncRef/DecRef/Free rails as the single registry-owned answer for roots that
cannot be extended to return-boundary cleanup.
Exception metadata has separate generated facts for separate invariants:
`exception_label_attr_opcodes` owns which ops carry a SimpleIR exception-label
attr, `exception_transfer_edge_opcodes` owns the subset that contributes
implicit CFG transfer edges, and `exception_region_nesting_roles` feeds
`opcode_exception_region_nesting_role_table` for TryStart/TryEnd lexical
nesting. DCE and SCCP own their try-depth traversal and dead-op/constant-fold
policy; the registry owns only the Enter/Exit role for the closed opcode set,
so nesting cannot drift into private TryStart/TryEnd matches beside the
label/transfer facts. The CFG derives which transfer edges bind their target's
arguments from the transfer and nesting facts
(`dominators::exception_edge_binds_handler_arguments`): a transfer edge whose op
enters a region is a registration, which keeps its handler reachable but binds
nothing.

Block retirement is owned by `TirFunction::retain_blocks`: it validates retained
terminator and exception-label references atomically, then removes blocks and
their label/value projections together. `structural_block_roots` includes every
loop metadata key and endpoint. A transform replacing a loop calls
`retire_loop_metadata` explicitly; dropping a live loop relation is not a cleanup
side effect. `block_retirement_metadata_roots` supplies reusable read-only
constraints for batched discovery such as branchless counting. Region-replacing
transforms use `validate_block_retirement` before mutation or staging: the plan
names retired blocks and loop owners, explicitly rewired incoming ordinary
targets, and whether it replaces the function entry. Every incoming ordinary
edge from outside that region must survive or target a declared rewiring,
including edges from unreachable or metadata-only retained blocks. Retired
source edges disappear with their source; labels inside cloned source remain
protected and are not authorized by ordinary-edge rewiring. Unrolling and
fusion share this preflight, rather than discovering incomplete rewiring after
mutating IDs, statistics, operations or loop metadata. Commit validation still
checks that the transform fulfilled its plan.
DCE, SCCP, branchless counting, loop unrolling and generator fusion
share this authority. Retention reachability uses the canonical iterative CFG
traversal with `CfgEdgePolicy::Retention`, which includes non-executable label
custody such as `TryEnd`. `Full` remains executable edges only; preserving a
block for metadata never manufactures an executable predecessor or type proof.

Generator poll-body eligibility is table-owned as a role lattice too:
`generator_fusion_poll_required_yield_opcodes` and
`generator_fusion_poll_reject_opcodes` generate
`opcode_generator_fusion_poll_role_table`, so `generator_fusion.rs` no longer
decides required-yield, reject, or neutral opcodes with a private `StateYield` /
`YieldFrom` / async-state hand match. `generator_fusion_iter_use_roles`
separately feeds `opcode_generator_fusion_iter_use_role_table` for the
IterNext/Is roles in the raw-iterator use scanner; the pass owns operand
position and terminator-use proof.
The registry also owns `state_machine_opcodes` and generates
`opcode_is_state_machine_table`; linear CFG transforms such as the TIR inliner
and module-slot promotion consume that table instead of carrying private
generator/async opcode sets. `lowered_state_machine_body_opcodes` separately
feeds `opcode_is_lowered_state_machine_body_table`, the opcode half of
`TirFunction::has_state_machine` beside the non-opcode `StateDispatch`
terminator check. Raw-i64 LIR arithmetic also uses generated opcode facts:
`overflow_peel_guard_compare_opcodes` and `overflow_peel_body_pure_opcodes`
feed overflow-peel legality predicates, so the dual-loop BigInt continuation
does not carry pass-local guard/body opcode allowlists beside the registry.
`i64_overflow_box_dispatch_opcodes` owns boxed-dispatch overflow custody, while
`i64_checked_overflow_triple_opcodes` owns checked-overflow triple eligibility.
Boxed augmented-assignment dispatch is generated too:
`boxed_runtime_inplace_dispatch_opcodes` feeds
`opcode_uses_boxed_runtime_inplace_dispatch_table`, so LLVM lowering asks the
registry whether a first-class opcode's boxed runtime fallback must call
`molt_inplace_*` and try `__i<op>__` before binary/reflected dunders. The
preserved-Copy `inplace_*` spellings remain string namespace facts carried by
`_original_kind`; this generated predicate owns the first-class OpCode half.
Refcount balance accounting is generated too:
`refcount_balance_inc_opcodes` / `refcount_balance_dec_opcodes` produce
`opcode_refcount_balance_role_table`, so `refcount_elim.rs` consumes a typed
Increment/Decrement/neutral role instead of private IncRef/DecRef hand-sets.
Escape allocation-site tracking is generated as well:
`escape_alloc_site_opcodes` produces `opcode_is_escape_alloc_site_table`, so
`escape_analysis.rs` tracks fresh allocation roots without carrying a private
Alloc/ObjectNewBound/Build*/AllocTask hand-set beside the registry.
Generator poll fusion eligibility is generated as a role table too:
`generator_fusion_poll_required_yield_opcodes` /
`generator_fusion_poll_reject_opcodes` produce
`opcode_generator_fusion_poll_role_table`, so `generator_fusion.rs` tests for
required-yield and rejecting poll opcodes without a private state-machine
hand-set. `generator_fusion_iter_use_roles` produces
`opcode_generator_fusion_iter_use_role_table`, keeping IterNext and optional
None-guard membership out of the iterator-use scanner.

1. **One table** `runtime/molt-ir/src/tir/op_kinds.toml` — rows `(canonical_kind, aliases[], semantics_class, arity, mapper_opcode|"copy", classifier_class ∈ {owned_value, transparent_alias, inert_marker, structural}, may_throw, side_effecting, purity ∈ {pure, pure_may_throw, impure}, backends_required[], runtime_symbol?)`.
2. **One generator** `tools/gen_op_kinds.py` (modeled on `tools/gen_intrinsics.py`) renders `runtime/molt-ir/src/tir/op_kinds_generated.rs` (the `kind_to_opcode` arms, the reverse `opcode_canonical_kind_table` backend spelling authority, the `classify_copy_kind`/`copy_kind_mints_owned_value` arms, generated `ALL_OPCODES`, and the typed effect-oracle arms) AND `src/molt/frontend/lowering/op_kinds_generated.py` (the canonical-spelling constants, raising/skip/binop tables, and pre-serialization frontend effect classes the emitter and midend use).
3. **One sync test** `tests/test_gen_op_kinds.py` (modeled on `tests/test_gen_intrinsics.py`) re-renders in memory and `assert_eq`s against the checked-in generated files → **drift = build/test error**.
4. **The effect oracles hook the same table:** `opcode_may_throw_table`, `opcode_is_side_effecting_table`, and `opcode_effects_table` are generated from the `may_throw`, `side_effecting`, `purity`, and `may_access_arbitrary_heap` columns, then consumed by `effects.rs` with no pass-local opcode lists. Impure opcodes default to arbitrary heap access; pure classes default false, and only positive runtime evidence may mark an impure opcode local. Callback-capable reads (`LoadAttr`, `Index`, dynamic `LEN`/attribute/`isinstance` helpers), module reads, and replacing typed-slot stores retain the coarse floor because callbacks or old-value finalization can mutate unrelated captured state. Only exact-site pristine/boxed-neutral evidence from the shared typed-slot planner discharges the replacing-store destructor path; spelling is not proof. Guarded field get/set operations fail closed because a guard miss can use generic attribute dispatch. The frontend projects the same axis as `FRONTEND_ARBITRARY_HEAP_EFFECT`; it may recover callback freedom only from exact built-in producer provenance, truthiness predicate facts, and the existing exact-list write-alias authority. Annotations and raw type-tag guards admit subclasses and are not callback-absence proof. A new opcode or frontend op-kind **requires** an explicit effect classification (kills bug-class instance #1 — the `matches!`-default-false trap — and the frontend private-set drift class).
5. **Generated facts require real consumers:** the dormant deforestation iterator-fusion lane and its barrier table are retired. Its yielded-element/iterator confusion and unconsumed `fused` tags did not implement a valid backend protocol. Tuple scalarization and the separate generator-fusion pass retain their real execution paths and authorities.
6. **Raw-i64 arithmetic lowering is table-owned too:** `i64_overflow_box_dispatch_opcodes` generates `opcode_requires_i64_overflow_box_dispatch_table`, `i64_checked_overflow_triple_opcodes` generates `opcode_supports_i64_checked_overflow_triple_table`, and `i64_zero_divisor_guard_opcodes` generates `opcode_requires_i64_zero_divisor_guard_table`, keeping overflow custody, checked-triple eligibility, boxed-dispatch retention, and proven-nonzero elimination on generated opcode facts.
7. **The terminal state becomes generated-exhaustive:** the LLVM fail-loud gate and the classifier `_ =>` default survive ONLY as a defense for kinds the table forgot — and the sync test makes "the table forgot" a build failure, so the fail-loud path becomes statically unreachable for any in-table kind (it stays as the runtime backstop, now provably dead for known kinds).

---

### Primitive operation result and effect facts

`molt-ir::tir::op_semantics` owns operand-dependent scalar results and effects
for arithmetic, bitwise, comparison, truth, containment and value-selection
operations. It replaces the predicate-only authority. The generated primitive
effect cases and comparison/warning domains remain in `op_kinds.toml`, shared
with the Python frontend. `TirType::semantic_type` removes storage wrappers;
it proves neither exact builtin provenance nor a physical unboxed carrier.
Boolean arithmetic and arbitrary-size integer arithmetic produce semantic
integers; Boolean bitwise pairs preserve Boolean results, but shifts do not.
Bytes/text concatenation and repetition preserve their sequence family and
retain length/size exceptions. Power results remain value-dependent.

Optimization effects consume only `extract_exact_scalar_map`, not annotations
or subclass-admitting guards. Ordinary type inference may consume result types
without using their effects. Representation projection removes scalar hints
before adding exact producer facts and separately requires integer range or
overflow evidence. Missing facts, malformed operators, callback-capable types,
version-dependent Boolean inversion and warning-sensitive bytes comparisons
retain conservative effects. IR result/effect-pair tests cover boxed and
unboxed forms; pass tests prove exact arithmetic-to-comparison transfer while
keeping annotated parameters out of effect and representation admission.

Unresolved SSA operands remain lattice bottom through governed operator results;
they must not widen to dynamic merely because a producer's block has a larger
ID. Effects remain conservative until convergence. Loop tests permute block
identities and retain exception-edge widening. Frontend callback boundaries use
instance effects, not a list of CALL spellings: operators and finalizing heap
mutations invalidate guards and cached heap reads. Exact builtin producers can
recover native length/index reads; annotations cannot. Mutable-object `TYPE_OF`
is a heap read, and arbitrary attribute access retains callback observability.

Reference-count cancellation preserves zero transitions: only a retain followed
by a release can cancel, across callback- and exception-free intervals. A unique
predecessor block is not proof of a unique entry: the shared exception-edge
authority can expose an earlier transfer from that same block, and a function
entry has an implicit initial arrival. Cross-block pairing rejects both cases
while retaining ordinary straight-line optimization. SCCP compound facts are
immutable tuples/ranges; mutable list/dict/set producers do not seed constants.
Generated operation shapes govern admission before invalid IR can become a
constant or lose its diagnostic.

SCCP call admission must follow executable identity and the production ABI:
`CallMethod` receives a bound callable, not a receiver value, so a method-name
hint cannot authorize direct receiver evaluation. Generic builtin lookup is not
an import of a dotted module name or proof of a fixed constructor. Truthiness
uses the first-class `Bool`/`Not` operations; dedicated `range_new` retains its
explicit three-operand primitive identity. Constant materialization is bounded
before allocation by recursive tuple cost and UTF-8 payload size.

Host evaluation is not target conformance: integer division may fold through
f64 only when both operand conversions are exact. Float power/divmod and host
transcendental calls remain executable until shared target semantics establish
their values and exceptions. Ordered min/max folds preserve the selected
operand's NaN and signed-zero bits. These admission rules prevent compiler
misfolds; they do not claim that the runtime's float-divmod or versioned Unicode
implementations are conformant. Those consumers still require shared semantic
authorities and native/WASM differential execution.

## 6. Remaining dangerous-cell burndown plan

### 6.1 Current order

The unit of work is the complete structural change (per CLAUDE.md). Phase 2 is ONE arc; intermediate commits are allowed only if each is itself a complete, byte-identical piece.

1. **Closed:** mirror current reality into `op_kinds.toml`, generate `op_kinds_generated.rs` + `op_kinds_generated.py`, and route `kind_to_opcode`/classifier/effect/operand-ownership/result-validity facts through generated tables.
2. **Closed:** add `tests/test_gen_op_kinds.py` and keep `audit_op_kinds.py --check` green against `op_kinds_baseline.json`.
3. **Dangerous-cell fixes, each a SEPARATE reviewed commit** (NOT folded into the migration):
   - (a) canonical `floordiv` spelling is closed: `lower_to_simple` emits `floordiv` and the generated mapper accepts `floordiv | floor_div`. Remaining cleanup, if scheduled, is deleting the explicit `floor_div` alias once no serialized or round-trip artifact can produce it.
   - (b) closed: `loop_index_*` is derived from `[[simpleir_control_kind]].pre_ssa_rewritten`, and the LLVM identity arms for `cast`/`widen`/`copy_var` are closed in the current audit.
   - (c) closed: `object_set_class` and `call_async` have dedicated LLVM arms with exact pointer/task ABI lowering. `call_async` remains explicitly non-eligible for the generic runtime fallback; it is covered only by its dedicated task-constructor arm.
   - (d) closed: `classifier_silent_fallthrough` is promoted to **explicit** `classifier_transparent_alias` rows, distinct from `classifier_no_heap_move`, so the `_ =>` default no longer silently buckets known runtime ops.

### 6.2 Key decisions / constraints

- **Canonical spelling = the frontend emission.** The frontend is the producer; `lower_to_simple` is a round-trip that should match it. Collapsing to the frontend spelling minimizes emitter churn and makes the wire vocabulary == the frontend vocabulary.
- **Aliases are first-class table data**, not code. The mapper's `|`-grouped arms (`"copy" | "store_var" | "load_var"`, `"shl" | "lshift"`, `"eq" | "string_eq"`, …) become `aliases[]` columns. This is where the round-trip/legacy spellings live, explicitly.
- **No default anywhere.** Every kind has an explicit `effect`, `classifier_class`, and `mapper_opcode` (or explicit `"copy"`). Rare path-sensitive result facts live in explicit rows such as `[[result_validity]]` (`IterNextUnboxed` result 0 is conditional-valid-only-on-edge). The generated Rust still ends in `_ =>` arms for runtime safety, but the sync test makes them unreachable for in-table kinds.
- **The vector reduction family** has four exact operation rows (`vec_sum`, `vec_prod`, `vec_min`, `vec_max`) in `classifier_owned_value`. Their manifest imports declare compiler-only boxed calls; LLVM uses the shared boxed runtime route, while native and WASM retain their numerical dispatch. A name prefix cannot establish a result ownership contract.
- **RC soundness invariant preserved** (per docs/design/foundation/20): the classifier's fail-closed direction (unknown → TransparentAlias = leak-not-UAF) is retained as the generated `_ =>` backstop; the table makes the *known* set explicit and total.

### 6.3 Anchors phase 2 edits (verified 2026-06-06)

- `src/molt/frontend/lowering/serialization.py:672` (`floordiv` emission), :267 (`copy_var` fusion), :2330 (BOX/UNBOX/CAST/WIDEN).
- `runtime/molt-ir/src/tir/ssa.rs:1902` (`kind_to_opcode` generated-table entry point), `runtime/molt-ir/src/tir/op_kinds_generated.rs:20` (`kind_to_opcode_table`), :29 (`floordiv`/`floor_div` alias arm).
- `runtime/molt-passes/src/tir/lower_to_simple/op_lowering.rs` (`_original_kind` restoration and `OpCode::FloorDiv => "floordiv"` lowering).
- `runtime/molt-ir/src/tir/op_kinds.toml` (`[[simpleir_control_kind]]`) for structural, CFG-boundary, pre-SSA, and SSA-only SimpleIR kinds.
- `runtime/molt-passes/src/tir/lower_from_simple.rs` (`rewrite_loop_index_to_store_load`) for the actual loop-index pre-SSA rewrite implementation.
- `runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops.rs` (`lower_preserved_simpleir_op` dispatcher), `runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops/{op_family,direct_ops,callable_ops,container_ops}.rs` (LLVM preserved-op routing authorities), `runtime/molt-backend-native/src/llvm_backend/lowering/runtime_helpers.rs` (`try_lower_preserved_runtime_call`), and `runtime/molt-backend-native/src/llvm_backend/lowering/op_dispatch.rs` (fail-loud gate).
- `runtime/molt-passes/src/tir/passes/alias_analysis.rs:496` (`copy_kind_mints_owned_value`), :535 (`classify_copy_kind`), :564 (`_ => TransparentAlias`), :645 (`copy_kind_is_explicit_no_heap_move`).
- `runtime/molt-passes/src/tir/passes/effects.rs` (`opcode_may_throw` / `is_side_effecting` / `opcode_effects` delegate to the generated effect oracle).
- `src/molt/frontend/lowering/op_kinds_generated.py` (`FRONTEND_EFFECT_CLASS` and the `FRONTEND_EFFECT_*_KINDS` sets) plus `src/molt/frontend/lowering/midend_canonicalization.py` (`_op_effect_class`) for pre-serialization frontend DCE/CSE/LICM effect authority. `midend_optimization.py` is only the composed MRO entrypoint.
- `runtime/molt-ir/src/tir/mod.rs` (`is_structural`), `runtime/molt-ir/src/tir/cfg.rs` (terminator/leader/ender/cond-branch), and `runtime/molt-passes/src/tir/lower_from_simple.rs` consume generated SimpleIR control-kind tables.
- Precedents: `tools/gen_intrinsics.py` + `tests/test_gen_intrinsics.py` (the generator + sync-test pattern); `tools/stdlib_full_coverage_manifest.py` (the manifest-table pattern); `tools/audit_op_kinds.py` (this task's check-mode tool).

---

## 7. CI seed

`tools/audit_op_kinds.py --check` exits non-zero on both **new** current findings and **stale** baseline-only findings in any category relative to committed `tools/op_kinds_baseline.json`. The comparison records changes in observations as well as bug preconditions. Diagnose the reported category against its source authority before changing mappings, ownership, routing, or the baseline; frontend dormancy alone does not justify adding or removing an opcode.

### 7.1 Current LLVM runtime ABI adjunct gate

`tools/llvm_runtime_abi_audit.py --check` guards the preserved-Copy runtime-call seam. `MOLT_RUNTIME_CALLABLE_SYMBOLS` is availability only. Generic direct and preserved calls consume the target-neutral generated boxed ABI projection from `runtime_boxed_call_specs`; each argument is an object value and the result follows its owned, borrowed, poll, or void contract. The audit uses all extracted serialization handlers and subtracts dedicated LLVM handler routes before requiring generic semantic eligibility. Exact fixed declarations retain their stronger attributes, while residual conservative declarations serve dedicated raw/mixed consumers only. Native declarations remain independently checked against Rust export arity, parameter carriers, and return carriers. Conservative rows must not mirror the generated boxed authority; raw i64 helpers do not become boxed by machine shape.

Fresh owned results require exact canonical operation membership. The four bounded
vector reductions declare fresh results individually in `classifier_owned_value`;
an operation name prefix never admits an ownership contract. Their canonical
manifest imports use `boxed_call = true`, with three object operands and an owned
return derived from the boxed ABI. The runtime packs `(result, last, count, more)`
into that result tuple. This compiler-only
admission does not make the reductions Python callables. LLVM obtains their
symbol, arity, machine declaration and result custody from the generated shared
projection. Its former reduction-specific symbol table, handler and conservative
signature rows are removed. The native and WASM numerical routes keep the same
runtime calls.

The shared LLVM positional emitter borrows each input, releases materialized
argument owners after the call, and binds or releases the owned tuple. The source
audits validate the same boxed contract against the runtime exports; no
reduction-specific exemption remains.

D10 (`owned_result_transparent_alias`) applies to the Copy operation vocabulary:
an operation must be emitted by a serialization handler or accepted by an exact
native routing slice or LLVM preserved-handler slice,
and the canonical mapper must reach Copy (explicitly or through its unmapped
fallback). This includes internally generated preserved operations. A same-named
boxed runtime callable alone does not establish this scope; ordinary Call results
have their own ownership path, so a service such as `platform_system` is not a
Copy-classifier entry. Advisory textual arm scans cannot establish scope either.
Within that scope, D10 reads `runtime_operation_return_specs`, projected from
the canonical import return authority plus op-loop, numeric-selector, and
constant-materializer operation mappings. This includes compiler-only mixed/raw
signatures without inferring ownership from an i64 machine carrier. Owned
runtime results have independent result custody; boxed bool, None, and inline
numbers obey the same protocol with no heap release. Borrowed binding/object
views, unpublished storage, raw bits, and void returns remain distinct contracts.
`TransparentAlias` means non-owning custody and does not itself union the result
with operand zero. Only the separate no-heap-move fact establishes that identity.
A D10 count is not a count of proven heap leaks; unexplained findings must not be
accepted into the baseline.

The explicit Copy custody classes are disjoint, so classifier precedence cannot
hide contradictory owner/non-owner rows. Numeric aliases `binop_floor_div`,
`unary_neg`, and `unary_pos` use their existing first-class opcode families;
`guarded_load` uses `LoadAttr` and preserves its field offset and original kind.
Its LLVM consumer shares the admitted field-load path with `load`. `call_async`
constructs an owned task or an owned async-sleep future. No allocation or escape
fact follows from these result-custody declarations.


### Owned lookup results and allocation facts

`classifier_owned_value` declares an independent result reference that its
consumer must release or transfer. It does not declare a newly allocated
object, disjoint storage, or a fixed callable identity. `builtin_func` covers
both unnamed runtime construction and named public lookup. The public lookup
may return any replacement object from the active namespace; it has dynamic
type, can alias any published object, and cannot inherit its name operand's
string type. Both forms return their own reference obligation.

The generated owned-value fact drives TIR type refinement, result custody,
LLVM's explicit-lowering check, the registry audit, and the binary-image
`owned_value_root` category. Fresh allocation remains a separate explicit
escape/constructor fact. Binary-image `heap_alloc_root` must not be inferred
from result ownership. Lookup effects remain conservative, including custom
mapping callbacks and exceptions.


### Mutable builtin calls are opaque

The `call_builtin` wire operation performs public namespace lookup. A name,
including a constant dynamic-name operand, is not callable identity. TIR does
not derive return types, constant results, container lengths, or leaf status
from that spelling. The old builtin-name result classifier and SCCP evaluator
are removed. A public replacement may return any value, mutate state, raise,
or call back into the program.

Explicit `range_new` and the `len` primitive retain their independent structural
contracts. Range specialization already admits only `range_new`; length-based
bounds elimination admits only the explicit length primitive. Public calls and
namespace mapping acquisition create opaque call-graph edges, preserving
recursion-guard requirements. Call effects remain conservative.


Dedicated module namespace acquisition shares the opaque runtime-builtin call
role: `ModuleGetGlobal` may invoke a captured mapping's item protocol;
`ModuleGetAttr`, `ModuleGetName`, and `ModuleImportFrom` may invoke module
attribute callbacks. Their result syntax must not establish a leaf function or
authorize native recursion-guard elision. `namespace_get`/`namespace_del` already
travel through ordinary opaque calls; no parallel spelling classifier is needed.


Explicit Python release operands have one generated projection in the ownership
module. `PythonLifetimeFacts` records canonical release roots once; lexical drop
placement consumes that set, and point availability consumes the same per-op
projection. `DecRef`, `DeleteVar` old-slot operands, and `DelBoundary` therefore
cannot drift between release placement and lifetime facts. Terminal activation
lowering exposes yield/pending exits as Returns before shared ownership analysis;
there is no separate suspension-retain classification lane.


## Standalone Rust Python values

`molt-ir::python_string::PythonString` owns the dependency-free compact Python
text carrier and its surrogatepass decoder. IR literal admission and standalone
Rust programs use the exact same source; `PYTHON_STRING_SOURCE` embeds it without
copying an algorithm. The sole storage is canonical UTF-8/surrogatepass bytes:
ASCII costs one byte, scalar code points one to four, and each surrogate three.
Adjacent surrogate halves stay separate Python characters. Length and indexing
decode code points without a parallel wide representation; this uses linear scans
and avoids a code-point cache. Equality and ordering use canonical encoded bytes.

All standalone string producers, concatenation, repetition, indexing, iteration,
unpacking, containment, comparison, repr/ascii, metadata, and str conversions use
that carrier. Rust scalar text and stdout encoding are explicit strict edges:
surrogates fail encoding rather than being replaced or reinterpreted. Invalid
UTF-8/surrogatepass byte sequences fail shared literal admission; bytes literals
remain arbitrary byte sequences.

The source backend selects ordinary literals by generated opcode mapping, so
aliases share one materializer. Floating values emit from their IEEE bit pattern,
preserving infinities, NaN payloads and negative zero. Ellipsis and NotImplemented
have distinct variants. NotImplemented truth conversion follows target version
state (a TypeError from Python 3.14). Owned and transparent copy transport share
the backend value-copy primitive; independent identity, heap mutation, finalizer,
and control-flow capabilities are unchanged. Canonical integer aliases inherit
their numeric admission role, including load_const.

This establishes carrier and lowering structure, not a universal Python string
or runtime support claim. Runtime capabilities still gate protocols and unknown
wire operations still fail closed. Unicode-category-exact repr printability and
the complete warning protocol are not newly claimed by the source target.


## Frontend and executable wire vocabularies

Frontend optimizer effect tokens belong to the pre-serialization IR; they do not
register runtime operations. Executable source-target admission derives its wire
vocabulary only from mapper, control, runtime-role and explicit neutral wire facts.
Generation rejects overlap between those facts and the frontend effect authority.

The serializer validates its final output against the generated frontend semantic
authority after scalarization and fusion, rejecting tokens that escaped lowering.
It preserves wire names such as store_var and load_var instead of collapsing their
binding field roles into Copy. Preserved runtime wire operations keep their release
backend consumers. Unknown wire names remain the target admission obligation; this
boundary creates no target support list and uses no case-conversion heuristic.

Value transport keeps operand shape separate from ownership transparency.
`guard_tag` and `guard_type` read both the guarded value and a dynamic expected
tag; their declared two-operand shapes survive serialization, source validation,
SSA and preserved-Copy verification. Their result aliases operand zero without
retaining it, but that ownership fact does not erase the tag read. Ordinary copy
and owned-alias transports still require one semantic read through their declared
field roles. The distinct `type_guard` TIR refinement remains unary. Missing,
extra, conflicting and undefined inputs remain invalid before backend lowering.

Runtime guard identity and execution effects remain separate. The shared TIR
`value_identity::no_heap_alias_source` validates the declared semantic read
count, then projects only operand zero into alias union, ownership, and container
provenance. Pure-copy emission uses `copy_value_source`; a no-heap ownership fact
never authorizes erasing a check. Both runtime guard spellings retain both reads
and bind an optional result to the original value and its existing scalar carrier.
`molt_guard_type` converts the expected tag with runtime `to_i64`; a rejected tag
raises, while a type mismatch counts profiling feedback and returns the source
unchanged. It proves no source type. The unary TIR `TypeGuard` remains a distinct
refinement operation. Native borrowed operands use the shared transaction, which
skips dependent calls on failed wide-integer boxing and releases temporary owners;
result aliases obey the same carrier and retain rules as other aliases.

SimpleIR guard elimination compares an exact constant expected tag with a proven
runtime type from unique dominating definitions. Dynamic tags, annotations,
carrier classes, generic arithmetic and container indexing do not satisfy a
check. A discharged guard with an output becomes an identity alias. Split-field
deforestation consumes the same proof and keeps guards whose outputs still need
the materialized object. Positive tag admission is a separate shared fact: a
unique dominating i64 literal tag lets native, LLVM, generic WASM and LIR-fast
execute the guard call only when a per-activation runtime profile flag is set.
The flag is stable within a runtime epoch and is never inferred from the compiler's
environment. Every executed mismatch remains counted when profiling is enabled;
frontend CFG deduplication must not collapse repeated runtime guards. Nonthrowing
analysis additionally proves that physical operand boxing cannot allocate or
retains its exception observation. Dynamic tags keep runtime admission, including
bool, integral direct-float and bounded integer carriers accepted by `to_i64`.
Frontend SCCP continues past mismatch and never overwrites source-type facts with
the requested tag.

Frontend serialization preserves a named runtime-guard result for either
spelling. Its canonicalization only erases a proven check when no result is
bound. JSON string-split scalarization treats an undischarged guard as an
observable use instead of maintaining a second tag-blind guard deletion path.
