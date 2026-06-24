<!-- Foundation blueprint 21a. Architect: M1-architect (Plan agent), 2026-06-23. Arc:
decomposition move #1, CORRECTED. Supersedes doc 21's original move-#1 "file split"
shape (refused — see dx_baseline.md §8). This is a function-extraction plan for
compile_func_inner, continuing the in-flight fc/ family extraction. Verified against
the working tree (post-T1 molt-tir extraction). Move-only / zero-logic-change. -->

# 21a — Decompose `function_compiler` (Move #1, function-extraction)

## Executive finding: premise corrected, work already ~40% done

doc 21's original move #1 ("STRICT move-only **file** split into opcode-family
submodules") was investigated, **REFUSED, and replaced** by the DX lane; execution of
the replacement is ~40% complete in the tree. Do not undo it. Three load-bearing facts:

1. **`function_compiler` is already a directory module, partially extracted.**
   `runtime/molt-backend/src/native_backend/function_compiler.rs` (now **28,144 lines**,
   down from doc 21's 39,043) declares `mod fc;` (line 10) → `function_compiler/fc/`, a
   subtree of **27 already-extracted family handler files** (~12,800 LOC: `list_ops.rs`,
   `dict_ops.rs`, `set_ops.rs`, `attrs.rs`, `exceptions.rs`, `text_predicates.rs` (1,716),
   `text_transform.rs` (1,203), `vec_reductions.rs` (1,140), …). 33 `fc::<family>::handle_*`
   delegations are already wired into the dispatch.

2. **The file-split buys ~0 build win and was explicitly rejected.** `dx_baseline.md`
   §3.3/§4/§6 (MEASURED) proves `function_compiler.rs` is essentially ONE method,
   `compile_func_inner`, with one giant inline `match op.kind.as_str()`. **A function is
   the atomic unit of rustc codegen** — `codegen-units` partitions at function boundaries,
   so a ~22K-line function is ONE codegen unit regardless of surrounding files.
   dx_baseline §8 lists "split function_compiler.rs FILE" as **"REFUSED as designed"**
   ("~0 build win"). Per doc 21 §0.3, 08's submodule boundary list is authoritative and
   this overrides doc 21's original move-#1 shape.

3. **The correct lever — already in flight — is splitting the FUNCTION.** `fc/mod.rs`
   documents the idiom: extract each `match op.kind` arm body into a standalone free
   `fn handle_X_op(...)` (each its own codegen unit) taking shared lowering state as
   explicit split-borrowed `&mut` params, with `OpFlow` returns replicating outer-loop
   `continue`. Arm bodies move **byte-identically**; only field-access paths change.

**Move #1 = continue the function-extraction of `compile_func_inner`** (still ~22,100
lines, lines 3097–25158, the single largest fn in the crate, ~16K lines of inline arms
still to extract). Strictly move-only / zero-logic-change / no-API-widening.

## 1. Current structure map

### 1.1 `function_compiler.rs` (28,144 lines)
| Region | Lines | Contents |
|---|---|---|
| `mod fc;` + free helpers | 1–2983 | ~50 free helpers (`var_get_boxed_overflow_safe_base` 728, `box_raw_i64_value_overflow_safe` 668, `ensure_boxed_*`, `def_var_from_*`, `merge_rebind_*`, loop-scan helpers). The shared private helper set every handler calls. |
| `impl SimpleBackend` open | 2984 | |
| `compile_func()` | 2985–3096 | Thin wrapper → `compile_func_inner`. |
| **`compile_func_inner()`** | **3097–25158** | THE MONOLITH (~22,060 lines). Preanalysis destructure (3133–~3227, ~45 shared `let mut` locals), pre-passes (3228–4298), dispatch loop `for op_idx in 0..ops.len()` at **4299**, central `match op.kind.as_str()` at **4475** ending at **24587**, per-op **epilogue** (24589–~25100: dec_ref of loop-reassigned vars, drain-cleanup). |
| `drain_dead_block_temps_for_suspend()` | ~25160–25229 | trailing helper |
| `#[cfg(test)] mod tests` | 25230–28144 | 61 tests |

