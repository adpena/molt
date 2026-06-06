# Round-8 baton: DropInsertion native activation — the loop-phi representation bug

## Status after round-7
Round-7 landed the PIPELINE-ORDERING structural arc: drop insertion now runs in a
SEPARATE terminal phase (`tir::drop_phase::finalize_module_drops` /
`finalize_simple_ir_drops`), AFTER the E1 inliner + module_slot_promotion, on
every backend driver. This cleared the bench_calls 5× regression (verified:
`inc` inlines, the module-global loop promotes 2 slots, runtime ≈ dormant 0.21s).

Native drop insertion is STILL GATED OFF (`target_uses_tir_drop_insertion`
NativeCranelift => false). The flip is blocked by the bug below.

## The blocker (drops-CAUSED, NOT ordering, NOT round-7)
Flipping native on surfaces a PRE-EXISTING drop-insertion correctness bug: a
loop block-arg (TIR phi) gets an INCONSISTENT REPRESENTATION (raw i64 on one
edge, boxed heap as the phi's declared/used type). Variable-keyed backends reject
it; the value-keyed LLVM backend tolerates it.

Evidence (all at the round-7 BASE commit `8dcb6ed33`, before round-7 — so this is
not introduced by round-7):
- **WASM** (drops ON in baseline): `bench_counter_words` FAILS structural
  validation: "func N failed to validate: type mismatch: expected i64 but nothing
  on stack". Reproduced on `8dcb6ed33` with the stash popped (clean base).
- **Native** (when flipped on): Cranelift codegen panic
  `simple_backend.rs:1224`: "native variable representation mismatch for
  _bb7_arg0: value vN has CLIF type i64; the types of variable 0 and value N are
  not the same."
- **LLVM** (drops ON in baseline): builds and runs CORRECTLY -> 97360. Localizes
  the bug to the drop pass's loop-phi representation handling, not the ordering.

## Minimal repro (saved: tmp/round8/dropbug_counter_loop_phi.py)
```python
from collections import Counter
def main() -> None:
    words = ["a", "b", "a", "c", "b", "a"]
    total = 0
    outer = 0
    while outer < 5:
        counts = Counter(words)   # heap object, loop-carried, dies at back-edge
        total += counts["a"]
        total += len(counts)
        outer += 1
    print(total)
main()
```
Reduction findings:
- Needs `collections.Counter(...)` specifically. `list(words)`, `dict(a=outer)`,
  and `{...}` dict literals loop-carried do NOT trigger it (they build/validate
  fine). So the trigger is the Counter constructor's lowering shape, not "any
  heap value loop-carried".
- Triggers even when `counts` is created but never read in the loop body
  (`total += 1`). So it is the loop-CARRIED dead-at-back-edge Counter object that
  the drop pass mishandles, independent of how `counts` is consumed.

## Root-cause pointer (from the drop debug dump)
`MOLT_DEBUG_DROP=ALL` on the WASM build dumps
`tmp/molt-backend/drop/repro_e__molt_user_main.txt`. The loop is `bb2(20, 21)`
with the back-edge from `bb4` passing `[41, 42]`. Block-arg **21 is used as
`[21:heap]`** (e.g. `Copy ops=[21] -> [23] [21:heap]` in bb3) but the loop-ENTRY
edge `bb1 -> bb2 args=[18, 19]` delivers value **19**, which is `Copy ops=[11,11]
-> [19]` of a RAW value (`11:raw`). So the phi expects heap on entry but receives
raw -> representation mismatch on the loop-carried block-arg.

This is the "phi-representation invariant" class. The drop pass (or its
interaction with type_refine over the loop phi) is producing/relying on a phi
whose incoming reprs are not unified. The §5 mixed-ownership-phi retain
(before_term_incref / edge-split) and the loop-carried drop (§2.7) both touch this
phi; the fix must keep the dropped/retained loop block-arg's repr consistent
across entry and back-edge so variable-keyed backends (native Cranelift, WASM)
lower it without a CLIF/stack type mismatch.

## What round-8 must do (structural, per CLAUDE.md)
1. Diagnose whether the inconsistency originates in (a) drop_insertion's
   IncRef/retain placement on the loop-entry edge that forces a heap view, or
   (b) type_refine assigning the phi a heap type while one incoming arc is a raw
   int, or (c) the entry-edge `Copy[11,11]->19` itself being mis-repr'd. The
   `Copy ops=[11,11]` (two operands to a Copy) is suspicious — likely a
   tuple/multi-value copy or a mis-lowered phi-arg materialization.
2. Fix the drop pass so a dropped/retained loop block-arg has a SINGLE consistent
   representation across all incoming edges (unify to the boxed/owned repr, or
   refuse to treat a mixed-repr phi as droppable — fail-closed).
3. Re-verify on WASM FIRST (it fails today at base, so it is the cheapest oracle —
   no flip needed): `MOLT_BACKEND=wasm molt build` of the repro + bench_counter_words
   must validate and print 97360.
4. THEN flip native and run the full round-7 activation protocol (bench table,
   RSS table, serial differential sweep). bench_calls is already clearing
   promotion, so once this phi-repr bug is fixed the native flip should be clean.

## Verified-green with round-7 (dormant) — do not re-litigate
- Rust gates: native lib 1020, native+llvm lib 1089, runtime 508, clippy x2 clean.
- Differential memory regression set (design-20): native-dormant 14/14, WASM
  14/14, LLVM 14/14 (the moved pass is byte-identical-to-CPython on all lanes).
- Honesty guard green.
