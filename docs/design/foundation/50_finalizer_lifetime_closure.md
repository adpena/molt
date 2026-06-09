# 50 — Finalizer Lifetime Closure (macro-tranche map)

Status: ACTIVE (2026-06-09). The finalizer/lifetime work is ONE vertical, not a pile
of leaf bugs. This doc is the macro-tranche map: the layered model, what is closed,
and the precisely-isolated open slices (each a structural arc, not a patch).

## The vertical (a finalizer-sensitive heap object's lifecycle)
1. **PLACEMENT** — the instance reaches `dec_ref_ptr` rc→0 when CPython would make it
   unreachable. (#87 / #63 — OPEN)
2. **TIMING / ORDERING** — it reaches rc→0 at the Python-visible lifetime boundary
   (`del` / reassignment / scope-exit / exception-unwind / loop-backedge / container
   removal), NOT at SSA-last-read. (#58 — OPEN)
3. **EXECUTION** — at rc→0, `__del__` runs exactly once; a raise is swallowed
   (unraisable); resurrection stops destruction. (#65 + dispatch — CLOSED, doc 48)
4. **FIELD RELEASE** — inline object-valued fields are released exactly once by the one
   runtime authority. (#86 — CLOSED, doc 49)
5. **CHILD FINALIZATION** — released children's `__del__` run (falls out of 3+4 once 1+2
   hold).

Layers 3 and 4 are CLOSED and green (native + LLVM). The open work is layers 1 and 2.

## Diagnosis (module-free isolation, 2026-06-09 — what #87 actually is)
`#87` was three distinct bugs. Isolated repros (`/tmp/fr/c_*.py`), native, vs CPython 3.14:
- `c_single` — `bag=[]; bag.append(A()); bag.clear()` → **MATCH**. Container element
  release on `clear()` WORKS. (Not a bug; pins the boundary.)
- `c_scope` — `bag=[A()]` held by a local, dropped at function return → **#58**. molt
  prints `DEL` BEFORE the later `print` (drops `bag` at its SSA-last-use = the
  assignment, since `bag` is never read again), CPython drops at scope exit. Finalizer
  fires too early.
- `c_loopapp` — `for i: bag.append(B())` then `bag.clear()` → **#63**. `entries 0`: the
  per-iteration `B()` call-result temporary is never released on dormant-native (so the
  list-held element never reaches rc 0 on `clear()`).
- `m86_dc` — `@dataclass` instance via function-return drop → instance never freed
  (`dealloc_object=0`), separate; partially module-muddied (the `dataclasses` import
  dominates the counters). Needs a module-free dataclass repro.

## Open slice A — #58 ORDERING (the keystone; council-mandated approach)
**Bug:** a finalizer-sensitive instance drops at its SSA last-READ, not the Python
`del`/scope-exit point. Drop-pass-wide (LLVM + flipped-native + dormant). Round-12b TIR
evidence (`/tmp/r12_tir3/...`): `drop_insertion` places `DecRef v8` right after the last
`LoadAttr`, but the consuming Call comes later in program order → `__del__` runs first.
**Approach (binding, CLAUDE.md council doctrine):** build the smallest slice of a minimal
OWNERSHIP LATTICE — `alias-root → ownership state → Python lifetime boundary → ordered
release obligation` (new `ownership_lattice_min.rs` / `ownership_boundaries.rs`) — and ship
ordering on it. NOT another DropInsertion special-case. `FinalizerSensitive` is one
ClassInfo/MRO/version-derived cached fact consumed by escape + refcount-elim + stack-alloc
+ Free-eligibility + this ordering. Non-finalizer objects KEEP SSA-last-use (no perf loss).
Matrix sections it unblocks: `del_statement`, `scope_exit`, `reassignment`, exception-unwind.

## Open slice B — #63 loop-body PLACEMENT (dormant-native value-tracking)
**Bug:** a per-iteration owned call-result temporary (`bag.append(B())`, `for i: x=R(i); del x`)
is not released on dormant-native; the object never reaches rc 0. The round-13 §1b fix
(`fe951364d`/drop_insertion) covers the DROP LANES (LLVM / flipped-native), NOT dormant
native, which uses the `function_compiler.rs` value-tracking substrate. Fix lives in the
value-tracking's per-iteration last-use handling for loop-body owned temporaries consumed
by a call (the `Transferred` operand-ownership, #70b).

## Open slice C — dataclass instance never freed
`@dataclass` instances reach the runtime free path's TYPE_ID_DATACLASS arm only if freed;
m86_dc shows `dealloc_object=0` → not freed. Get a module-free repro (define `__init__`
manually mimicking a dataclass, or measure the dataclass instance's RC directly) to split
"dataclass lowering keeps an extra owner" from "drop not placed." Likely overlaps A/B.

## Acceptance for the macro-tranche
Instance reaches rc→0 where CPython would make it unreachable (#87/#63); finalizer-
sensitive objects drop at the Python lifetime boundary (#58); #86 field-release + #65
swallow stay green; one consolidated finalizer matrix (once / raises / resurrect /
inherited / instance-`__del__`-ignored-if-type-level / child-attr release / dataclass /
container-clear / del-reassign-scope-exit-unwind timing / primitive fast path);
native + LLVM; a POINTER-FIELD heap-free microbench (not `bench_struct`, which bypasses the
HAS_PTRS path); NO duplicate field-release authority (doc 49).

## Commit ladder (small commits, large mission)
1. invariant (doc 49) + this map (doc 50) + diagnosis. ← here
2. #58 ownership-lattice minimal slice (the keystone).
3. #63 dormant-native loop-body temporary release.
4. dataclass instance-drop (module-free repro → fix).
5. consolidated finalizer matrix + pointer-field heap-free microbench.
6. cleanup / delete any duplicate authority discovered.
