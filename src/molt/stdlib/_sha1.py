"""Intrinsic-backed `_sha1` compatibility surface."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

_GIL_MINSIZE = 2048


class sha1(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__(
            "sha1",
            data,
            _hashlib._validate_options(
                "sha1", {"usedforsecurity": usedforsecurity}, "sha1"
            ),
        )


SHA1Type = sha1

__all__ = ["SHA1Type", "_GIL_MINSIZE", "sha1"]

globals().pop("_require_intrinsic", None)
