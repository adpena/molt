"""Minimal codec entrypoints for Molt (intrinsic-backed)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = ["CodecInfo", "decode", "encode", "lookup"]

_MOLT_CODECS_DECODE = _require_intrinsic("molt_codecs_decode", globals())
_MOLT_CODECS_ENCODE = _require_intrinsic("molt_codecs_encode", globals())
_MOLT_CODECS_LOOKUP_NAME = _require_intrinsic("molt_codecs_lookup_name", globals())


class CodecInfo:
    __slots__ = ("name", "_encode", "_decode")

    def __init__(self, name: str, encode, decode) -> None:
        self.name = name
        self._encode = encode
        self._decode = decode

    def encode(self, obj: object, errors: object = "strict") -> tuple[object, int]:
        out = self._encode(obj, self.name, errors)
        return out, len(obj)  # type: ignore[arg-type]

    def decode(self, obj: object, errors: object = "strict") -> tuple[object, int]:
        out = self._decode(obj, self.name, errors)
        return out, len(obj)  # type: ignore[arg-type]


_CODECS_CACHE: dict[str, CodecInfo] = {}


def lookup(encoding: object) -> CodecInfo:
    name = _MOLT_CODECS_LOOKUP_NAME(encoding)
    if not isinstance(name, str):
        raise TypeError("lookup() argument must be str, not None")
    cached = _CODECS_CACHE.get(name)
    if cached is not None:
        return cached
    info = CodecInfo(name, _MOLT_CODECS_ENCODE, _MOLT_CODECS_DECODE)
    _CODECS_CACHE[name] = info
    return info


def decode(
    obj: object, encoding: object = "utf-8", errors: object = "strict"
) -> object:
    return _MOLT_CODECS_DECODE(obj, encoding, errors)


def encode(
    obj: object, encoding: object = "utf-8", errors: object = "strict"
) -> object:
    return _MOLT_CODECS_ENCODE(obj, encoding, errors)