### 1.2 Dispatch + already-extracted families
Dispatch fn `SimpleBackend::compile_func_inner` (3097); match at 4475 (~140 arms) closing 24587; epilogue at 24589 for any arm that fell through (did not `continue`). Already delegated (33): vec_reductions, scalar_builtins, callargs, list_ops, dict_ops, set_ops, generators, indexing, text_predicates, text_transform, statistics, type_conversions, memoryview_buffer, dataclass, parse_ops, future_promise, object_construct, modules, class_ops, type_checks, exceptions, context_mgmt, exception_stack, file_io, attrs.

### 1.3 Remaining inline families → extraction targets (largest-value first)
| Handler | Op-kinds (arm labels, line) | Range | ≈LOC |
|---|---|---|---|
| `fc::arith::handle_arith_op` | add(4725),checked_add(5077),inplace_add(5184),sub(5515),inplace_sub(5787),mul(6038),inplace_mul(6304),bit_or/inplace(6548/6686),bit_and/inplace(6777/6914),bit_xor/inplace(7005/7142),lshift\|shl\|inplace(7233),rshift\|shr\|inplace(7282),matmul\|inplace(7329),div\|inplace(7376),floordiv\|inplace(7709),mod\|inplace(7988),floor_div\|binop_floor_div(8253),pow\|inplace(8359),pow_mod(8626),round(8678),trunc(8732) | 4725–8759 | ~4,030 |
| `fc::compare::handle_compare_op` | lt(11439),le(11623),gt(11799),ge(11976),eq(12156),ne(12313),string_eq(12469) | 11439–12509 | ~1,070 |
| `fc::unary_logic::handle_unary_logic_op` | is(12579),not(12619),neg\|unary_neg(12687),pos\|unary_pos(12820),abs(12909),invert(13017),bool\|cast_bool\|builtin_bool(13119),and(13232),or(13322),contains(13408) | 12579–13455 | ~877 |
| `fc::calls::handle_call_op` | call(15494),call_internal(15991),call_guarded(16260),call_func(16634),invoke_ffi(16987),call_bind\|call_indirect(17096),call_method_ic(17205),call_super_method_ic(17307),call_method(17419) | 15494–17629 | ~2,135 |
| `fc::control_flow::handle_branch_op` | if(18145),else(18655),end_if(18890) | 18145–19714 | ~1,570 |
| `fc::loops::handle_loop_op` | loop_start(19715),loop_index_start(19945),loop_break_if_exception(20573),loop_break_if_true(20713),loop_break_if_false(20906),loop_break(21117),loop_index_next(21198),loop_continue(21245),loop_end(21396) | 19715–21459 | ~1,745 |
| `fc::memory::handle_store_load_op` | alloc\|stack_alloc(21460),alloc_class*(21502–21597),alloc_task(21598),store(21696),store_init(22110),load(22421),closure_load(22451),closure_store(22486),guarded_load(22532),guarded_field_*(22562–22864),guard_type\|guard_tag(22865),guard_layout\|guard_dict_shape(22929) | 21460–23017 | ~1,560 |
| `fc::ret_jump::handle_ret_jump_op` | ret(23018),ret_void(23351),jump(23457),br_if(23542),label\|state_label(23776),store_var(23859),delete_var(24145),load_var\|copy_var(24220),load_param(24567) | 23018–24586 | ~1,570 |
| `fc::coroutine::handle_coroutine_op` | state_switch(13569),state_transition(13608),state_yield(13843),chan_send_yield(13925),chan_recv_yield(14093),chan_new(14249),chan_drop(14277),spawn(14302),cancel_token_*(14326–14544),call_async(14546) | 13569–14611 | ~1,040 |
| `fc::funcobj::handle_funcobj_op` | builtin_func(14612),func_new(14666),func_new_closure(14747),code_new(14859),code_slot_set(15006),fn_ptr_code_set(15032),asyncgen_locals_register(15086),gen_locals_register(15152),code_slots_init(15219),frame_locals_set(15247),line(15276),missing(15401),function_closure_bits(15416),bound_method_new(15446) | 14612–15493 | ~880 |

Small arms (constants, len/id/ord/chr, iter*, print*, raise, check_exception) stay inline. Total extractable ≈16,000 lines → `compile_func_inner` shrinks to ~6K.

