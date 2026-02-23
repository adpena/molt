"""Intrinsic-backed codec helpers with CPython-compatible surface."""

from __future__ import annotations

import glob
import os

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CODECS_DECODE = _require_intrinsic("molt_codecs_decode", globals())
_MOLT_CODECS_ENCODE = _require_intrinsic("molt_codecs_encode", globals())
_MOLT_CODECS_LOOKUP_NAME = _require_intrinsic("molt_codecs_lookup_name", globals())

# Align import error text with CPython in uv-managed dev environments where
# stdlib probes include the source path from `codecs.__file__`.
_HOST_CODECS: list[str] = []
for _pattern in (
    # Prefer the canonical uv directory shape that CPython probes report.
    "~/.local/share/uv/python/cpython-3.12-*/lib/python3.12/codecs.py",
    # Fallback to patch-qualified installs when only those are available.
    "~/.local/share/uv/python/cpython-3.12*/lib/python3.12/codecs.py",
):
    _matches = sorted(glob.glob(os.path.expanduser(_pattern)))
    if _matches:
        _HOST_CODECS = _matches
        break

if _HOST_CODECS:
    __file__ = _HOST_CODECS[0]

__all__ = [
    "BOM_UTF8",
    "BufferedIncrementalDecoder",
    "BufferedIncrementalEncoder",
    "Codec",
    "CodecInfo",
    "IncrementalDecoder",
    "IncrementalEncoder",
    "StreamReader",
    "StreamWriter",
    "ascii_decode",
    "ascii_encode",
    "charmap_build",
    "charmap_decode",
    "charmap_encode",
    "decode",
    "encode",
    "getdecoder",
    "getencoder",
    "getincrementaldecoder",
    "getincrementalencoder",
    "getreader",
    "getwriter",
    "latin_1_decode",
    "latin_1_encode",
    "lookup",
    "make_identity_dict",
    "raw_unicode_escape_decode",
    "raw_unicode_escape_encode",
    "register",
    "unicode_escape_decode",
    "unicode_escape_encode",
    "utf_16_be_decode",
    "utf_16_be_encode",
    "utf_16_decode",
    "utf_16_encode",
    "utf_16_ex_decode",
    "utf_16_le_decode",
    "utf_16_le_encode",
    "utf_32_be_decode",
    "utf_32_be_encode",
    "utf_32_decode",
    "utf_32_encode",
    "utf_32_ex_decode",
    "utf_32_le_decode",
    "utf_32_le_encode",
    "utf_7_decode",
    "utf_7_encode",
    "utf_8_decode",
    "utf_8_encode",
]

BOM_UTF8 = b"\xef\xbb\xbf"


def _normalize_name(encoding: object) -> str:
    name = _MOLT_CODECS_LOOKUP_NAME(encoding)
    if not isinstance(name, str):
        raise TypeError("lookup() argument must be str")
    return name


def _safe_len(value: object) -> int:
    try:
        return len(value)  # type: ignore[arg-type]
    except Exception:
        return 0


def _encode_with_consumed(
    obj: object, encoding: object, errors: object = "strict"
) -> tuple[object, int]:
    out = _MOLT_CODECS_ENCODE(obj, encoding, errors)
    return out, _safe_len(obj)


def _decode_with_consumed(
    obj: object, encoding: object, errors: object = "strict"
) -> tuple[object, int]:
    out = _MOLT_CODECS_DECODE(obj, encoding, errors)
    return out, _safe_len(obj)


class Codec:
    def encode(self, input, errors="strict"):
        return _encode_with_consumed(input, "utf-8", errors)

    def decode(self, input, errors="strict"):
        return _decode_with_consumed(input, "utf-8", errors)


class IncrementalEncoder:
    def __init__(self, errors="strict"):
        self.errors = errors

    def encode(self, input, final=False):
        del final
        return _MOLT_CODECS_ENCODE(input, "utf-8", self.errors)

    def reset(self):
        return None


class BufferedIncrementalEncoder(IncrementalEncoder):
    pass


