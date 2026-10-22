"""Lockstep and lagging tee consumers, with optional runtime allocation epochs.

The ordinary benchmark runner owns uninstrumented whole-workload throughput.
The existing profiler owns cache allocation/live-byte observations at the two
phase endpoints; profiler-enabled execution is not a latency baseline.
"""

import itertools
import sys

if sys.implementation.name == "molt":
    from _intrinsics import load_intrinsic

    _reset = load_intrinsic("molt_profile_epoch_reset")
    _dump = load_intrinsic("molt_profile_epoch_dump")
else:
    _reset = None
    _dump = None


def consume(count: int) -> int:
    if _reset is not None:
        _reset("tee_lockstep")
    a, b = itertools.tee(range(count))
    total = 0
    for _ in range(count):
        left = next(a)
        right = next(b)
        assert left is right
        total += left + right
    if _dump is not None:
        _dump()
    del a, b

    if _reset is not None:
        _reset("tee_lag")
    a, b = itertools.tee(range(count))
    for _ in range(count):
        total += next(a)
    if _dump is not None:
        _dump()
    if _reset is not None:
        _reset("tee_catchup")
    for _ in range(count):
        total += next(b)
    if _dump is not None:
        _dump()
    return total


def main() -> None:
    count = 200_000
    total = consume(count)
    assert total == 2 * count * (count - 1)
    print(total)


if __name__ == "__main__":
    main()
