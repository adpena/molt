# Round-12b baton: finalizer DISPATCH fixed (committed); finalizer ORDERING is the remaining sub-arc

## What round-12b LANDED (committed to main, NOT pushed — supervisor re-verifies + pushes)

Three commits closing the baton's P0 (`__del__` never running on any drop lane):

* `fe951364d` — backend: drop-insert dead-result (zero-use) owned values (§1b).
  A zero-use owned heap result (`def f(): x=Demo(); del x`) was absent from
  `last_use` → no DecRef → finalizer never ran / leak. §1b drops it after its
  defining op with the §1 guard set; disjoint from §1 by the `last_use`
  ROOT-space check (no double-drop).
* `08a8cf5a0` — frontend+backend: never lifetime-optimize a `__del__`-bearing
  instance (thread-1). The REAL root cause of the production LLVM/WASM hole: the
  escape pass classified a non-escaping finalizer instance as `NoEscape` and
  (a) stripped its RC and/or (b) stack-promoted it to IMMORTAL → the rc=0
  transition never happens → `dec_ref_ptr` never reaches
  `maybe_run_object_finalizer`. Fix: `_class_constructor_fold_safe`=False for
  `__del__` classes (declines the whole fold/inline-init/field-fold/stack family)
  + `defines_del` threaded serialization.py→ir.rs→ssa.rs→escape_analysis.rs
  (`finalizer_alloc_roots` excludes them from stack-promote AND RC-strip,
  defense-in-depth).
* `26feda8b8` — test: finalizer-dispatch contract matrix.

THREAD 3 (DecRef bypassing the finalizer) is NOT a real bug — confirmed from
source: native `function_compiler.rs` and LLVM `lowering.rs` both lower
`OpCode::DecRef` through `molt_dec_ref_obj` → `molt_dec_ref` → `dec_ref_ptr` →
`maybe_run_object_finalizer` (object/mod.rs:1930). The release authority is
already single + finalizer-aware. The instinct in the round-12 baton was right
about the SYMPTOM but the mechanism was thread-1 (IMMORTAL early-return at
`dec_ref_ptr` line 1789 / RC stripped before any DecRef), not a second release
path.

### Per-test mechanism classification (empirical, R12 probe + TIR dumps)
* `finalizer_exit_semantics` (`item=Demo(1); del item`): thread-1 (native:
  stack-alloc→IMMORTAL, probe fired pre-fix; LLVM: RC-strip) + thread-2 (§1b
  drop for zero-use `item`). FIXED, byte-identical LLVM + flipped-native +
  dormant.
* `finalizer_resurrection_once`: same. FIXED. Run-once via `HEADER_FLAG_FINALIZER_RAN`.
* `object_finalizer_dict_class_lifetime` (dict-routed `item.tag=`): was already
  heap (dict-routed exclusion) but needed §1b for the zero-use `item`. FIXED.

### Gates (all green on the lanes round-12b owns)
* finalizer_exit_semantics + finalizer_resurrection_once + dict_class_lifetime +
  finalizer_matrix: BYTE-IDENTICAL on LLVM, temp-flipped native, dormant native.
* cargo -p molt-backend --features "llvm native-backend" --lib: 1103 passed, 0 warn.
* cargo -p molt-runtime --lib: 510 passed (16 ignored), 0 warn.
* peel (shift_overflow_matrix) 9/9 native + 9/9 llvm. compliance 46/46 (.venv -n2, 222s).
* Memory corpus RC/alias tripwires (rc_sites_*, alias_reassign_*,
  bigint_accumulator, string_concat, list_comprehension) byte-identical on LLVM
  with §1b — no over-release.
* NOT my regressions (verified pre-existing): `rc_sites_loop_break` Cranelift
  verifier panic ("block cannot be empty") on FLIPPED-native — STILL panics with
  §1b DISABLED (`R12_DISABLE_1B=1`), and PASSES on LLVM with §1b → it is a
  flipped-native lowering bug, the round-13 flip's domain. `glob_iglob_bounded_rss`
  = build-OOM (~2GB build, heavy). async_generator_* ×N = known bug #3
  (asend multi-suspend). class_prepare_decorator_order ×2 = known __prepare__ gap
  (task #50); my fold-gate code is unreachable for them (decorated/metaclass
  early-return precedes it).

## THE REMAINING SUB-ARC: finalizer ORDERING (eager-drop vs Python-scope drop)

DISPATCH is fixed (does __del__ run, once, with resurrection/exception-swallow).
A SEPARATE, deeper bug class remains: **a non-escaping `__del__` instance is
dropped at its SSA last-READ, not at the Python `del`-statement / scope-exit
point.** When the finalizer has a side effect ORDERED against a later statement,
molt fires it too early.

