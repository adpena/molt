<!-- Foundation blueprint 21a. Architect: M1-architect (Plan agent), 2026-06-23. Arc:
decomposition move #1, CORRECTED. Supersedes doc 21's original move-#1 "file split"
shape (refused — see dx_baseline.md §8). This is a function-extraction plan for
compile_func_inner, continuing the in-flight fc/ family extraction. Verified against
the working tree (post-T1 molt-tir extraction). Move-only / zero-logic-change. -->

# 21a — Decompose `function_compiler` (Move #1, function-extraction)

## Executive finding: premise corrected; M1.1-M1.9 now landed

doc 21's original move #1 ("STRICT move-only **file** split into opcode-family
submodules") was investigated, **REFUSED, and replaced** by the DX lane. Execution of
the replacement is in flight in the tree; M1.1 `arith`, M1.2 `compare`, M1.3
`unary_logic`, M1.4 `funcobj`, M1.5 `coroutine`, M1.6 `calls`, M1.7 `memory`,
M1.8 `ret_jump`, and M1.9 `control_flow` are landed as standalone `fc/`
handlers. Do not undo it. Three
load-bearing facts:

1. **`function_compiler` is already a directory module, partially extracted.**
   `runtime/molt-backend/src/native_backend/function_compiler.rs` (now **13,952 lines**,
   down from doc 21's 39,043) declares `mod fc;` (line 10) → `function_compiler/fc/`, a
   subtree of **34 already-extracted handler files** (`arith.rs`, `compare.rs`,
   `unary_logic.rs`, `funcobj.rs`, `coroutine.rs`, `calls.rs`, `memory.rs`, `ret_jump.rs`, `control_flow.rs`, `list_ops.rs`,
   `dict_ops.rs`, `set_ops.rs`, `attrs.rs`, `exceptions.rs`, `text_predicates.rs` (1,716),
   `text_transform.rs` (1,203), `vec_reductions.rs` (1,140), …). The dispatch now routes
   arithmetic, comparison, unary/logic, function-object, coroutine, call, memory,
   return/jump/variable-transfer, and structured control-flow families through
   `fc::<family>::handle_*` handlers as well.

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

**Move #1 = continue the function-extraction of `compile_func_inner`** (now ~7,842
lines, lines 3125-10966, the single largest fn in the crate, with the remaining
high-risk inline arm clusters still to extract). Strictly move-only / zero-logic-change /
no-API-widening.

## 1. Current structure map

### 1.1 `function_compiler.rs` (13,952 lines)
| Region | Lines | Contents |
|---|---|---|
| `mod fc;` + free helpers | 1-3011 | ~50 free helpers (`var_get_boxed_overflow_safe_base` 728, `box_raw_i64_value_overflow_safe` 668, `ensure_boxed_*`, `def_var_from_*`, `merge_rebind_*`, loop-scan helpers). The shared private helper set every handler calls. |
| `impl SimpleBackend` open | 3012 | |
| `compile_func()` | 3013-3124 | Thin wrapper -> `compile_func_inner`. |
| **`compile_func_inner()`** | **3125-10966** | THE MONOLITH (~7,842 lines). Preanalysis destructure (3133-~3227, ~45 shared `let mut` locals), pre-passes (3228-4217), dispatch loop `for op_idx in 0..ops.len()` at **4218**, central `match op.kind.as_str()` at **4394**, per-op **epilogue** at **10397** (post-dispatch: dec_ref of loop-reassigned vars, drain-cleanup, deferred define). |
| `drain_dead_block_temps_for_suspend()` | 10993-11035 | trailing helper |
| `#[cfg(test)] mod tests` | 11038-13952 | 61 tests |

### 1.2 Dispatch + already-extracted families
Dispatch fn `SimpleBackend::compile_func_inner` (3125); match at 4394 (~140 arms);
epilogue follows the dispatch for any arm that fell through (did not `continue`).
Already delegated families: vec_reductions, scalar_builtins, callargs, list_ops,
dict_ops, set_ops, generators, indexing, text_predicates, text_transform, statistics,
type_conversions, memoryview_buffer, dataclass, parse_ops, future_promise,
object_construct, modules, class_ops, type_checks, exceptions, context_mgmt,
exception_stack, file_io, attrs, arith, compare, unary_logic, funcobj, coroutine, calls, memory, ret_jump, control_flow.

