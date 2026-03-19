"""Intrinsic-backed multibyte codec family shim."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import codecs as _py_codecs

_require_intrinsic("molt_capabilities_has")


class MultibyteCodec:
    def __init__(self, encoding: str):
        self.encoding = encoding

    def encode(self, input, errors="strict"):
        out = _py_codecs.encode(input, self.encoding, errors)
        return out, len(input)

    def decode(self, input, errors="strict"):
        out = _py_codecs.decode(input, self.encoding, errors)
        return out, len(input)


def getcodec(name):
    return MultibyteCodec(str(name))


globals().pop("_require_intrinsic", None)
