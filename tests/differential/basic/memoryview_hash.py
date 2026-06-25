"""Purpose: differential coverage for memoryview hashing — CPython memory_hash parity.

CPython (Objects/memoryobject.c: memory_hash) makes a memoryview hashable iff it
is read-only AND its format is a one-byte format ('B', 'b' or 'c'); the hash then
equals hash(mv.tobytes()). The error precedence is fixed: writable views raise
ValueError("cannot hash writable memoryview object") *before* the format is even
inspected; a read-only non-byte-format view raises
ValueError("memoryview: hashing is restricted to formats 'B', 'b' or 'c'"); and a
read-only view whose exporter is itself unhashable (a bytearray reached via
.toreadonly()) propagates that exporter's TypeError.

Hash values are compared as *relationships* (hash(mv) == hash(equivalent bytes))
so the assertions are independent of the interpreter's hash seed and algorithm.
"""


def show(label, fn):
    try:
        print(label, "->", fn())
    except Exception as exc:  # noqa: BLE001 — differential compares exact type + message
        print(label, "raised", type(exc).__name__, str(exc))


# (a) Read-only, byte-format views are hashable; hash == hash(equivalent bytes).
show("ro_bytes_eq", lambda: hash(memoryview(b"abc")) == hash(b"abc"))
show("ro_empty_eq", lambda: hash(memoryview(b"")) == hash(b""))
show("ro_slice_eq", lambda: hash(memoryview(b"abcdef")[1:4]) == hash(b"bcd"))
show("ro_stable", lambda: hash(memoryview(b"abc")) == hash(memoryview(b"abc")))
# 'b' (signed char) and 'c' (char) are the other two hashable byte formats.
show("ro_cast_b_eq", lambda: hash(memoryview(b"abc").cast("b")) == hash(b"abc"))
show("ro_cast_c_eq", lambda: hash(memoryview(b"abc").cast("c")) == hash(b"abc"))

# (b) Writable views are unhashable -> ValueError, raised before the format rule.
show("writable", lambda: hash(memoryview(bytearray(b"abc"))))
# Writable *and* non-byte-format: still reports "writable" first (CPython order).
show("writable_nonbyte", lambda: hash(memoryview(bytearray(b"abcd")).cast("i")))

# (c) Read-only but non-byte format -> ValueError on the format restriction.
show("fmt_int", lambda: hash(memoryview(b"abcd").cast("i")))
show("fmt_double", lambda: hash(memoryview(b"abcdefgh").cast("d")))

# Read-only view whose exporter is unhashable (bytearray via .toreadonly())
# propagates TypeError: unhashable type: 'bytearray'.
show("ro_over_bytearray", lambda: hash(memoryview(bytearray(b"ab")).toreadonly()))
