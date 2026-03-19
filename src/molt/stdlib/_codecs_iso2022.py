"""Intrinsic-backed multibyte codec family shim."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import codecs as _py_codecs

_require_intrinsic("molt_capabilities_has")


class _MultibyteCodec:
    def __init__(self, encoding: str):
        self.encoding = encoding

    def encode(self, input, errors="strict"):
        out = _py_codecs.encode(input, self.encoding, errors)
        return out, len(input)

    def decode(self, input, errors="strict"):
        out = _py_codecs.decode(input, self.encoding, errors)
        return out, len(input)


class _BuiltinFunctionOrMethod:
    __slots__ = ("_func",)

    def __init__(self, func):
        self._func = func

    def __call__(self, *args, **kwargs):
        return self._func(*args, **kwargs)


_BuiltinFunctionOrMethod.__name__ = "builtin_function_or_method"


def _getcodec(name):
    return _MultibyteCodec(str(name))


getcodec = _BuiltinFunctionOrMethod(_getcodec)
