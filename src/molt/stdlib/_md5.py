"""Intrinsic-backed `_md5` compatibility surface."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

_GIL_MINSIZE = 2048


class md5(_hashlib._Hash):
    def __init__(self, data: Any = b"", *, usedforsecurity: bool = True) -> None:
        super().__init__(
            "md5",
            data,
            _hashlib._validate_options(
                "md5", {"usedforsecurity": usedforsecurity}, "md5"
            ),
        )


MD5Type = md5

__all__ = ["MD5Type", "_GIL_MINSIZE", "md5"]


globals().pop("_require_intrinsic", None)
