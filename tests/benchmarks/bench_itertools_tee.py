"""Lockstep and lagging tee consumers, with optional runtime allocation epochs.

The ordinary benchmark runner owns uninstrumented whole-workload throughput.
The existing profiler owns cache allocation/live-byte observations at the two
phase endpoints; profiler-enabled execution is not a latency baseline.
Use --phase-timing for one phase-workload timing observation on stderr.
It includes loop, source iteration, arithmetic and assertions, not isolated
next() latency or a latency distribution. Imports and clocks are diagnostic-only.
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


def consume(count: int, phase_timing: bool = False) -> int:
    if phase_timing:
        from time import perf_counter_ns

    if _reset is not None:
        _reset("tee_lockstep")
    a, b = itertools.tee(range(count))
    total = 0
    if phase_timing:
        started_ns = perf_counter_ns()
    for _ in range(count):
        left = next(a)
        right = next(b)
        assert left is right
        total += left + right
    if phase_timing:
        lockstep_ns = perf_counter_ns() - started_ns
    if _dump is not None:
        _dump()
    del a, b

    if _reset is not None:
        _reset("tee_lag")
    a, b = itertools.tee(range(count))
    if phase_timing:
        started_ns = perf_counter_ns()
    for _ in range(count):
        total += next(a)
    if phase_timing:
        lag_ns = perf_counter_ns() - started_ns
    if _dump is not None:
        _dump()
    if _reset is not None:
        _reset("tee_catchup")
    if phase_timing:
        started_ns = perf_counter_ns()
    for _ in range(count):
        total += next(b)
    if phase_timing:
        catchup_ns = perf_counter_ns() - started_ns
    if _dump is not None:
        _dump()
    if phase_timing:
        import json

        print(
            "MOLT_TEE_PHASE_TIMING "
            + json.dumps(
                {
                    "count": count,
                    "phases": [
                        {
                            "phase": "lockstep",
                            "elapsed_ns": lockstep_ns,
                            "reader_advances": 2 * count,
                        },
                        {
                            "phase": "lag",
                            "elapsed_ns": lag_ns,
                            "reader_advances": count,
                        },
                        {
                            "phase": "catchup",
                            "elapsed_ns": catchup_ns,
                            "reader_advances": count,
                        },
                    ],
                }
            ),
            file=sys.stderr,
        )
    return total


def main() -> None:
    count = 200_000
    total = consume(count, "--phase-timing" in sys.argv[1:])
    assert total == 2 * count * (count - 1)
    print(total)


if __name__ == "__main__":
    main()
