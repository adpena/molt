# 3-deep nested generator delegation streaming-memory regression (task #46).
#
# Companion to generator_delegation_nested_2deep.py — see that file for the full
# root-cause writeup. The leak was LINEAR in delegation depth (each level leaked
# one `(value, done)` pair tuple per element), so a 3-deep chain leaked ~2x what a
# 2-deep chain did. This test pins the depth-scaling: a correct fix is O(1) at any
# chain depth.
#
# Run under:  python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Fixed     -> O(1) regardless of depth, far under 64 MiB (exit 0).
# Regressed -> ~2 tuples leaked per element; RSS climbs unbounded / cap trips.
#
# The printed summary is a single integer, byte-identical to CPython.
#
# NOTE: consumed by a plain `for` loop (not `itertools.islice`) to isolate the
# generator-delegation `_poll` fix from the separate itertools `iter_next_pair`
# wrapper-tuple leak. See generator_delegation_nested_2deep.py.


def inner(n):
    for i in range(n):
        yield i


def mid(n):
    for y in inner(n):
        yield y + 1


def outer(n):
    for z in mid(n):
        yield z + 1


def main() -> int:
    total = 0
    for v in outer(250_000):
        total += v
    return total


print(main())
