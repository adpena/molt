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
