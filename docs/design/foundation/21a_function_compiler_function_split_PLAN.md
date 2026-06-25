<!-- Foundation blueprint 21a. Architect: M1-architect (Plan agent), 2026-06-23. Arc:
decomposition move #1, CORRECTED. Supersedes doc 21's original move-#1 "file split"
shape (refused — see dx_baseline.md §8). This is a function-extraction plan for
compile_func_inner, continuing the in-flight fc/ family extraction. Verified against
the working tree (post-T1 molt-tir extraction). Move-only / zero-logic-change. -->

# 21a — Decompose `function_compiler` (Move #1, function-extraction)

## Executive finding: premise corrected; M1.1-M1.18 now landed

doc 21's original move #1 ("STRICT move-only **file** split into opcode-family
submodules") was investigated, **REFUSED, and replaced** by the DX lane. Execution of
the replacement is in flight in the tree; M1.1 `arith`, M1.2 `compare`, M1.3
`unary_logic`, M1.4 `funcobj`, M1.5 `coroutine`, M1.6 `calls`, M1.7 `memory`,
M1.8 `ret_jump`, M1.9 `control_flow`, M1.10 `loops`, M1.11 full
`indexing`, M1.12 `sequence_ops`, M1.13 residual dict mutation, M1.14
exception control, M1.15 value transfer, M1.16 runtime ops, and M1.17 const
literals are landed as standalone `fc/` handlers. The shared scalar carrier,
boxing, merge-rebind, live-through, guarded bitwise, and float transport helper
belt is now lifted into the sibling private authority module
`function_compiler/scalar_carriers.rs`. Do not undo it.
Three load-bearing facts:

1. **`function_compiler` is already a directory module, partially extracted.**
   `runtime/molt-backend/src/native_backend/function_compiler.rs` (now **7,727 lines**,
   down from doc 21's 39,043) declares `mod scalar_carriers; use scalar_carriers::*; mod fc;`
   (lines 7-10) → a sibling helper-authority module plus `function_compiler/fc/`, a
   subtree of **42 already-extracted handler files** (`arith.rs`, `compare.rs`,
   `unary_logic.rs`, `funcobj.rs`, `coroutine.rs`, `calls.rs`, `memory.rs`, `ret_jump.rs`, `control_flow.rs`, `loops.rs`, `list_ops.rs`,
   `dict_ops.rs`, `set_ops.rs`, `attrs.rs`, `exceptions.rs`, `const_literals.rs`,
   `text_predicates.rs` (1,716),
   `text_transform.rs` (1,203), `vec_reductions.rs` (1,140), …). The dispatch now routes
   arithmetic, comparison, unary/logic, function-object, coroutine, call, memory,
   return/jump/variable-transfer, structured control-flow, loop, subscript
   read/write, sequence/iterator, complete dict mutation, exception-control,
   value-custody transfer, runtime shim, and constant/literal materialization
   families through `fc::<family>::handle_*` handlers as well. The shared
   scalar-carrier module is a private implementation authority for raw/boxed
   transport, merge-rebind storage, live-through params, guarded bitwise, and
   float compare/value helpers that multiple `fc::*` handlers consume.

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

**Move #1 = continue the function-extraction of `compile_func_inner`** (now ~2,670
lines, lines 2042-4715, with the planned large opcode-family clusters extracted).
The current continuation moves shared helper authority out of the shell only
when that authority is consumed across extracted families. Strictly move-only /
zero-logic-change / no-API-widening.

## 1. Current structure map

### 1.1 `function_compiler.rs` (7,727 lines)
| Region | Lines | Contents |
|---|---|---|
| `mod scalar_carriers; use scalar_carriers::*; mod fc;` + orchestration helpers | 1-1928 | Loop/body scans, container-store helpers, imports, cleanup-root helpers, data-segment/module helpers, and other shell-owned orchestration. Scalar carrier, boxing, merge-rebind, live-through, guarded bitwise, and float transport helpers moved to `scalar_carriers.rs`. |
| `impl SimpleBackend` open | 1929 | |
| `compile_func()` | 1930-2041 | Thin wrapper -> `compile_func_inner`. |
| **`compile_func_inner()`** | **2042-4715** | THE MONOLITH (~2,670 lines). Preanalysis destructure, pre-passes, literal-family prologue call, dispatch loop `for op_idx in 0..ops.len()` at **2888**, central `match op.kind.as_str()` at **3071**, and per-op epilogue/cleanup after the family dispatch. |
| `drain_dead_block_temps_for_suspend()` | 4716-4759 | trailing helper |
| `#[cfg(test)] mod tests` | 4761-7727 | 63 tests |

