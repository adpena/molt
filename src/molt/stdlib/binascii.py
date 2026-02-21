"""Intrinsic-backed ``binascii`` surface."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class Error(ValueError):
    pass


class Incomplete(Error):
    pass


# Keep public callables as intrinsic-backed builtins so API-shape probes
# observe CPython-like ``builtin_function_or_method`` entries.
a2b_base64 = _require_intrinsic("molt_binascii_a2b_base64", globals())
b2a_base64 = _require_intrinsic("molt_binascii_b2a_base64", globals())
a2b_hex = _require_intrinsic("molt_binascii_a2b_hex", globals())
b2a_hex = _require_intrinsic("molt_binascii_b2a_hex", globals())
a2b_qp = _require_intrinsic("molt_binascii_a2b_qp", globals())
b2a_qp = _require_intrinsic("molt_binascii_b2a_qp", globals())
a2b_uu = _require_intrinsic("molt_binascii_a2b_uu", globals())
b2a_uu = _require_intrinsic("molt_binascii_b2a_uu", globals())
crc32 = _require_intrinsic("molt_binascii_crc32", globals())
crc_hqx = _require_intrinsic("molt_binascii_crc_hqx", globals())

hexlify = b2a_hex
unhexlify = a2b_hex

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
