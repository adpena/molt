# MOLT_ENV: MOLT_ASSERT_NO_LEAK=1 MOLT_LEAK_TOLERANCE=8
# Post-teardown TRUE-LEAK gauge: cycle collector positive case.
#
# The historical RC-only negative expected Molt's exact leak gauge to fire on
# unreachable cycles. The cyclic collector retires that duplicate truth: the same
# exact-gauge invocation must now exit cleanly after gc.collect() reclaims cycles.

import gc


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
    return made


n = _make_unreachable_cycles(64)
collected = gc.collect()
print("created", n, "unreachable reference cycles")
print("collected_is_int", isinstance(collected, int))
print("collected_at_least_nodes", collected >= n * 2)
if collected < n * 2:
    raise AssertionError(f"cycle collector reclaimed {collected}, expected at least {n * 2}")
