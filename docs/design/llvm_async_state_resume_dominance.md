# LLVM async/coroutine state-resume SSA-dominance class — diagnosis + fix design

Status: **diagnosed, fix direction validated (Part 1), NOT landed.** The complete
fix is a delicate rework of the LLVM `_poll` state-machine lowering that must be
differentially verified across native/wasm/llvm; this note is the baton. Nothing
is committed because a half-applied structural change (Part 1 without Part 2) is
worse than nothing — it changes the TIR block structure all backends consume and
leaves the LLVM async lane *more* broken (dominance + a const-resolution panic),
which the zero-workaround / complete-structural-change policy forbids.

## Symptom

On `--target llvm`, every async/coroutine test and the auto-generated
`builtins_symbol_*` cluster BUILDFAIL with the LLVM verifier error
`Instruction does not dominate all uses!`. Observed users: `%closure_load…`
feeding `molt_exception_stack_set_depth`, and phi nodes threading
`%exception_stack_enter` / `%exception_stack_depth` / the coroutine-self frame
pointer across `state_resume_*` blocks. Native + wasm compile and run the same
programs (they consume the SimpleIR round-trip, which re-materializes every value
through `load_var`/`store_var` slots — no SSA dominance is asked of them).
Minimal trigger: `await` + `try/except` in one function (or `yield` + `try` in a
plain generator).

## Root cause (definitive — a TIR/SSA construction bug, not an LLVM-only bug)

The `_poll` state machine re-enters at a saved state via the `state_switch`
dispatch: on resume, control jumps from the entry block's `state_switch`
*straight to the op after the suspend op* (`state_yield` / `state_transition` /
`chan_*_yield`) that established that state. **`tir/cfg.rs` does not model this
re-entrant control flow at all.** `is_terminator` / `is_block_leader` /
`is_block_ender` (cfg.rs:59-79) omit every state op, so:

- `state_switch` is treated as a plain straight-line op (falls through; **no
  edges to the resume points**);
- `state_yield` / `state_transition` / `chan_*_yield` are treated as plain
  straight-line ops (the resume continuation is modeled as ordinary
  fall-through, when in reality the suspend op `ret`s and the continuation is
  reached *only* via the dispatch).

So `ssa.rs` computes dominance and places block arguments (phis) on a CFG that is
**missing the dispatch re-entry edges**. The frontend threads values live across a
suspend through `_bbN_argK` `store_var`/`load_var` pairs (visible in the
SimpleIR; e.g. `store_var _bb7_arg1 ← v102` where `v102 = exception_stack_enter`);
the SSA pass turns those into block args. Because the dispatch edges are
invisible, a resume-reachable block ends up *using* a value (block arg / phi)
defined only on the linear first-entry path, which the dispatch bypasses → the
malformed TIR the LLVM lane lowers verbatim. Concretely (generator `yield`-in-try
repro, original TIR): `^bb22` uses `%phi_60` (defined in `^bb9`), but the
`state_switch → state_resume_10 → bb17 → … → bb22` dispatch path reaches `bb22`
without passing through `bb9`, so `bb9` does not dominate `bb22`.

This is the **exact analogue of the exception-handler-edge problem** that
`2a450ecfe` solved by folding `cfg.exception_edges` into the SSA pass's augmented
predecessor relation (`ssa.rs::build_augmented_cfg`). A suspend op `ret`s, so its
resume continuation has no *regular* predecessor — exactly like an exception
handler block — and the dispatch edge is the implicit re-entry that must be
folded into the augmented CFG.

### Evidence pointers (how to reproduce)

- Repros: an `async def` with `await asyncio.sleep(0)` inside `try/except`; a
  plain generator `def gen(): try: x = yield 1; yield x+10 / except ValueError:
  yield 99`. (The generator `.send()` value-capture is itself broken on native
  independent of this bug — use it for BUILDFAIL classification, not for the
  byte-identical oracle.)
- Dump the SimpleIR the LLVM lane lifts to TIR — instrument
  `native_backend/simple_backend.rs` just before `lower_to_tir(func)` (the
  per-function map at ~line 2955) to `eprintln!` `func.ops`. `state_yield` sits at
  op K with the resume continuation at op K+1; `state_switch` at op 3 has **no**
  outgoing edges to those continuations.
- Dump the TIR the LLVM lane receives — instrument the top of
  `llvm_backend/lowering.rs::try_lower_tir_to_llvm_with_pgo` with
  `crate::tir::printer::print_function(func)`. `^bb24` etc. reference block args
  threaded from the entry region across `state_switch`.