### 1.4 Shared helper + shared-state sets
- **Free helpers** (1–2983, reached via `super::*`): `var_get_boxed_overflow_safe_base`, `box_raw_i64_value_overflow_safe`, `ensure_boxed_overflow_safe`, `def_var_from_*`, `def_var_named`, `import_func_ref`, `merge_rebind_*`. Plus assoc fns `SimpleBackend::import_func_id_split`, `SimpleBackend::intern_data_segment`.
- **lib.rs `pub(crate)` surface** (via `crate::`): `NanBoxConsts`, `VarValue`, `DeferredDefine`, `block_has_terminator`, `switch_to_block_tracking`, `extend_unique_tracked`, `unbox_int`, `box_int`. **Already pub(crate) — no widening.**
- **Shared `let mut` locals** (~45 from preanalysis + in-loop caches): `builder`, `import_refs`, `sealed_blocks`, `vars`, `int/float/bool_primary_vars`, `bool_like_vars`, `loop_stack`, `if_stack`, `label_blocks`, element caches, `tracked_obj_vars`, `entry_vars`, `already_decrefed`, `alias_roots`, `last_use`, … → passed as split-borrowed explicit params (existing `handle_list_op` threads 20).

## 2. Target layout
Extend existing `function_compiler/fc/`. Each family → one `fc/<family>.rs` with a single
free `fn handle_<family>_op(...) -> OpFlow` (or `-> ()` if no `continue`), registered in
`fc/mod.rs` with `pub(in crate::native_backend::function_compiler) mod <family>;`.
File header idiom: `use super::super::*; use super::OpFlow;` (+ shared helpers as needed).

**Stays in `function_compiler.rs`:** the `compile_func_inner` shell (preanalysis, pre-passes,
dispatch match reduced to thin delegating arms, epilogue at 24589), `compile_func`, the §1.4
free helpers, trailing `drain_dead_block_temps_for_suspend`, `mod tests`. NOTE: struct defs
(`SimpleBackend`, `NativeBackendModuleContext`) live in `simple_backend.rs`, not here — handlers
call `SimpleBackend::` assoc fns (path-independent).

**Per-arm rewrite (only delta — byte-identical bodies):** each extracted arm becomes a thin
delegation: `"add" | "checked_add" | ... => { match fc::arith::handle_arith_op(&op, op_idx, …split-borrowed params…) { fc::OpFlow::Continue => continue, fc::OpFlow::Proceed => {} } }`.
Inside the handler the moved body changes only: `self.module`→`module`, `Self::`→`SimpleBackend::`,
op-local closures reconstructed with identical captures (template: `list_ops.rs:41-67`), bare
`continue;`→`return OpFlow::Continue;`, fall-through end→`OpFlow::Proceed`.

