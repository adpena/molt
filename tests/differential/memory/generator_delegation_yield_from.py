# `yield from` delegation streaming-memory regression (task #46).
#
# Companion to generator_delegation_nested_2deep.py — see that file for the full
# root-cause writeup. The `yield from inner()` spelling lowers differently from the
# manual `for y in inner(): yield y` spelling: the delegation `(value, done)` pair
# from `iter_next` is persisted in the generator's CLOSURE CELL across the suspend
# (`closure_store` writes it, the resume `closure_load`s it back). The task-#46
# suspend-boundary drain + per-iteration-temporary lifetime fix releases the OUTER
# yielded pair and the extracted value per-iteration, which bounds this 250k stream
# well under the cap (≈9 MiB).
#
# KNOWN RESIDUAL (separate follow-up, see memory baton): the closure-cell pair
# round-trip still leaks ~1 wrapper tuple per element at very large N (O(1) is not
# yet reached for `yield from` specifically — manual for-yield IS O(1), verified to
# 2M at ≈8 MiB). At this test's 250k scale the residual is far under 64 MiB; a
# regression of the task-#46 fix (which would re-leak the outer pair + value too)
# pushes it well over.
#
# Run under:  python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Fixed     -> bounded, ≈9 MiB at 250k (exit 0).
# Regressed -> outer pair + value leaked too; RSS climbs / cap trips.
#
# The printed summary is a single integer, byte-identical to CPython.
#
# NOTE: consumed by a plain `for` loop (not `itertools.islice`) to isolate the
# generator-delegation `_poll` fix from the separate itertools `iter_next_pair`
# wrapper-tuple leak. See generator_delegation_nested_2deep.py.


def inner(n):
    for i in range(n):
        yield i


def outer(n):
    # `yield from` delegation spelling.
    yield from inner(n)


def main() -> int:
    total = 0
    for v in outer(250_000):
        total += v
    return total


print(main())
