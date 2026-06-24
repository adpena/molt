"""Purpose: #63 — a per-iteration constructor result appended to a container
inside a loop must still be RELEASED by the temp's owner (the append increfs;
the call-result +1 must be balanced), so the container's later ``clear()``
drops each element to rc->0 and fires ``__del__``.

CPython prints ``entries 100``; molt now matches it with a clean exit under
``MOLT_ASSERT_NO_LEAK``.

STATUS (2026-06-24): PASSES — a regression guard, no longer a known-bad anchor.
The historical gap: on the dormant native value-tracking lane the loop-body
``B()`` call-result temporary had its last use extended to function return
(Swift-ARC "release at func_end") and so was never released per iteration; the
elements stayed above zero after ``clear()`` and the log stayed empty
(``entries 0``). Fixed by flipping native onto the TIR drop-insertion lane —
the round-10/11/12 native-drop arc (Blocker B loop body/exit polarity derived
from the CFG in ``13ecbdb16``) merged via ``df8f080d0`` — which retires the
value-tracking lane and DecRefs the temp at its true last use (right after the
``append`` consumes it), inside the loop body. Distinct from #58 (ordering) and
from the round-13 drop-lane fix.
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
