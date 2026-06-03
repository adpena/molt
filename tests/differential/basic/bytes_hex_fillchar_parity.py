"""Purpose: CPython parity for bytes/bytearray .hex(sep, bytes_per_sep) and the
.center/.ljust/.rjust fillchar validation.

hex: bytes_per_sep==0 means "no grouping" (ungrouped hex, no ValueError); the
bytes_per_sep arg is converted before the separator is validated, so a non-int
bytes_per_sep error wins. fillchar: only bytes/bytearray accepted (memoryview
rejected); a wrong-length fillchar uses the 3.14 long-form message naming the
actual type, the short form on 3.12/3.13 (version-gated).
"""


def show(label, fn):
    try:
        print(label, "OK", repr(fn()))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# bytes.hex / bytearray.hex / memoryview.hex
show("hex_bps0_bytes", lambda: b"abcd".hex("-", 0))
show("hex_bps0_ba", lambda: bytearray(b"abcd").hex("-", 0))
show("hex_bps0_mv", lambda: memoryview(b"abcd").hex("-", 0))
show("hex_order", lambda: b"abcd".hex(123, "x"))
show("hex_2", lambda: b"abcd".hex("-", 2))
show("hex_neg2", lambda: b"abcd".hex("-", -2))
show("hex_plain", lambda: b"abcd".hex())

# .center / .ljust / .rjust fillchar validation
show("center_ba1", lambda: b"abc".center(7, bytearray(b"-")))
show("center_mv", lambda: b"abc".center(7, memoryview(b"-")))
show("ljust_mv", lambda: b"abc".ljust(7, memoryview(b"-")))
show("rjust_int", lambda: b"abc".rjust(7, 5))
show("center_len2_bytes", lambda: b"abc".center(7, b"xy"))
show("ljust_len2_ba", lambda: bytearray(b"abc").ljust(7, bytearray(b"xy")))
