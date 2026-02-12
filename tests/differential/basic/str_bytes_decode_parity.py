"""Purpose: differential coverage for str(bytes-like, encoding, errors)."""


def show(label: str, thunk) -> None:
    try:
        print(label, thunk())
    except Exception as exc:
        print(label, type(exc).__name__, exc)


payload = b"\xff"
show("bytes-latin1", lambda: str(payload, "latin-1"))
show("bytes-utf8-ignore", lambda: str(payload, "utf-8", "ignore"))
show("bytearray-latin1", lambda: str(bytearray(payload), "latin-1"))
show("memoryview-latin1", lambda: str(memoryview(payload), "latin-1"))
show("missing-encoding", lambda: str(payload))