### 1.2 Dispatch + already-extracted families
Dispatch fn `SimpleBackend::compile_func_inner` (2042); match at 3071;
epilogue follows the dispatch for any arm that fell through (did not `continue`).
Already delegated families: vec_reductions, scalar_builtins, callargs, list_ops,
dict_ops, set_ops, generators, indexing, sequence_ops, text_predicates, text_transform, runtime_ops, statistics,
type_conversions, memoryview_buffer, dataclass, parse_ops, future_promise,
object_construct, modules, class_ops, type_checks, exceptions, context_mgmt,
exception_stack, file_io, attrs, const_literals, arith, compare, unary_logic, funcobj, coroutine, calls, memory, ret_jump, control_flow, loops.

### 1.3 Extracted families and residual inline shell
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
- `fc::loops::handle_loop_op` (`loops.rs`) covers structured loop starts, index loops, break/continue variants, exception-break cleanup, hoisted list loop caches, and loop-end sealing.
- `fc::indexing::handle_indexing_op` (`indexing.rs`) covers `index`, `store_index`, `del_index`, `slice`, and `slice_new`, including stack-tuple indexing, list-int/list-bool fast paths, conditional list-bool carriers, and container-store refcount absorption.
- `fc::sequence_ops::handle_sequence_op` (`sequence_ops.rs`) covers `len`, `range_new`, `tuple_new`, `unpack_sequence`, `tuple_count`, `tuple_index`, `iter`, `enumerate`, `iter_next_unboxed`, and `iter_next`, including stack-tuple materialization, representation-plan-specialized `len`, and zero-allocation iterator-to-unpack fusions that own `skip_ops`.
- `fc::dict_ops::handle_dict_op` (`dict_ops.rs`) now also covers `dict_set` and `dict_update_missing`, including flat-list-int and integer-key fast-path dispatch, so dict mutation no longer has a residual inline lane.
- `fc::exception_control::handle_exception_control_op` (`exception_control.rs`) covers `raise` and `check_exception`, including exception-pending branching, tracked cleanup drainage at exception boundaries, fallthrough sealing, and remaining cleanup-root propagation.
- `fc::value_transfer::handle_value_transfer_op` (`value_transfer.rs`) covers `inc_ref`, `borrow`, `dec_ref`, `release`, `box`, `unbox`, `cast`, `widen`, `identity_alias`, and `binding_alias`, including alias-preserving refcount adjustment and tracked cleanup-root scrubbing for explicit release operations.
- `fc::runtime_ops::handle_runtime_op` (`runtime_ops.rs`) covers `env_get`, `exception_pending`, `function_defaults_version`, `print`, `warn_stderr`, `print_newline`, `block_on`, and `bridge_unavailable`, including runtime state probes and side-effecting runtime helper calls that always fall through to the parent epilogue.
- `fc::const_literals::handle_const_literal_op` (`const_literals.rs`) covers `const`, `const_bigint`, `const_bool`, `const_none`, `const_not_implemented`, `const_ellipsis`, `const_float`, `const_str`, and `const_bytes`, including inline-int range policy, const-str payload fallback, loop-entry constant pre-materialization, heap-literal prologue hoisting, data-segment interning, per-kind stack-slot maps, string-output slot exports for module ops, and `rc_skip_dec` updates for hoisted heap constants.

Remaining inline families, current as of the M1.18 landing:

No planned M1 opcode-family cluster remains inline. Constant/literal materialization and hoisting moved as one structural contract into `fc::const_literals`; `op_family::INLINE_DISPATCH_KINDS` is empty and remains only as an enforcement hook for future extracted-family routing.

