"""Intrinsic-first `_socket` shim (CPython internal module)."""

from __future__ import annotations

import socket as _socket_mod

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_socket_constants", globals())
_MOLT_OS_CLOSE = _require_intrinsic("molt_os_close", globals())
_MOLT_OS_DUP = _require_intrinsic("molt_os_dup", globals())


def _unsupported(name: str):
    def _fn(*_args, **_kwargs):
        raise RuntimeError(f"_socket.{name} is not implemented for this target")

    return _fn


class _BuiltinFunctionWrapper:
    __slots__ = ("_fn", "__doc__", "__name__", "__qualname__")

    def __init__(self, fn, name: str):
        self._fn = fn
        self.__name__ = name
        self.__qualname__ = name
        self.__doc__ = getattr(fn, "__doc__", None)

    def __call__(self, *args, **kwargs):
        return self._fn(*args, **kwargs)


_BuiltinFunctionWrapper.__name__ = "builtin_function_or_method"
_BuiltinFunctionWrapper.__qualname__ = "builtin_function_or_method"


class _PyCapsuleStub:
    pass


_PyCapsuleStub.__name__ = "PyCapsule"
_PyCapsuleStub.__qualname__ = "PyCapsule"


def _as_builtin_function(name: str, fn):
    if callable(fn):
        return _BuiltinFunctionWrapper(fn, name)
    return _BuiltinFunctionWrapper(_unsupported(name), name)


error = _socket_mod.error
timeout = _socket_mod.timeout
gaierror = _socket_mod.gaierror
herror = _socket_mod.herror
SocketType = _socket_mod.socket
socket = _socket_mod.socket
has_ipv6 = bool(getattr(_socket_mod, "has_ipv6", False))
CAPI = _PyCapsuleStub()

# CPython `_socket` exports a large set of integer constants. Keep this shim
# thin by mirroring whatever the intrinsic-backed `socket` module exposes.
for _name, _val in list(_socket_mod.__dict__.items()):
    if _name.startswith("_"):
        continue
    if isinstance(_val, bool):
        continue
    if isinstance(_val, int):
        globals()[_name] = int(_val)


def _gethostbyname_ex(hostname: str):
    primary = _socket_mod.gethostbyname(hostname)
    return hostname, [], [primary]


def _getprotobyname(name: str):
    table = {"icmp": 1, "tcp": 6, "udp": 17}
    key = str(name).lower()
    if key in table:
        return table[key]
    raise OSError(f"protocol not found: {name}")


def _if_nameindex():
    return []


def _if_nametoindex(name: str):
    raise OSError(f"interface name not supported: {name}")


def _if_indextoname(index: int):
    raise OSError(f"interface index not supported: {index}")


def _sethostname(name: str):
    raise RuntimeError("sethostname is not implemented in Molt runtime yet")


def _cmsg_len(length: int):
    n = int(length)
    if n < 0:
        raise ValueError("length must be non-negative")
    return 12 + n


def _cmsg_space(length: int):
    n = int(length)
    if n < 0:
        raise ValueError("length must be non-negative")
    return 12 + ((n + 3) & ~3)


_CALLABLES = {
    "CMSG_LEN": _cmsg_len,
    "CMSG_SPACE": _cmsg_space,
    "close": _MOLT_OS_CLOSE,
    "dup": _MOLT_OS_DUP,
    "getaddrinfo": getattr(_socket_mod, "getaddrinfo", _unsupported("getaddrinfo")),
    "getdefaulttimeout": getattr(
        _socket_mod, "getdefaulttimeout", _unsupported("getdefaulttimeout")
    ),
    "gethostbyaddr": getattr(
        _socket_mod, "gethostbyaddr", _unsupported("gethostbyaddr")
    ),
    "gethostbyname": getattr(
        _socket_mod, "gethostbyname", _unsupported("gethostbyname")
    ),
    "gethostbyname_ex": _gethostbyname_ex,
    "gethostname": getattr(_socket_mod, "gethostname", _unsupported("gethostname")),
    "getnameinfo": getattr(_socket_mod, "getnameinfo", _unsupported("getnameinfo")),
    "getprotobyname": _getprotobyname,
    "getservbyname": getattr(
        _socket_mod, "getservbyname", _unsupported("getservbyname")
    ),
    "getservbyport": getattr(
        _socket_mod, "getservbyport", _unsupported("getservbyport")
    ),
    "htonl": getattr(_socket_mod, "htonl", _unsupported("htonl")),
    "htons": getattr(_socket_mod, "htons", _unsupported("htons")),
    "if_indextoname": _if_indextoname,
    "if_nameindex": _if_nameindex,
    "if_nametoindex": _if_nametoindex,
    "inet_aton": getattr(_socket_mod, "inet_aton", _unsupported("inet_aton")),
    "inet_ntoa": getattr(_socket_mod, "inet_ntoa", _unsupported("inet_ntoa")),
    "inet_ntop": getattr(_socket_mod, "inet_ntop", _unsupported("inet_ntop")),
    "inet_pton": getattr(_socket_mod, "inet_pton", _unsupported("inet_pton")),
    "ntohl": getattr(_socket_mod, "ntohl", _unsupported("ntohl")),
    "ntohs": getattr(_socket_mod, "ntohs", _unsupported("ntohs")),
    "setdefaulttimeout": getattr(
        _socket_mod, "setdefaulttimeout", _unsupported("setdefaulttimeout")
    ),
    "sethostname": _sethostname,
    "socketpair": getattr(_socket_mod, "socketpair", _unsupported("socketpair")),
}

for _name, _fn in _CALLABLES.items():
    globals()[_name] = _as_builtin_function(_name, _fn)


__all__ = sorted(
    name
    for name in globals()
    if not name.startswith("_")
    and name
    not in {
        "__all__",
        "__annotations__",
        "__builtins__",
        "__cached__",
        "__doc__",
        "__file__",
        "__loader__",
        "__name__",
        "__package__",
        "__spec__",
    }
)
