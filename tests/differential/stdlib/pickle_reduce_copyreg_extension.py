"""Differential coverage for pickle reducers, BUILD semantics, and extensions."""

from __future__ import annotations

import copyreg
import pickle


class ViaReduce:
    def __init__(self, x: int) -> None:
        self.x = x
        self.y = x + 1

    def __reduce_ex__(self, protocol: int):
        return (type(self), (self.x,), {"y": self.y})


class ViaCopyreg:
    def __init__(self, value: int) -> None:
        self.value = value


def _reduce_via_copyreg(obj: ViaCopyreg):
    return (ViaCopyreg, (obj.value,))


def main() -> None:
    reduced = pickle.loads(pickle.dumps(ViaReduce(7), protocol=4))
    print("reduce_build", reduced.x, reduced.y)

    copyreg.pickle(ViaCopyreg, _reduce_via_copyreg)
    copied = pickle.loads(pickle.dumps(ViaCopyreg(9), protocol=4))
    print("copyreg_reduce", copied.value)

    copyreg.add_extension(__name__, "ViaReduce", 220)
    ext_blob = pickle.dumps(ViaReduce, protocol=2)
    print("extension_opcode", any(op in ext_blob for op in (b"\x82", b"\x83", b"\x84")))
    print("extension_roundtrip", pickle.loads(ext_blob) is ViaReduce)


if __name__ == "__main__":
    main()
