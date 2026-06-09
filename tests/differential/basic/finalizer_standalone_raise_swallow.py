"""Purpose: a STANDALONE __del__ that RAISES must have its exception swallowed
(written unraisable to stderr, NEVER propagated) and the program must continue —
the #65 contract.

Regression: molt's exception model is value-based, but `molt_raise` runs an
uncaught-exception terminator (`std::process::exit(1)`) when no handler frame is
on the stack. A finalizer at a plain rc->0 point runs with an EMPTY handler
stack, so the FIRST standalone raising __del__ killed the whole process before
the swallow could run. The bug was composition-dependent: a raising finalizer
"survived" only when a surrounding try/except (or a prior finalizer) happened to
leave a handler frame on the stack. The fix runs __del__ under a synthetic
handler frame so the raise is always recorded value-based and swallowed.

Drops go through the function-return path (a function-local owned object released
at return), which avoids the unrelated #63 (loop-body `del`) and #86 (object-
valued-attribute cascade) DecRef-PLACEMENT gaps. stderr (the unraisable text plus
a non-deterministic address) is not compared by the differential harness; stdout
and the exit code (0) are the contract.
"""

progress = []


class Boom:
    def __init__(self, tag: int) -> None:
        self.tag = tag

    def __del__(self) -> None:
        raise ValueError("boom " + str(self.tag))


def drop_one(tag: int) -> None:
    Boom(tag)  # owned, released at function return -> __del__ raises -> swallowed


# 1. A single standalone raising finalizer: the program continues past it.
drop_one(1)
progress.append("after-first")

# 2. Several in sequence — composition independence: the FIRST raising finalizer
#    must be swallowed too, not only ones that run after a non-raising finalizer.
drop_one(2)
drop_one(3)
drop_one(4)
progress.append("after-sequence")

# 3. A raising finalizer must not disturb a later surrounding computation.
total = 0
for i in range(5):
    drop_one(100 + i)
    total += i
progress.append(("total", total))

# 4. A non-raising finalizer still runs normally alongside the raising ones.
seen = []


class Quiet:
    def __init__(self, tag: int) -> None:
        self.tag = tag

    def __del__(self) -> None:
        seen.append(self.tag)


def drop_quiet(tag: int) -> None:
    Quiet(tag)


drop_quiet(7)
drop_one(8)
drop_quiet(9)
progress.append(("seen", sorted(seen)))

print(progress)
print("done")
