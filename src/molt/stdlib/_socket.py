"""Intrinsic-first `_socket` shim (CPython internal module)."""

from __future__ import annotations

import socket as _socket_mod

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_SOCKET_CONSTANTS = _require_intrinsic("molt_socket_constants")
_MOLT_OS_CLOSE = _require_intrinsic("molt_os_close")
_MOLT_OS_DUP = _require_intrinsic("molt_os_dup")
_MOLT_SOCKET_GETPROTOBYNAME = _require_intrinsic("molt_socket_getprotobyname")
_MOLT_GETHOSTBYNAME_EX = _require_intrinsic("molt_socket_gethostbyname_ex")
_MOLT_IF_NAMEINDEX = _require_intrinsic("molt_socket_if_nameindex")
_MOLT_IF_NAMETOINDEX = _require_intrinsic("molt_socket_if_nametoindex")
_MOLT_IF_INDEXTONAME = _require_intrinsic("molt_socket_if_indextoname")
_MOLT_CMSG_LEN = _require_intrinsic("molt_socket_cmsg_len")
_MOLT_CMSG_SPACE = _require_intrinsic("molt_socket_cmsg_space")


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


def _gethostbyname_ex(
    hostname: str, _gethostbyname_ex_intrinsic=_MOLT_GETHOSTBYNAME_EX
):
    return _gethostbyname_ex_intrinsic(hostname)


def _getprotobyname(name: str, _getprotobyname_intrinsic=_MOLT_SOCKET_GETPROTOBYNAME):
    return _getprotobyname_intrinsic(name)


def _if_nameindex(_if_nameindex_intrinsic=_MOLT_IF_NAMEINDEX):
    return _if_nameindex_intrinsic()


def _if_nametoindex(name: str, _if_nametoindex_intrinsic=_MOLT_IF_NAMETOINDEX):
    return _if_nametoindex_intrinsic(name)


def _if_indextoname(index: int, _if_indextoname_intrinsic=_MOLT_IF_INDEXTONAME):
    return _if_indextoname_intrinsic(index)


_MOLT_SOCKET_SETHOSTNAME = _require_intrinsic("molt_socket_sethostname")


def _sethostname(name: str, _sethostname_intrinsic=_MOLT_SOCKET_SETHOSTNAME):
    _sethostname_intrinsic(name)


def _cmsg_len(length: int, _cmsg_len_intrinsic=_MOLT_CMSG_LEN):
    return _cmsg_len_intrinsic(length)


def _cmsg_space(length: int, _cmsg_space_intrinsic=_MOLT_CMSG_SPACE):
    return _cmsg_space_intrinsic(length)


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

for _name in (
    "_MOLT_SOCKET_CONSTANTS",
    "_MOLT_OS_CLOSE",
    "_MOLT_OS_DUP",
    "_MOLT_SOCKET_GETPROTOBYNAME",
    "_MOLT_GETHOSTBYNAME_EX",
    "_MOLT_IF_NAMEINDEX",
    "_MOLT_IF_NAMETOINDEX",
    "_MOLT_IF_INDEXTONAME",
    "_MOLT_CMSG_LEN",
    "_MOLT_CMSG_SPACE",
    "_MOLT_SOCKET_SETHOSTNAME",
):
    globals().pop(_name, None)


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

globals().pop("_require_intrinsic", None)
