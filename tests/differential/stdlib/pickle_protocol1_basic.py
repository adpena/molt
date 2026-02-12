"""Planned protocol-1 pickle parity coverage (not yet promoted)."""

from __future__ import annotations

import pickle


def show(label: str, value) -> None:
    print(f"{label}: {value!r}")


payload = {
    "bytes": b"\x00\x01molt",
    "bytearray": bytearray(b"abc"),
    "ints": [1, 2, 3],
    "nested": {"k": ("v", None, True, False)},
}

blob = pickle.dumps(payload, protocol=1)
roundtrip = pickle.loads(blob)

# Pickle byte stream length is implementation-dependent; assert semantic roundtrip.
show("roundtrip", roundtrip)
show("bytearray_type", type(roundtrip["bytearray"]).__name__)
