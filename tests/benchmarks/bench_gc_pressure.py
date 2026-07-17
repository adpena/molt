"""Measures allocation-heavy workload that stresses GC/refcount."""

import sys

if sys.implementation.name == "molt":
    from _intrinsics import load_intrinsic

    _profile_epoch_reset = load_intrinsic("molt_profile_epoch_reset")
    _profile_epoch_dump = load_intrinsic("molt_profile_epoch_dump")
else:
    _profile_epoch_reset = None
    _profile_epoch_dump = None


def main() -> None:
    results = []
    if _profile_epoch_reset is not None:
        _profile_epoch_reset("gc_pressure_allocation")
    for i in range(1_000_000):
        results.append({"key": i, "value": [i, i + 1, i + 2]})
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()
    print(len(results))


if __name__ == "__main__":
    main()
