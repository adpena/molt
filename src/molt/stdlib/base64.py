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
bytes_types = (bytes, bytearray)
_urlsafe_decode_translation = None

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


def _bytes_from_decode_data(s):
    if isinstance(s, str):
        try:
            return s.encode("ascii")
        except UnicodeEncodeError:
            raise ValueError("string argument should contain only ASCII characters")
    if isinstance(s, bytes_types):
        return s
    try:
        return memoryview(s).tobytes()
    except TypeError:
        raise TypeError(
            "argument should be a bytes-like object or ASCII "
            "string, not %r" % s.__class__.__name__
        ) from None


def b64encode(s: Any, altchars: Any | None = None) -> bytes:
    # An exact immutable two-byte alphabet cannot run user code or change while
    # data is exported. Preserve the single-allocation native encoding path.
    if altchars is None or (type(altchars) is bytes and len(altchars) == 2):
        return _molt_base64_b64encode(s, altchars)
    encoded = _molt_base64_standard_b64encode(s)
    assert len(altchars) == 2, repr(altchars)
    return encoded.translate(bytes.maketrans(b"+/", altchars))


def b64decode(s: Any, altchars: Any | None = None, validate: bool = False) -> bytes:
    kind = type(s)
    if (
        (kind is bytes or kind is bytearray or kind is str)
        and type(validate) is bool
        and (
            altchars is None
            or (
                type(altchars) is bytes
                and len(altchars) == 2
                and altchars[0] != altchars[1]
            )
        )
    ):
        return _molt_base64_b64decode(s, altchars, validate)
    import binascii

    s = _bytes_from_decode_data(s)
    if altchars is not None:
        altchars = _bytes_from_decode_data(altchars)
        assert len(altchars) == 2, repr(altchars)
        s = s.translate(bytes.maketrans(altchars, b"+/"))
    return binascii.a2b_base64(s, strict_mode=validate)


def standard_b64encode(s: Any) -> bytes:
    return _molt_base64_standard_b64encode(s)


def standard_b64decode(s: Any) -> bytes:
    kind = type(s)
    if kind is bytes or kind is bytearray or kind is str:
        return _molt_base64_standard_b64decode(s)
    return b64decode(s)


def urlsafe_b64encode(s: Any) -> bytes:
    return _molt_base64_urlsafe_b64encode(s)


def urlsafe_b64decode(s: Any) -> bytes:
    kind = type(s)
    if kind is bytes or kind is bytearray or kind is str:
        return _molt_base64_urlsafe_b64decode(s)
    global _urlsafe_decode_translation
    if _urlsafe_decode_translation is None:
        _urlsafe_decode_translation = bytes.maketrans(b"-_", b"+/")
    s = _bytes_from_decode_data(s)
    return b64decode(s.translate(_urlsafe_decode_translation))


def b32encode(s: Any) -> bytes:
    return _molt_base64_b32encode(s)


