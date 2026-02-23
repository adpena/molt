"""Codec for quoted-printable encoding.

This codec de/encodes from bytes to bytes.
"""

from _intrinsics import require_intrinsic as _require_intrinsic

import codecs
import quopri
from io import BytesIO


_MOLT_QUOPRI_ENCODE = _require_intrinsic("molt_quopri_encode", globals())
_MOLT_QUOPRI_DECODE = _require_intrinsic("molt_quopri_decode", globals())

_CodecBase = getattr(codecs, "Codec", object)
_IncrementalEncoderBase = getattr(codecs, "IncrementalEncoder", object)
_IncrementalDecoderBase = getattr(codecs, "IncrementalDecoder", object)
_StreamWriterBase = getattr(codecs, "StreamWriter", object)
_StreamReaderBase = getattr(codecs, "StreamReader", object)
_CodecInfoFactory = getattr(codecs, "CodecInfo", None)


class _CodecInfoFallback:
    def __init__(self, **kwargs):
        self.__dict__.update(kwargs)


def quopri_encode(input, errors="strict"):
    assert errors == "strict"
    out = _MOLT_QUOPRI_ENCODE(input, True, False)
    return (bytes(out), len(input))


def quopri_decode(input, errors="strict"):
    assert errors == "strict"
    out = _MOLT_QUOPRI_DECODE(input, False)
    return (bytes(out), len(input))


class Codec(_CodecBase):
    def encode(self, input, errors="strict"):
        return quopri_encode(input, errors)

    def decode(self, input, errors="strict"):
        return quopri_decode(input, errors)


class IncrementalEncoder(_IncrementalEncoderBase):
    def __init__(self, errors="strict"):
        try:
            super().__init__(errors)  # type: ignore[misc]
        except Exception:
            self.errors = errors
        if not hasattr(self, "errors"):
            self.errors = errors

    def encode(self, input, final=False):
        return quopri_encode(input, self.errors)[0]


class IncrementalDecoder(_IncrementalDecoderBase):
    def __init__(self, errors="strict"):
        try:
            super().__init__(errors)  # type: ignore[misc]
        except Exception:
            self.errors = errors
        if not hasattr(self, "errors"):
            self.errors = errors

    def decode(self, input, final=False):
        return quopri_decode(input, self.errors)[0]


class StreamWriter(Codec, _StreamWriterBase):
    charbuffertype = bytes


class StreamReader(Codec, _StreamReaderBase):
    charbuffertype = bytes


# encodings module API


def getregentry():
    return _CodecInfoFallback(
        name="quopri",
        encode=quopri_encode,
        decode=quopri_decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamwriter=StreamWriter,
        streamreader=StreamReader,
        _is_text_encoding=False,
    )
