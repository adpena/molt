# P45 — silent miscompile: `for` nested in `with` runs ONCE instead of N

## Repro (CPython 3, molt native 1)
```python
class CM:
    def __enter__(self): return self
    def __exit__(self, *a): return False
n=0
with CM():
    for i in range(3):
        with CM():
            n+=1
print("TOTAL", n)   # CPython: 3 ; molt native: 1
```
Also wrong in a `def` (CPython 5 / molt 1). Simple non-nested `for`+`with` is CORRECT.

## Root cause — the back-edge is UNREACHABLE in the *pre-optimization* TIR

The refined diagnosis in the task brief (a TIR optimization pass relocating /
dropping the inner for-loop back-edge) is **WRONG**. The back-edge is already
mis-wired in the **PRE-OPT** TIR — i.e. immediately after `lower_from_simple`
(SIR→TIR CFG/SSA construction), BEFORE any optimization pass runs. The fault is
in SIR→TIR CFG construction (or in the frontend SIR block stream it consumes),
not in `range_devirt` / `iter_devirt` / `licm` / `sccp` / `dce` / etc.

Evidence (`MOLT_DUMP_IR=control:p45_nested__molt_module_chunk_1`, "pre" dump for
`p45_nested__molt_module_chunk_1`, `loop_roles={BlockId(2): LoopHeader}`):

PRE-OPT TIR CFG (Discriminant 0=Branch, 1=CondBranch, 3=Return):
```
block 2  LoopHeader   → 3
block 3               → 4
block 4  CondBranch cond=124  then=18 (body)  else=5 (loop-exit)
   ... loop body (inner `with`) ...
block 18 (try-body, InplaceAdd n)        → 19
block 17 (inner-with __exit__ join)      → 19
block 19 (inner-with exit cont + loop INCREMENT `Add→224`) → 20   <-- BUG
block 20 (print "TOTAL", n / function tail) → 21 → 22 → 25 Return
block 26 (loop latch carriers, 14 args)  → 27
block 27 (latch copies)                  → 2   (THE BACK-EDGE)   <-- UNREACHABLE
```

`block 27 → block 2` is the genuine loop back-edge (re-supplies all 14
loop-carried args to the header). But **nothing branches to block 26** (its sole
intended predecessor), so the entire back-edge chain `26 → 27 → 2` is dead. The
loop body's normal continuation (block 19, which even computes the next
induction value `Add → ValueId(224)`) instead falls into **block 20, the
print/function-exit**. Net effect: body executes once, increments, then prints
and returns — `TOTAL 1`.

The correct (simple, non-nested) case keeps the body→latch→header edge live and
`lower_to_simple` re-emits a clean structured loop
(`loop_start / loop_break_if_false / … / loop_continue / loop_end`); the nested
case bails to the generic block-by-block path and there the (already dead) latch
is dropped, so the final TIR shows `loop_index_next` with NO `loop_continue` /
`loop_end` and a body that jumps to the exit label after one trip.

## Why nesting matters
The inner `with` introduces an exception-cleanup diamond (try/__exit__/raise)
inside the loop body. The block that joins the inner-with's normal-exit and
exception-exit paths (block 17 → 19) is the same block that must carry the loop
increment AND route to the latch (block 26). SIR→TIR construction wires that
join's successor to the *function-tail* (print) block instead of the loop latch,
orphaning the latch. A non-nested body has no such cleanup join, so its
body→latch edge is wired correctly.

## Exact location (to fix)
- SIR→TIR CFG/SSA construction: `runtime/molt-tir/src/tir/lower_from_simple.rs`
  (CFG build at :115 `CFG::build`, SSA at :120, assembly at :123) and/or the
  CFG builder `runtime/molt-tir/src/tir/cfg.rs`.
- Candidate upstream cause: the frontend SIR block/label stream for
  `with`-wrapping-`for` mis-targets the loop back-edge label when the loop body
  ends inside a `with` cleanup join (the `loop_continue`/latch label vs the
  with-exit/after label). Pending confirmation via a raw pre-`lower_from_simple`
  SIR dump.

## Fix discipline
Preserve the body→latch→header back-edge for arbitrary `with`/`for` nesting at
the point of construction (one structural fix), then verify the bug-class
variants (break/continue/exception-in-body/exception-in-__exit__/
nested-with-in-loop/loop-in-with/with-in-loop-in-with) all match CPython.

## Status
- Confirmed: back-edge dead in PRE-OPT TIR (construction-time bug, not a pass).
- Next: raw frontend-SIR dump to localize construction vs frontend, then the
  structural fix + differential tests.

## UPDATE — root cause is the PYTHON MIDEND, not SIR→TIR construction

Frontend `emit` produces the CORRECT op order (verified by tracing
`SimpleTIRGenerator.emit`):
`LOOP_INDEX_NEXT → LOOP_CONTINUE → LOOP_END` (emit_range_loop_body, init.py:8590-8598).

In the retired pre-midend snapshot mode, the `to_json()` ops were correct:
`… loop_index_next, loop_continue, loop_end`.
With the midend ENABLED they are corrupted:
`… loop_end, loop_index_next` — `loop_continue` DELETED, `loop_end` hoisted
ABOVE `loop_index_next`. That orphans the back-edge (latch becomes unreachable),
so the body runs once.

Bisected the midend sub-passes: the corrupting pass is
**`SimpleTIRGenerator._ensure_structural_cfg_validity`**
(`src/molt/frontend/__init__.py:24805`). Exact transition it produces:
```
before: LOOP_START, LOOP_INDEX_START, LOOP_BREAK_IF_FALSE, LOOP_INDEX_NEXT, LOOP_CONTINUE, LOOP_END
after:  LOOP_START, LOOP_INDEX_START, LOOP_BREAK_IF_FALSE, LOOP_END,        LOOP_INDEX_NEXT
```
i.e. it drops `LOOP_CONTINUE` and moves `LOOP_END` before `LOOP_INDEX_NEXT`.

Fix target: `_ensure_structural_cfg_validity` must preserve the
`LOOP_INDEX_NEXT → LOOP_CONTINUE → LOOP_END` back-edge tail when a loop body
ends inside a `with` (nested exception-scope) region. Non-nested loops are not
hit because their body has no exception-scope structure that confuses this pass.
