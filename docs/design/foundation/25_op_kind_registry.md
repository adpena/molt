<!-- Design doc (task #57, phase 1 — enumeration audit + implementation design). All anchors verified against origin/main worktree 2026-06-06. No implementation landed; phase 2 is the Rust-side landing under a build slot. -->

# Op-Kind Single-Source-of-Truth Registry

**Status:** Design doc — phase 1 (enumeration audit + design) complete; phase 2 (implementation) pending a build slot. The machine-generated enumeration lives in `tools/audit_op_kinds.py` + `tools/op_kinds_baseline.json`. This doc is phase 2's spec.

**Bug class killed:** cross-component op-"kind"-string drift — molt's most prolific silent-miscompile family (5 proven instances; see "Motivation").

---

## 1. Motivation — the bug class (5 proven instances)

A `MoltOp` produced by the frontend visitors is serialized to a JSON op whose `"kind"` string is the **wire contract** between the Python frontend and the Rust backend. Five independent components must agree on that vocabulary, and each keeps its **own private copy** of the table:

1. **Frontend emitter** — `src/molt/frontend/lowering/serialization.py`, the giant `map_ops_to_json` if/elif chain (line 396). Emits the JSON `"kind"` string (lowercase). **This is the authoritative wire vocabulary** (see §3).
2. **TIR SSA mapper** — `kind_to_opcode` in `runtime/molt-backend/src/tir/ssa.rs:1789`. Maps a kind string → `OpCode`. **Its `_ => OpCode::Copy` arm (ssa.rs:1923) silently lifts any unrecognized kind to `Copy`, stashing the spelling in `_original_kind`.**
3. **LLVM lowering** — `lower_preserved_simpleir_op` (`runtime/molt-backend/src/llvm_backend/lowering.rs:6472`) dedicated arms + the generic `molt_<kind>` by-symbol fallback `try_lower_preserved_runtime_call` (lowering.rs:9855), guarded by a **terminal fail-loud** state (lowering.rs:2348-2424).
4. **RC/alias classifier** — `classify_copy_kind` / `copy_kind_mints_fresh_owned_ref` / `copy_kind_is_explicit_no_heap_move` in `runtime/molt-backend/src/tir/passes/alias_analysis.rs:426/360/487`. **Its `_ => CopyLowering::TransparentAlias` default (alias_analysis.rs:456) is the UAF-escalation precondition.**
5. **Native + WASM SimpleIR dispatch** — `function_compiler.rs` / `wasm.rs`, reached via the `lower_to_simple` `_original_kind` restoration (`runtime/molt-backend/src/tir/lower_to_simple.rs:1676`).

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

- **Frontend (Python):** `ast`-based. 405 constant `"kind": "literal"` dict-value literals + 4 computed sites resolved structurally:
  - `serialization.py:599` `op.kind.lower()` under `op.kind in ("ADD","SUB","MUL")` → `{add,sub,mul}`.
  - `serialization.py:611` `op.kind.lower()` under `("INPLACE_ADD","INPLACE_SUB","INPLACE_MUL")` → `{inplace_add,inplace_sub,inplace_mul}`.
  - `serialization.py:2330` `{"BOX":"box","UNBOX":"unbox","CAST":"cast","WIDEN":"widen"}[op.kind]` → `{box,unbox,cast,widen}`.
  - `serialization.py:4226` bare `op.kind` under `("gpu_thread_id",…,"gpu_barrier")` → the 5 gpu kinds.
  Resolution walks the AST parent chain to the enclosing `if op.kind == …`/`in (…)` guard, then interprets the local assignment (`.lower()` transform or dict-subscript). **An unresolved computed site is a hard error** (the extractor cannot prove the wire vocabulary).
  **Total: 420 emitted JSON kinds.**
- **Rust `match` arms** (`kind_to_opcode`, `lower_preserved_simpleir_op`, `classify_copy_kind`): a line-anchored brace/comment-aware state machine. It locates `fn NAME`, finds `match X {`, brace-matches the body, then collects the string literals of every **top-level** arm pattern (left of `=>`), skipping `//`+`/* */` comments and `"strings"`, and skipping each arm body whether `{}`-block or comma-terminated. **Validated against floordiv/floor_div/matmul + `index` (a `{}`-block arm following another `{}`-block arm).** Failure modes (each absent in the parsed functions, asserted/documented): a `=>` inside a pattern literal (impossible — kinds are identifiers); raw strings `r"…"` in a pattern (asserted absent via `(?<![A-Za-z0-9_])r#*"`); macro-generated arms (none); nested `match` in a body (handled by the balanced-brace skip).
- **`matches!(…)` arms** (`copy_kind_mints_fresh_owned_ref`, `copy_kind_is_explicit_no_heap_move`): balanced-paren extraction of the macro body's literals + `.starts_with("PREFIX")` prefix rules.
- **LLVM `VEC_REDUCTION_OPS`** (lowering.rs:411): the 24-entry `(kind, arity)` table — real LLVM coverage the arm-extractor misses because `vec_reduction_runtime_symbol(kind)` runs **before** the `match`.
- **Runtime symbol surface:** all `pub extern "C" fn molt_*` across `runtime/molt-runtime/src` (3254 symbols) — the surface the LLVM generic `molt_<kind>` fallback probes.
- **Structural kinds** (CFG/SSA-consumed, not routed through `kind_to_opcode`): **derived** from the union of `is_structural` (tir/mod.rs:48), `is_terminator`/`is_block_leader`/`is_block_ender`/`is_conditional_branch` (tir/cfg.rs) + `{phi}`. (Drift-proof: a new structural kind in those functions auto-updates the audit.)
- **Native/WASM SimpleIR arm presence** (advisory): a textual scan for arm-shaped `"a" | "b" … =>` tokens (every OR-alternative captured). **Advisory only** — it can over-/under-count (guards, bindings, unrelated helper arms); never a sole basis for a disposition.

### Source table sizes (origin/main, 2026-06-06)

| table | size |
|---|---|
| frontend emitted JSON kinds | **420** (405 const + 4 computed sites) |
| `ssa.rs kind_to_opcode` arms | 146 |
| LLVM `lower_preserved_simpleir_op` dedicated arms | 141 |
| LLVM `VEC_REDUCTION_OPS` table | 24 |
| classifier FreshValue allow-list | 28 (+ `vec_*` prefix) |
| classifier InertMarker arms | 13 |
| classifier no-heap-move (alias) set | 7 |
| structural (CFG/SSA-consumed) kinds | 20 |
| runtime `molt_*` exports | 3254 |

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
| `llvm_coverage_gap` | **28** | emitted + unmapped + NOT llvm-covered (no arm, not in vec table, no `molt_<kind>` symbol) → **LLVM build-fails loud** (fail-loud guard). |
| `freshvalue_llvm_gap` | **0** | FreshValue + not llvm-covered → the UAF/double-free precondition. **EMPTY = the LLVM fatal contract holds.** |
| `classifier_silent_fallthrough` | **196** | emitted + unmapped + classifier fell to `_ => TransparentAlias` (no explicit class) + is a real runtime op (`molt_<kind>` exists) → **leak-safe today, promotion-hazard latent.** |
| `simpleir_lane_gap` | **0** | emitted + unmapped + no native AND no wasm arm AND no symbol → nothing can lower it on the SimpleIR lanes. **EMPTY.** |
| `mapped_never_emitted` | **45** | a mapper arm the frontend never emits — mostly round-trip spellings (benign), one genuine schism (`floor_div`). |
| `freshvalue_never_emitted` | **0** | dead FreshValue allow-list entry. **EMPTY.** |

### 4.1 Disposition of every dangerous category

**`freshvalue_llvm_gap = 0` and `simpleir_lane_gap = 0` are the headline:** on current main there is **NO silent miscompile and NO UAF from kind drift.** The original floordiv-class *silent* miscompile (operand-0 passthrough on LLVM) was already closed by a dedicated LLVM `"floordiv"` arm (lowering.rs:9789) and the universal LLVM fail-loud gate (lowering.rs:2388). Every remaining gap is either fail-loud (a build error) or leak-safe (a non-UAF reference leak).

**`llvm_coverage_gap` (28) — LATENT, fail-loud.** All 28 have native+wasm coverage; they fail-loud on LLVM only. Breakdown:
- **18 async/concurrency runtime ops** (`block_on`, `spawn`, `call_async`, `cancel_token_*` (8), `cancelled`, `cancel_current`, `chan_drop`, `future_cancel{,_clear,_msg}`, `promise_set_{result,exception}`, `task_register_token_owned`, `thread_submit`). These have runtime functions under **different spellings** (e.g. `spawn`→`molt_thread_spawn`, not `molt_spawn`), so the LLVM `molt_<kind>` probe misses them. *Disposition: latent LLVM gap* — the asyncio runtime surface is less mature on the LLVM lane; an async-heavy program targeting LLVM would hit a build error (not a miscompile). Repro sketch: `asyncio.run(main())` with a `create_task`/cancel path, `--target llvm`.
- **3 repr-identity ops** (`cast`, `widen`, `copy_var`). On NaN-boxed values these are **identities**; native/wasm lower them as operand-0 passthrough (`"box"|"unbox"|"cast"|"widen" => op` at function_compiler.rs:1490; wasm.rs:12511). On LLVM they carry `_original_kind` (set), so they hit the fatal gate. *Disposition: latent LLVM gap with a trivial fix* — add an identity arm to `lower_preserved_simpleir_op` returning operand 0. `copy_var` is emitted by the string-split-field fusion (`serialization.py:267`), so the trigger is narrow. Repro sketch: a program whose only `.split()[i]` consumers fuse, `--target llvm`.
- **2 loop-IV ops** (`loop_index_start`, `loop_index_next`). Consumed specially by `lower_from_simple.rs:201/278` (folded into a counted-loop IV) — they should never reach `kind_to_opcode`'s Copy fallback on the lift. *Disposition: benign* (structural-IV machinery; the audit flags them only because they are not in the CFG leader/terminator helpers — they could be added to the derived structural set in phase 2).
- **3 other** (`dataclass_new_values`, `guarded_field_init`, `object_set_class`). Each has a native arm (`guarded_field_init` at function_compiler.rs; `object_set_class` shares `class_apply_set_name`'s arm) and a wasm arm; none has a `molt_<kind>` symbol or LLVM arm. *Disposition: latent LLVM gap* — dataclass field-init / `__class__` reassignment on the LLVM lane fails loud. Repro sketch: `@dataclass`-heavy code or `obj.__class__ = C`, `--target llvm`.

**`classifier_silent_fallthrough` (196) — LATENT promotion-hazard, leak-safe today.** These are real runtime value/effect ops (a `molt_<kind>` exists) that the classifier does NOT place in an explicit class — they take the `_ => TransparentAlias` default (alias_analysis.rs:456). **Today this is leak-safe**: the drop pass emits an independent `DecRef` *only* for `FreshValue` Copies, so a mis-defaulted op can at worst leak a `+1` (never double-free). **The hazard is future promotion**: if someone adds such a kind to `copy_kind_mints_fresh_owned_ref` (FreshValue) without simultaneously adding its explicit LLVM arm, the LLVM fatal gate catches it (`freshvalue_llvm_gap` would become non-zero) — *but* the classifier's silent default means the *initial* mis-classification is invisible. Representative members: `alloc_class*`, `bound_method_new`, `asyncgen_new`, `buffer2d_*`, `bytearray_*` (the large bytearray method family), `super_new`, `dict_get`, `gen_send`. *Disposition: latent* — every one is correctly leak-safe under the current drop pass; the registry's value here is forcing an **explicit** ownership class on each so the default can never silently mis-bucket a future op.

**`mapped_never_emitted` (45) — mostly BENIGN round-trip vocabulary; ONE genuine schism.** The module phase re-lifts post-pipeline SimpleIR on every build (the "CheckedAdd round-trip" comment at ssa.rs:1871), so `kind_to_opcode` MUST recognize `lower_to_simple`'s **output** spellings even though the *frontend* never emits them. Verified round-trip outputs (benign): `build_list`, `get_attr`, `set_attr`, `for_iter`, `yield`, `yield_from`, `checked_add`, `exception_pending`, `iter_next_unboxed`, … The genuine finding:
- **`floor_div` — the bidirectional spelling schism (latent, real today).** The *frontend* emits `"floordiv"` (serialization.py:633); `kind_to_opcode("floordiv")` has no arm → `Copy{_original_kind="floordiv"}`. But `lower_to_simple` emits `OpCode::FloorDiv` → **`"floor_div"`** (lower_to_simple.rs:1516), which `kind_to_opcode("floor_div")` round-trips to the real `OpCode::FloorDiv` (ssa.rs:1798). **The same logical operation thus has TWO wire spellings**: a frontend `floordiv` NEVER becomes the first-class `OpCode::FloorDiv` (it stays Copy-carried, always taking the boxed `molt_floordiv` slow path), while a round-tripped `floor_div` does. This is a latent perf+correctness asymmetry and exactly the divergence vector that produced bug #5 on stale bases. *Disposition: real-today (asymmetry), fix = one canonical spelling.* The registry collapses `floordiv`/`floor_div` to a single canonical kind with a single opcode mapping. (The audit's `mapped_never_emitted` also contains alias-arms like `load_attr`/`store_attr`/`get_iter`/`const_int`/`call_function` that are alternate spellings for *other* round-trip/legacy inputs — benign, but the registry makes the alias set explicit.)

---

## 5. Phase-2 mechanism (the recommendation, 5 lines)

1. **One table** `runtime/molt-backend/src/tir/op_kinds.toml` — rows `(canonical_kind, aliases[], semantics_class, arity, mapper_opcode|"copy", classifier_class ∈ {fresh_value, transparent_alias, inert_marker, structural}, effect ∈ {pure, observe, throw, side_effect}, backends_required[], runtime_symbol?)`.
2. **One generator** `tools/gen_op_kinds.py` (modeled on `tools/gen_intrinsics.py`) renders `runtime/molt-backend/src/tir/op_kinds_generated.rs` (the `kind_to_opcode` arms, the `classify_copy_kind`/`copy_kind_mints_fresh_owned_ref` arms, the effect-oracle arms) AND `src/molt/frontend/lowering/op_kinds_generated.py` (the canonical-spelling constants the emitter uses).
3. **One sync test** `tests/test_gen_op_kinds.py` (modeled on `tests/test_gen_intrinsics.py`) re-renders in memory and `assert_eq`s against the checked-in generated files → **drift = build/test error**.
4. **The effect oracle hooks the same table:** `opcode_may_throw`/`is_side_effecting` (effects.rs) are generated from the `effect` column → a new kind **requires** an explicit effect classification (kills bug-class instance #1 — the `matches!`-default-false trap — because the table has no default; every kind has an explicit `effect`).
5. **The terminal state becomes generated-exhaustive:** the LLVM fail-loud gate and the classifier `_ =>` default survive ONLY as a defense for kinds the table forgot — and the sync test makes "the table forgot" a build failure, so the fail-loud path becomes statically unreachable for any in-table kind (it stays as the runtime backstop, now provably dead for known kinds).

---

## 6. Phase-2 implementation plan

### 6.1 Build order (each step byte-identical until the last)

The unit of work is the complete structural change (per CLAUDE.md). Phase 2 is ONE arc; intermediate commits are allowed only if each is itself a complete, byte-identical piece.

1. **Mirror current reality into `op_kinds.toml`** — every row reflects the *current* code exactly (including BOTH `floordiv` and `floor_div` as canonical+alias of one logical kind, every classifier bucket as it is today, the round-trip spellings as aliases). Generate `op_kinds_generated.rs` + `op_kinds_generated.py`; wire `kind_to_opcode`/`classify_copy_kind`/`copy_kind_mints_fresh_owned_ref`/effect-oracle to include the generated arms; route the emitter's spelling constants through the generated Python. **Verify byte-identical codegen** (diff TIR/SimpleIR + binary on the corpus + compliance) — this commit changes the *source of truth*, not the *output*.
2. **Add the sync test** `tests/test_gen_op_kinds.py` + wire `audit_op_kinds.py --check` into CI against `op_kinds_baseline.json`. (The audit tool is already check-ready; this step wires it.)
3. **Dangerous-cell fixes, each a SEPARATE reviewed commit** (NOT folded into the migration):
   - (a) collapse `floordiv`/`floor_div` to one canonical wire spelling (decide canonical = `floordiv`, the frontend spelling; update `lower_to_simple.rs:1516` to emit `floordiv`; delete the `floor_div` alias once nothing emits it). Verify the round-trip stays idempotent and the first-class `OpCode::FloorDiv` arith path now also covers frontend floordiv.
   - (b) add LLVM identity arms for `cast`/`widen`/`copy_var` (return operand 0). Add `loop_index_*` to the derived structural set.
   - (c) the 18 async/concurrency + 3 dataclass/class ops: either add LLVM dedicated arms with the correct runtime symbol, or (if asyncio-on-LLVM is out of scope) record them as `backends_required = [native, wasm]` so the table documents the LLVM gap and the fail-loud message points at the table.
   - (d) promote the `classifier_silent_fallthrough` (196) to **explicit** `transparent_alias` rows so the `_ =>` default can never silently bucket them; this is byte-identical (they already classify TransparentAlias) but makes each a compiler-checked decision.

### 6.2 Key decisions / constraints

- **Canonical spelling = the frontend emission.** The frontend is the producer; `lower_to_simple` is a round-trip that should match it. Collapsing to the frontend spelling minimizes emitter churn and makes the wire vocabulary == the frontend vocabulary.
- **Aliases are first-class table data**, not code. The mapper's `|`-grouped arms (`"copy" | "store_var" | "load_var"`, `"shl" | "lshift"`, `"eq" | "string_eq"`, …) become `aliases[]` columns. This is where the round-trip/legacy spellings live, explicitly.
- **No default anywhere.** Every kind has an explicit `effect`, `classifier_class`, and `mapper_opcode` (or explicit `"copy"`). The generated Rust still ends in `_ =>` arms for runtime safety, but the sync test makes them unreachable for in-table kinds.
- **The `vec_*` family** stays a generated prefix expansion (the 24 `VEC_REDUCTION_OPS` rows + the classifier `vec_` prefix) — encode the prefix rule in the table, generate the explicit table for LLVM + the prefix check for the classifier.
- **RC soundness invariant preserved** (per docs/design/foundation/20): the classifier's fail-closed direction (unknown → TransparentAlias = leak-not-UAF) is retained as the generated `_ =>` backstop; the table makes the *known* set explicit and total.

### 6.3 Anchors phase 2 edits (verified 2026-06-06)

- `src/molt/frontend/lowering/serialization.py:396` (`map_ops_to_json`), :633 (`floordiv`), :736 (`matmul`), :267 (`copy_var` fusion), :2330 (BOX/UNBOX/CAST/WIDEN).
- `runtime/molt-backend/src/tir/ssa.rs:1789` (`kind_to_opcode`), :1798 (`floor_div` arm), :1923 (`_ => Copy`).
- `runtime/molt-backend/src/tir/lower_to_simple.rs:1676` (`_original_kind` restoration), :1516 (`OpCode::FloorDiv => "floor_div"`).
- `runtime/molt-backend/src/tir/lower_from_simple.rs:201/278` (loop_index special handling).
- `runtime/molt-backend/src/llvm_backend/lowering.rs:6472` (`lower_preserved_simpleir_op`), :9855 (`try_lower_preserved_runtime_call`), :2348-2424 (fail-loud gate), :411 (`VEC_REDUCTION_OPS`), :9789 (`floordiv` arm).
- `runtime/molt-backend/src/tir/passes/alias_analysis.rs:360` (`copy_kind_mints_fresh_owned_ref`), :426 (`classify_copy_kind`), :456 (`_ => TransparentAlias`), :487 (`copy_kind_is_explicit_no_heap_move`).
- `runtime/molt-backend/src/tir/passes/effects.rs` (`opcode_may_throw` / `is_side_effecting` — the effect oracle to hook).
- `runtime/molt-backend/src/tir/mod.rs:48` (`is_structural`), `runtime/molt-backend/src/tir/cfg.rs:59/68/77/83` (terminator/leader/ender/cond-branch).
- Precedents: `tools/gen_intrinsics.py` + `tests/test_gen_intrinsics.py` (the generator + sync-test pattern); `tools/stdlib_full_coverage_manifest.py` (the manifest-table pattern); `tools/audit_op_kinds.py` (this task's check-mode tool).

---

## 7. CI seed

`tools/audit_op_kinds.py --check` exits non-zero on any **new** member of any dangerous-cell category vs `tools/op_kinds_baseline.json` (committed). It is wire-ready; the CI wiring lands in phase 2 step 2 (alongside the `gen_op_kinds.py` sync test). The baseline is the contract: a new emitted kind that drifts (no mapper arm + no coverage, or a silent classifier fallthrough) becomes a build error until it gets a table row.
