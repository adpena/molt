"""Intrinsic-backed `_codecs` compatibility shim."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import codecs as _py_codecs

_MOLT_CODECS_DECODE = _require_intrinsic("molt_codecs_decode")
_MOLT_CODECS_ENCODE = _require_intrinsic("molt_codecs_encode")
_MOLT_CODECS_LOOKUP_NAME = _require_intrinsic("molt_codecs_lookup_name")


class _CodecProxy:
    def __init__(self, encoding: str):
        self.encoding = encoding

    def encode(self, input, errors="strict"):
        return _MOLT_CODECS_ENCODE(input, self.encoding, errors, True)

    def decode(self, input, errors="strict"):
        return _MOLT_CODECS_DECODE(input, self.encoding, errors, True)


def getcodec(name):
    normalized = _MOLT_CODECS_LOOKUP_NAME(name)
    if not isinstance(normalized, str):
        raise LookupError(name)
    return _CodecProxy(normalized)


def encode(obj, encoding="utf-8", errors="strict"):
    return _MOLT_CODECS_ENCODE(obj, encoding, errors, False)


def decode(obj, encoding="utf-8", errors="strict"):
    return _MOLT_CODECS_DECODE(obj, encoding, errors, False)


def lookup(encoding):
    return _py_codecs.lookup(encoding)


globals().pop("_require_intrinsic", None)