- `MOLT_LLVM_DUMP_IR=1` + `MOLT_DEBUG_ARTIFACT_DIR=…` writes `llvm/before_opt.ll`;
  grep the failing `_poll` function for `state_resume_` and the `phi` whose
  incoming value's def block does not dominate a `state_resume`-reachable use.
- Daemon caveat: the backend daemon snapshots the binary at start and only
  forwards env keys in `DAEMON_REQUEST_ENV_KEYS` (main.rs:42) — mirror new debug
  env keys into both that list and the CLI's `_BACKEND_REQUEST_ENV_KNOBS`
  (cli.py:178) or the dump never reaches the daemon. Kill your own
  `moltbd.*<session>` daemon after each rebuild so the fresh binary is picked up.

## Fix design

### Part 1 — model the dispatch CFG for SSA (VALIDATED: 1125 lib tests pass, 0 warn; reverted, not committed)

`tir/cfg.rs`:
- New `is_suspend_op` (`state_yield`/`state_transition`/`chan_send_yield`/
  `chan_recv_yield`) and `is_repoll_op` (the await/channel ops — they re-poll
  from their *own* position on resume, so they are also block leaders).
- `is_block_ender` ∪= `is_suspend_op` (op after a suspend op = resume-continuation
  leader). `is_block_leader` ∪= `is_repoll_op`.
