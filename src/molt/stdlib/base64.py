"""Base16, Base32, Base64, Base85, and Ascii85 encodings for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import Any

_molt_base64_b64encode = _require_intrinsic("molt_base64_b64encode")
_molt_base64_b64decode = _require_intrinsic("molt_base64_b64decode")
_molt_base64_standard_b64encode = _require_intrinsic("molt_base64_standard_b64encode")
_molt_base64_standard_b64decode = _require_intrinsic("molt_base64_standard_b64decode")
_molt_base64_urlsafe_b64encode = _require_intrinsic("molt_base64_urlsafe_b64encode")
_molt_base64_urlsafe_b64decode = _require_intrinsic("molt_base64_urlsafe_b64decode")
_molt_base64_b32encode = _require_intrinsic("molt_base64_b32encode")
_molt_base64_b32decode = _require_intrinsic("molt_base64_b32decode")
_molt_base64_b32hexencode = _require_intrinsic("molt_base64_b32hexencode")
_molt_base64_b32hexdecode = _require_intrinsic("molt_base64_b32hexdecode")
_molt_base64_b16encode = _require_intrinsic("molt_base64_b16encode")
_molt_base64_b16decode = _require_intrinsic("molt_base64_b16decode")
_molt_base64_a85encode = _require_intrinsic("molt_base64_a85encode")
_molt_base64_a85decode = _require_intrinsic("molt_base64_a85decode")
_molt_base64_b85encode = _require_intrinsic("molt_base64_b85encode")
_molt_base64_b85decode = _require_intrinsic("molt_base64_b85decode")
_molt_base64_encodebytes = _require_intrinsic("molt_base64_encodebytes")
_molt_base64_decodebytes = _require_intrinsic("molt_base64_decodebytes")


__all__ = [
    "encode",
    "decode",
    "encodebytes",
    "decodebytes",
    "b64encode",
    "b64decode",
    "b32encode",
    "b32decode",
    "b32hexencode",
    "b32hexdecode",
    "b16encode",
    "b16decode",
    "b85encode",
    "b85decode",
    "a85encode",
    "a85decode",
    "z85encode",
    "z85decode",
    "standard_b64encode",
    "standard_b64decode",
    "urlsafe_b64encode",
    "urlsafe_b64decode",
    "encodestring",
    "decodestring",
]

_MAXLINESIZE = 76
_MAXBINSIZE = (_MAXLINESIZE // 4) * 3

_B85_ALPHABET = (
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    b"abcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~"
)
_Z85_ALPHABET = (
    b"0123456789abcdefghijklmnopqrstuvwxyz"
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#"
)
_Z85_ENCODE_TRANSLATION = bytes.maketrans(_B85_ALPHABET, _Z85_ALPHABET)
_Z85_B85_DIFF = b";_`|~"
_Z85_DECODE_TRANSLATION = bytes.maketrans(
    _Z85_ALPHABET + _Z85_B85_DIFF,
    _B85_ALPHABET + b"\x00" * len(_Z85_B85_DIFF),
)


def _bytes_from_decode_data(data: Any) -> bytes:
    if isinstance(data, str):
        try:
            return data.encode("ascii")
        except UnicodeEncodeError as exc:
            raise ValueError(
                "string argument should contain only ASCII characters"
            ) from exc
    if isinstance(data, (bytes, bytearray)):
        return bytes(data)
    if isinstance(data, memoryview):
        return data.tobytes()
    try:
        return memoryview(data).tobytes()
    except TypeError as exc:
        raise TypeError(
            "argument should be a bytes-like object or ASCII string, "
            f"not '{type(data).__name__}'"
        ) from exc


def b64encode(s: Any, altchars: Any | None = None) -> bytes:
    return _molt_base64_b64encode(s, altchars)


def b64decode(s: Any, altchars: Any | None = None, validate: bool = False) -> bytes:
    return _molt_base64_b64decode(s, altchars, validate)


def standard_b64encode(s: Any) -> bytes:
    return _molt_base64_standard_b64encode(s)


def standard_b64decode(s: Any) -> bytes:
    return _molt_base64_standard_b64decode(s)


def urlsafe_b64encode(s: Any) -> bytes:
    return _molt_base64_urlsafe_b64encode(s)


def urlsafe_b64decode(s: Any) -> bytes:
    return _molt_base64_urlsafe_b64decode(s)


def b32encode(s: Any) -> bytes:
    return _molt_base64_b32encode(s)


def b32decode(s: Any, casefold: bool = False, map01: Any | None = None) -> bytes:
    return _molt_base64_b32decode(s, casefold, map01)


def b32hexencode(s: Any) -> bytes:
    return _molt_base64_b32hexencode(s)


def b32hexdecode(s: Any, casefold: bool = False) -> bytes:
    return _molt_base64_b32hexdecode(s, casefold)


def b16encode(s: Any) -> bytes:
    return _molt_base64_b16encode(s)


def b16decode(s: Any, casefold: bool = False) -> bytes:
    return _molt_base64_b16decode(s, casefold)


def a85encode(
    b: Any,
    *,
    foldspaces: bool = False,
    wrapcol: int = 0,
    pad: bool = False,
    adobe: bool = False,
) -> bytes:
    return _molt_base64_a85encode(b, foldspaces, wrapcol, pad, adobe)


def a85decode(
    b: Any,
    *,
    foldspaces: bool = False,
    adobe: bool = False,
    ignorechars: bytes = b" \t\n\r\v",
) -> bytes:
    return _molt_base64_a85decode(b, foldspaces, adobe)


def b85encode(b: Any, pad: bool = False) -> bytes:
    return _molt_base64_b85encode(b, pad)


def b85decode(b: Any) -> bytes:
    return _molt_base64_b85decode(b)


def z85encode(s: Any) -> bytes:
    return b85encode(s).translate(_Z85_ENCODE_TRANSLATION)


def z85decode(s: Any) -> bytes:
    raw = _bytes_from_decode_data(s)
    raw = raw.translate(_Z85_DECODE_TRANSLATION)
    try:
        return b85decode(raw)
    except ValueError as exc:
        message = exc.args[0].replace("base85", "z85")
        raise ValueError(message) from None


def encode(input, output) -> None:
    while True:
        chunk = input.read(_MAXBINSIZE)
        if not chunk:
            break
        output.write(b64encode(chunk) + b"\n")


def decode(input, output) -> None:
    while True:
        line = input.readline()
        if not line:
            break
        output.write(b64decode(line))


def encodebytes(s: Any) -> bytes:
    return _molt_base64_encodebytes(s)


def decodebytes(s: Any) -> bytes:
    return _molt_base64_decodebytes(s)


def encodestring(s: Any) -> bytes:
    return encodebytes(s)


def decodestring(s: Any) -> bytes:
    return decodebytes(s)


globals().pop("_require_intrinsic", None)
