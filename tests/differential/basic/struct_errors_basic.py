"""Purpose: differential coverage for struct errors + buffer edge cases."""

import struct


def show(label, fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, exc)
    else:
        print(label, "ok")


show("fmt_type", lambda: struct.pack(1, 2))
show("pack_unsigned_neg", lambda: struct.pack("B", -1))
show("pack_signed_oor", lambda: struct.pack("b", 128))
show("pack_char_len", lambda: struct.pack("c", bytearray(b"a")))
show("pack_s_memview", lambda: struct.pack("2s", memoryview(b"ab")))

buf = bytearray(4)
show("pack_into_neg", lambda: struct.pack_into("I", buf, -1, 1))
show("pack_into_oob", lambda: struct.pack_into("I", buf, -5, 1))

show("unpack_from_neg", lambda: struct.unpack_from("I", b"1234", -1))
show("unpack_from_oob", lambda: struct.unpack_from("I", b"1234", -5))

show("unpack_str", lambda: struct.unpack("I", "abcd"))

mv = memoryview(b"abcdefgh")[::2]
show("unpack_noncontig", lambda: struct.unpack("I", mv))
