"""Real CPython crypto states for strict, host-side Molt facade probes.

Import this helper before replacing hashlib/hmac/_hashlib/_hmac in sys.modules.
The captured host callables retain their original module globals. Handles are
CPython state objects themselves; normal reference lifetime owns their storage.
This tests the facade boundary, not native exporter flags or runtime custody.
"""

from __future__ import annotations

import _hashlib as _host_native
import hashlib as _host_hashlib
import hmac as _host_hmac
import sys

_HOST_PRIVATE_HASH_NEW = _host_native.new
_HOST_HMAC_NEW = _host_native.hmac_new
_HOST_HMAC_DIGEST = _host_native.hmac_digest
_HOST_UNSUPPORTED_DIGEST = _host_native.UnsupportedDigestmodError
_HOST_COMPARE_DIGEST = _host_hmac.compare_digest
_HOST_PBKDF2_HMAC = _host_hashlib.pbkdf2_hmac
_HOST_SCRYPT = _host_hashlib.scrypt
_HOST_VERSION = tuple(sys.version_info)
_HOST_HMAC_TYPE = _host_hmac.HMAC
# Actual host states remain the oracle; older hosts lack the 3.14 _hmac module.
# This strict protocol projection uses captured constructors, never fake states.
_HOST_BUILTIN_CONSTRUCTORS = {
    name: getattr(_host_hashlib, name)
    for name in (
        "md5",
        "sha1",
        "sha224",
        "sha256",
        "sha384",
        "sha512",
        "sha3_224",
        "sha3_256",
        "sha3_384",
        "sha3_512",
        "blake2b",
        "blake2s",
    )
}
if _HOST_VERSION[:2] >= (3, 14):
    import _hmac as _host_builtin_hmac

    _HOST_BUILTIN_NEW = _host_builtin_hmac.new
    _HOST_BUILTIN_DIGEST = _host_builtin_hmac.compute_digest
    _HOST_UNKNOWN_HASH = _host_builtin_hmac.UnknownHashError
else:
    _HOST_BUILTIN_NEW = _HOST_BUILTIN_DIGEST = _HOST_UNKNOWN_HASH = None


def _hash_new(name, data, options, unsupported_error):
    if (
        unsupported_error is None
        and type(name) is str
        and name in ("blake2b", "blake2s")
    ):
        state = _HOST_BUILTIN_CONSTRUCTORS[name](data, **(options or {}))
    else:
        try:
            state = _HOST_PRIVATE_HASH_NEW(name, data, **(options or {}))
        except _HOST_UNSUPPORTED_DIGEST as error:
            if unsupported_error is None:
                raise ValueError(f"unsupported hash type {name}") from None
            raise unsupported_error(str(error)) from None
    return state, state.name, state.digest_size, state.block_size


def _pbkdf2_hmac(name, password, salt, iterations, dklen, unsupported_error):
    try:
        return _HOST_PBKDF2_HMAC(name, password, salt, iterations, dklen)
    except _HOST_UNSUPPORTED_DIGEST as error:
        raise unsupported_error(str(error)) from None


def _update(state, data):
    state.update(data)


def _copy(state):
    return state.copy()


def _hash_digest(state, length):
    return state.digest() if length is None else state.digest(length)


def _builtin_hmac(key, msg, digestmod, operation, unsupported_error, present):
    if not present:
        raise TypeError("new() missing 1 required argument 'digestmod'")
    if _HOST_BUILTIN_NEW is not None:
        try:
            if operation == 3:
                return _HOST_BUILTIN_DIGEST(key, msg, digestmod)
            state = _HOST_BUILTIN_NEW(key, msg, digestmod)
        except _HOST_UNKNOWN_HASH as error:
            raise unsupported_error(str(error)) from None
    else:

        def constructor(value):
            if not isinstance(value, str):
                raise TypeError("bad argument type for built-in operation")
            name = str.split(str.__str__(value), "\0", 1)[0]
            if name in ("sha2_224", "sha2_256", "sha2_384", "sha2_512"):
                name = "sha" + name[5:]
            elif name in ("blake2b_32", "blake2s_32"):
                name = name[:-3]
            return _HOST_BUILTIN_CONSTRUCTORS.get(name)

        digest_constructor = None
        if isinstance(digestmod, str):
            digest_constructor = constructor(digestmod)
            if digest_constructor is None:
                digest_constructor = constructor(digestmod.lower())
        if digest_constructor is None:
            raise unsupported_error(f"unsupported hash type: {digestmod!r}")
        state = _HOST_HMAC_TYPE(key, msg, digestmod=digest_constructor)
        if operation == 3:
            return state.digest()
    return state, state.name.removeprefix("hmac-"), state.digest_size, state.block_size


def _hmac_new(key, msg, digestmod, constructors, operation, unsupported_error, present):
    if type(operation) is not int or operation not in (0, 1, 2, 3):
        raise SystemError("invalid HMAC operation role")
    if type(present) is not bool:
        raise SystemError("invalid HMAC digestmod presence")
    if operation in (2, 3):
        return _builtin_hmac(key, msg, digestmod, operation, unsupported_error, present)
    if not present:
        raise TypeError("Missing required parameter 'digestmod'.")
    # Molt's declared constructor identities map to names. Unmapped native
    # callables go to CPython's own constructor validation, never a .name probe.
    resolved = (
        digestmod
        if isinstance(digestmod, str)
        else constructors.get(digestmod, digestmod)
    )
    try:
        if operation == 1:
            return _HOST_HMAC_DIGEST(key, msg, resolved)
        state = _HOST_HMAC_NEW(key, msg, resolved)
    except _HOST_UNSUPPORTED_DIGEST as error:
        raise unsupported_error(str(error)) from None
    return state, state.name.removeprefix("hmac-"), state.digest_size, state.block_size


def _hmac_digest(state):
    return state.digest()


def _scrypt(password, salt, n, r, p, maxmem, dklen):
    return _HOST_SCRYPT(password, salt=salt, n=n, r=r, p=p, maxmem=maxmem, dklen=dklen)


def _version_info():
    return _HOST_VERSION


def crypto_intrinsics():
    """The actual HashNew4/tuple4, PBKDF6 and HMAC7 contract for the existing strict registry."""
    return {
        "molt_hash_new": _hash_new,
        "molt_hash_update": _update,
        "molt_hash_copy": _copy,
        "molt_hash_digest": _hash_digest,
        "molt_compare_digest": _HOST_COMPARE_DIGEST,
        "molt_pbkdf2_hmac": _pbkdf2_hmac,
        "molt_scrypt": _scrypt,
        "molt_hmac_new": _hmac_new,
        "molt_hmac_update": _update,
        "molt_hmac_copy": _copy,
        "molt_hmac_digest": _hmac_digest,
        "molt_sys_version_info": _version_info,
    }