## 3. Move mechanics that preserve compilation
1. **Free fn, not method** — so the borrow checker can split-borrow `self.module` and
   `self.ctx.func` simultaneously (`builder: &mut FunctionBuilder` already borrows `self.ctx.func`;
   handler can't also take `&mut self`). Same reason `import_func_id_split` exists.
2. **Reachability without widening:** Cranelift/std + sibling private items via
   `use super::super::*` → `function_compiler`'s `mod fc; use super::*;` → `native_backend/mod.rs`'s
   `use super::*;` (module-ancestry privacy, lib.rs precedent 34e3bddbf). Cross-`fc`-file shared
   items (`OpFlow`, `var_get_boxed_overflow_safe_fn`) are `pub(in crate::native_backend::function_compiler)`
   — narrower than pub(crate), zero external-API change. `function_compiler.rs` bare-private
   helpers are reachable by `fc` descendants via the glob (ancestry privacy) — **no `pub` needed**.
3. **`continue`/`break`/epilogue fidelity (correctness-critical):** outer op-loop is UNLABELED
   (4299). `OpFlow::Continue` ⇒ caller `continue` (skips epilogue 24589+); `OpFlow::Proceed` ⇒
   fall through (runs epilogue). Inner-loop breaks stay inside handlers. The labeled `break 'find_phi`
   (15566) is fully inside the `call` arm — moves verbatim. **Audit each candidate's arm range for a
   bare outer-loop `break;` (not inside a nested for/while/loop) before moving**; if found, add an
   `OpFlow::Break` variant + a `fc::OpFlow::Break => break,` caller arm (mod.rs anticipates this).
4. **Op-local closures** (e.g. `var_get_boxed_overflow_safe` capturing `bool_primary_vars`+`nbc`)
   reconstructed at handler top with identical captures (pattern `list_ops.rs:41`).

## 4. Ordering — each an independently-compiling move-only commit, green build
- **M1.0 Prep audit:** per family, grep arm range for outer-loop `break;` + any private helper used; confirm `OpFlow` sufficiency; record exact param set (split-borrowed locals + caches).
- **M1.1 `fc::arith`** (4725–8759, ~4,030) — biggest win, self-contained.
- **M1.2 `fc::compare`** (11439–12509, ~1,070).
- **M1.3 `fc::unary_logic`** (12579–13455, ~877).
- **M1.4 `fc::funcobj`** (14612–15493, ~880) and **M1.5 `fc::coroutine`** (13569–14611, ~1,040) — independent.
- **M1.6 `fc::calls`** (15494–17629, ~2,135) — highest care (labeled-block, IC dispatch); audit `break` first.
- **M1.7 `fc::memory`** (21460–23017, ~1,560) and **M1.8 `fc::ret_jump`** (23018–24586, ~1,570).
- **M1.9 `fc::control_flow`** (18145–19714, ~1,570) and **M1.10 `fc::loops`** (19715–21459, ~1,745) — LAST (densest shared-mutable-state: if_stack/loop_stack/phi side-tables; most likely to need `OpFlow::Break`).
Stop-anywhere: M1.1–M1.3 alone removes ~6,000 lines.

## 5. Verification gates (per commit — 34e3bddbf / dx_baseline §9; isolated CARGO_TARGET_DIR)
- **G1 0-warning builds, both feature sets:** `cargo build -p molt-backend --features native-backend --profile dev-fast` (0 warns); `--features wasm-backend` (fc is `#[cfg(feature="native-backend")]` → compiles out under wasm-only; diff warning set vs pre-split, no NEW warns); `cargo clippy -p molt-backend --features native-backend -- -D warnings`; `cargo clippy --features "native-backend llvm" --lib -- -D warnings`.
- **G2 lib tests:** `cargo test -p molt-backend --features native-backend --lib` all pass (baseline ~983; 61 in-file tests stay).
- **G3 byte-identical artifacts (the move-only proof):** before/after, compile a fixed `.py` corpus to native `.o` (`python -m molt build --target native --rebuild`) + capture stderr diagnostics; `diff` `.o` + diagnostics → must be byte-identical. Any diff ⇒ a body changed ⇒ reject.
- **G4 differential e2e:** `python -m molt test` (guarded harness, never raw binary) on fib/bigint/generator/exception/dict/list subset vs CPython — identical output.
- **G5 symbol/diagnostic identity:** `nm` the rlib before/after — no new exported symbols (move uses `pub(in …function_compiler)`). Embedded panic/diagnostic messages move verbatim.
A commit is not done until G1–G5 pass.

## 6. The win
1. **Intra-crate codegen parallelism (the function-split win):** today `compile_func_inner` is ~22,100 lines = ONE indivisible codegen unit (cu=256 can't touch it); each extracted `handle_*_op` becomes its own cu → ~16K lines of arms become ~10 units codegen'd in parallel; the shell drops to ~6K. The only mechanism that moves this file's intra-crate compile (why file-split was refused, function-split is move #1). Fill doc 21 §5 `{DX-BASELINE:fc-incremental}` with measured before/after once a family lands.
2. **Ownership-collision blast-radius (headline friction win, now):** the #1 god-file collision source. The 33 extracted families already turned ~13K LOC into independently-ownable files; completing move #1 converts the remaining ~16K into ~10 more. An arith fix touches only `fc/arith.rs` (~4K) + a 1-line dispatch arm. Two agents editing disjoint families = zero collision. Realizes doc 21's "39K→~4-6K per family" via function-extraction.

## Critical files
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (shell + `compile_func_inner` 3097–25158)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/mod.rs` (register families; `OpFlow`; shared `var_get_boxed_overflow_safe_fn`)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/list_ops.rs` (reference template: handler signature, split-borrow params, closure reconstruction, `OpFlow`)
- `runtime/molt-backend/src/native_backend/mod.rs` (the `use super::*` ancestry — do not change)
- `docs/design/foundation/dx_baseline.md` (§3.3/§4/§6 rationale, §9 gates)