### 1.4 Shared helper + shared-state sets
- **Private shared helper module** (`scalar_carriers.rs`, reached via `super::*`): `name_is_int_like`, `int_raw_value`, `def_inline_int_value`, `bool_raw_value`, `ensure_boxed_*`, `box_raw_*`, `var_get_boxed_overflow_safe_base`, `def_var_from_*`, `emit_protect_borrowed_args_aliased_return`, `merge_rebind_*`, live-through param rebinding, guarded boxed bitwise, `float_value_for*`, dead-scrub value selection, and float compare emission. This is the extracted representation transport authority used by multiple `fc::*` handlers.
- **Shell-owned free helpers** (1-1928, reached via `super::*`): loop/body scans, container-store helpers, `def_var_named`, `import_func_ref`, cleanup-root helpers, data-segment/module helpers, and other orchestration that remains coupled to `compile_func_inner`. Plus assoc fns `SimpleBackend::import_func_id_split`, `SimpleBackend::intern_data_segment`; shared `fc` helpers own `op_prefers_int_lane` for extracted arithmetic/unary/control-flow handlers.
- **lib.rs `pub(crate)` surface** (via `crate::`): `NanBoxConsts`, `VarValue`, `DeferredDefine`, `block_has_terminator`, `switch_to_block_tracking`, `extend_unique_tracked`, `unbox_int`, `box_int`. **Already pub(crate) — no widening.**
- **Shared `let mut` locals** (~45 from preanalysis + in-loop caches): `builder`, `import_refs`, `sealed_blocks`, `vars`, `int/float/bool_primary_vars`, `bool_like_vars`, `loop_stack`, `if_stack`, `label_blocks`, element caches, `tracked_obj_vars`, `entry_vars`, `already_decrefed`, `alias_roots`, `last_use`, … → passed as split-borrowed explicit params (existing `handle_list_op` threads 20).

## 2. Target layout
Extend existing `function_compiler/fc/`. Each opcode family → one `fc/<family>.rs`
with a single free `fn handle_<family>_op(...) -> OpFlow` (or `-> ()` if no
`continue`), registered in `fc/mod.rs` with
`pub(in crate::native_backend::function_compiler) mod <family>;`. Shared helper
authority modules that serve multiple families may live as sibling private
modules under `function_compiler/` (currently `scalar_carriers.rs`) and be
glob-imported by the shell; they must stay narrower than `pub(crate)` and must
not become backend-wide API. File header idiom: `use super::super::*; use
super::OpFlow;` (+ shared helpers as needed).

**Stays in `function_compiler.rs`:** the `compile_func_inner` shell (preanalysis, pre-passes,
dispatch match reduced to thin delegating arms, epilogue after the family dispatch), `compile_func`,
the §1.4 shell-owned orchestration helpers, trailing `drain_dead_block_temps_for_suspend`, `mod tests`. NOTE: struct defs
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
   `use super::super::*` → `function_compiler`'s `mod scalar_carriers; use scalar_carriers::*; mod fc; use super::*;` → `native_backend/mod.rs`'s
   `use super::*;` (module-ancestry privacy, lib.rs precedent 34e3bddbf). Cross-`fc`-file shared
   items (`OpFlow`, `var_get_boxed_overflow_safe_fn`) are `pub(in crate::native_backend::function_compiler)`
   — narrower than pub(crate), zero external-API change. `function_compiler.rs` bare-private
   helpers are reachable by `fc` descendants via the glob (ancestry privacy).
   Cross-file helpers that live in `scalar_carriers.rs` are
   `pub(in crate::native_backend::function_compiler)`, not `pub(crate)`.
3. **`continue`/`break`/epilogue fidelity (correctness-critical):** outer op-loop is UNLABELED
   (2888). `OpFlow::Continue` ⇒ caller `continue` (skips the post-dispatch epilogue); `OpFlow::Proceed` ⇒
   fall through (runs epilogue). Inner-loop breaks stay inside handlers. The labeled `break 'find_phi`
   (`fc/loops.rs:357-407`) is fully inside its local arm. **Audit each candidate's arm range for a
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
- **M1.10 `fc::loops`** — landed.
- **M1.11 `fc::indexing` full `index`/`store_index` expansion** — landed.
- **M1.12 `fc::sequence_ops`** — landed for sequence construction/query,
  unpacking, specialized `len`, and iterator-next zero-allocation fusions.
- **M1.13 `fc::dict_ops` residual mutation completion** — landed for `dict_set`
  and `dict_update_missing` fast-path dispatch.
- **M1.14 `fc::exception_control`** — landed for `raise` and `check_exception`
  control transfer plus exception-boundary tracked cleanup.
- **M1.15 `fc::value_transfer`** — landed for explicit refcount, release,
  conversion-alias, identity-alias, and binding-alias custody transfer.
- **M1.16 `fc::runtime_ops`** - landed for runtime state probes and
  side-effecting runtime helper calls that fall through to the parent epilogue.
- **M1.17 `fc::const_literals`** - landed for constant/literal materialization,
  loop-entry constant pre-materialization, heap-literal hoisting, data-segment
  interning, stack-slot maps, module string-slot exports, and heap-literal
  `rc_skip_dec` custody.