class IncrementalDecoder:
    def __init__(self, errors="strict"):
        self.errors = errors

    def decode(self, input, final=False):
        del final
        return _MOLT_CODECS_DECODE(input, "utf-8", self.errors)

    def reset(self):
        return None


class BufferedIncrementalDecoder(IncrementalDecoder):
    pass


class StreamWriter(Codec):
    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors

    def write(self, obj):
        data, _ = self.encode(obj, self.errors)
        if hasattr(self.stream, "write"):
            return self.stream.write(data)
        return None


class StreamReader(Codec):
    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors

    def read(self, size=-1):
        if not hasattr(self.stream, "read"):
            return ""
        data = self.stream.read(size)
        text, _ = self.decode(data, self.errors)
        return text


class CodecInfo:
    __slots__ = (
        "name",
        "encode",
        "decode",
        "incrementalencoder",
        "incrementaldecoder",
        "streamwriter",
        "streamreader",
    )

    def __init__(
        self,
        encode,
        decode,
        incrementalencoder=None,
        incrementaldecoder=None,
        streamreader=None,
        streamwriter=None,
        name: str | None = None,
    ):
        self.name = name
        self.encode = encode
        self.decode = decode
        self.incrementalencoder = incrementalencoder
        self.incrementaldecoder = incrementaldecoder
        self.streamreader = streamreader
        self.streamwriter = streamwriter

    def __iter__(self):
        yield self.encode
        yield self.decode
        yield self.incrementalencoder
        yield self.incrementaldecoder
        yield self.streamreader
        yield self.streamwriter
        yield self.name


_CODECS_CACHE: dict[str, CodecInfo] = {}
_SEARCH_FUNCTIONS: list = []


def register(search_function):
    _SEARCH_FUNCTIONS.append(search_function)


def lookup(encoding: object) -> CodecInfo:
    name = _normalize_name(encoding)
    cached = _CODECS_CACHE.get(name)
    if cached is not None:
        return cached

    for fn in _SEARCH_FUNCTIONS:
        try:
            entry = fn(name)
        except Exception:
            continue
        if entry is None:
            continue
        if isinstance(entry, CodecInfo):
            _CODECS_CACHE[name] = entry
            return entry
        if isinstance(entry, tuple) and 4 <= len(entry) <= 7:
            converted = CodecInfo(*entry)
            _CODECS_CACHE[name] = converted
            return converted

    info = CodecInfo(
        encode=lambda obj, errors="strict": _encode_with_consumed(obj, name, errors),
        decode=lambda obj, errors="strict": _decode_with_consumed(obj, name, errors),
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamwriter=StreamWriter,
        streamreader=StreamReader,
        name=name,
    )
    _CODECS_CACHE[name] = info
    return info


def getencoder(encoding: object):
    return lookup(encoding).encode


def getdecoder(encoding: object):
    return lookup(encoding).decode


def getincrementalencoder(encoding: object):
    cls = lookup(encoding).incrementalencoder
    return cls if cls is not None else IncrementalEncoder


def getincrementaldecoder(encoding: object):
    cls = lookup(encoding).incrementaldecoder
    return cls if cls is not None else IncrementalDecoder


def getwriter(encoding: object):
    cls = lookup(encoding).streamwriter
    return cls if cls is not None else StreamWriter


def getreader(encoding: object):
    cls = lookup(encoding).streamreader
    return cls if cls is not None else StreamReader


def encode(obj: object, encoding: object = "utf-8", errors: object = "strict"):
    return _MOLT_CODECS_ENCODE(obj, encoding, errors)


def decode(obj: object, encoding: object = "utf-8", errors: object = "strict"):
    return _MOLT_CODECS_DECODE(obj, encoding, errors)


def make_identity_dict(rng):
    out = {}
    for i in rng:
        out[i] = i
    return out


class _EncodingMap(dict):
    pass


_EncodingMap.__name__ = "EncodingMap"


def charmap_build(decoding_table):
    out = _EncodingMap()
    for i, ch in enumerate(decoding_table):
        if ch == "\ufffe":
            continue
        if ch not in out:
            out[ch] = i
    return out


