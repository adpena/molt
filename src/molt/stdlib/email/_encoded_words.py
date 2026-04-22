"""Public API surface shim for ``email._encoded_words``."""

from __future__ import annotations

import base64

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_EMAIL_HEADER_ENCODE_WORD = _require_intrinsic("molt_email_header_encode_word")


def decode_q(encoded: bytes) -> bytes:
    return encoded.replace(b"_", b" ")


def encode_q(data: bytes) -> bytes:
    return data.replace(b" ", b"_")


def len_q(data: bytes) -> int:
    return len(encode_q(data))


def decode_b(encoded: bytes) -> bytes:
    try:
        return base64.b64decode(encoded, validate=False)
    except Exception:
        return b""


def encode_b(data: bytes) -> bytes:
    try:
        return base64.b64encode(data)
    except Exception:
        return b""


def len_b(data: bytes) -> int:
    return len(encode_b(data))


def decode(ew: str):
    return ("", ew, None, [])


def encode(text: str, charset: str = "utf-8", encoding: str | None = None):
    del encoding
    return _MOLT_EMAIL_HEADER_ENCODE_WORD(text, charset)


globals().pop("_require_intrinsic", None)
