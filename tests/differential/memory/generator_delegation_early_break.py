# Generator delegation with early termination — frame/temporary release on break
# and explicit close() (task #46).
#
# Companion to generator_delegation_nested_2deep.py — see that file for the full
# root-cause writeup. This test exercises the EARLY-TERMINATION paths of the same
# fix: breaking out of a delegating generator mid-stream (so the generator is
# abandoned and finalized/closed rather than run to exhaustion), repeated many
# times. Each (create delegating generator -> pull a bounded prefix -> break ->
# drop/close) cycle must release the per-iteration delegation temporaries and the
# generator frames it created; a leak of either the `(value, done)` pair tuples
# or the sub-generator frames would accumulate across the outer repetitions.
#
# Run under:  python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Fixed     -> each cycle's state is reclaimed; O(1) across cycles (exit 0).
# Regressed -> abandoned-iteration temporaries/frames pile up; RSS climbs / trips.
#
# The printed summary is deterministic, byte-identical to CPython.
#
# NOTE: consumed by plain `for` loops (not `itertools.islice`) to isolate the
# generator-delegation `_poll` early-release fix from the separate itertools
# `iter_next_pair` wrapper-tuple leak. See generator_delegation_nested_2deep.py.


def inner(n):
    for i in range(n):
        yield i


def outer_foryield(n):
    for y in inner(n):
        yield y + 1


def outer_yieldfrom(n):
    yield from inner(n)


def main() -> int:
    checksum = 0
    cycles = 20_000
    prefix = 8  # pull only a short prefix, then abandon the generator
    for c in range(cycles):
        # Alternate both delegation spellings so the early-break release path is
        # covered for each lowering.
        gen = outer_foryield(1_000_000) if (c & 1) == 0 else outer_yieldfrom(1_000_000)
        pulled = 0
        for v in gen:
            checksum += v
            pulled += 1
            if pulled >= prefix:
                break  # mid-iteration break: abandon a deep, far-from-exhausted chain
        # `gen` goes out of scope here and must be finalized/closed, releasing the
        # sub-generator frame and any pinned per-iteration temporaries.
        gen.close()  # explicit close() through the delegation chain is also exercised
    # Also drive one chain to full exhaustion to keep the normal path covered.
    for v in outer_yieldfrom(50_000):
        checksum += v
    return checksum


print(main())
