"""Differential coverage for CPython-emitted LONG/EXT opcode payloads."""

from __future__ import annotations

import copyreg
import pickle


class ExtTarget:
    pass


def main() -> None:
    # CPython protocol-2 LONG1 payload for 2**40.
    long_blob = b"\x80\x02\x8a\x06\x00\x00\x00\x00\x00\x01."
    print("long_literal", pickle.loads(long_blob))

    copyreg.add_extension(__name__, "ExtTarget", 220)
    # PROTO 2, EXT1 220, STOP.
    ext_blob = b"\x80\x02\x82\xdc."
    print("ext_literal", pickle.loads(ext_blob) is ExtTarget)


if __name__ == "__main__":
    main()
