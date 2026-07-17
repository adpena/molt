"""Canonical singleton/intern-cache steady-state allocation benchmark.

Run under the standard benchmark memory guard with allocation profiling enabled.
Every loop is warmed before measurement; the measured million-hit phases must
report zero new heap objects, zero allocated bytes, and a flat live-byte floor.
"""

# ruff: noqa: F632 -- identity is the behavior under measurement.

import sys

if sys.implementation.name == "molt":
    from _intrinsics import load_intrinsic

    _profile_epoch_reset = load_intrinsic("molt_profile_epoch_reset")
    _profile_epoch_dump = load_intrinsic("molt_profile_epoch_dump")
else:
    _profile_epoch_reset = None
    _profile_epoch_dump = None


def main() -> None:
    empty_tuple = ()
    empty_string = ""
    empty_bytes = b""
    ascii_char = "a"
    identifier = "canonical_identifier"
    explicit = sys.intern("canonical-name-with-dashes")
    hits = 0

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("canonical_cache_hits")
    for _ in range(1_000_000):
        hits += () is empty_tuple
        hits += "" is empty_string
        hits += b"" is empty_bytes
        hits += "a" is ascii_char
        hits += "canonical_identifier" is identifier
        hits += sys.intern("canonical-name-with-dashes") is explicit
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    print(hits)


if __name__ == "__main__":
    main()