def b32decode(s: Any, casefold: bool = False, map01: Any | None = None) -> bytes:
    kind = type(s)
    if (
        (kind is bytes or kind is bytearray or kind is str)
        and type(casefold) is bool
        and map01 is None
    ):
        decoded = _molt_base64_b32decode(s, casefold)
        if decoded is not None:
            return decoded
    return _b32decode(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567", s, casefold, map01)


def b32hexencode(s: Any) -> bytes:
    return _molt_base64_b32hexencode(s)


def b32hexdecode(s: Any, casefold: bool = False) -> bytes:
    kind = type(s)
    if (kind is bytes or kind is bytearray or kind is str) and type(casefold) is bool:
        decoded = _molt_base64_b32hexdecode(s, casefold)
        if decoded is not None:
            return decoded
    return _b32decode(b"0123456789ABCDEFGHIJKLMNOPQRSTUV", s, casefold)


def b16encode(s: Any) -> bytes:
    return _molt_base64_b16encode(s)


def b16decode(s: Any, casefold: bool = False) -> bytes:
    kind = type(s)
    if (kind is bytes or kind is bytearray or kind is str) and type(casefold) is bool:
        decoded = _molt_base64_b16decode(s, casefold)
        if decoded is not None:
            return decoded
    return _generic_b16decode(s, casefold)


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
    kind = type(b)
    if (
        (kind is bytes or kind is bytearray or kind is str)
        and type(foldspaces) is bool
        and type(adobe) is bool
        and type(ignorechars) is bytes
        and ignorechars == b" \t\n\r\v"
    ):
        return _molt_base64_a85decode(b, foldspaces, adobe)
    return _generic_a85decode(
        b, foldspaces=foldspaces, adobe=adobe, ignorechars=ignorechars
    )


def b85encode(b: Any, pad: bool = False) -> bytes:
    return _molt_base64_b85encode(b, pad)


def b85decode(b: Any) -> bytes:
    kind = type(b)
    if kind is bytes or kind is bytearray or kind is str:
        return _molt_base64_b85decode(b)
    return _generic_b85decode(b)


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
    import binascii

    while True:
        line = input.readline()
        if not line:
            break
        output.write(binascii.a2b_base64(line))


def _input_type_check(s: Any) -> None:
    # CPython's legacy-interface check: a 1-D buffer of single bytes, never str.
    try:
        m = memoryview(s)
    except TypeError as err:
        msg = "expected bytes-like object, not %s" % s.__class__.__name__
        raise TypeError(msg) from err
    if m.format not in ("c", "b", "B"):
        msg = "expected single byte elements, not %r from %s" % (
            m.format,
            s.__class__.__name__,
        )
        raise TypeError(msg)
    if m.ndim != 1:
        msg = "expected 1-D data, not %d-D data from %s" % (
            m.ndim,
            s.__class__.__name__,
        )
        raise TypeError(msg)


def encodebytes(s: Any) -> bytes:
    _input_type_check(s)
    if type(s) is bytes or type(s) is bytearray:
        return _molt_base64_encodebytes(s)
    import binascii

    return b"".join(
        binascii.b2a_base64(s[i : i + _MAXBINSIZE])
        for i in range(0, len(s), _MAXBINSIZE)
    )


def decodebytes(s: Any) -> bytes:
    _input_type_check(s)
    return _molt_base64_decodebytes(s)


def encodestring(s: Any) -> bytes:
    return encodebytes(s)


def decodestring(s: Any) -> bytes:
    return decodebytes(s)


globals().pop("_require_intrinsic", None)


# Generic operand semantics follow the pinned CPython 3.12.13 source. Native
# kernels above specialize only proven exact operands; user protocol results
# remain objects until the source operation consumes them.
# https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Lib/base64.py
_b32rev = {}
_b85dec = None
_A85START = b"<~"
_A85END = b"~>"
_b85alphabet = _B85_ALPHABET


def _b32decode(alphabet, s, casefold=False, map01=None):
    import binascii

    global _b32rev
    # Delay the initialization of the table to not waste memory
    # if the function is never called
    if alphabet not in _b32rev:
        _b32rev[alphabet] = {v: k for k, v in enumerate(alphabet)}
    s = _bytes_from_decode_data(s)
    if len(s) % 8:
        raise binascii.Error("Incorrect padding")
    # Handle section 2.4 zero and one mapping.  The flag map01 will be either
    # False, or the character to map the digit 1 (one) to.  It should be
    # either L (el) or I (eye).
    if map01 is not None:
        map01 = _bytes_from_decode_data(map01)
        assert len(map01) == 1, repr(map01)
        s = s.translate(bytes.maketrans(b"01", b"O" + map01))
    if casefold:
        s = s.upper()
    # Strip off pad characters from the right.  We need to count the pad
    # characters because this will tell us how many null bytes to remove from
    # the end of the decoded string.
    l = len(s)
    s = s.rstrip(b"=")
    padchars = l - len(s)
    # Now decode the full quanta
    decoded = bytearray()
    b32rev = _b32rev[alphabet]
    for i in range(0, len(s), 8):
        quanta = s[i : i + 8]
        acc = 0
        try:
            for c in quanta:
                acc = (acc << 5) + b32rev[c]
        except KeyError:
            raise binascii.Error("Non-base32 digit found") from None
        decoded += acc.to_bytes(5)  # big endian
    # Process the last, partial quanta
    if l % 8 or padchars not in {0, 1, 3, 4, 6}:
        raise binascii.Error("Incorrect padding")
    if padchars and decoded:
        acc <<= 5 * padchars
        last = acc.to_bytes(5)  # big endian
        leftover = (43 - 5 * padchars) // 8  # 1: 4, 3: 3, 4: 2, 6: 1
        decoded[-5:] = last[:leftover]
    return bytes(decoded)


def _generic_b16decode(s, casefold=False):
    """Decode the Base16 encoded bytes-like object or ASCII string s.

    Optional casefold is a flag specifying whether a lowercase alphabet is
    acceptable as input.  For security purposes, the default is False.

    The result is returned as a bytes object.  A binascii.Error is raised if
    s is incorrectly padded or if there are non-alphabet characters present
    in the input.
    """

    import binascii
    import sys

    s = _bytes_from_decode_data(s)
    if casefold:
        s = s.upper()
    if sys.version_info >= (3, 14):
        if s.translate(None, delete=b"0123456789ABCDEF"):
            raise binascii.Error("Non-base16 digit found")
    else:
        import re

        if re.search(b"[^0-9A-F]", s):
            raise binascii.Error("Non-base16 digit found")
    return binascii.unhexlify(s)


def _generic_a85decode(b, *, foldspaces=False, adobe=False, ignorechars=b" \t\n\r\v"):
    """Decode the Ascii85 encoded bytes-like object or ASCII string b.

    foldspaces is a flag that specifies whether the 'y' short sequence should be
    accepted as shorthand for 4 consecutive spaces (ASCII 0x20). This feature is
    not supported by the "standard" Adobe encoding.

    adobe controls whether the input sequence is in Adobe Ascii85 format (i.e.
    is framed with <~ and ~>).

    ignorechars should be a byte string containing characters to ignore from the
    input. This should only contain whitespace characters, and by default
    contains all whitespace characters in ASCII.

    The result is returned as a bytes object.
    """

    import struct

    b = _bytes_from_decode_data(b)
    if adobe:
        if not b.endswith(_A85END):
            raise ValueError(
                "Ascii85 encoded byte sequences must end with {!r}".format(_A85END)
            )
        if b.startswith(_A85START):
            b = b[2:-2]  # Strip off start/end markers
        else:
            b = b[:-2]
    #
    # We have to go through this stepwise, so as to ignore spaces and handle
    # special short sequences
    #
    packI = struct.Struct("!I").pack
    decoded = []
    decoded_append = decoded.append
    curr = []
    curr_append = curr.append
    curr_clear = curr.clear
    for x in b + b"u" * 4:
        if b"!"[0] <= x <= b"u"[0]:
            curr_append(x)
            if len(curr) == 5:
                acc = 0
                for x in curr:
                    acc = 85 * acc + (x - 33)
                try:
                    decoded_append(packI(acc))
                except struct.error:
                    raise ValueError("Ascii85 overflow") from None
                curr_clear()
        elif x == b"z"[0]:
            if curr:
                raise ValueError("z inside Ascii85 5-tuple")
            decoded_append(b"\0\0\0\0")
        elif foldspaces and x == b"y"[0]:
            if curr:
                raise ValueError("y inside Ascii85 5-tuple")
            decoded_append(b"\x20\x20\x20\x20")
        elif x in ignorechars:
            # Skip whitespace
            continue
        else:
            raise ValueError("Non-Ascii85 digit found: %c" % x)

    result = b"".join(decoded)
    padding = 4 - len(curr)
    if padding:
        # Throw away the extra padding
        result = result[:-padding]
    return result


def _generic_b85decode(b):
    """Decode the base85-encoded bytes-like object or ASCII string b

    The result is returned as a bytes object.
    """

    import struct

    global _b85dec
    # Delay the initialization of tables to not waste memory
    # if the function is never called
    if _b85dec is None:
        # Publish a complete table, as CPython 3.13+ does for concurrent calls.
        table = [None] * 256
        for i, c in enumerate(_b85alphabet):
            table[c] = i
        _b85dec = table

    b = _bytes_from_decode_data(b)
    padding = (-len(b)) % 5
    b = b + b"~" * padding
    out = []
    packI = struct.Struct("!I").pack
    for i in range(0, len(b), 5):
        chunk = b[i : i + 5]
        acc = 0
        try:
            for c in chunk:
                acc = acc * 85 + _b85dec[c]
        except TypeError:
            for j, c in enumerate(chunk):
                if _b85dec[c] is None:
                    raise ValueError(
                        "bad base85 character at position %d" % (i + j)
                    ) from None
            raise
        try:
            out.append(packI(acc))
        except struct.error:
            raise ValueError(
                "base85 overflow in hunk starting at byte %d" % i
            ) from None

    result = b"".join(out)
    if padding:
        result = result[:-padding]
    return result
