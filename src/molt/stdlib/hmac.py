"""HMAC implementation backed by Rust intrinsics."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib
from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = ["HMAC", "new", "digest", "compare_digest"]

_molt_hmac_new = _require_intrinsic("molt_hmac_new", globals())
_molt_hmac_update = _require_intrinsic("molt_hmac_update", globals())
_molt_hmac_copy = _require_intrinsic("molt_hmac_copy", globals())
_molt_hmac_digest = _require_intrinsic("molt_hmac_digest", globals())
_molt_hmac_drop = _require_intrinsic("molt_hmac_drop", globals())
_molt_compare_digest = _require_intrinsic("molt_compare_digest", globals())


def _resolve_digestmod(digestmod: Any) -> tuple[str, dict[str, Any] | None, int, int]:
    if digestmod is None:
        raise TypeError("Missing required argument 'digestmod'.")
    if isinstance(digestmod, str):
        digest = _hashlib.new(digestmod)
    elif callable(digestmod):
        digest = digestmod()
    else:
        digest_new = getattr(digestmod, "new")
        digest = digest_new()
    if not isinstance(digest, _hashlib._Hash):
        raise TypeError("digestmod must be a name or callable")
    return digest.name, digest._options, digest.digest_size, digest.block_size


class HMAC:
    __slots__ = (
        "_handle",
        "_digest_name",
        "_options",
        "name",
        "digest_size",
        "block_size",
    )

    def __init__(self, key: Any, msg: Any | None, digestmod: Any) -> None:
        digest_name, options, digest_size, block_size = _resolve_digestmod(digestmod)
        self._handle = _molt_hmac_new(key, msg, digest_name, options)
        self._digest_name = digest_name
        self._options = options
        self.name = f"hmac-{digest_name}"
        self.digest_size = digest_size
        self.block_size = block_size

    def update(self, msg: Any) -> None:
        _molt_hmac_update(self._handle, msg)

    def copy(self) -> "HMAC":
        other = object.__new__(type(self))
        other._handle = _molt_hmac_copy(self._handle)
        other._digest_name = self._digest_name
        other._options = self._options
        other.name = self.name
        other.digest_size = self.digest_size
        other.block_size = self.block_size
        return other

    def digest(self) -> bytes:
        return _molt_hmac_digest(self._handle)

    def hexdigest(self) -> str:
        return self.digest().hex()

    def __del__(self) -> None:
        try:
            _molt_hmac_drop(self._handle)
        except Exception:
            pass


def new(key: Any, msg: Any | None = None, digestmod: Any | None = None) -> HMAC:
    if digestmod is None:
        raise TypeError("Missing required argument 'digestmod'.")
    return HMAC(key, msg, digestmod)


def digest(key: Any, msg: Any, digestmod: Any) -> bytes:
    return new(key, msg, digestmod).digest()


def compare_digest(a: Any, b: Any) -> bool:
    return bool(_molt_compare_digest(a, b))
