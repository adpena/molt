"""Intrinsic-backed `_sha2` compatibility surface."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib


_GIL_MINSIZE = 2048


class sha224(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__(
            "sha224",
            data,
            {"usedforsecurity": usedforsecurity},
        )


class sha256(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__(
            "sha256",
            data,
            {"usedforsecurity": usedforsecurity},
        )


class sha384(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__(
            "sha384",
            data,
            {"usedforsecurity": usedforsecurity},
        )


class sha512(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__(
            "sha512",
            data,
            {"usedforsecurity": usedforsecurity},
        )


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
