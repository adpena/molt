"""Hashlib shim for Molt (Rust intrinsics only)."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "blake2b",
    "blake2s",
    "compare_digest",
    "file_digest",
    "md4",
    "md5",
    "new",
    "pbkdf2_hmac",
    "ripemd160",
    "scrypt",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
    "algorithms_available",
    "algorithms_guaranteed",
    "UnsupportedDigestmodError",
]

_molt_hash_new = _require_intrinsic("molt_hash_new")
_molt_hash_update = _require_intrinsic("molt_hash_update")
_molt_hash_copy = _require_intrinsic("molt_hash_copy")
_molt_hash_digest = _require_intrinsic("molt_hash_digest")
_molt_compare_digest = _require_intrinsic("molt_compare_digest")
_molt_pbkdf2_hmac = _require_intrinsic("molt_pbkdf2_hmac")
_molt_scrypt = _require_intrinsic("molt_scrypt")

class UnsupportedDigestmodError(ValueError):
    pass


# Native module-state identity survives rebinding its exported alias.
_UnsupportedDigestmodError = UnsupportedDigestmodError


class _Hash:
    __slots__ = ("_handle", "name", "digest_size", "block_size")

    def __init__(
        self,
        name: str,
        data: Any = b"",
        options: dict[str, Any] | None = None,
        unsupported_error: type[Exception] | None = None,
    ) -> None:
        self._handle, self.name, self.digest_size, self.block_size = _molt_hash_new(
            name, data, options, unsupported_error
        )

    def update(self, data: Any) -> None:
        _molt_hash_update(self._handle, data)

    def copy(self) -> "_Hash":
        other = object.__new__(type(self))
        other._handle = _molt_hash_copy(self._handle)
        other.name = self.name
        other.digest_size = self.digest_size
        other.block_size = self.block_size
        return other

    def digest(self, length: int | None = None) -> bytes:
        return _molt_hash_digest(self._handle, length)

    def hexdigest(self, length: int | None = None) -> str:
        return self.digest(length).hex()


_GUARANTEED_ALGORITHMS = {
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
    "shake_128",
    "shake_256",
    "blake2b",
    "blake2s",
}

algorithms_guaranteed = set(_GUARANTEED_ALGORITHMS)
algorithms_available = algorithms_guaranteed | {
    "md4", "ripemd160", "sha512_224", "sha512_256"
}


def new(name: str, data: Any = b"", **kwargs: Any) -> _Hash:
    # CPython's public dispatch observes the original object's hash before
    # native argument conversion. Its populated builtin cache is separately
    # queried only for the two BLAKE constructors.
    if name in _BLAKE2_NAMES:
        return _BLAKE2_CONSTRUCTORS[name](data, **kwargs)
    for key in kwargs:
        if key != "usedforsecurity":
            raise TypeError(f"'{key}' is an invalid keyword argument for new()")
    return _Hash(name, data, kwargs or None)


def md4(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("md4", data, usedforsecurity=usedforsecurity)


def md5(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("md5", data, usedforsecurity=usedforsecurity)


def ripemd160(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("ripemd160", data, usedforsecurity=usedforsecurity)


def sha1(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha1", data, usedforsecurity=usedforsecurity)


def sha224(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha224", data, usedforsecurity=usedforsecurity)


def sha256(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha256", data, usedforsecurity=usedforsecurity)


def sha384(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha384", data, usedforsecurity=usedforsecurity)


def sha512(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha512", data, usedforsecurity=usedforsecurity)


def sha3_224(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha3_224", data, usedforsecurity=usedforsecurity)


def sha3_256(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha3_256", data, usedforsecurity=usedforsecurity)


def sha3_384(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha3_384", data, usedforsecurity=usedforsecurity)


def sha3_512(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("sha3_512", data, usedforsecurity=usedforsecurity)


def shake_128(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("shake_128", data, usedforsecurity=usedforsecurity)


def shake_256(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("shake_256", data, usedforsecurity=usedforsecurity)


def blake2b(
    data: Any = b"",
    *,
    digest_size: int = 64,
    key: Any = b"",
    salt: Any = b"",
    person: Any = b"",
    fanout: int = 1,
    depth: int = 1,
    leaf_size: int = 0,
    node_offset: int = 0,
    node_depth: int = 0,
    inner_size: int = 0,
    last_node: bool = False,
    usedforsecurity: bool = True,
) -> _Hash:
    options = {
        "digest_size": digest_size,
        "key": key,
        "salt": salt,
        "person": person,
        "fanout": fanout,
        "depth": depth,
        "leaf_size": leaf_size,
        "node_offset": node_offset,
        "node_depth": node_depth,
        "inner_size": inner_size,
        "last_node": last_node,
        "usedforsecurity": usedforsecurity,
    }
    return _Hash("blake2b", data, options)


def blake2s(
    data: Any = b"",
    *,
    digest_size: int = 32,
    key: Any = b"",
    salt: Any = b"",
    person: Any = b"",
    fanout: int = 1,
    depth: int = 1,
    leaf_size: int = 0,
    node_offset: int = 0,
    node_depth: int = 0,
    inner_size: int = 0,
    last_node: bool = False,
    usedforsecurity: bool = True,
) -> _Hash:
    options = {
        "digest_size": digest_size,
        "key": key,
        "salt": salt,
        "person": person,
        "fanout": fanout,
        "depth": depth,
        "leaf_size": leaf_size,
        "node_offset": node_offset,
        "node_depth": node_depth,
        "inner_size": inner_size,
        "last_node": last_node,
        "usedforsecurity": usedforsecurity,
    }
    return _Hash("blake2s", data, options)


# Capture the declared constructors once, as CPython's populated builtin cache
# does. Rebinding the public blake2 aliases does not alter new(name)'s dispatch.
_BLAKE2_NAMES = {"blake2b", "blake2s"}
_BLAKE2_CONSTRUCTORS = {"blake2b": blake2b, "blake2s": blake2s}


def pbkdf2_hmac(
    name: str,
    password: Any,
    salt: Any,
    iterations: int,
    dklen: int | None = None,
) -> bytes:
    return _molt_pbkdf2_hmac(
        name, password, salt, iterations, dklen, _UnsupportedDigestmodError
    )


def scrypt(
    password: Any,
    *,
    salt: Any,
    n: int,
    r: int,
    p: int,
    maxmem: int = 0,
    dklen: int = 64,
) -> bytes:
    return _molt_scrypt(password, salt, n, r, p, maxmem, dklen)


def file_digest(fileobj: Any, digest: Any) -> _Hash:
    if isinstance(digest, str):
        h = new(digest)
    elif callable(digest):
        h = digest()
    elif hasattr(digest, "new") and callable(digest.new):
        h = digest.new()
    else:
        raise TypeError("digest must be a string or callable")
    if not isinstance(h, _Hash):
        raise TypeError("digest must resolve to a Molt hash object")
    readinto = getattr(fileobj, "readinto", None)
    if callable(readinto):
        buf = bytearray(8192)
        view = memoryview(buf)
        while True:
            count = readinto(view)
            if count is None:
                break
            if count == 0:
                return h
            h.update(view[:count])
    read = getattr(fileobj, "read", None)
    if not callable(read):
        raise TypeError("file object must have a read() method")
    while True:
        chunk = read(8192)
        if not chunk:
            break
        h.update(chunk)
    return h


def compare_digest(a: Any, b: Any) -> bool:
    return bool(_molt_compare_digest(a, b))


globals().pop("_require_intrinsic", None)
