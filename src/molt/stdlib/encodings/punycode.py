"""Codec for the Punycode encoding, as specified in RFC 3492

Written by Martin v. Löwis.
"""

import codecs

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_punycode_encode = _require_intrinsic("molt_punycode_encode")
_punycode_decode = _require_intrinsic("molt_punycode_decode")


def punycode_encode(text):
    return _punycode_encode(text)


def punycode_decode(text, errors):
    return _punycode_decode(text, errors)


### Codec APIs


class Codec(codecs.Codec):
    def encode(self, input, errors="strict"):
        res = punycode_encode(input)
        return res, len(input)

    def decode(self, input, errors="strict"):
        if errors not in ("strict", "replace", "ignore"):
            raise UnicodeError("Unsupported error handling " + errors)
        res = punycode_decode(input, errors)
        return res, len(input)


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return punycode_encode(input)


class IncrementalDecoder(codecs.IncrementalDecoder):
    def decode(self, input, final=False):
        if self.errors not in ("strict", "replace", "ignore"):
            raise UnicodeError("Unsupported error handling " + self.errors)
        return punycode_decode(input, self.errors)


class StreamWriter(Codec, codecs.StreamWriter):
    pass


class StreamReader(Codec, codecs.StreamReader):
    pass


### encodings module API


def getregentry():
    return codecs.CodecInfo(
        name="punycode",
        encode=Codec().encode,
        decode=Codec().decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamwriter=StreamWriter,
        streamreader=StreamReader,
    )


globals().pop("_require_intrinsic", None)
