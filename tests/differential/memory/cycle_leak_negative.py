# MOLT_ENV: MOLT_ASSERT_NO_LEAK=1 MOLT_LEAK_TOLERANCE=8
# MOLT_META: expect_fail=molt expect_fail_reason=exact_leak_gauge_must_fire_on_unreachable_rc_cycle
# Post-teardown TRUE-LEAK gauge — NEGATIVE case (ownership_lattice_phase0.md §2.4).
#
# molt is reference-counted with NO cycle collector: formal/quint/molt_gc_safety.qnt
# proves absence of leaks for ACYCLIC object graphs only. An unreachable reference
# cycle is therefore molt's canonical leak class — RC pins each node at refcount 1
# (its peer holds the only reference), so nothing ever reclaims it. CPython's cyclic
# gc WOULD collect it; molt leaks it.
#
# The coarse pre-teardown 200K ceiling launders this (the cycle is tiny), but the
# post-teardown exact-survivor gauge — which runs after teardown has reclaimed every
# reachable acyclic graph — sees the cycle nodes as un-reclaimed survivors and fires.
#
#   MOLT_ASSERT_NO_LEAK=1 MOLT_LEAK_TOLERANCE=8 <binary>  -> exit 137  (gauge FIRES)
#   MOLT_ASSERT_NO_LEAK=1 <binary>                        -> exit 0    (200K ceiling launders)
#
# Stdout is byte-identical to CPython (parity preserved); the leak is exit-code-only.
# This is the falsification artifact: WITHOUT the post-teardown gauge the leak is
# invisible; WITH it (exact mode) the gauge fails. That delta IS the acceptance
# (CLAUDE.md: prove the gate fails on a synthetic violation).


class _Node:
    __slots__ = ("peer",)

    def __init__(self) -> None:
        self.peer = None


def _make_unreachable_cycles(count: int) -> int:
    made = 0
    for _ in range(count):
        a = _Node()
        b = _Node()
        a.peer = b
        b.peer = a  # 2-cycle: a -> b -> a, both become unreachable at loop end
        made += 1
        # `a` and `b` leave scope here. RC drops the stack references, but each
        # node's refcount stays at 1 (held by its peer). No cycle collector exists,
        # so the pair is never reclaimed — a genuine leak.
    return made


n = _make_unreachable_cycles(64)
print("created", n, "unreachable reference cycles (leaked: no cycle collector)")
