"""Purpose: differential coverage for nested memoryview buffer windows in struct."""

import struct


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, exc)


base = bytearray(b"abcdefghijkl")
outer = memoryview(base)[1:11]
inner = outer[2:6]

struct.pack_into("I", inner, 0, 0x01020304)
print("base_after", bytes(base))
print("unpack_inner", struct.unpack("I", inner))
print("unpack_from_inner", struct.unpack_from("I", inner, 0))

ro_inner = memoryview(bytes(base))[2:6]
show("pack_into_ro_inner", lambda: struct.pack_into("I", ro_inner, 0, 1))
