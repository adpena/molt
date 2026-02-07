"""Purpose: differential coverage for struct offset and iterator edge semantics."""

import struct


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, exc)


buf = bytearray(4)
struct.pack_into("I", buf, -4, 1)
print("pack_into_neg_exact", bytes(buf))
print("unpack_from_neg_exact", struct.unpack_from("I", b"1234", -4))

show(
    "pack_into_noncontig_rw",
    lambda: struct.pack_into("I", memoryview(bytearray(8))[::2], 0, 1),
)
show("iter_unpack_bad_mult", lambda: list(struct.iter_unpack("I", b"12345")))