Repro (fails byte-identical today on LLVM AND flipped-native — it is drop-pass-
wide, NOT native-specific):
```python
log=[]
class D:
    def __init__(self,t): self.t=t
    def __del__(self): log.append(("del",self.t))
def run():
    obj=D(11)
    log.append(("use", obj.t))   # reads obj.t
    del obj                        # CPython drops HERE (after the append)
run(); print(log)
# CPython:  [('use',11), ('del',11)]
# molt:     [('del',11), ('use',11)]   <-- __del__ fires at obj.t's last READ,
#                                            before the log.append that consumes it
```
TIR evidence (`/tmp/r12_tir3/tir/r12_min__run_post.txt`): obj is `Call` result
v8; its last operand use is the `LoadAttr obj.t` (v16/v17); drop_insertion places
`DecRef v8` right after that LoadAttr — but the `log.append(("use",11))` Call that
consumes the loaded value comes LATER in program order, so `__del__` runs first.

Matrix sections that expose it (all FAIL with last-read drop, PASS with
Python-scope drop): `del_statement`, `scope_exit`, `reassignment`,
multi-object `ordering`. (The committed `finalizer_matrix.py` deliberately uses
order-stable summaries so it targets DISPATCH; restore the strict-ordering
sections when this arc lands — the original strict matrix is preserved in this
baton's git history at the first `finalizer_matrix.py` write if needed, or
reconstruct from the section list above.)

### Why it is a distinct, larger structural arc (not shippable in this session)
The correct semantics: a finalizer instance must be released at the Python
`del`-statement program point, or at function-return for pure scope-exit — NEVER
at SSA last-read. This needs Python-scope-aware drop placement for finalizer
objects, which spans:
1. The drop pass must DEFER a finalizer object's drop past its last read to the
   enclosing-scope boundary. But it lacks the `defines_del` fact on a generic
   `type.__call__` `Call` result (the constructor-fold gate routes `__del__`
   classes through the generic path precisely to avoid the field-fold, which
   LOSES the fact at TIR). Options: (a) thread `defines_del` onto the generic
   call-bind result too (needs the static class at the call site — calls.py,
   FORBIDDEN while partner-dirty), or (b) a runtime "this type has __del__"
   query the drop pass can't make at compile time, or (c) keep `ObjectNewBound`
   for `__del__` classes (revert the fold gate) so `defines_del` survives, then
   suppress the field-fold in the inlined __init__ — but the field-fold lives in
   calls.py `_try_inline_init_assigns` (FORBIDDEN).
2. `del obj` in a NON-`molt_main` function does not anchor the release to the
   `del` point. `molt_main` DOES (`__init__.py` `_emit_delete_name` lines
   13643-13689 emit an explicit DEC_REF at the del). Replicating that for regular
   functions (line 13703-13712) is in `__init__.py` (MINE, not forbidden) — BUT
   an explicit frontend `DecRef(obj)` makes obj's last_use the DecRef, so
   drop_insertion §1 would add a SECOND drop after it → double-free. So it must
   be coordinated: the drop pass must recognize an explicit-`del` DecRef as the
   ownership-consuming terminal use and NOT add its own (analogous to the
   existing `op_consumed_operand_root` CallArgs ownership-transfer rule).
3. Pure scope-exit (no `del`) needs func-return deferral, which is correct ONLY
   when no explicit `del` precedes; the two cases must be distinguished by the
   frontend (it knows both).

Escaping finalizer objects ALREADY work (matrix `container_hold` PASSES: an
instance held in a list drops at `box.clear()`, correctly) — proving the
deferred-drop machinery exists; the gap is purely the eager last-read drop for
NON-escaping finalizer locals.

### Suggested end-state for the ordering arc (one coherent structural change)
Treat a `defines_del` instance as "live to its Python-scope boundary": keep
`ObjectNewBound`+`defines_del` (do NOT route through the generic fold — instead
suppress ONLY the field-access fold for finalizer instances, which needs a
calls.py change → coordinate with the partner who owns calls.py, or land it when
calls.py is clean), and extend the drop pass so a `defines_del` root's drop is
placed at function-return (scope-exit) unless an explicit `del`-statement DecRef
(emitted by `_emit_delete_name` for regular functions, with §1 taught to treat it
as the terminal consumer) releases it earlier. That single change closes ordering
on every drop lane. Until then DISPATCH is correct and the production hole is shut.

## Inherited WIP files (now SUPERSEDED — safe to delete)
`memory/recovery/round12_wip/drop_insertion_dead_result_1b.patch` (landed as
`fe951364d`) and `.../object_mod_immortal_probe.patch` (diagnostic only, probe
removed; finding folded into `08a8cf5a0`). Left UNSTAGED; delete at will.

## Diagnostic aid added then REVERTED (not committed)
`MOLT_DUMP_IR_ALL=1` (dump TIR for non-loop functions too) was temporarily added
to pass_manager.rs and reverted — pass_manager.rs is byte-clean vs HEAD. The flip
wire (`NativeCranelift => true`) was temp-set for flipped-native verification and
reverted; pass_manager.rs diff vs HEAD is EMPTY.
