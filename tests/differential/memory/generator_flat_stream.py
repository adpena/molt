# FLAT (non-delegating) generator streaming-memory CONTROL (task #46).
#
# This is the control for the generator-delegation leak fixed in task #46 (see
# generator_delegation_nested_2deep.py). A single, non-delegating generator
# streamed over many elements must run in O(1) memory. It was the baseline the
# delegation probes were measured against; this test guards that the leak fix did
# not regress the flat-generator path (the `(value, done)` pair tuple emitted
# before each `state_yield` must still be released per-iteration here too).
#
# Run under:  python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Correct   -> O(1), far under 64 MiB (exit 0).
# Regressed -> one pair tuple leaked per element; RSS climbs unbounded / cap trips.
#
# The printed summary is a single integer, byte-identical to CPython.
#
# NOTE: consumed by a plain `for` loop (not `itertools.islice`) — see
# generator_delegation_nested_2deep.py for why the itertools consumers are
# avoided here.


def flat(n):
    for i in range(n):
        yield i + 1


def main() -> int:
    total = 0
    for v in flat(250_000):
        total += v
    return total


print(main())
