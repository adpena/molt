"""HMAC using native hash states and the PEP 247 digest-object protocol."""

from __future__ import annotations

from typing import Any
from types import BuiltinFunctionType as _builtin_function_type
import warnings as _warnings
import hashlib as _hashlib
from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["HMAC", "new", "digest", "compare_digest"]

_molt_hmac_new = _require_intrinsic("molt_hmac_new")
_molt_hmac_update = _require_intrinsic("molt_hmac_update")
_molt_hmac_copy = _require_intrinsic("molt_hmac_copy")
_molt_hmac_digest = _require_intrinsic("molt_hmac_digest")
_molt_compare_digest = _require_intrinsic("molt_compare_digest")
_TARGET_314 = _require_intrinsic("molt_sys_version_info")()[:2] >= (3, 14)

# Intrinsic HmacOperation's closed roles. The singleshot Clinic role acquires
# both buffers before resolving digestmod; New acquires msg after key setup.
_NEW = 0
_SINGLESHOT = 1
_BUILTIN_NEW = 2
_BUILTIN_SINGLESHOT = 3
trans_5C = bytes(x ^ 0x5C for x in range(256))
trans_36 = bytes(x ^ 0x36 for x in range(256))
digest_size = None


def _native_operation(key, msg, digestmod, operation, digestmod_present=True):
    if operation in (_BUILTIN_NEW, _BUILTIN_SINGLESHOT):
        import _hmac as _builtin

        constructors = None
        unsupported_error = _builtin._UnknownHashError
    else:
        import _hashlib as _native

        constructors = _native._constructor_table
        unsupported_error = _native._UnsupportedDigestmodError
    return _molt_hmac_new(
        key, msg, digestmod, constructors, operation, unsupported_error,
        digestmod_present,
    )


def _uses_native_digest(digestmod):
    if isinstance(digestmod, (str, _builtin_function_type)):
        return True
    import _hashlib as _native

    # These are declared constructor identities, never arbitrary .name or
    # __call__ probes. Native dictionary lookup owns any protocol callbacks.
    return any(digestmod is constructor for constructor in _native._constructors)


def _digest_constructor(digestmod):
    if callable(digestmod):
        return digestmod
    if isinstance(digestmod, str):
        return lambda data=b"": _hashlib.new(digestmod, data)
    return lambda data=b"": digestmod.new(data)


class HMAC:
    blocksize = 64
    __slots__ = (
        "_handle",
        "_digest_name",
        "_inner",
        "_outer",
        "digest_size",
        "block_size",
    )

    def __init__(self, key: Any, msg: Any = None, digestmod: Any = "") -> None:
        if not isinstance(key, (bytes, bytearray)):
            raise TypeError(
                f"key: expected bytes or bytearray, but got {type(key).__name__!r}"
            )
        if not digestmod:
            raise TypeError("Missing required argument 'digestmod'.")
        if _uses_native_digest(digestmod):
            import _hashlib as _native

            try:
                self._set_native(_native_operation(key, msg, digestmod, _NEW))
                return
            except _native.UnsupportedDigestmodError:
                # This is CPython's public digest-object dispatch. Its native
                # key acquisition and release are observable before PEP 247.
                pass
        if _TARGET_314 and isinstance(digestmod, str):
            import _hmac as _builtin

            try:
                self._set_native(_native_operation(key, msg, digestmod, _BUILTIN_NEW))
                return
            except _builtin.UnknownHashError:
                pass
        constructor = _digest_constructor(digestmod)
        self._handle = None
        self._outer = constructor()
        self._inner = constructor()
        self.digest_size = self._inner.digest_size
        if hasattr(self._inner, "block_size"):
            blocksize = self._inner.block_size
            if blocksize < 16:
                _warnings.warn(
                    f"block_size of {blocksize} seems too small; using our default of {self.blocksize}.",
                    RuntimeWarning,
                    2,
                )
                blocksize = self.blocksize
        else:
            _warnings.warn(
                f"No block_size attribute on given digest object; Assuming {self.blocksize}.",
                RuntimeWarning,
                2,
            )
            blocksize = self.blocksize
        if len(key) > blocksize:
            key = constructor(key).digest()
        self.block_size = blocksize
        key = key.ljust(blocksize, b"\0")
        self._outer.update(key.translate(trans_5C))
        self._inner.update(key.translate(trans_36))
        if msg is not None:
            self.update(msg)

    def _set_native(self, state):
        self._handle, self._digest_name, self.digest_size, self.block_size = state
        self._inner = self._outer = None

    @property
    def name(self):
        if self._handle is not None:
            return "hmac-" + self._digest_name
        return "hmac-" + self._inner.name

    def update(self, msg: Any) -> None:
        if self._handle is not None:
            _molt_hmac_update(self._handle, msg)
        else:
            self._inner.update(msg)

    def copy(self) -> "HMAC":
        other = self.__class__.__new__(self.__class__)
        other.digest_size = self.digest_size
        other.block_size = self.block_size
        if self._handle is not None:
            other._handle = _molt_hmac_copy(self._handle)
            other._digest_name = self._digest_name
            other._inner = other._outer = None
        else:
            other._handle = None
            other._inner = self._inner.copy()
            other._outer = self._outer.copy()
        return other

    def _current(self):
        outer = self._outer.copy()
        outer.update(self._inner.digest())
        return outer

    def digest(self) -> bytes:
        if self._handle is not None:
            return _molt_hmac_digest(self._handle)
        return self._current().digest()

    def hexdigest(self) -> str:
        if self._handle is not None:
            return _molt_hmac_digest(self._handle).hex()
        return self._current().hexdigest()


def new(key: Any, msg: Any = None, digestmod: Any = "") -> HMAC:
    return HMAC(key, msg, digestmod)


def digest(key: Any, msg: Any, digest: Any) -> bytes:
    native_overflow = False
    if _uses_native_digest(digest):
        import _hashlib as _native

        try:
            return _native_operation(key, msg, digest, _SINGLESHOT)
        except _native.UnsupportedDigestmodError:
            pass
        except OverflowError:
            if not _TARGET_314:
                raise
            native_overflow = True
    if _TARGET_314 and not native_overflow and isinstance(digest, str):
        import _hmac as _builtin

        try:
            return _native_operation(key, msg, digest, _BUILTIN_SINGLESHOT)
        except (_builtin.UnknownHashError, OverflowError):
            pass
    constructor = _digest_constructor(digest)
    inner = constructor()
    outer = constructor()
    blocksize = getattr(inner, "block_size", 64)
    if len(key) > blocksize:
        key = constructor(key).digest()
    if _TARGET_314:
        key = key.ljust(blocksize, b"\0")
    else:
        key = key + b"\0" * (blocksize - len(key))
    inner.update(key.translate(trans_36))
    outer.update(key.translate(trans_5C))
    inner.update(msg)
    outer.update(inner.digest())
    return outer.digest()


def compare_digest(a: Any, b: Any) -> bool:
    return bool(_molt_compare_digest(a, b))


globals().pop("_require_intrinsic", None)
