# Task #51 baton — gen.throw resumption (HARVEST verdict, 2026-06-06)

## Verdict on the 37-file WIP (worktree /tmp/wt_genthrow @ 8e6a0b63a, unmerged)
HARVEST: the design is RIGHT — Terminator::StateDispatch + CFG::state_resume_edges
(partner baton 7a9566b26 + doc 26 aligned; exhaustive ~30-site threading, no
matches!-default traps; lib suites 1128/1020/508 GREEN; fixes the LLVM
"does not dominate" class). NOT mergeable as-is: TWO blocking defects.

1. REGRESSES native generators e2e: generator_methods.py (plain iteration, no
   throw) -> [0]+crash vs baseline [0,1,2]. Lib tests green = the e2e coverage
   gap is real.
2. Does NOT fix the throw-drop: gen.throw() into yield-inside-try still drops
   the injected exception on BOTH backends ("throw tail" vs CPython
   "throw caught:boom").

## Root cause (precisely localized)
The WIP's resume-continuation BLOCK-BOUNDARY handling:
- compute_state_resume_edges (cfg.rs:850) maps state_yield{value:N} ->
  block_containing(ops, idx+1); the resulting dispatch is SCRAMBLED
  (state 9 -> ^bb17 = the FIRST yield's block).
- The frontend emits closure_load self[GEN_THROW_OFFSET=8]; is None; raise
  after EVERY state_yield (all 4 present in raw SimpleIR) — during
  lower_to_tir/SSA, 3 of 4 are LOST (exactly the in-try yields; the
  handler-yield's survives). molt_generator_throw (generators.rs:516) stores
  the injected exception in the GEN_THROW closure slot, NOT the pending flag,
  so the dropped check was the only conversion point -> silent drop.
- The LLVM Copy[_original_kind=unreachable] failure on every _poll is the SAME
  defect: is_block_ender(state_yield) strands a load-bearing `unreachable` op
  at the resume-continuation leader. Do NOT remove it in lower_to_simple —
  native function_compiler block-fill DEPENDS on it (tried; regressed native;
  reverted). Fix belongs in the re-lift/resume-edge boundary handling.
- Investigate: does the CFG-block-index -> TIR BlockId correspondence
  (ssa.rs:672, BlockId(bid as u32)) survive the is_block_ender(state_yield)
  split? Are in-try resume continuations merged/pruned?

## Coordination gate (IMPORTANT)
A partner is ACTIVELY building runtime/molt-backend generator_fusion.rs and
editing docs/design/foundation/26_real-async-generators.md — the same
machinery. RECONCILE before any further #51 work: the partner may land the
resumption redesign that subsumes this; the WIP's StateDispatch threading is
the harvestable asset either way.

## Repro + diagnostics
- genthrow_matrix.py (full matrix) + gt1.py (minimal) in this directory.
- CPython oracle (3.12/13/14 identical): first a / throw caught:boom / next tail.
- TIR dump knobs: add to BOTH _BACKEND_REQUEST_ENV_KNOBS and
  _BACKEND_DIAGNOSTIC_ENV_KNOBS in cli.py; MOLT_BACKEND_DAEMON=0 + nuke
  ~/Library/Caches/molt; MOLT_DUMP_FUNC_IR=<pat> + MOLT_DEBUG_ARTIFACT_DIR
  (raw SimpleIR), MOLT_DUMP_FINAL_FUNC_IR=<pat> (final native) — file-reliable.
  Build harness swallows backend stderr on success; --target llvm runs the
  feature-tagged molt-backend.llvm_native_backend binary.
- Baseline truth: origin/main native generators are CORRECT — diff WIP TIR
  against a main build to isolate the divergence.
