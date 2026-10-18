"""Intrinsic-backed ``binascii`` surface."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


class Error(ValueError):
    pass


class Incomplete(Exception):
    pass


# Callables that never raise ``Error`` stay intrinsic-backed builtins, so
# API-shape probes observe CPython-like ``builtin_function_or_method`` entries.
# A callable that raises ``Error`` passes this module's class to its intrinsic,
# so ``base64`` and every other caller see one identity.
_a2b_base64 = _require_intrinsic("molt_binascii_a2b_base64")
b2a_base64 = _require_intrinsic("molt_binascii_b2a_base64")
_a2b_hex = _require_intrinsic("molt_binascii_a2b_hex")
b2a_hex = _require_intrinsic("molt_binascii_b2a_hex")
a2b_qp = _require_intrinsic("molt_binascii_a2b_qp")
b2a_qp = _require_intrinsic("molt_binascii_b2a_qp")
_a2b_uu = _require_intrinsic("molt_binascii_a2b_uu")
_b2a_uu = _require_intrinsic("molt_binascii_b2a_uu")
_crc32 = _require_intrinsic("molt_binascii_crc32")
crc_hqx = _require_intrinsic("molt_binascii_crc_hqx")
_unhexlify = _require_intrinsic("molt_binascii_unhexlify")


def a2b_base64(data, /, *, strict_mode=False):
    """Decode a line of base64 data."""
    return _a2b_base64(Error, data, strict_mode)


def a2b_hex(hexstr, /):
    """Binary data of hexadecimal representation."""
    return _a2b_hex(Error, hexstr)


def unhexlify(hexstr, /):
    """Binary data of hexadecimal representation."""
    return _unhexlify(Error, hexstr)


def a2b_uu(data, /):
    """Decode a line of uuencoded data."""
    return _a2b_uu(Error, data)


def b2a_uu(data, /):
    """Uuencode line of data."""
    return _b2a_uu(Error, data)


def crc32(data, value=0, /):
    """Compute a CRC-32 of *data*, continuing from the running *value*."""
    return _crc32(data, value)


hexlify = _require_intrinsic("molt_binascii_hexlify")

__all__ = [
    "Error",
    "Incomplete",
    "a2b_base64",
    "a2b_hex",
    "a2b_qp",
    "a2b_uu",
    "b2a_base64",
    "b2a_hex",
    "b2a_qp",
    "b2a_uu",
    "crc32",
    "crc_hqx",
    "hexlify",
    "unhexlify",
]

globals().pop("_require_intrinsic", None)