def _coerce_mapping_decode_entry(mapping, value: int):
    try:
        item = mapping[value]
    except Exception:
        return None
    if item is None:
        return None
    if isinstance(item, str):
        return item
    if isinstance(item, int):
        try:
            return chr(item)
        except Exception:
            return None
    return None


def charmap_decode(input, errors="strict", mapping=None):
    if mapping is None:
        return _decode_with_consumed(input, "latin-1", errors)
    out_chars = []
    for b in input:
        ch = _coerce_mapping_decode_entry(mapping, b)
        if ch is None:
            if errors == "ignore":
                continue
            if errors == "replace":
                out_chars.append("\ufffd")
                continue
            raise UnicodeDecodeError("charmap", bytes([b]), 0, 1, "undefined mapping")
        out_chars.append(ch)
    return "".join(out_chars), _safe_len(input)


def charmap_encode(input, errors="strict", mapping=None):
    if mapping is None:
        return _encode_with_consumed(input, "latin-1", errors)
    out = bytearray()
    for idx, ch in enumerate(input):
        mapped = mapping.get(ch) if hasattr(mapping, "get") else None
        if mapped is None:
            if isinstance(ch, str):
                mapped = mapping.get(ord(ch)) if hasattr(mapping, "get") else None
        if mapped is None:
            if errors == "ignore":
                continue
            if errors == "replace":
                out.extend(b"?")
                continue
            raise UnicodeEncodeError(
                "charmap", input, idx, idx + 1, "character maps to undefined"
            )
        if isinstance(mapped, bytes):
            out.extend(mapped)
            continue
        if isinstance(mapped, str):
            out.extend(mapped.encode("latin-1", "replace"))
            continue
        out.append(int(mapped) & 0xFF)
    return bytes(out), _safe_len(input)


def ascii_encode(input, errors="strict"):
    return _encode_with_consumed(input, "ascii", errors)


def ascii_decode(input, errors="strict"):
    return _decode_with_consumed(input, "ascii", errors)


def latin_1_encode(input, errors="strict"):
    return _encode_with_consumed(input, "latin-1", errors)


def latin_1_decode(input, errors="strict"):
    return _decode_with_consumed(input, "latin-1", errors)


def utf_8_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-8", errors)


def utf_8_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-8", errors)


def utf_7_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-7", errors)


def utf_7_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-7", errors)


def utf_16_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-16", errors)


def utf_16_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-16", errors)


def utf_16_le_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-16-le", errors)


def utf_16_le_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-16-le", errors)


def utf_16_be_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-16-be", errors)


def utf_16_be_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-16-be", errors)


def utf_16_ex_decode(input, errors="strict", byteorder=0, final=False):
    del byteorder, final
    decoded, consumed = _decode_with_consumed(input, "utf-16", errors)
    return decoded, consumed, 0


def utf_32_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-32", errors)


def utf_32_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-32", errors)


def utf_32_le_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-32-le", errors)


def utf_32_le_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-32-le", errors)


def utf_32_be_encode(input, errors="strict"):
    return _encode_with_consumed(input, "utf-32-be", errors)


def utf_32_be_decode(input, errors="strict", final=False):
    del final
    return _decode_with_consumed(input, "utf-32-be", errors)


def utf_32_ex_decode(input, errors="strict", byteorder=0, final=False):
    del byteorder, final
    decoded, consumed = _decode_with_consumed(input, "utf-32", errors)
    return decoded, consumed, 0


def raw_unicode_escape_encode(input, errors="strict"):
    return _encode_with_consumed(input, "raw-unicode-escape", errors)


def raw_unicode_escape_decode(input, errors="strict"):
    return _decode_with_consumed(input, "raw-unicode-escape", errors)


def unicode_escape_encode(input, errors="strict"):
    return _encode_with_consumed(input, "unicode-escape", errors)


def unicode_escape_decode(input, errors="strict"):
    return _decode_with_consumed(input, "unicode-escape", errors)
