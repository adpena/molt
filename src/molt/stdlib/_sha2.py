"""Intrinsic-backed `_sha2` compatibility surface."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

_GIL_MINSIZE = 2048


class sha224(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha224", data, _hashlib._validate_options("sha224", {"usedforsecurity": usedforsecurity}, "sha224"))


class sha256(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha256", data, _hashlib._validate_options("sha256", {"usedforsecurity": usedforsecurity}, "sha256"))


class sha384(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha384", data, _hashlib._validate_options("sha384", {"usedforsecurity": usedforsecurity}, "sha384"))


class sha512(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha512", data, _hashlib._validate_options("sha512", {"usedforsecurity": usedforsecurity}, "sha512"))


SHA224Type = sha224
SHA256Type = sha256
SHA384Type = sha384
SHA512Type = sha512

__all__ = [
    "SHA224Type",
    "SHA256Type",
    "SHA384Type",
    "SHA512Type",
    "_GIL_MINSIZE",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
]