### 1.3 Extracted and remaining inline families
Landed in `fc/`:
- `fc::arith::handle_arith_op` (`arith.rs`) covers arithmetic, bitwise, shift, division/modulo, power, `round`, and `trunc`.
- `fc::compare::handle_compare_op` (`compare.rs`) covers `lt|le|gt|ge|eq|ne|string_eq`.
- `fc::unary_logic::handle_unary_logic_op` (`unary_logic.rs`) covers `is|not|neg|unary_neg|pos|unary_pos|abs|invert|bool|cast_bool|builtin_bool|and|or|contains`.
- `fc::funcobj::handle_funcobj_op` (`funcobj.rs`) covers function objects, code metadata, trace slots/frame line metadata, `missing`, and `function_closure_bits`; `handle_gpu_intrinsic_op` covers the adjacent native GPU runtime intrinsics.
- `fc::coroutine::handle_coroutine_op` (`coroutine.rs`) covers coroutine/generator state transitions, yield/channel suspend points, async spawn/cancellation token ops, and `call_async`.
- `fc::calls::handle_call_op` (`calls.rs`) covers direct/internal/guarded/function/FFI calls, call binding, method/super ICs, bound-method dispatch, `getargv`, `getframe`, and `sys_executable`.
- `fc::memory::handle_memory_op` (`memory.rs`) covers allocation, object stores/loads, closure state loads/stores, guarded field access, and runtime type/layout guards.
- `fc::ret_jump::handle_ret_jump_op` (`ret_jump.rs`) covers returns, jumps/branches, labels, phi no-ops, variable stores/deletes/loads/copies, and parameter loads.
- `fc::control_flow::handle_control_flow_op` (`control_flow.rs`) covers structured `if`, `else`, and `end_if` lowering, including branch-local cleanup transfer and phi/rebind state.

Remaining inline families, current as of the M1.9 landing:

| Handler | Op-kinds (arm labels, line) | Range | ≈LOC |
|---|---|---|---|
| `fc::loops::handle_loop_op` | loop_start(8497),loop_index_start(8727),loop_break_if_exception(9355),loop_break_if_true(9495),loop_break_if_false(9688),loop_break(9899),loop_index_next(9980),loop_continue(10027),loop_end(10178) | 8497-10241 | ~1,745 |

Small arms (constants, len/id/ord/chr, iter*, print*, raise, check_exception) stay inline. Remaining extractable work is now concentrated in the loop family above.

### 1.4 Shared helper + shared-state sets
- **Free helpers** (1–2983, reached via `super::*`): `var_get_boxed_overflow_safe_base`, `box_raw_i64_value_overflow_safe`, `ensure_boxed_overflow_safe`, `def_var_from_*`, `def_var_named`, `import_func_ref`, `merge_rebind_*`. Plus assoc fns `SimpleBackend::import_func_id_split`, `SimpleBackend::intern_data_segment`; shared `fc` helpers own `op_prefers_int_lane` for extracted arithmetic/unary/control-flow handlers.
- **lib.rs `pub(crate)` surface** (via `crate::`): `NanBoxConsts`, `VarValue`, `DeferredDefine`, `block_has_terminator`, `switch_to_block_tracking`, `extend_unique_tracked`, `unbox_int`, `box_int`. **Already pub(crate) — no widening.**
- **Shared `let mut` locals** (~45 from preanalysis + in-loop caches): `builder`, `import_refs`, `sealed_blocks`, `vars`, `int/float/bool_primary_vars`, `bool_like_vars`, `loop_stack`, `if_stack`, `label_blocks`, element caches, `tracked_obj_vars`, `entry_vars`, `already_decrefed`, `alias_roots`, `last_use`, … → passed as split-borrowed explicit params (existing `handle_list_op` threads 20).

## 2. Target layout
Extend existing `function_compiler/fc/`. Each family → one `fc/<family>.rs` with a single
free `fn handle_<family>_op(...) -> OpFlow` (or `-> ()` if no `continue`), registered in
`fc/mod.rs` with `pub(in crate::native_backend::function_compiler) mod <family>;`.
File header idiom: `use super::super::*; use super::OpFlow;` (+ shared helpers as needed).

**Stays in `function_compiler.rs`:** the `compile_func_inner` shell (preanalysis, pre-passes,
dispatch match reduced to thin delegating arms, epilogue at 10397), `compile_func`, the §1.4
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
   (4218). `OpFlow::Continue` ⇒ caller `continue` (skips epilogue 10397+); `OpFlow::Proceed` ⇒
   fall through (runs epilogue). Inner-loop breaks stay inside handlers. The labeled `break 'find_phi`
   (8822) is fully inside its local arm — moves verbatim with that arm. **Audit each candidate's arm range for a
   bare outer-loop `break;` (not inside a nested for/while/loop) before moving**; if found, add an
   `OpFlow::Break` variant + a `fc::OpFlow::Break => break,` caller arm (mod.rs anticipates this).
