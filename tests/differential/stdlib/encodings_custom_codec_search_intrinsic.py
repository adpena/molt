"""Purpose: validate codec registry custom search semantics."""

import codecs


def _custom_search(name: str):
    if name != "molt_custom_codec":
        return None

    def _encode(text: str, errors: str = "strict"):
        del errors
        return text.upper().encode("ascii"), len(text)

    def _decode(data: bytes, errors: str = "strict"):
        del errors
        raw = bytes(data)
        return raw.decode("ascii").lower(), len(raw)

    return codecs.CodecInfo(
        name="molt_custom_codec",
        encode=_encode,
        decode=_decode,
    )


codecs.register(_custom_search)
lookup = codecs.lookup("molt-custom codec")
print(lookup.name)
print(codecs.encode("MiX", "molt-custom codec"))
print(codecs.decode(b"MIX", "molt-custom codec"))


def _broken_search(name: str):
    if name == "molt_broken_codec":
        raise RuntimeError("codec search boom")
    return None


codecs.register(_broken_search)
try:
    codecs.lookup("molt-broken codec")
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__)
    print(str(exc))
else:
    print("NO_ERROR")
