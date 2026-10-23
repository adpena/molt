"""Purpose: str(bytes, encoding, errors) decoding parity."""


def show(label, thunk):
    try:
        print(f"{label}:{thunk()}")
    except Exception as exc:  # pragma: no cover - output is the check
        print(f"{label}:{type(exc).__name__}:{exc}")


print("bytes", str(b"abc", "utf-8"))
print("bytearray", str(bytearray(b"abc"), "utf-8"))
print("memoryview", str(memoryview(b"abc"), "utf-8"))
print("ignore", str(b"\xff", "utf-8", "ignore"))
print("replace", str(b"\xff", "utf-8", "replace"))

show("bad-encoding", lambda: str(b"abc", "bad-enc"))
show("bad-errors", lambda: str(b"abc", "utf-8", "bad"))
show("bad-errors-fail", lambda: str(b"\xff", "utf-8", "bad"))
show("bad-encoding-type", lambda: str(b"abc", 1))
show("bad-errors-type", lambda: str(b"abc", "utf-8", 1))
show("not-bytes-like", lambda: str("abc", "utf-8"))
show("decode-error", lambda: str(b"\xff", "utf-8"))

try:
    str(bytearray(b"\xff"), "utf-8")
except UnicodeDecodeError as exc:
    print("object-type", type(exc.object).__name__, exc.object)
    print("encoding", exc.encoding)
    print("start-end", exc.start, exc.end)
    print("reason", exc.reason)

# Omitted arguments take their defaults; an explicit None is refused by name.
show("errors-none", lambda: str(b"abc", "utf-8", None))
show("encoding-none", lambda: str(b"abc", None))
show("bytes-errors-none", lambda: bytes("abc", "utf-8", None))
show("bytearray-errors-none", lambda: bytearray("abc", "utf-8", None))
show("bytes-encoding-none", lambda: bytes("abc", None))
show("keyword-errors", lambda: str(b"\xff", errors="replace"))
show("keyword-encoding", lambda: str(b"\xe9", encoding="latin-1"))
# A failed buffer request becomes str()'s own TypeError.
show("not-bytes-like-int", lambda: str(1, "utf-8"))
released = memoryview(b"abc")
released.release()
show("released-view", lambda: str(released, "utf-8"))
