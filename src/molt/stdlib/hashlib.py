"""Hashlib shim for Molt (Rust intrinsics only)."""

from __future__ import annotations

from typing import Any, Callable

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "blake2b",
    "blake2s",
    "compare_digest",
    "file_digest",
    "md5",
    "new",
    "pbkdf2_hmac",
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

_molt_hash_new = _require_intrinsic("molt_hash_new", globals())
_molt_hash_update = _require_intrinsic("molt_hash_update", globals())
_molt_hash_copy = _require_intrinsic("molt_hash_copy", globals())
_molt_hash_digest = _require_intrinsic("molt_hash_digest", globals())
_molt_hash_drop = _require_intrinsic("molt_hash_drop", globals())
_molt_compare_digest = _require_intrinsic("molt_compare_digest", globals())
_molt_pbkdf2_hmac = _require_intrinsic("molt_pbkdf2_hmac", globals())
_molt_scrypt = _require_intrinsic("molt_scrypt", globals())

_HASH_INFO: dict[str, tuple[int, int, bool]] = {
    "md5": (16, 64, False),
    "sha1": (20, 64, False),
    "sha224": (28, 64, False),
    "sha256": (32, 64, False),
    "sha384": (48, 128, False),
    "sha512": (64, 128, False),
    "sha3_224": (28, 144, False),
    "sha3_256": (32, 136, False),
    "sha3_384": (48, 104, False),
    "sha3_512": (64, 72, False),
    "shake_128": (0, 168, True),
    "shake_256": (0, 136, True),
    "blake2b": (64, 128, False),
    "blake2s": (32, 64, False),
}

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): add optional OpenSSL algorithms (sha512_224/sha512_256, ripemd160, md4) once Rust intrinsics land.

_ALIASES = {
    "sha-1": "sha1",
    "sha-224": "sha224",
    "sha-256": "sha256",
    "sha-384": "sha384",
    "sha-512": "sha512",
    "sha3-224": "sha3_224",
    "sha3-256": "sha3_256",
    "sha3-384": "sha3_384",
    "sha3-512": "sha3_512",
    "shake-128": "shake_128",
    "shake-256": "shake_256",
    "shake128": "shake_128",
    "shake256": "shake_256",
}

_BLAKE2_OPTIONS = {
    "digest_size",
    "key",
    "salt",
    "person",
    "fanout",
    "depth",
    "leaf_size",
    "node_offset",
    "node_depth",
    "inner_size",
    "last_node",
}


def _normalize_name(name: str) -> str:
    lowered = name.strip().lower()
    return _ALIASES.get(lowered, lowered)


def _validate_options(
    name: str,
    options: dict[str, Any],
    func_name: str,
) -> dict[str, Any] | None:
    opts = dict(options)
    if "usedforsecurity" in opts:
        opts.pop("usedforsecurity", None)
    if not opts:
        return None
    allowed = _BLAKE2_OPTIONS if name in {"blake2b", "blake2s"} else set()
    for key in opts:
        if key not in allowed:
            raise TypeError(f"{func_name}() got an unexpected keyword argument '{key}'")
    return opts


class UnsupportedDigestmodError(ValueError):
    pass


class _Hash:
    __slots__ = ("_handle", "name", "digest_size", "block_size", "_is_xof", "_options")

    def __init__(
        self,
        name: str,
        data: Any = b"",
        options: dict[str, Any] | None = None,
    ) -> None:
        info = _HASH_INFO.get(name)
        if info is None:
            raise ValueError(f"unsupported hash type {name}")
        self.name = name
        digest_size, block_size, is_xof = info
        if options and "digest_size" in options:
            try:
                digest_size = int(options["digest_size"])
            except Exception:
                pass
        self.digest_size = digest_size
        self.block_size = block_size
        self._is_xof = is_xof
        self._options = options
        self._handle = _molt_hash_new(name, data, options)

    def update(self, data: Any) -> None:
        _molt_hash_update(self._handle, data)

    def copy(self) -> "_Hash":
        other = object.__new__(type(self))
        other._handle = _molt_hash_copy(self._handle)
        other.name = self.name
        other.digest_size = self.digest_size
        other.block_size = self.block_size
        other._is_xof = self._is_xof
        other._options = self._options
        return other

    def digest(self, length: int | None = None) -> bytes:
        return _molt_hash_digest(self._handle, length)

    def hexdigest(self, length: int | None = None) -> str:
        return self.digest(length).hex()

    def __del__(self) -> None:
        try:
            _molt_hash_drop(self._handle)
        except Exception:
            pass


algorithms_guaranteed = set(_HASH_INFO.keys())
algorithms_available = set(_HASH_INFO.keys())


def new(name: str, data: Any = b"", **kwargs: Any) -> _Hash:
    if not isinstance(name, str):
        raise TypeError(
            f"new() argument 'name' must be str, not {type(name).__name__}"
        )
    normalized = _normalize_name(name)
    if normalized not in _HASH_INFO:
        raise ValueError(f"unsupported hash type {name}")
    options = _validate_options(normalized, kwargs, "new")
    return _Hash(normalized, data, options)


def __get_builtin_constructor(name: str) -> Callable[..., _Hash]:
    if not isinstance(name, str):
        raise TypeError("name must be str")
    normalized = _normalize_name(name)
    constructor = _CONSTRUCTORS.get(normalized)
    if constructor is None:
        raise ValueError(f"unsupported hash type {name}")
    return constructor


def md5(data: Any = b"", *, usedforsecurity: bool = True) -> _Hash:
    return new("md5", data, usedforsecurity=usedforsecurity)


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
    options = _validate_options("blake2b", options, "blake2b")
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
    options = _validate_options("blake2s", options, "blake2s")
    return _Hash("blake2s", data, options)


def pbkdf2_hmac(
    name: str,
    password: Any,
    salt: Any,
    iterations: int,
    dklen: int | None = None,
) -> bytes:
    return _molt_pbkdf2_hmac(name, password, salt, iterations, dklen)


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


_CONSTRUCTORS: dict[str, Callable[..., _Hash]] = {
    "md5": md5,
    "sha1": sha1,
    "sha224": sha224,
    "sha256": sha256,
    "sha384": sha384,
    "sha512": sha512,
    "sha3_224": sha3_224,
    "sha3_256": sha3_256,
    "sha3_384": sha3_384,
    "sha3_512": sha3_512,
    "shake_128": shake_128,
    "shake_256": shake_256,
    "blake2b": blake2b,
    "blake2s": blake2s,
}
