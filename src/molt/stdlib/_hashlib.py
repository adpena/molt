"""Intrinsic-backed `_hashlib` compatibility surface."""

from __future__ import annotations

import hashlib as _hashlib
import hmac as _hmac

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

HASH = _hashlib._Hash


class HASHXOF(_hashlib._Hash):
    pass


HMAC = _hmac.HMAC
UnsupportedDigestmodError = _hashlib.UnsupportedDigestmodError

_GIL_MINSIZE = 2048

new = _hashlib.new
compare_digest = _hashlib.compare_digest
pbkdf2_hmac = _hashlib.pbkdf2_hmac
scrypt = _hashlib.scrypt
hmac_new = _hmac.new
hmac_digest = _hmac.digest

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

_constructors = {
    "md4": openssl_md4,
    "md5": openssl_md5,
    "ripemd160": openssl_ripemd160,
    "sha1": openssl_sha1,
    "sha224": openssl_sha224,
    "sha256": openssl_sha256,
    "sha384": openssl_sha384,
    "sha512": openssl_sha512,
    "sha3_224": openssl_sha3_224,
    "sha3_256": openssl_sha3_256,
    "sha3_384": openssl_sha3_384,
    "sha3_512": openssl_sha3_512,
    "shake_128": openssl_shake_128,
    "shake_256": openssl_shake_256,
    "blake2b": _hashlib.blake2b,
    "blake2s": _hashlib.blake2s,
}

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


globals().pop("_require_intrinsic", None)
