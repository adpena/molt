"""Python 'mbcs' Codec for Windows


Cloned by Mark Hammond (mhammond@skippinet.com.au) from ascii.py,
which was written by Marc-Andre Lemburg (mal@lemburg.com).

(c) Copyright CNRI, All Rights Reserved. NO WARRANTY.

"""

import codecs

# Keep CPython import-error semantics on non-Windows platforms.
if not hasattr(codecs, "mbcs_encode"):
    raise ImportError(
        f"cannot import name 'mbcs_encode' from 'codecs' ({getattr(codecs, '__file__', None)})"
    )
if not hasattr(codecs, "mbcs_decode"):
    raise ImportError(
        f"cannot import name 'mbcs_decode' from 'codecs' ({getattr(codecs, '__file__', None)})"
    )

mbcs_encode = codecs.mbcs_encode
mbcs_decode = codecs.mbcs_decode

### Codec APIs

encode = mbcs_encode


def decode(input, errors="strict"):
    return mbcs_decode(input, errors, True)


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return mbcs_encode(input, self.errors)[0]


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    _buffer_decode = mbcs_decode


class StreamWriter(codecs.StreamWriter):
    encode = mbcs_encode


class StreamReader(codecs.StreamReader):
    decode = mbcs_decode


### encodings module API


def getregentry():
    return codecs.CodecInfo(
        name="mbcs",
        encode=encode,
        decode=decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamreader=StreamReader,
        streamwriter=StreamWriter,
    )


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())
