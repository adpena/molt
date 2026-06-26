# Baton: conditional-`del` alias-group double-release (silent wrong value)

**Status:** root-caused precisely; fix designed, NOT yet implemented.
**Severity:** P0 — silent WRONG ANSWER (memory corruption surfacing as a wrong value).

## Repro

`tests/differential/memory/alias_reassign_conditional_del.py` → Molt prints `219`,
CPython prints `123`. (`x[-3:]` of the accumulator; the last three appended chars
are `1`,`2`,`3` for i=57,58,59. `219` is reused-freed-memory garbage = double-free.)

```python
def aliased_with_del(n):
    x = "s"; i = 0
    while i < n:
        y = x                 # y aliases x  (alias GROUP {x_phi, y}, ONE owned +1)
        x = y + str(i % 7)    # x = NEW string; old accumulator now reachable only via y
        if i % 2 == 0:
            del y             # CONDITIONAL path-authoritative release of the group
        i = i + 1
    return x[-3:]
```

## Root cause (file:line)

`runtime/molt-tir/src/tir/passes/drop_insertion/runner.rs`, §1 last-use placement
(lines ~1366–1417) and its del-suppression guard (line 1378
`python_lifetime_facts.has_explicit_release_boundary(v)`).

- The alias group `{x_phi (accumulator), y=Copy(x_phi)}` owns exactly ONE `+1`.
- `x_phi`'s last DIRECT use is `x_new = y + str(...)` in the loop body block, BEFORE
  the `CondBranch`. So §1 places `x_phi`'s drop **pre-branch** — a single point that
  executes on BOTH the even and odd paths.
- The suppression at line 1378 only fires when the root has an **unconditional**
  explicit-release boundary. The `del y` here is **conditional** (even branch only),
  so it does NOT suppress `x_phi`'s pre-branch drop.
- Result on the even path: pre-branch drop releases the group, then `del y` releases
  the SAME group again → **double-free** → memory reused → `219`. (Odd path: the
  single pre-branch drop is correct there.)

### Why §3 edge-dying (per-path, rail-#3 del-aware) does NOT catch it
The §3 edge-dying rule (runner.rs ~1643) places per-successor-entry drops and
already excludes del-released roots (rail #3), which is exactly what's needed. BUT
§3 only handles values **live-out** of the body block. `x_phi` is dead after its
direct last use, so it is NOT live-out → §1 (pre-branch) owns it instead of §3.
The missing fact: the alias `y` is used (by `del y`) in a SUCCESSOR block, so in
alias-root liveness the ROOT `x_phi` SHOULD be live-out of the body block.

## The fix (structural — preferred)

Make the alias-root **live-out** computation alias-aware: a use of any alias of root
`v` in a successor block — INCLUDING a `DeleteVar`/`DelBoundary` on that alias — keeps
`v` live-out of the block holding `v`'s direct last use. Then `x_phi` is live-out of
the body → §3 edge-dying places its drop **per path**: dropped at the odd-branch
entry (dead there, not del'd), EXCLUDED at the even-branch (rail #3 — already
del-released). This realizes the "alias-GROUP live-out reasoning across blocks" the
test header specifies: the group's one `+1` is released exactly once on every path.

**Alternative (only if the liveness change proves too broad):** a targeted
del-in-successor pass — for each root del'd in a successor block whose §1 pre-branch
drop would precede the branch, suppress that pre-branch drop and emit edge-dying
drops at the successor entries where the root is dead AND not del-released. (This
duplicates §3's rail-#3 logic, so the liveness fix is cleaner.)

## Verification (mandatory — a wrong fix is double-free OR leak)
- `alias_reassign_conditional_del.py` → `123` on native AND LLVM.
- FULL `tests/differential/memory/` suite green — especially `cycle_leak_*`,
  `custom_object_loop_phi_retain`, the `alias_reassign_*` variants,
  `finalizer_resurrection_leak_gauge`. Bounded RSS (prove no leak introduced on the
  odd path).
- `drop_insertion` unit tests: `tests/core_rc.rs`, `tests/python_lifetimes.rs`.

## Note: second memory P0 found the same run
`tests/differential/memory/cycle_leak_clean_control.py` **OOMs** — the cycle
collector does not reclaim reference cycles yet (the GC gap; separate work).
