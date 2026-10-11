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

import io
import sys

old_digits = sys.get_int_max_str_digits()
try:
    sys.set_int_max_str_digits(640)
    rejected = 10**640
    for protocol in (0, 1):
        output = io.BytesIO()
        try:
            pickle.dump(rejected, output, protocol=protocol)
            print("text pickle", protocol, "accepted")
        except ValueError as exc:
            print("text pickle", protocol, type(exc).__name__, str(exc), output.getvalue() == b"")
    for protocol in (2, 4, 5):
        print("binary pickle", protocol, pickle.loads(pickle.dumps(rejected, protocol=protocol)) == rejected)
finally:
    sys.set_int_max_str_digits(old_digits)
