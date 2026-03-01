"""Intrinsic-backed codec helpers with CPython-compatible surface."""

from __future__ import annotations

import os

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CODECS_DECODE = _require_intrinsic("molt_codecs_decode", globals())
_MOLT_CODECS_ENCODE = _require_intrinsic("molt_codecs_encode", globals())
_MOLT_CODECS_LOOKUP_NAME = _require_intrinsic("molt_codecs_lookup_name", globals())
_molt_codecs_normalize_encoding = _require_intrinsic(
    "molt_codecs_normalize_encoding", globals()
)
_molt_codecs_register_error = _require_intrinsic(
    "molt_codecs_register_error", globals()
)
_molt_codecs_lookup_error = _require_intrinsic("molt_codecs_lookup_error", globals())
_molt_codecs_bom_utf8 = _require_intrinsic("molt_codecs_bom_utf8", globals())
_molt_codecs_bom_utf16_le = _require_intrinsic("molt_codecs_bom_utf16_le", globals())
_molt_codecs_bom_utf16_be = _require_intrinsic("molt_codecs_bom_utf16_be", globals())
_molt_codecs_bom_utf32_le = _require_intrinsic("molt_codecs_bom_utf32_le", globals())
_molt_codecs_bom_utf32_be = _require_intrinsic("molt_codecs_bom_utf32_be", globals())
_molt_inc_enc_new = _require_intrinsic("molt_codecs_incremental_encoder_new", globals())
_molt_inc_enc_encode = _require_intrinsic(
    "molt_codecs_incremental_encoder_encode", globals()
)
_molt_inc_enc_reset = _require_intrinsic(
    "molt_codecs_incremental_encoder_reset", globals()
)
_molt_inc_enc_drop = _require_intrinsic(
    "molt_codecs_incremental_encoder_drop", globals()
)
_molt_inc_dec_new = _require_intrinsic("molt_codecs_incremental_decoder_new", globals())
_molt_inc_dec_decode = _require_intrinsic(
    "molt_codecs_incremental_decoder_decode", globals()
)
_molt_inc_dec_reset = _require_intrinsic(
    "molt_codecs_incremental_decoder_reset", globals()
)
_molt_inc_dec_drop = _require_intrinsic(
    "molt_codecs_incremental_decoder_drop", globals()
)

# Align import-error provenance with uv-managed CPython layouts without
# importing `glob` (which pulls in `re`/`warnings` during bootstrap).
_uv_root = os.path.expanduser("~/.local/share/uv/python")
if os.path.isdir(_uv_root):
    _best_host_codecs: str | None = None
    for _entry in sorted(os.listdir(_uv_root)):
        if not _entry.startswith("cpython-3.12"):
            continue
        _candidate = os.path.join(_uv_root, _entry, "lib", "python3.12", "codecs.py")
        if os.path.isfile(_candidate):
            _best_host_codecs = _candidate
            break
    if _best_host_codecs is not None:
        __file__ = _best_host_codecs

