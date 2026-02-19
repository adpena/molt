"""Minimal multibyte codec helpers for encodings.* modules."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class MultibyteIncrementalEncoder:
    codec = None

    def __init__(self, errors="strict"):
        self.errors = errors

    def encode(self, input, final=False):
        del final
        if self.codec is None:
            raise LookupError("codec is not configured")
        out, _ = self.codec.encode(input, self.errors)
        return out

    def reset(self):
        return None


class MultibyteIncrementalDecoder:
    codec = None

    def __init__(self, errors="strict"):
        self.errors = errors

    def decode(self, input, final=False):
        del final
        if self.codec is None:
            raise LookupError("codec is not configured")
        out, _ = self.codec.decode(input, self.errors)
        return out

    def reset(self):
        return None


class MultibyteStreamReader:
    codec = None

    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors

    def read(self, size=-1):
        data = self.stream.read(size)
        if self.codec is None:
            return data
        out, _ = self.codec.decode(data, self.errors)
        return out


class MultibyteStreamWriter:
    codec = None

    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors

    def write(self, data):
        if self.codec is None:
            return self.stream.write(data)
        out, _ = self.codec.encode(data, self.errors)
        return self.stream.write(out)
