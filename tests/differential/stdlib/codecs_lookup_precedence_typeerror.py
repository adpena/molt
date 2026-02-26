"""Purpose: verify codecs built-in precedence and TypeError message shape."""

import codecs


def _override_utf8(name):
    if name != "utf_8":
        return None

    def _encode(text: str, errors: str = "strict"):
        del errors
        return b"OVERRIDE", len(text)

    def _decode(data: bytes, errors: str = "strict"):
        del errors
        return "override", len(data)

    return codecs.CodecInfo(name="override_utf8", encode=_encode, decode=_decode)


codecs.register(_override_utf8)
print(codecs.lookup("utf-8").name)

try:
    codecs.lookup(1)
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__)
    print(str(exc))
