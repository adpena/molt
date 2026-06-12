# MOLT_META: expect_fail=molt expect_fail_reason=native_value_tracking_never_releases_loop_body_call_result_temp_task63
"""Purpose: #63 — a per-iteration constructor result appended to a container
inside a loop must still be RELEASED by the temp's owner (the append increfs;
the call-result +1 must be balanced), so the container's later ``clear()``
drops each element to rc->0 and fires ``__del__``.

CPython prints ``entries 100``; the dormant-native value-tracking lane never
releases the loop-body ``B()`` call-result temporary, so the elements stay
above zero after ``clear()`` and the log stays empty (``entries 0``). This is
the c_loopapp module-free isolation repro (doc 50) promoted to a durable
known-bad anchor — a DEBT WITH AN OWNER (task #63), not an accepted state.
Distinct from #58 (ordering) and from the round-13 drop-lane fix.
"""

log = []


class B:
    def __del__(self) -> None:
        log.append(1)


def run(n: int) -> None:
    bag = []
    for _ in range(n):
        bag.append(B())
    bag.clear()


run(100)
print("entries", len(log))