__all__ = [
    "BOM",
    "BOM_BE",
    "BOM_LE",
    "BOM_UTF8",
    "BOM_UTF16",
    "BOM_UTF16_BE",
    "BOM_UTF16_LE",
    "BOM_UTF32",
    "BOM_UTF32_BE",
    "BOM_UTF32_LE",
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
    "lookup_error",
    "make_identity_dict",
    "raw_unicode_escape_decode",
    "raw_unicode_escape_encode",
    "register",
    "register_error",
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

import sys as _sys

BOM_UTF8 = bytes(_molt_codecs_bom_utf8())
BOM_UTF16_LE = bytes(_molt_codecs_bom_utf16_le())
BOM_UTF16_BE = bytes(_molt_codecs_bom_utf16_be())
BOM_UTF32_LE = bytes(_molt_codecs_bom_utf32_le())
BOM_UTF32_BE = bytes(_molt_codecs_bom_utf32_be())
if _sys.byteorder == "little":
    BOM = BOM_UTF16 = BOM_UTF16_LE
    BOM_LE = BOM_UTF16_LE
    BOM_BE = BOM_UTF16_BE
    BOM_UTF32 = BOM_UTF32_LE
else:
    BOM = BOM_UTF16 = BOM_UTF16_BE
    BOM_LE = BOM_UTF16_LE
    BOM_BE = BOM_UTF16_BE
    BOM_UTF32 = BOM_UTF32_BE


def _lookup_builtin_name(encoding: str) -> str | None:
    name = _MOLT_CODECS_LOOKUP_NAME(encoding)
    if name is None:
        return None
    if not isinstance(name, str):
        raise RuntimeError("invalid codec lookup payload: expected str|None")
    return name


def _normalize_search_name(encoding: object) -> str:
    if not isinstance(encoding, str):
        raise TypeError(f"lookup() argument must be str, not {type(encoding).__name__}")
    out: list[str] = []
    punct = False
    for ch in encoding:
        if ch.isalnum() or ch == ".":
            if punct and out:
                out.append("_")
            out.append(ch.lower())
            punct = False
        else:
            punct = True
    return "".join(out)


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
    _encoding = "utf-8"

    def __init__(self, errors="strict"):
        self.errors = errors
        self._handle = _molt_inc_enc_new(self._encoding, errors)

    def encode(self, input, final=False):
        return _molt_inc_enc_encode(self._handle, input, final)

    def reset(self):
        _molt_inc_enc_reset(self._handle)

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _molt_inc_enc_drop(handle)
            except Exception:
                pass


class BufferedIncrementalEncoder(IncrementalEncoder):
    pass


class IncrementalDecoder:
    _encoding = "utf-8"

    def __init__(self, errors="strict"):
        self.errors = errors
        self._handle = _molt_inc_dec_new(self._encoding, errors)

    def decode(self, input, final=False):
        return _molt_inc_dec_decode(self._handle, input, final)

    def reset(self):
        _molt_inc_dec_reset(self._handle)

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _molt_inc_dec_drop(handle)
            except Exception:
                pass


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
    if not callable(search_function):
        raise TypeError("argument must be callable")
    _SEARCH_FUNCTIONS.append(search_function)
    _CODECS_CACHE.clear()


def _cache_codec_info(search_name: str, info: CodecInfo) -> CodecInfo:
    _CODECS_CACHE[search_name] = info
    info_name = getattr(info, "name", None)
    if isinstance(info_name, str) and info_name:
        _CODECS_CACHE[info_name] = info
    return info


def _coerce_codec_entry(search_name: str, entry: object) -> CodecInfo:
    if isinstance(entry, CodecInfo):
        return _cache_codec_info(search_name, entry)
    if isinstance(entry, tuple) and 4 <= len(entry) <= 7:
        return _cache_codec_info(search_name, CodecInfo(*entry))
    raise TypeError("codec search functions must return 4-tuples")


def lookup(encoding: object) -> CodecInfo:
    search_name = _normalize_search_name(encoding)
    cached = _CODECS_CACHE.get(search_name)
    if cached is not None:
        return cached

    try:
        name = _lookup_builtin_name(search_name)
    except LookupError:
        name = None

    if name is not None:
        cached = _CODECS_CACHE.get(name)
        if cached is not None:
            _CODECS_CACHE[search_name] = cached
            return cached

        info = CodecInfo(
            encode=lambda obj, errors="strict": _encode_with_consumed(
                obj, name, errors
            ),
            decode=lambda obj, errors="strict": _decode_with_consumed(
                obj, name, errors
            ),
            incrementalencoder=IncrementalEncoder,
            incrementaldecoder=IncrementalDecoder,
            streamwriter=StreamWriter,
            streamreader=StreamReader,
            name=name,
        )
        _CODECS_CACHE[name] = info
        _CODECS_CACHE[search_name] = info
        return info

    for fn in _SEARCH_FUNCTIONS:
        entry = fn(search_name)
        if entry is None:
            continue
        return _coerce_codec_entry(search_name, entry)

    raise LookupError(f"unknown encoding: {encoding}")


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


def register_error(name, error_handler):
    return _molt_codecs_register_error(name, error_handler)


def lookup_error(name):
    return _molt_codecs_lookup_error(name)


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
