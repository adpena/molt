"""Python 'uu_codec' Codec - UU content transfer encoding.

This codec de/encodes from bytes to bytes.

Written by Marc-Andre Lemburg (mal@lemburg.com). Some details were
adapted from uu.py which was written by Lance Ellinghouse and
modified by Jack Jansen and Fredrik Lundh.
"""

import codecs

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())
_uu_encode = _require_intrinsic("molt_uu_codec_encode", globals())
_uu_decode = _require_intrinsic("molt_uu_codec_decode", globals())

### Codec APIs


def uu_encode(input, errors="strict", filename="<data>", mode=0o666):
    assert errors == "strict"
    return (_uu_encode(input, filename, mode), len(input))


def uu_decode(input, errors="strict"):
    assert errors == "strict"
    return (_uu_decode(input), len(input))


class Codec(codecs.Codec):
    def encode(self, input, errors="strict"):
        return uu_encode(input, errors)

    def decode(self, input, errors="strict"):
        return uu_decode(input, errors)


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return uu_encode(input, self.errors)[0]


class IncrementalDecoder(codecs.IncrementalDecoder):
    def decode(self, input, final=False):
        return uu_decode(input, self.errors)[0]


class StreamWriter(Codec, codecs.StreamWriter):
    charbuffertype = bytes


class StreamReader(Codec, codecs.StreamReader):
    charbuffertype = bytes


### encodings module API


def getregentry():
    return codecs.CodecInfo(
        name="uu",
        encode=uu_encode,
        decode=uu_decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamreader=StreamReader,
        streamwriter=StreamWriter,
        _is_text_encoding=False,
    )
