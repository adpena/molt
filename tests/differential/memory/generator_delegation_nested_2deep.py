# Nested generator delegation streaming-memory regression (task #46).
#
# Root cause (fixed): a generator/async `_poll` is a state machine that RETURNS
# ON EVERY YIELD and is re-entered on resume, so its "function return" is a yield
# SUSPENSION, not the generator's scope exit. The native value-tracking RC used a
# Swift-ARC "release at function return" model that extended every loop-carried
# value's lifetime to func_end. For a `_poll` that deferred per-iteration heap
# temporaries — chiefly the `(value, done)` pair tuple emitted right before each
# `state_yield` — to a return that never drains them on the suspend path. Each
# resume re-allocated and orphaned one ~40-byte tuple, so a generator DELEGATING
# to a sub-generator (`for y in inner(): yield ...`) leaked one tuple per element
# per delegation level: an unbounded O(iterations x depth) leak that OOMed at a
# few hundred MiB over a few hundred-thousand elements, while CPython streams it
# in O(active-chain-depth) memory.
#
# Fix: per-iteration temporaries in a `_poll` are released at their real last use
# (the suspend boundary), not deferred to func_end. A FLAT generator was already
# O(1); this test guards the 2-deep DELEGATION case.
#
# Run under:  python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Fixed     -> streams at O(1), stays far under 64 MiB (exit 0).
# Regressed -> leaks one pair tuple per element; RSS climbs unbounded / cap trips.
#
# The printed summary is a single integer, byte-identical to CPython.
#
# NOTE: the generator is consumed by a plain `for` loop, NOT `itertools.islice`.
# `islice` (and the other itertools/functools consumers) route through a shared
# `iter_next_pair` helper that has a SEPARATE, pre-existing wrapper-tuple leak
# (tracked independently of task #46); consuming with a bare `for` loop isolates
# THIS test to the generator-delegation `_poll` fix.


def inner(n):
    for i in range(n):
        yield i


def outer(n):
    # Manual for-yield delegation (one of the two spellings; the other,
    # `yield from`, is covered by generator_delegation_yield_from.py).
    for y in inner(n):
        yield y + 1


def main() -> int:
    total = 0
    # 250k elements: far past the historical OOM threshold for the leak, while a
    # correct O(1) stream stays trivially under the cap.
    for v in outer(250_000):
        total += v
    return total


print(main())