- **M1.18 `scalar_carriers` shared helper authority** - landed for raw/boxed
  scalar carriers, merge-rebind storage, live-through params, guarded bitwise,
  dead-scrub value selection, float transport, and float compare helpers shared
  by the shell and extracted `fc::*` handlers.
Stop-anywhere: M1.1-M1.18 removed the largest arithmetic/compare/unary/function-object,
coroutine, call, memory, ret/jump, structured control-flow, loop, subscript, and
sequence/iterator families plus complete dict mutation, exception control, value-custody transfer, runtime shims, constant/literal materialization, and the shared scalar carrier helper authority. Future work should start
from the next foundation routing doc or a fresh residual-inline contract rather than
reopening these landed family moves.

## 5. Verification gates (per commit — 34e3bddbf / dx_baseline §9; isolated CARGO_TARGET_DIR)
- **G1 0-warning builds, both feature sets:** `cargo build -p molt-backend --features native-backend --profile dev-fast` (0 warns); `--features wasm-backend` (fc is `#[cfg(feature="native-backend")]` → compiles out under wasm-only; diff warning set vs pre-split, no NEW warns); `cargo clippy -p molt-backend --features native-backend -- -D warnings`; `cargo clippy --features "native-backend llvm" --lib -- -D warnings`.
- **G2 lib tests:** `cargo test -p molt-backend --features native-backend --lib` all pass (baseline ~983; 61 in-file tests stay).
- **G3 byte-identical artifacts (the move-only proof):** before/after, compile a fixed `.py` corpus to native `.o` (`python -m molt build --target native --rebuild`) + capture stderr diagnostics; `diff` `.o` + diagnostics → must be byte-identical. Any diff ⇒ a body changed ⇒ reject.
- **G4 differential e2e:** `python -m molt test` (guarded harness, never raw binary) on fib/bigint/generator/exception/dict/list subset vs CPython — identical output.
- **G5 symbol/diagnostic identity:** `nm` the rlib before/after — no new exported symbols (move uses `pub(in …function_compiler)`). Embedded panic/diagnostic messages move verbatim.
A commit is not done until G1–G5 pass.

## 6. The win
1. **Intra-crate codegen parallelism (the function-split win):** `compile_func_inner` is now ~2,670 lines instead of the original ~39K-line god-file center, and each extracted `handle_*_op` is its own codegen unit. The shell now holds orchestration, shared setup, and epilogue logic; the large M1 opcode families codegen independently.
2. **Ownership-collision blast-radius (headline friction win, now):** the #1 god-file collision source is materially smaller. The dominant opcode families now live in independently-owned `fc/*.rs` handlers; an arith fix touches only `fc/arith.rs` (~4K) + a 1-line dispatch arm, while loop/subscript work touches `fc/loops.rs` or `fc/indexing.rs` instead of the monolith.
3. **Representation-helper authority:** scalar raw/boxed carriers, merge-rebind storage, live-through params, guarded bitwise, and float transport helpers now live in one private helper module instead of being buried in the shell above `compile_func_inner`.

## Critical files
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (shell + `compile_func_inner` 2042-4715)
- `runtime/molt-backend/src/native_backend/function_compiler/scalar_carriers.rs` (shared raw/boxed scalar carrier, merge-rebind, live-through, guarded bitwise, and float transport helpers)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/mod.rs` (register families; `OpFlow`; shared `var_get_boxed_overflow_safe_fn`)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/const_literals.rs` (constant/literal materialization, loop-entry constants, heap-literal hoists, and string slot exports for module ops)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/runtime_ops.rs` (runtime state probes and side-effecting runtime helper calls)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/value_transfer.rs` (explicit refcount, release, conversion-alias, identity-alias, and binding-alias custody transfer)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/exception_control.rs` (exception control transfer, fallthrough sealing, tracked cleanup drainage)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/sequence_ops.rs` (sequence/iterator handler, `skip_ops`-owned iterator fusions)
- `runtime/molt-backend/src/native_backend/function_compiler/fc/list_ops.rs` (reference template: handler signature, split-borrow params, closure reconstruction, `OpFlow`)
- `runtime/molt-backend/src/native_backend/mod.rs` (the `use super::*` ancestry — do not change)
- `docs/design/foundation/dx_baseline.md` (§3.3/§4/§6 rationale, §9 gates)
