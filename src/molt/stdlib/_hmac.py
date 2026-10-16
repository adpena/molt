"""Intrinsic-backed `_hmac` compatibility surface."""

from __future__ import annotations

from typing import Any

import hmac as _hmac


class UnknownHashError(ValueError):
    pass


_UnknownHashError = UnknownHashError


HMAC = _hmac.HMAC
_GIL_MINSIZE = 2048


_MISSING = object()


def new(key: Any, msg: Any = None, digestmod: Any = _MISSING) -> HMAC:
    state = _hmac._molt_hmac_new(
        key, msg, digestmod, None, _hmac._BUILTIN_NEW, _UnknownHashError,
        digestmod is not _MISSING,
    )
    result = object.__new__(HMAC)
    result._set_native(state)
    return result


def compute_digest(key: Any, msg: Any, digest: Any) -> bytes:
    return _hmac._molt_hmac_new(
        key, msg, digest, None, _hmac._BUILTIN_SINGLESHOT, _UnknownHashError, True,
    )


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
    return compute_digest(key, msg, "blake2b")


def compute_blake2s_32(key: Any, msg: Any) -> bytes:
    return compute_digest(key, msg, "blake2s")


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
