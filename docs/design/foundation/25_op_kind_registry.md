<!-- Design doc (task #57). Audit anchors refreshed against the current worktree 2026-06-13. The op-kind registry source is live; this doc tracks the remaining dangerous-cell burndown. -->

# Op-Kind Single-Source-of-Truth Registry

**Status:** Registry source + generated sync are live; dangerous-cell burndown remains active. The machine-generated enumeration lives in `tools/audit_op_kinds.py` + `tools/op_kinds_baseline.json`.

**Bug class killed:** cross-component op-"kind"-string drift — molt's most prolific silent-miscompile family (5 proven instances; see "Motivation").

---

## 1. Motivation — the bug class (5 proven instances)

A `MoltOp` produced by the frontend visitors is serialized to a JSON op whose `"kind"` string is the **wire contract** between the Python frontend and the Rust backend. Five independent components must agree on that vocabulary, and each keeps its **own private copy** of the table:

1. **Frontend emitter** — `src/molt/frontend/lowering/serialization.py`, the giant `map_ops_to_json` if/elif chain (line 396). Emits the JSON `"kind"` string (lowercase). **This is the authoritative wire vocabulary** (see §3).
2. **TIR SSA mapper** — `kind_to_opcode` in `runtime/molt-tir/src/tir/ssa.rs:1902`, backed by `op_kinds_generated.rs:20`. Maps a kind string → `OpCode`. Unknown kinds deliberately fall back to `OpCode::Copy`, stashing the spelling in `_original_kind`, as the runtime backstop behind the generated registry.
3. **LLVM lowering** — `lower_preserved_simpleir_op` (`runtime/molt-backend/src/llvm_backend/lowering.rs:6837`) dedicated arms + the ABI-exact `molt_<kind>` runtime fallback `try_lower_preserved_runtime_call` (lowering.rs:10426), guarded by a **terminal fail-loud** state (lowering.rs:2410-2502).
4. **RC/alias classifier** — `classify_copy_kind` / `copy_kind_mints_fresh_owned_ref` / `copy_kind_is_explicit_no_heap_move` in `runtime/molt-tir/src/tir/passes/alias_analysis.rs:535/496/645`. **Its `_ => CopyLowering::TransparentAlias` default (alias_analysis.rs:564) is the UAF-escalation precondition.**
5. **Native + WASM SimpleIR dispatch** — `function_compiler.rs` / `wasm.rs`, reached via the `lower_to_simple` `_original_kind` restoration (`runtime/molt-tir/src/tir/lower_to_simple.rs:1547`).

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

- **Frontend (Python):** `ast`-based. 416 constant `"kind": "literal"` dict-value literals + 4 computed sites resolved structurally:
  - `serialization.py:635` `op.kind.lower()` under `op.kind in ("ADD","SUB","MUL")` → `{add,sub,mul}`.
  - `serialization.py:647` `op.kind.lower()` under `("INPLACE_ADD","INPLACE_SUB","INPLACE_MUL")` → `{inplace_add,inplace_sub,inplace_mul}`.
  - `serialization.py:2419` `{"BOX":"box","UNBOX":"unbox","CAST":"cast","WIDEN":"widen"}[op.kind]` → `{box,unbox,cast,widen}`.
  - `serialization.py:4329` bare `op.kind` under `("gpu_thread_id",…,"gpu_barrier")` → the 5 gpu kinds.
  Resolution walks the AST parent chain to the enclosing `if op.kind == …`/`in (…)` guard, then interprets the local assignment (`.lower()` transform or dict-subscript). **An unresolved computed site is a hard error** (the extractor cannot prove the wire vocabulary).
  **Total: 431 emitted JSON kinds** (416 literals + 15 spellings from the 4 computed sites).
- **Rust `match` arms** (`kind_to_opcode`, `lower_preserved_simpleir_op`, `classify_copy_kind`): a line-anchored brace/comment-aware state machine. It locates `fn NAME`, finds `match X {`, brace-matches the body, then collects the string literals of every **top-level** arm pattern (left of `=>`), skipping `//`+`/* */` comments and `"strings"`, and skipping each arm body whether `{}`-block or comma-terminated. **Validated against floordiv/floor_div/matmul + `index` (a `{}`-block arm following another `{}`-block arm).** Failure modes (each absent in the parsed functions, asserted/documented): a `=>` inside a pattern literal (impossible — kinds are identifiers); raw strings `r"…"` in a pattern (asserted absent via `(?<![A-Za-z0-9_])r#*"`); macro-generated arms (none); nested `match` in a body (handled by the balanced-brace skip).
- **`matches!(…)` arms** (`copy_kind_mints_fresh_owned_ref`, `copy_kind_is_inert_marker`, `copy_kind_is_explicit_transparent_alias`, `copy_kind_is_explicit_no_heap_move`): balanced-paren extraction of the macro body's literals + `.starts_with("PREFIX")` prefix rules.
- **LLVM `VEC_REDUCTION_OPS`** (lowering.rs:415): the 24-entry `(kind, arity)` table — real LLVM coverage the arm-extractor misses because `vec_reduction_runtime_symbol(kind)` runs **before** the `match`.
- **Runtime extern ABI surface:** all `pub (unsafe)? extern "C" fn molt_*` across `runtime/molt-runtime/src` (3531 symbols). LLVM fallback coverage is counted only when the parsed ABI is one the fallback can emit: boxed integer parameters plus boxed integer return for the generic path, or an explicit void-return entry in `PRESERVED_VOID_RUNTIME_OPS` whose table arity exactly matches boxed extern parameters.
- **Structural/pre-SSA consumed kinds** (not routed through `kind_to_opcode`): **derived** from the union of `is_structural` (tir/mod.rs:48), `is_terminator`/`is_block_leader`/`is_block_ender`/`is_conditional_branch` (tir/cfg.rs), `PRE_SSA_REWRITTEN_KINDS` (lower_from_simple.rs), + `{phi}`. (Drift-proof: a new structural kind in those authorities auto-updates the audit.)
- **Native/WASM SimpleIR arm presence** (advisory): a textual scan for arm-shaped `"a" | "b" … =>` tokens (every OR-alternative captured). **Advisory only** — it can over-/under-count (guards, bindings, unrelated helper arms); never a sole basis for a disposition.

### Source table sizes (current worktree, 2026-06-13)

| table | size |
|---|---|
| frontend emitted JSON kinds | **431** (416 const literals + 4 computed sites resolving to 15 spellings) |
| `ssa.rs kind_to_opcode` arms | 150 |
| LLVM `lower_preserved_simpleir_op` dedicated arms | 153 |
| LLVM `VEC_REDUCTION_OPS` table | 24 |
| classifier FreshValue allow-list | 48 (+ `vec_*` prefix) |
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

The audit categorizes by the **precise bug preconditions** (not the coarse "emitted-but-unmapped", which is BY DESIGN — the architecture deliberately lifts most value/effect ops to `Copy{_original_kind}` and restores/re-symbol-dispatches them).

| category | count | meaning |
|---|---|---|
| `llvm_coverage_gap` | **0** | emitted + unmapped + NOT llvm-covered (no arm, not in vec table, no ABI-exact runtime fallback) → **LLVM build-fails loud** (fail-loud guard). **EMPTY.** |
| `freshvalue_llvm_gap` | **0** | FreshValue + not llvm-covered → the UAF/double-free precondition. **EMPTY = the LLVM fatal contract holds.** |
| `classifier_silent_fallthrough` | **0** | emitted + unmapped + classifier fell to `_ => TransparentAlias` (no explicit class) + is a real runtime op (`molt_<kind>` exists). **EMPTY = known transparent-alias decisions are table-visible.** |
| `simpleir_lane_gap` | **0** | emitted + unmapped + no native AND no wasm arm AND no symbol → nothing can lower it on the SimpleIR lanes. **EMPTY.** |
| `mapped_never_emitted` | **45** | a mapper arm the frontend never emits — mostly round-trip or explicit alias spellings (benign); `floor_div` is now an explicit alias of canonical `floordiv`. |
| `freshvalue_never_emitted` | **0** | dead FreshValue allow-list entry. **EMPTY.** |
| `llvm_void_runtime_abi_mismatch` | **0** | explicit `PRESERVED_VOID_RUNTIME_OPS` entry without a matching boxed-parameter, void-return extern of the same arity. **EMPTY = the void fallback table is ABI-clean.** |

### 4.1 Disposition of every dangerous category

**`freshvalue_llvm_gap = 0` and `simpleir_lane_gap = 0` are the headline:** on current main there is **NO silent miscompile and NO UAF from kind drift.** The original floordiv-class *silent* miscompile (operand-0 passthrough on LLVM) was already closed by a dedicated LLVM `"floordiv"` arm (lowering.rs:10325) and the universal LLVM fail-loud gate (lowering.rs:2410). Every remaining gap is either fail-loud (a build error) or leak-safe (a non-UAF reference leak).

**`llvm_coverage_gap` (26) — LATENT, fail-loud.** All 26 have native+wasm coverage; they fail-loud on LLVM only. Breakdown:
- **18 async/concurrency runtime ops** (`block_on`, `spawn`, `call_async`, `cancel_token_*` (8), `cancelled`, `cancel_current`, `chan_drop`, `future_cancel{,_clear,_msg}`, `promise_set_{result,exception}`, `task_register_token_owned`, `thread_submit`). These have runtime functions under **different spellings** (e.g. `spawn`→`molt_thread_spawn`, not `molt_spawn`), so the LLVM `molt_<kind>` probe misses them. *Disposition: latent LLVM gap* — the asyncio runtime surface is less mature on the LLVM lane; an async-heavy program targeting LLVM would hit a build error (not a miscompile). Repro sketch: `asyncio.run(main())` with a `create_task`/cancel path, `--target llvm`.
- **3 repr-identity ops** (`cast`, `widen`, `copy_var`). On NaN-boxed values these are **identities**; native/wasm lower them as operand-0 passthrough (`"box"|"unbox"|"cast"|"widen" => op` at function_compiler.rs:1490; wasm.rs:12511). On LLVM they carry `_original_kind` (set), so they hit the fatal gate. *Disposition: latent LLVM gap with a trivial fix* — add an identity arm to `lower_preserved_simpleir_op` returning operand 0. `copy_var` is emitted by the string-split-field fusion (`serialization.py:267`), so the trigger is narrow. Repro sketch: a program whose only `.split()[i]` consumers fuse, `--target llvm`.
- **2 loop-IV ops** (`loop_index_start`, `loop_index_next`). Consumed specially by `lower_from_simple.rs:201/278` (folded into a counted-loop IV) — they should never reach `kind_to_opcode`'s Copy fallback on the lift. *Disposition: benign* (structural-IV machinery; the audit flags them only because they are not in the CFG leader/terminator helpers — they could be added to the derived structural set in phase 2).
- **1 other** (`object_set_class`). It has native+wasm coverage, and shares `class_apply_set_name`'s native arm, but no LLVM arm. *Disposition: latent LLVM gap* — `obj.__class__ = C` on the LLVM lane fails loud. Repro sketch: `obj.__class__ = C`, `--target llvm`.

Closed in the current audit: the repr-identity ops (`cast`, `widen`, `copy_var`) now have explicit LLVM identity arms that bind result values to operand 0, matching the native/WASM NaN-box passthrough contract without weakening the terminal fail-loud guard. The loop-IV helpers (`loop_index_start`, `loop_index_next`) are also closed as LLVM-gap false positives: `lower_from_simple.rs` exposes them through `PRE_SSA_REWRITTEN_KINDS`, and the audit derives that pre-SSA consumed set directly from the lowering authority. Runtime fallback coverage now derives from parsed extern ABI, including `unsafe extern "C"` exports; boxed async/cancellation/channel/thread ops are covered only when their ABI is boxed-integer compatible, and void-return side-effect ops (`print_newline`, `spawn`) are covered through the explicit `PRESERVED_VOID_RUNTIME_OPS` table only when table arity and boxed extern parameters match. The pointer-ABI ops `object_set_class` and `guarded_field_init` are closed by dedicated LLVM arms that unbox the receiver pointer and call the exact runtime symbols (`molt_object_set_class`, `molt_guarded_field_init_ptr`) rather than widening the generic boxed fallback. `call_async` is closed by reusing the LLVM task-frame allocation authority already used by `AllocTask`, plus the native-compatible `molt_async_sleep` constructor special case.

**`llvm_void_runtime_abi_mismatch = 0` - CLOSED.** The `PRESERVED_VOID_RUNTIME_OPS` table is audited as source data, not consumed opportunistically. A missing extern, non-void return, arity mismatch, or non-boxed parameter becomes a dangerous-cell finding even before the frontend emits that kind.

**`classifier_silent_fallthrough = 0` — CLOSED.** The 207 table-visible transparent-alias decisions now live in `classifier_transparent_alias`, a generated table distinct from `classifier_no_heap_move`. This preserves the same leak-safe drop-insertion behavior (`TransparentAlias`, never `FreshValue`) while making each known decision explicit: a future ownership promotion must move the kind out of the transparent-alias table and into `classifier_fresh_value` with matching backend evidence, rather than hiding behind the `_ => TransparentAlias` default.

**`mapped_never_emitted` (45) — mostly BENIGN round-trip or explicit-alias vocabulary.** The module phase re-lifts post-pipeline SimpleIR on every build (the "CheckedAdd round-trip" comment at ssa.rs:1871), so `kind_to_opcode` MUST recognize generated round-trip spellings even when the *frontend* never emits them. Verified round-trip outputs (benign): `build_list`, `get_attr`, `set_attr`, `for_iter`, `yield`, `yield_from`, `checked_add`, `exception_pending`, `iter_next_unboxed`, … The prior `floordiv`/`floor_div` schism is closed in the live registry: canonical spelling is frontend `floordiv`, `floor_div` remains a table-visible alias, and `lower_to_simple` emits `floordiv` so round-trip output no longer recreates the old split. The remaining entries are alias arms such as `load_attr`/`store_attr`/`get_iter`/`const_int`/`call_function` plus generated round-trip vocabulary; they are benign as long as the alias set stays explicit and generated.

---

## 5. Phase-2 mechanism (the recommendation, 5 lines)

Current schema note: `op_kinds.toml` now also owns `result_arity`
(`zero`, `one`, `two`, or `variable`) and generates
`opcode_fixed_result_count_table`, so TIR verification consumes the registry
instead of maintaining a parallel opcode-to-result-count match. The generator
rejects `variable` unless the opcode is on the audited context-dependent
whitelist, so fixed-result opcodes cannot quietly escape verifier coverage.
The registry also owns `state_machine_opcodes` and generates
`opcode_is_state_machine_table`; linear CFG transforms such as the TIR inliner
and module-slot promotion consume that table instead of carrying private
generator/async opcode sets. `lowered_state_machine_body_opcodes` separately
feeds `opcode_is_lowered_state_machine_body_table`, the opcode half of
`TirFunction::has_state_machine` beside the non-opcode `StateDispatch`
terminator check.

1. **One table** `runtime/molt-tir/src/tir/op_kinds.toml` — rows `(canonical_kind, aliases[], semantics_class, arity, mapper_opcode|"copy", classifier_class ∈ {fresh_value, transparent_alias, inert_marker, structural}, effect ∈ {pure, observe, throw, side_effect}, backends_required[], runtime_symbol?)`.
2. **One generator** `tools/gen_op_kinds.py` (modeled on `tools/gen_intrinsics.py`) renders `runtime/molt-tir/src/tir/op_kinds_generated.rs` (the `kind_to_opcode` arms, the `classify_copy_kind`/`copy_kind_mints_fresh_owned_ref` arms, the effect-oracle arms) AND `src/molt/frontend/lowering/op_kinds_generated.py` (the canonical-spelling constants the emitter uses).
3. **One sync test** `tests/test_gen_op_kinds.py` (modeled on `tests/test_gen_intrinsics.py`) re-renders in memory and `assert_eq`s against the checked-in generated files → **drift = build/test error**.
4. **The effect oracle hooks the same table:** `opcode_may_throw`/`is_side_effecting` (effects.rs) are generated from the `effect` column → a new kind **requires** an explicit effect classification (kills bug-class instance #1 — the `matches!`-default-false trap — because the table has no default; every kind has an explicit `effect`).
5. **Deforestation fusion eligibility is table-owned too:** `fusion_barrier_opcodes` generates `opcode_is_fusion_barrier_table` for `deforestation.rs`. This is deliberately separate from side effects/may-throw because iterator-chain fusion preserves per-element evaluation order while still rejecting cross-iteration/control-state barriers.
6. **Raw-i64 zero-divisor guard eligibility is table-owned too:** `i64_zero_divisor_guard_opcodes` generates `opcode_requires_i64_zero_divisor_guard_table` for both LIR lowering and `check_exception_elim`, keeping boxed-dispatch retention and proven-nonzero elimination on one opcode fact.
7. **The terminal state becomes generated-exhaustive:** the LLVM fail-loud gate and the classifier `_ =>` default survive ONLY as a defense for kinds the table forgot — and the sync test makes "the table forgot" a build failure, so the fail-loud path becomes statically unreachable for any in-table kind (it stays as the runtime backstop, now provably dead for known kinds).

---

## 6. Remaining dangerous-cell burndown plan

### 6.1 Current order

The unit of work is the complete structural change (per CLAUDE.md). Phase 2 is ONE arc; intermediate commits are allowed only if each is itself a complete, byte-identical piece.

1. **Closed:** mirror current reality into `op_kinds.toml`, generate `op_kinds_generated.rs` + `op_kinds_generated.py`, and route `kind_to_opcode`/classifier/effect/operand-ownership/result-validity facts through generated tables.
2. **Closed:** add `tests/test_gen_op_kinds.py` and keep `audit_op_kinds.py --check` green against `op_kinds_baseline.json`.
3. **Dangerous-cell fixes, each a SEPARATE reviewed commit** (NOT folded into the migration):
   - (a) canonical `floordiv` spelling is closed: `lower_to_simple` emits `floordiv` and the generated mapper accepts `floordiv | floor_div`. Remaining cleanup, if scheduled, is deleting the explicit `floor_div` alias once no serialized or round-trip artifact can produce it.
   - (b) closed: `loop_index_*` is derived from `PRE_SSA_REWRITTEN_KINDS` in `lower_from_simple.rs`, and the LLVM identity arms for `cast`/`widen`/`copy_var` are closed in the current audit.
   - (c) closed: `guarded_field_init`, `object_set_class`, and `call_async` have dedicated LLVM arms with exact pointer/task ABI lowering. `call_async` remains explicitly non-eligible for the generic runtime fallback; it is covered only by its dedicated task-constructor arm.
   - (d) closed: `classifier_silent_fallthrough` is promoted to **explicit** `classifier_transparent_alias` rows, distinct from `classifier_no_heap_move`, so the `_ =>` default no longer silently buckets known runtime ops.

### 6.2 Key decisions / constraints

- **Canonical spelling = the frontend emission.** The frontend is the producer; `lower_to_simple` is a round-trip that should match it. Collapsing to the frontend spelling minimizes emitter churn and makes the wire vocabulary == the frontend vocabulary.
- **Aliases are first-class table data**, not code. The mapper's `|`-grouped arms (`"copy" | "store_var" | "load_var"`, `"shl" | "lshift"`, `"eq" | "string_eq"`, …) become `aliases[]` columns. This is where the round-trip/legacy spellings live, explicitly.
- **No default anywhere.** Every kind has an explicit `effect`, `classifier_class`, and `mapper_opcode` (or explicit `"copy"`). Rare path-sensitive result facts live in explicit rows such as `[[result_validity]]` (`IterNextUnboxed` result 0 is conditional-valid-only-on-edge). The generated Rust still ends in `_ =>` arms for runtime safety, but the sync test makes them unreachable for in-table kinds.
- **The `vec_*` family** stays a generated prefix expansion (the 24 `VEC_REDUCTION_OPS` rows + the classifier `vec_` prefix) — encode the prefix rule in the table, generate the explicit table for LLVM + the prefix check for the classifier.
- **RC soundness invariant preserved** (per docs/design/foundation/20): the classifier's fail-closed direction (unknown → TransparentAlias = leak-not-UAF) is retained as the generated `_ =>` backstop; the table makes the *known* set explicit and total.

### 6.3 Anchors phase 2 edits (verified 2026-06-06)

- `src/molt/frontend/lowering/serialization.py:672` (`floordiv` emission), :267 (`copy_var` fusion), :2330 (BOX/UNBOX/CAST/WIDEN).
- `runtime/molt-tir/src/tir/ssa.rs:1902` (`kind_to_opcode` generated-table entry point), `runtime/molt-tir/src/tir/op_kinds_generated.rs:20` (`kind_to_opcode_table`), :29 (`floordiv`/`floor_div` alias arm).
- `runtime/molt-tir/src/tir/lower_to_simple.rs:1547` (`_original_kind` restoration), :1644 (`OpCode::FloorDiv => "floordiv"`).
- `runtime/molt-tir/src/tir/lower_from_simple.rs` (`PRE_SSA_REWRITTEN_KINDS`, `rewrite_loop_index_to_store_load`) for loop-index pre-SSA consumption.
- `runtime/molt-backend/src/llvm_backend/lowering.rs:6837` (`lower_preserved_simpleir_op`), :10426 (`try_lower_preserved_runtime_call`), :2410-2502 (fail-loud gate), :415 (`VEC_REDUCTION_OPS`), :10325 (`floordiv` arm).
- `runtime/molt-tir/src/tir/passes/alias_analysis.rs:496` (`copy_kind_mints_fresh_owned_ref`), :535 (`classify_copy_kind`), :564 (`_ => TransparentAlias`), :645 (`copy_kind_is_explicit_no_heap_move`).
- `runtime/molt-tir/src/tir/passes/effects.rs` (`opcode_may_throw` / `is_side_effecting` — the effect oracle to hook).
- `runtime/molt-tir/src/tir/mod.rs:48` (`is_structural`), `runtime/molt-tir/src/tir/cfg.rs:59/68/77/83` (terminator/leader/ender/cond-branch).
- Precedents: `tools/gen_intrinsics.py` + `tests/test_gen_intrinsics.py` (the generator + sync-test pattern); `tools/stdlib_full_coverage_manifest.py` (the manifest-table pattern); `tools/audit_op_kinds.py` (this task's check-mode tool).

---

## 7. CI seed

`tools/audit_op_kinds.py --check` exits non-zero on any **new** member of any dangerous-cell category vs `tools/op_kinds_baseline.json` (committed). It is wire-ready; the CI wiring lands in phase 2 step 2 (alongside the `gen_op_kinds.py` sync test). The baseline is the contract: a new emitted kind that drifts (no mapper arm + no coverage, or a silent classifier fallthrough) becomes a build error until it gets a table row.

### 7.1 Current LLVM runtime ABI adjunct gate

`tools/llvm_runtime_abi_audit.py --check` guards the preserved-Copy runtime-call ABI seam that the op-kind audit only identifies by symbol coverage. `MOLT_RUNTIME_INTRINSIC_SYMBOLS` is availability only; LLVM call signatures come from `CLASSIFIED_RUNTIME_IMPORTS` until a generated ABI table replaces it. The gate derives emitted-but-unmapped frontend kinds, intersects them with `pub extern "C" fn molt_*` exports, requires boxed/i64 or void ABI facts, verifies every classified fact against the Rust export arity and return ABI, rejects duplicate facts, and keeps non-boxed returns fail-closed (`molt_chan_new` / `ChanHandle` today).
