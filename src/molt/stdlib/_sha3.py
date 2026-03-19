"""Intrinsic-backed `_sha3` compatibility surface."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

_GIL_MINSIZE = 2048
implementation = "molt"


class sha3_224(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha3_224", data, _hashlib._validate_options("sha3_224", {"usedforsecurity": usedforsecurity}, "sha3_224"))


class sha3_256(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha3_256", data, _hashlib._validate_options("sha3_256", {"usedforsecurity": usedforsecurity}, "sha3_256"))


class sha3_384(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha3_384", data, _hashlib._validate_options("sha3_384", {"usedforsecurity": usedforsecurity}, "sha3_384"))


class sha3_512(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("sha3_512", data, _hashlib._validate_options("sha3_512", {"usedforsecurity": usedforsecurity}, "sha3_512"))


class shake_128(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("shake_128", data, _hashlib._validate_options("shake_128", {"usedforsecurity": usedforsecurity}, "shake_128"))


class shake_256(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__("shake_256", data, _hashlib._validate_options("shake_256", {"usedforsecurity": usedforsecurity}, "shake_256"))


__all__ = [
    "_GIL_MINSIZE",
    "implementation",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
]

globals().pop("_require_intrinsic", None)