- `build_edges`: `state_yield` → no successor (it `ret`s; resume is dispatch-only);
  `state_transition`/`chan_*_yield` → fall through to the next op only (the READY
  path's next-state continuation).
- New `CFG.state_resume_edges: Vec<(state_switch_block, resume_block)>` from
  `compute_state_resume_edges`: find the single `state_switch` block; for each
  suspend op add an edge to the block of op K+1 (post-yield / next-state
  continuation) and, for re-poll ops, to the block of op K itself (pending
  re-poll re-entry).

`tir/ssa.rs`:
- `build_augmented_cfg`: fold `state_resume_edges` into `aug_preds` (mirror the
  `exception_edges` loop exactly).
- `compute_live_in_vars(include_implicit_edges)`: also add `state_resume_edges` as
  liveness successors of the `state_switch` block (the dispatch supplies the
  resume block's live-in on re-entry — mirror the exception-handler edge).

Measured: generator `yield`-in-try repro LLVM dominance errors 18 → 0; all 1125
`molt-backend` lib tests pass; 0 warnings. **No shippable value alone** — async
still fails (Part 2). One limb of one structural change; do not land alone.

### Part 2 — LLVM lane must DISPATCH TO THE REAL RESUME BLOCKS and SUPPLY THEIR PHIS (the remaining, correctness-critical work)

After Part 1 splits the TIR blocks, the resume continuations are **real TIR
blocks** (in `block_map`), but `llvm_backend/lowering.rs` still creates *synthetic*
`state_resume_*` blocks (`initialize_state_resume_blocks`) and dispatches
`state_switch` to those now-empty blocks, while the real continuation TIR blocks
are reached only on the normal-flow path → their phis are missing the dispatch
incoming (post-Part-1 async failure: `%phi_71 = phi i64 [ 24, %bb10 ]` with no
entry for the `state_resume` predecessor). The `StateYield`/`StateTransition`/
`Chan*Yield` arms also `ret` then `position_at_end(synthetic_resume_bb)` and emit
the continuation inline — but the continuation is now its own TIR block lowered by
the main loop, so the synthetic block is dead and the reposition is wrong.

**The load-bearing obstacle (why this is a representation change, not an arm
tweak).** Splitting at the suspend op gives the suspend block NO regular
successor (it `ret`s), so the TIR **loses the link** from a suspend point to its
resume continuation. Today that link is implicit: the suspend op carries its
state id in `value` and the LLVM lowering re-establishes adjacency by
`position_at_end`-ing the synthetic `state_resume_<id>` block. Once the
continuation is a real block, the `state_switch` needs an explicit
`state_id → resume_BlockId` map, and that map must survive the TIR pass pipeline
(BlockIds are reassigned/merged by passes). Two ways to carry it:
- **Fragile**: stash the map as an attr on `state_switch` at SSA-build time and
  hope every pass leaves state-machine blocks intact. Most loop/opt passes *do*
  bail on `has_state_machine`/`has_exception_handlers`, but relying on that is the
  kind of cross-pass invariant the zero-tech-debt policy rejects (one future pass
  that renumbers a `_poll` block silently miscompiles).
- **Robust (recommended)**: make `state_switch` a **first-class multi-target
  dispatch terminator** — extend `tir/blocks.rs::Terminator` with a
  `StateDispatch { default, cases: Vec<(state_id, BlockId, args)> }` (shape mirrors
  `Switch`), built by the SSA terminator builder for the entry block, updated by
  passes exactly like any other terminator's block references, and lowered to the
  LLVM `switch` directly. This makes the dispatch edges *the same objects* the SSA
  pass already reasons about (the `state_resume_edges` become real terminator
  edges), so phi placement, `record_branch_args`, dominator updates, and
  block-renumbering passes all handle them for free — and the synthetic
  `state_resume_*` machinery + the `state_switch`/`state_yield`/`state_transition`
  reposition arms are deleted, not patched. This is the larger but correct unit of
  work; it also subsumes the dispatch-arg-supply below.

Required rework:
- `initialize_state_resume_blocks` → map each state-id to the **real** TIR block
  holding its resume op (post-yield continuation = block of op K+1; re-poll = the
  block of the `state_transition`/`chan_*_yield` op itself), not a fresh synthetic
  block.
- `StateSwitch` arm → build the `switch` to those real `block_map` blocks and, for
  each, supply its block-arg incomings (values live at the `state_switch` point).
  Only the SSA rename walk knows what flows on the dispatch edge, so encode it
  there: in `ssa.rs::translate_op` for `state_switch`, append per-resume-block
  `collect_branch_args(resume_bid)` (the way `check_exception` appends
  `collect_branch_args(handler_bid)` — ssa.rs:1148), with attr-encoded group
  boundaries (state_switch fans out to many blocks vs check_exception's one). The
  LLVM `StateSwitch` arm decodes the groups and calls `record_branch_args` per
  resume block (mirror the `CheckException` arm at lowering.rs:3934); `finalize_phis`
  then fills the resume-block phis on the dispatch edge.
- `StateYield`/`StateTransition`/`Chan*Yield` arms → drop the synthetic-block
  reposition. `StateYield` emits `ret` as the suspend block's exit; the LLVM
  lowering must SKIP that block's TIR terminator (the block is terminated by the
  `ret`). `StateTransition`/`Chan*Yield` keep their pending(`ret`)/ready split,
  but the ready path branches to the **real** next-state continuation TIR block.
- State-id const operands: after Part 1 the `pending_state` const is threaded
  through block args (the re-poll block gains the dispatch predecessor), so the
  syntactic `const_i64_operand` (lowering.rs:10148) panics. Resolve it through the
  SSA def-chain (direct `ConstInt`, `Copy`-forwarded, or a block arg whose
  terminator incomings all resolve to the same constant — a state id is constant
  on every edge). A scratch `try_const_i64_operand` removes the panic but is moot
  until the dispatch-supply lands (the phi is malformed regardless).

### Part 3 — verify the SimpleIR round-trip for native/wasm

Part 1 changes `cfg.blocks` (block-split at suspend points), which feeds the TIR
block structure `lower_to_simple` re-emits for native/wasm. `lower_to_simple`
already has an "external-reentry guard" (lower_to_simple.rs:635) that declines
structured-loop reconstruction when a resume `jump` re-enters a loop region from
outside, falling back to label-preserving lowering — the split should land in
that generic path, but confirm e2e (native + wasm suites, 20-sample native, the
memory corpus), not just by lib tests. Many async tests are already broken on
native independent of this work (e.g. `async_for_with_exception_propagation` →
`InvalidStateError` on stock `main`), so the byte-identical oracle is only
meaningful where native already passes — a constraint that makes verifying this
state-machine codegen rework genuinely hard and is the main reason it was batoned
rather than rushed.

## Scope discipline / connection to the drop pass

This arc is the **dominance class only**. The same `state_resume_edges` model is
the StateSwitch-aware liveness extension the RC drop-insertion pass deferred
(`tir/function.rs::has_state_machine` doc; `drop_insertion` bails on
`has_state_machine`). Do **not** scope-creep into drop insertion here — but once
`state_resume_edges` is a first-class CFG citizen, the drop pass can consume it to
become sound over `_poll` bodies (a separate follow-up).

## Gate checklist for the completed fix

`cargo test -p molt-backend --features llvm,native-backend,wasm-backend --lib`
(0 warnings); peel matrix 9/9 llvm + 9/9 native; `nested_try_handler_reraise` +
`inline_llvm_module_phase_activation` byte-identical on llvm; memory corpus green
on native; compliance `-n 4` zero failures; async/generator llvm BUILDFAIL count
for this class → 0 (60-file async/generator sweep + 25-file exception sweep); no
native/wasm regression (suites + 20-sample native). Commit prefix:
`backend: fix LLVM async/coroutine state-resume dominance class — <mechanism>`.
Add differential pins (must pass native baseline) for the minimal repros.
