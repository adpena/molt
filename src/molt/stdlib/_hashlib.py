"""Intrinsic-backed `_hashlib` compatibility surface."""

from __future__ import annotations

import hashlib as _hashlib
import hmac as _hmac
from types import MappingProxyType as _MappingProxyType


HASH = _hashlib._Hash


class HASHXOF(_hashlib._Hash):
    pass


HMAC = _hmac.HMAC
_UnsupportedDigestmodError = _hashlib._UnsupportedDigestmodError
UnsupportedDigestmodError = _UnsupportedDigestmodError

_GIL_MINSIZE = 2048

def new(name, data=b"", *, usedforsecurity=True):
    return _hashlib._Hash(
        name, data, {"usedforsecurity": usedforsecurity}, _UnsupportedDigestmodError
    )


compare_digest = _hashlib.compare_digest
pbkdf2_hmac = _hashlib.pbkdf2_hmac
scrypt = _hashlib.scrypt
_MISSING = object()


def hmac_new(key, msg=b"", digestmod=_MISSING):
    state = _hmac._native_operation(
        key, msg, digestmod, _hmac._NEW, digestmod is not _MISSING
    )
    result = object.__new__(HMAC)
    result._set_native(state)
    return result


def hmac_digest(key, msg, digest):
    return _hmac._native_operation(key, msg, digest, _hmac._SINGLESHOT)


openssl_md4 = _hashlib.md4
openssl_md5 = _hashlib.md5
openssl_ripemd160 = _hashlib.ripemd160
openssl_sha1 = _hashlib.sha1
openssl_sha224 = _hashlib.sha224
openssl_sha256 = _hashlib.sha256
openssl_sha384 = _hashlib.sha384
openssl_sha512 = _hashlib.sha512
openssl_sha3_224 = _hashlib.sha3_224
openssl_sha3_256 = _hashlib.sha3_256
openssl_sha3_384 = _hashlib.sha3_384
openssl_sha3_512 = _hashlib.sha3_512
openssl_shake_128 = _hashlib.shake_128
openssl_shake_256 = _hashlib.shake_256

_constructor_table = {
    openssl_md4: "md4",
    openssl_md5: "md5",
    openssl_ripemd160: "ripemd160",
    openssl_sha1: "sha1",
    openssl_sha224: "sha224",
    openssl_sha256: "sha256",
    openssl_sha384: "sha384",
    openssl_sha512: "sha512",
    openssl_sha3_224: "sha3_224",
    openssl_sha3_256: "sha3_256",
    openssl_sha3_384: "sha3_384",
    openssl_sha3_512: "sha3_512",
    openssl_shake_128: "shake_128",
    openssl_shake_256: "shake_256",
}
_constructors = _MappingProxyType(_constructor_table)

openssl_md_meth_names = frozenset(_hashlib.algorithms_available)


def get_fips_mode() -> int:
    return 0


__all__ = [
    "HASH",
    "HASHXOF",
    "HMAC",
    "UnsupportedDigestmodError",
    "_GIL_MINSIZE",
    "_constructors",
    "compare_digest",
    "get_fips_mode",
    "hmac_digest",
    "hmac_new",
    "new",
    "openssl_md4",
    "openssl_md5",
    "openssl_md_meth_names",
    "openssl_ripemd160",
    "openssl_sha1",
    "openssl_sha224",
    "openssl_sha256",
    "openssl_sha384",
    "openssl_sha3_224",
    "openssl_sha3_256",
    "openssl_sha3_384",
    "openssl_sha3_512",
    "openssl_sha512",
    "openssl_shake_128",
    "openssl_shake_256",
    "pbkdf2_hmac",
    "scrypt",
]