4. **Op-local closures** (e.g. `var_get_boxed_overflow_safe` capturing `bool_primary_vars`+`nbc`)
   reconstructed at handler top with identical captures (pattern `list_ops.rs:41`).

## 4. Ordering — each an independently-compiling move-only commit, green build
- **M1.0 Prep audit:** per family, grep arm range for outer-loop `break;` + any private helper used; confirm `OpFlow` sufficiency; record exact param set (split-borrowed locals + caches).
- **M1.1 `fc::arith`** — landed.
- **M1.2 `fc::compare`** — landed.
- **M1.3 `fc::unary_logic`** — landed.
- **M1.4 `fc::funcobj`** — landed.
- **M1.5 `fc::coroutine`** — landed.
- **M1.6 `fc::calls`** — landed.
- **M1.7 `fc::memory`** — landed.
- **M1.8 `fc::ret_jump`** — landed.
- **M1.9 `fc::control_flow`** — landed.
- **M1.10 `fc::loops`** (8497-10241, ~1,745) — LAST (densest shared-mutable-state: loop_stack/phi side-tables; most likely to need `OpFlow::Break`).
Stop-anywhere: M1.1-M1.9 removed the largest arithmetic/compare/unary/function-object,
coroutine, call, memory, ret/jump, and structured control-flow families and converted
them into separate codegen units; continue with the M1.10 loop family next.

## 5. Verification gates (per commit — 34e3bddbf / dx_baseline §9; isolated CARGO_TARGET_DIR)
- **G1 0-warning builds, both feature sets:** `cargo build -p molt-backend --features native-backend --profile dev-fast` (0 warns); `--features wasm-backend` (fc is `#[cfg(feature="native-backend")]` → compiles out under wasm-only; diff warning set vs pre-split, no NEW warns); `cargo clippy -p molt-backend --features native-backend -- -D warnings`; `cargo clippy --features "native-backend llvm" --lib -- -D warnings`.
- **G2 lib tests:** `cargo test -p molt-backend --features native-backend --lib` all pass (baseline ~983; 61 in-file tests stay).
- **G3 byte-identical artifacts (the move-only proof):** before/after, compile a fixed `.py` corpus to native `.o` (`python -m molt build --target native --rebuild`) + capture stderr diagnostics; `diff` `.o` + diagnostics → must be byte-identical. Any diff ⇒ a body changed ⇒ reject.
- **G4 differential e2e:** `python -m molt test` (guarded harness, never raw binary) on fib/bigint/generator/exception/dict/list subset vs CPython — identical output.
- **G5 symbol/diagnostic identity:** `nm` the rlib before/after — no new exported symbols (move uses `pub(in …function_compiler)`). Embedded panic/diagnostic messages move verbatim.
A commit is not done until G1–G5 pass.

## 6. The win
1. **Intra-crate codegen parallelism (the function-split win):** today `compile_func_inner` is ~7,842 lines = ONE indivisible codegen unit (cu=256 can't touch it); each extracted `handle_*_op` becomes its own cu → remaining large arm clusters become separate units codegen'd in parallel; the shell keeps shrinking toward the ~6K target. The only mechanism that moves this file's intra-crate compile (why file-split was refused, function-split is move #1). Fill doc 21 §5 `{DX-BASELINE:fc-incremental}` with measured before/after once a family lands.
2. **Ownership-collision blast-radius (headline friction win, now):** the #1 god-file collision source. The extracted handler files already turned the dominant opcode families into independently-ownable files; completing move #1 converts the remaining large clusters into a few more disjoint handler modules. An arith fix touches only `fc/arith.rs` (~4K) + a 1-line dispatch arm. Two agents editing disjoint families = zero collision. Realizes doc 21's "39K→~4-6K per family" via function-extraction.

## Critical files
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (shell + `compile_func_inner` 3125-10966)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/mod.rs` (register families; `OpFlow`; shared `var_get_boxed_overflow_safe_fn`)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/list_ops.rs` (reference template: handler signature, split-borrow params, closure reconstruction, `OpFlow`)
- `runtime/molt-backend/src/native_backend/mod.rs` (the `use super::*` ancestry — do not change)
- `docs/design/foundation/dx_baseline.md` (§3.3/§4/§6 rationale, §9 gates)
