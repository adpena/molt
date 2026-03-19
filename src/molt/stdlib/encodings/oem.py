"""Python 'oem' Codec for Windows"""

import codecs
import os


def _canonical_codecs_file(path):
    if not isinstance(path, str):
        return path
    marker = "/cpython-3.12."
    idx = path.find(marker)
    if idx < 0:
        return path
    suffix = path[idx + len(marker) :]
    dash = suffix.find("-")
    if dash < 0:
        return path
    candidate = path[:idx] + "/cpython-3.12-" + suffix[dash + 1 :]
    if os.path.exists(candidate):
        return candidate
    return path


# Keep CPython import-error semantics on non-Windows platforms.
_CODECS_FILE = _canonical_codecs_file(getattr(codecs, "__file__", None))
if not hasattr(codecs, "oem_encode"):
    raise ImportError(f"cannot import name 'oem_encode' from 'codecs' ({_CODECS_FILE})")
if not hasattr(codecs, "oem_decode"):
    raise ImportError(f"cannot import name 'oem_decode' from 'codecs' ({_CODECS_FILE})")

oem_encode = codecs.oem_encode
oem_decode = codecs.oem_decode

### Codec APIs

encode = oem_encode


def decode(input, errors="strict"):
    return oem_decode(input, errors, True)


class IncrementalEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return oem_encode(input, self.errors)[0]


class IncrementalDecoder(codecs.BufferedIncrementalDecoder):
    _buffer_decode = oem_decode


class StreamWriter(codecs.StreamWriter):
    encode = oem_encode


class StreamReader(codecs.StreamReader):
    decode = oem_decode


### encodings module API


def getregentry():
    return codecs.CodecInfo(
        name="oem",
        encode=encode,
        decode=decode,
        incrementalencoder=IncrementalEncoder,
        incrementaldecoder=IncrementalDecoder,
        streamreader=StreamReader,
        streamwriter=StreamWriter,
    )


from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
