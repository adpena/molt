"""Base16, Base32, Base64, Base85, and Ascii85 encodings for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

from typing import Any

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


_BYTES_TYPES = (bytes, bytearray)


def _bytes_from_decode_data(data: Any) -> bytes:
    if isinstance(data, str):
        try:
            return data.encode("ascii")
        except UnicodeEncodeError as exc:
            raise ValueError(
                "string argument should contain only ASCII characters"
            ) from exc
    if isinstance(data, _BYTES_TYPES):
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


def _bytes_from_encode_data(data: Any) -> bytes:
    if isinstance(data, _BYTES_TYPES):
        return bytes(data)
    if isinstance(data, memoryview):
        return data.tobytes()
    try:
        return memoryview(data).tobytes()
    except TypeError as exc:
        raise TypeError(
            f"a bytes-like object is required, not '{type(data).__name__}'"
        ) from exc


def _input_type_check(data: Any) -> bytes:
    try:
        view = memoryview(data)
    except TypeError as exc:
        msg = f"expected bytes-like object, not {type(data).__name__}"
        raise TypeError(msg) from exc
    if view.format not in ("c", "b", "B"):
        raise TypeError(
            "expected single byte elements, not "
            f"{view.format!r} from {type(data).__name__}"
        )
    if view.ndim != 1:
        raise TypeError(
            f"expected 1-D data, not {view.ndim}-D data from {type(data).__name__}"
        )
    return view.tobytes()


_B64_ALPHABET = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
_B64_URLSAFE = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"


def _b64_encode_bytes(data: bytes, alphabet: bytes) -> bytes:
    if not data:
        return b""
    out = bytearray()
    for idx in range(0, len(data), 3):
        chunk = data[idx : idx + 3]
        pad = 3 - len(chunk)
        val = int.from_bytes(chunk, "big") << (pad * 8)
        out.append(alphabet[(val >> 18) & 0x3F])
        out.append(alphabet[(val >> 12) & 0x3F])
        out.append(alphabet[(val >> 6) & 0x3F])
        out.append(alphabet[val & 0x3F])
        if pad:
            out[-pad:] = b"=" * pad
    return bytes(out)


def _b64_decode_bytes(data: bytes, *, validate: bool) -> bytes:
    if not data:
        return b""
    rev = {val: idx for idx, val in enumerate(_B64_ALPHABET)}
    if not validate:
        filtered = bytearray()
        for ch in data:
            if ch in rev or ch == ord("="):
                filtered.append(ch)
        data = bytes(filtered)
        data = b"".join(data.split())
    else:
        for ch in data:
            if ch in (ord("\n"), ord("\r"), ord("\t"), ord(" ")):
                raise ValueError("invalid base64 input")
            if ch not in rev and ch != ord("="):
                raise ValueError("invalid base64 input")
    if len(data) % 4:
        if validate:
            raise ValueError("incorrect padding")
        data += b"=" * ((4 - len(data) % 4) % 4)
    out = bytearray()
    for idx in range(0, len(data), 4):
        chunk = data[idx : idx + 4]
        if len(chunk) < 4:
            break
        pad = chunk.count(b"=")
        if pad and chunk[-pad:] != b"=" * pad:
            if validate:
                raise ValueError("invalid padding")
            pad = 0
        val = 0
        for ch in chunk:
            if ch == ord("="):
                val <<= 6
            else:
                val = (val << 6) | rev.get(ch, 0)
        raw = val.to_bytes(3, "big")
        if pad:
            out.extend(raw[:-pad])
        else:
            out.extend(raw)
    return bytes(out)


def b64encode(s: Any, altchars: Any | None = None) -> bytes:
    data = _bytes_from_encode_data(s)
    alphabet = _B64_ALPHABET
    if altchars is not None:
        alt = _bytes_from_decode_data(altchars)
        if len(alt) != 2:
            raise TypeError("altchars must be a length-2 bytes-like object")
        trans = bytes.maketrans(b"+/", alt)
        alphabet = alphabet.translate(trans)
    return _b64_encode_bytes(data, alphabet)


def b64decode(s: Any, altchars: Any | None = None, validate: bool = False) -> bytes:
    data = _bytes_from_decode_data(s)
    if altchars is not None:
        alt = _bytes_from_decode_data(altchars)
        if len(alt) != 2:
            raise TypeError("altchars must be a length-2 bytes-like object")
        data = data.translate(bytes.maketrans(alt, b"+/"))
    return _b64_decode_bytes(data, validate=validate)


def standard_b64encode(s: Any) -> bytes:
    return b64encode(s)


def standard_b64decode(s: Any) -> bytes:
    return b64decode(s)


def urlsafe_b64encode(s: Any) -> bytes:
    return _b64_encode_bytes(_bytes_from_encode_data(s), _B64_URLSAFE)


def urlsafe_b64decode(s: Any) -> bytes:
    data = _bytes_from_decode_data(s)
    data = data.translate(bytes.maketrans(b"-_", b"+/"))
    return _b64_decode_bytes(data, validate=False)


_B32_ALPHABET = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
_B32HEX_ALPHABET = b"0123456789ABCDEFGHIJKLMNOPQRSTUV"
_B32_REV: dict[bytes, dict[int, int]] = {}


def _b32_encode(alphabet: bytes, data: Any) -> bytes:
    raw = _bytes_from_encode_data(data)
    if not raw:
        return b""
    leftover = len(raw) % 5
    if leftover:
        raw += b"\0" * (5 - leftover)
    out = bytearray()
    for idx in range(0, len(raw), 5):
        val = int.from_bytes(raw[idx : idx + 5], "big")
        out.append(alphabet[(val >> 35) & 0x1F])
        out.append(alphabet[(val >> 30) & 0x1F])
        out.append(alphabet[(val >> 25) & 0x1F])
        out.append(alphabet[(val >> 20) & 0x1F])
        out.append(alphabet[(val >> 15) & 0x1F])
        out.append(alphabet[(val >> 10) & 0x1F])
        out.append(alphabet[(val >> 5) & 0x1F])
        out.append(alphabet[val & 0x1F])
    if leftover == 1:
        out[-6:] = b"======"
    elif leftover == 2:
        out[-4:] = b"===="
    elif leftover == 3:
        out[-3:] = b"==="
    elif leftover == 4:
        out[-1:] = b"="
    return bytes(out)


def _b32_decode(
    alphabet: bytes,
    data: Any,
    *,
    casefold: bool = False,
    map01: Any | None = None,
) -> bytes:
    raw = _bytes_from_decode_data(data)
    if len(raw) % 8:
        raise ValueError("Incorrect padding")
    if map01 is not None:
        map01_bytes = _bytes_from_decode_data(map01)
        if len(map01_bytes) != 1:
            raise ValueError("map01 must be length 1")
        raw = raw.translate(bytes.maketrans(b"01", b"O" + map01_bytes))
    if casefold:
        raw = raw.upper()
    lsize = len(raw)
    stripped = raw.rstrip(b"=")
    padchars = lsize - len(stripped)
    if alphabet not in _B32_REV:
        _B32_REV[alphabet] = {val: idx for idx, val in enumerate(alphabet)}
    rev = _B32_REV[alphabet]
    decoded = bytearray()
    acc = 0
    for idx in range(0, len(stripped), 8):
        quanta = stripped[idx : idx + 8]
        acc = 0
        try:
            for ch in quanta:
                acc = (acc << 5) + rev[ch]
        except KeyError as exc:
            raise ValueError("Non-base32 digit found") from exc
        decoded.extend(acc.to_bytes(5, "big"))
    if lsize % 8 or padchars not in {0, 1, 3, 4, 6}:
        raise ValueError("Incorrect padding")
    if padchars and decoded:
        acc <<= 5 * padchars
        last = acc.to_bytes(5, "big")
        leftover = (43 - 5 * padchars) // 8
        decoded[-5:] = last[:leftover]
    return bytes(decoded)


def b32encode(s: Any) -> bytes:
    return _b32_encode(_B32_ALPHABET, s)


def b32decode(s: Any, casefold: bool = False, map01: Any | None = None) -> bytes:
    return _b32_decode(_B32_ALPHABET, s, casefold=casefold, map01=map01)


def b32hexencode(s: Any) -> bytes:
    return _b32_encode(_B32HEX_ALPHABET, s)


def b32hexdecode(s: Any, casefold: bool = False) -> bytes:
    return _b32_decode(_B32HEX_ALPHABET, s, casefold=casefold)


def b16encode(s: Any) -> bytes:
    raw = _bytes_from_encode_data(s)
    return raw.hex().upper().encode("ascii")


def b16decode(s: Any, casefold: bool = False) -> bytes:
    raw = _bytes_from_decode_data(s)
    if casefold:
        raw = raw.upper()
    if len(raw) % 2:
        raise ValueError("Odd-length string")
    if any(ch not in b"0123456789ABCDEF" for ch in raw):
        raise ValueError("Non-base16 digit found")
    out = bytearray()
    for idx in range(0, len(raw), 2):
        out.append(int(raw[idx : idx + 2], 16))
    return bytes(out)


_A85_START = b"<~"
_A85_END = b"~>"
_A85_ALPHABET = bytes(range(33, 118))
_B85_ALPHABET = (
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    b"abcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~"
)
_B85_DECODE: list[int | None] = [None] * 256
for _idx, _ch in enumerate(_B85_ALPHABET):
    _B85_DECODE[_ch] = _idx


def _85_encode_word(value: int, alphabet: bytes) -> bytes:
    digits = bytearray(5)
    for idx in range(4, -1, -1):
        digits[idx] = alphabet[value % 85]
        value //= 85
    return bytes(digits)


def _85encode(
    data: Any,
    alphabet: bytes,
    *,
    pad: bool,
    foldnuls: bool = False,
    foldspaces: bool = False,
) -> bytes:
    raw = _bytes_from_encode_data(data)
    if not raw:
        return b""
    padding = (-len(raw)) % 4
    if padding:
        raw += b"\0" * padding
    chunks: list[bytes] = []
    for idx in range(0, len(raw), 4):
        word = int.from_bytes(raw[idx : idx + 4], "big")
        if foldnuls and word == 0:
            chunks.append(b"z")
            continue
        if foldspaces and word == 0x20202020:
            chunks.append(b"y")
            continue
        chunks.append(_85_encode_word(word, alphabet))
    if padding and not pad:
        if chunks[-1] == b"z":
            chunks[-1] = bytes([alphabet[0]]) * 5
        chunks[-1] = chunks[-1][:-padding]
    return b"".join(chunks)


def a85encode(
    b: Any,
    *,
    foldspaces: bool = False,
    wrapcol: int = 0,
    pad: bool = False,
    adobe: bool = False,
) -> bytes:
    result = _85encode(
        b,
        _A85_ALPHABET,
        pad=pad,
        foldnuls=True,
        foldspaces=foldspaces,
    )
    if adobe:
        result = _A85_START + result
    if wrapcol:
        wrapcol = max(2 if adobe else 1, wrapcol)
        chunks = [result[i : i + wrapcol] for i in range(0, len(result), wrapcol)]
        if adobe and chunks:
            if len(chunks[-1]) + 2 > wrapcol:
                chunks.append(b"")
        result = b"\n".join(chunks)
    if adobe:
        result += _A85_END
    return result


def a85decode(
    b: Any,
    *,
    foldspaces: bool = False,
    adobe: bool = False,
    ignorechars: bytes = b" \t\n\r\v",
) -> bytes:
    raw = _bytes_from_decode_data(b)
    if adobe:
        if not raw.endswith(_A85_END):
            msg = "Ascii85 encoded byte sequences must end with {!r}".format(_A85_END)
            raise ValueError(msg)
        if raw.startswith(_A85_START):
            raw = raw[2:-2]
        else:
            raw = raw[:-2]
    decoded: list[bytes] = []
    curr: list[int] = []
    for ch in raw + b"u" * 4:
        if 33 <= ch <= 117:
            curr.append(ch)
            if len(curr) == 5:
                acc = 0
                for digit in curr:
                    acc = acc * 85 + (digit - 33)
                if acc > 0xFFFFFFFF:
                    raise ValueError("Ascii85 overflow")
                decoded.append(acc.to_bytes(4, "big"))
                curr.clear()
        elif ch == ord("z"):
            if curr:
                raise ValueError("z inside Ascii85 5-tuple")
            decoded.append(b"\0\0\0\0")
        elif foldspaces and ch == ord("y"):
            if curr:
                raise ValueError("y inside Ascii85 5-tuple")
            decoded.append(b"\x20\x20\x20\x20")
        elif ch in ignorechars:
            continue
        else:
            raise ValueError(f"Non-Ascii85 digit found: {chr(ch)}")
    result = b"".join(decoded)
    padding = 4 - len(curr)
    if padding:
        result = result[:-padding]
    return result


def b85encode(b: Any, pad: bool = False) -> bytes:
    return _85encode(b, _B85_ALPHABET, pad=pad)


def b85decode(b: Any) -> bytes:
    raw = _bytes_from_decode_data(b)
    padding = (-len(raw)) % 5
    raw = raw + b"~" * padding
    out: list[bytes] = []
    for idx in range(0, len(raw), 5):
        chunk = raw[idx : idx + 5]
        acc = 0
        for jdx, ch in enumerate(chunk):
            val = _B85_DECODE[ch]
            if val is None:
                raise ValueError(f"bad base85 character at position {idx + jdx}")
            acc = acc * 85 + val
        if acc > 0xFFFFFFFF:
            raise ValueError(f"base85 overflow in hunk starting at byte {idx}")
        out.append(acc.to_bytes(4, "big"))
    result = b"".join(out)
    if padding:
        result = result[:-padding]
    return result


_Z85_ALPHABET = (
    b"0123456789abcdefghijklmnopqrstuvwxyz"
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#"
)
_Z85_B85_DIFF = b";_`|~"
_Z85_DECODE_TRANSLATION = bytes.maketrans(
    _Z85_ALPHABET + _Z85_B85_DIFF,
    _B85_ALPHABET + b"\x00" * len(_Z85_B85_DIFF),
)
_Z85_ENCODE_TRANSLATION = bytes.maketrans(_B85_ALPHABET, _Z85_ALPHABET)


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


_MAXLINESIZE = 76
_MAXBINSIZE = (_MAXLINESIZE // 4) * 3


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
    raw = _input_type_check(s)
    if not raw:
        return b""
    pieces = []
    for idx in range(0, len(raw), _MAXBINSIZE):
        chunk = raw[idx : idx + _MAXBINSIZE]
        pieces.append(b64encode(chunk) + b"\n")
    return b"".join(pieces)


def decodebytes(s: Any) -> bytes:
    raw = _input_type_check(s)
    return b64decode(raw)


def encodestring(s: Any) -> bytes:
    return encodebytes(s)


def decodestring(s: Any) -> bytes:
    return decodebytes(s)
