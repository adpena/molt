"""Canonical singleton/intern-cache steady-state allocation benchmark.

Run under the standard benchmark memory guard with allocation profiling enabled.
Every loop is warmed before measurement; the measured million-hit phases must
report zero new heap objects, zero allocated bytes, and a flat live-byte floor.
"""

import sys


def main() -> None:
    empty_tuple = ()
    empty_string = ""
    empty_bytes = b""
    ascii_char = "a"
    identifier = "canonical_identifier"
    explicit = sys.intern("canonical-name-with-dashes")
    hits = 0

    for _ in range(1_000_000):
        hits += () is empty_tuple
        hits += "" is empty_string
        hits += b"" is empty_bytes
        hits += "a" is ascii_char
        hits += "canonical_identifier" is identifier
        hits += sys.intern("canonical-name-with-dashes") is explicit

    print(hits)


if __name__ == "__main__":
    main()
