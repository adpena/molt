"""Differential coverage for pickle protocols 2-5 core roundtrip semantics."""

from __future__ import annotations

import pickle


def main() -> None:
    payload = {
        "bytes": b"\x00\x01molt",
        "bytearray": bytearray(b"abc"),
        "tuple": (1, 2, 3),
        "list": [4, 5, {"x": 6}],
        "dict": {"a": 7, "b": 8},
        "set": {9, 10},
        "frozenset": frozenset({11, 12}),
        "slice": slice(1, 9, 2),
    }

    for proto in (2, 3, 4, 5):
        blob = pickle.dumps(payload, protocol=proto)
        out = pickle.loads(blob)
        print(
            "proto",
            proto,
            out["bytes"],
            type(out["bytearray"]).__name__,
            sorted(out["set"]),
            sorted(out["frozenset"]),
            (out["slice"].start, out["slice"].stop, out["slice"].step),
            out["list"][2]["x"],
        )


if __name__ == "__main__":
    main()
