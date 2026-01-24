"""Purpose: differential coverage for bytes/bytearray error handlers."""


def show(label: str, thunk) -> None:
    try:
        print(label, thunk())
    except Exception as exc:
        print(label, type(exc).__name__, exc)


def encode(errors: str) -> bytes:
    return bytes("hi€", "ascii", errors=errors)


def decode(errors: str) -> str:
    return bytes([0xFF]).decode("utf-8", errors=errors)


for handler in ("ignore", "replace", "backslashreplace", "namereplace"):
    show(f"encode-{handler}", lambda h=handler: encode(h))

for handler in ("ignore", "replace", "backslashreplace", "surrogateescape"):
    show(f"decode-{handler}", lambda h=handler: decode(h))
