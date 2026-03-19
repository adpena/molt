"""Intrinsic-backed `_hmac` compatibility surface."""

from __future__ import annotations

from typing import Any

import hashlib as _hashlib
import hmac as _hmac

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")


class UnknownHashError(ValueError):
    pass


HMAC = _hmac.HMAC
_GIL_MINSIZE = 2048


def _normalize_digest_name(digest: Any) -> str:
    if isinstance(digest, str):
        return digest
    raise UnknownHashError(f"unsupported hash type: {digest!r}")


def new(digest: Any, key: Any, msg: Any | None = None) -> HMAC:
    digest_name = _normalize_digest_name(digest)
    try:
        return _hmac.new(key, msg, digest_name)
    except ValueError as exc:
        raise UnknownHashError(str(exc)) from exc


def compute_digest(key: Any, msg: Any, digest: Any) -> bytes:
    return new(digest, key, msg).digest()


def compute_md5(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "md5")


def compute_sha1(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha1")


def compute_sha224(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha224")


def compute_sha256(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha256")


def compute_sha384(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha384")


def compute_sha512(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha512")


def compute_sha3_224(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha3_224")


def compute_sha3_256(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha3_256")


def compute_sha3_384(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha3_384")


def compute_sha3_512(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "sha3_512")


def compute_blake2b_32(key: Any, msg: Any) -> bytes:
    return _hmac.digest(key, msg, lambda: _hashlib.blake2b(digest_size=32))


def compute_blake2s_32(key: Any, msg: Any) -> bytes:
    return _hmac.digest(key, msg, lambda: _hashlib.blake2s(digest_size=32))


__all__ = [
    "HMAC",
    "UnknownHashError",
    "_GIL_MINSIZE",
    "compute_blake2b_32",
    "compute_blake2s_32",
    "compute_digest",
    "compute_md5",
    "compute_sha1",
    "compute_sha224",
    "compute_sha256",
    "compute_sha384",
    "compute_sha3_224",
    "compute_sha3_256",
    "compute_sha3_384",
    "compute_sha3_512",
    "compute_sha512",
    "new",
]


globals().pop("_require_intrinsic", None)
