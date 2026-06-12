# State-repair verdict: finalizer DISPATCH is SHIPPED on origin — NOT split-brain (2026-06-09)

Triggered by the directive "reconcile the local-only finalizer-dispatch commits before #58."
Verified against current `origin/main` = **a2fbab1a2** with a freshly-built `fin-probe`
session (no `--rebuild`; warm session).

## VERDICT: the 3 local commits are STALE / SUPERSEDED. Do NOT integrate them.

Their CONTENT shipped to origin under different SHAs via the worktree-at-origin
re-push pattern (local main `bc6…` is SHA-divergent; supervisor re-verified + pushed
equivalents). Commit-level reconciliation (test cherry-pick onto a fresh origin/main worktree):

| local commit | what it was | reconcile result |
|---|---|---|
| `fe951364d` §1b drop-insert dead-result (zero-use owned) | dead-result drop | cherry-pick residual **NONE** → already on origin |
| `26feda8b8` finalizer-dispatch contract matrix test | test | residual **NONE** → `tests/differential/basic/finalizer_matrix.py` already on origin |
| `08a8cf5a0` never lifetime-optimize `__del__` instances (`finalizer_alloc_roots`/`defines_del`/`_class_constructor_fold_safe`) | the real thread-1 fix | cherry-pick **CONFLICTS** in `escape_analysis.rs` + `classes.py`, net **0 files changed** → origin has a REFINED/divergent equivalent (worktree-at-origin re-push) |

## FUNCTIONAL PROOF (the ground truth, not the SHAs) — origin a2fbab1a2

`finalizer_matrix.py` (the committed dispatch contract) is **BYTE-IDENTICAL to CPython 3.14**
on origin, all sections:
`del_statement [11] / scope_exit [22] / reassignment [31,32] / after_first ['del'] 1 /
after_second ['del'] 0 / raise_in_del ['del-enter'] survived / container_hold [...] /
many [1,2,3] / with_state [...]`.
Plus standalone confirmations (build+safe_run vs CPython, STDOUT MATCH):
* `fires_once` — `__del__` runs exactly once ✓
* `resurrect_once` — resurrecting `__del__` runs once (HEADER_FLAG_FINALIZER_RAN) ✓
* `no_del_plain` — non-`__del__` class unchanged, no regression ✓

→ **Dispatch ("does `__del__` run, once, with resurrection") is fully landed on origin.**
   Every downstream finalizer/RC/native-drop/NO_LEAK claim rests on a PRESENT foundation,
   not a missing one. The split-brain concern is REFUTED.

## NEW BUG CONFIRMED LIVE on origin — #65 exception-swallow, WORSE than described + GATE-MASKED

Task #65 ("exception-swallow is composition-dependent") is LIVE on origin AND the minimal
standalone case fails (not only "when other finalizer classes precede it"):

Repro (CPython 3.14: stderr "Exception ignored…", stdout `after del, still alive`, **exit 0**;
molt: **exit 1**, `ValueError` PROPAGATES):
```python
class A:
    def __del__(self): raise ValueError("boom in del")
def run():
    x = A(); del x
run(); print("after del, still alive")
```
Adding `gc.collect()` after `del x` does NOT fix it (still exit 1).

**Gate-soundness hole:** the *exact* `RaiseInDel` section of `finalizer_matrix.py`, extracted
and run STANDALONE, FAILS (exit 1) — yet inside the full matrix it prints `survived`. So
`finalizer_matrix.py`'s `raise_in_del` passes ONLY because 8 prior finalizer sections run
first (composition masks it). The dispatch contract test gives FALSE confidence on
exception-swallow. Fix lane must: (a) make inline-drop finalizer dispatch swallow `__del__`
exceptions like the GC path / CPython, and (b) add a SELF-CONTAINED exception-swallow
regression (or split the matrix so each section is standalone-valid).

## #58 finalizer ORDERING — LIVE on origin (separate arc, calls.py-blocked for full fix)
* `obj=D(11); log.append(obj.t); del obj` → molt `[('del',11),('use',11)]` vs CPython
  `[('use',11),('del',11)]` (drops at SSA last-READ, not the `del` point).
* scope-exit 2-object case worse (early drop + missing 2nd finalizer in output).
Root-cause locus CONFIRMED: `drop_insertion.rs` does NOT consult `finalizer_alloc_roots`
(only escape_analysis produces it + refcount_elim consumes it for the strip/Free gate) — so
the drop PLACEMENT pass is finalizer-unaware and keys every release on `last_use`. The
council-ratified fix = a minimal `ownership_boundaries.rs` mapping each `finalizer_alloc_roots`
member → its Python lifetime boundary (del-point / scope-exit), consumed by drop_insertion to
defer the release. The `defines_del` fact IS present on `ObjectNewBound` results on origin
(common statically-known-class case is NOT calls.py-blocked); the generic `type.__call__`
result sub-case loses the fact and IS calls.py-blocked (calls.py partner-dirty as of now).

## Cleanup
`memory/recovery/round12_wip/drop_insertion_dead_result_1b.patch` (= fe951364d, CONFIRMED
shipped) and `object_mod_immortal_probe.patch` (reverted diagnostic) are definitively
superseded — safe to delete. LEAVE `p59_call_path_wip.patch` (separate p59 IC WIP, not this arc).
