# MOLT_ENV: MOLT_ASSERT_NO_LEAK=1 MOLT_LEAK_TOLERANCE=8
# Post-teardown TRUE-LEAK gauge — POSITIVE control (ownership_lattice_phase0.md §2.4).
#
# Allocate-and-release ACYCLIC churn: 1000 dicts each holding a fresh list, every
# one dropped at the end of its iteration. RC reclaims each acyclic graph eagerly,
# so at post-teardown the survivor count returns to the immortal floor. The exact
# gauge MUST pass here — this proves it does not false-positive on ordinary programs
# that allocate heavily but leak nothing.
#
#   MOLT_ASSERT_NO_LEAK=1 MOLT_LEAK_TOLERANCE=8 <binary>  -> exit 0  (no leak)
#
# Paired with cycle_leak_negative.py: same exact-mode invocation, opposite verdict.
# The pair brackets the gauge — clean churn passes, an unreachable cycle fails.


def churn(n: int) -> int:
    total = 0
    for i in range(n):
        d = {"k": i, "v": [i, i + 1, i + 2]}  # acyclic; refcount hits 0 each iter
        total += d["k"] + d["v"][2]
    return total


print("churn total", churn(1000))
