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
_MOLT_SOCKET_INET_PTON = _require_intrinsic("molt_socket_inet_pton")
_MOLT_SOCKET_INET_NTOP = _require_intrinsic("molt_socket_inet_ntop")
_MOLT_SOCKET_HTONS = _require_intrinsic("molt_socket_htons")
_MOLT_SOCKET_NTOHS = _require_intrinsic("molt_socket_ntohs")
_MOLT_SOCKET_HTONL = _require_intrinsic("molt_socket_htonl")
_MOLT_SOCKET_NTOHL = _require_intrinsic("molt_socket_ntohl")
_MOLT_AF_INET = int(getattr(_socket_mod, "AF_INET", 2))
_MOLT_HAS_CMSG = hasattr(_socket_mod, "SCM_RIGHTS")


def _unsupported(name: str):
    def _fn(*_args, **_kwargs):
        raise RuntimeError(f"_socket.{name} is not implemented for this target")

    return _fn


class _PyCapsuleStub:
    pass


_PyCapsuleStub.__name__ = "PyCapsule"
_PyCapsuleStub.__qualname__ = "PyCapsule"


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


def gethostbyname_ex(
    hostname: str, _gethostbyname_ex_intrinsic=_MOLT_GETHOSTBYNAME_EX
):
    return _gethostbyname_ex_intrinsic(hostname)


def getprotobyname(name: str, _getprotobyname_intrinsic=_MOLT_SOCKET_GETPROTOBYNAME):
    return _getprotobyname_intrinsic(name)


def if_nameindex(_if_nameindex_intrinsic=_MOLT_IF_NAMEINDEX):
    return _if_nameindex_intrinsic()


def if_nametoindex(name: str, _if_nametoindex_intrinsic=_MOLT_IF_NAMETOINDEX):
    return _if_nametoindex_intrinsic(name)


def if_indextoname(index: int, _if_indextoname_intrinsic=_MOLT_IF_INDEXTONAME):
    return _if_indextoname_intrinsic(index)


_MOLT_SOCKET_SETHOSTNAME = _require_intrinsic("molt_socket_sethostname")


def sethostname(name: str, _sethostname_intrinsic=_MOLT_SOCKET_SETHOSTNAME):
    _sethostname_intrinsic(name)


def CMSG_LEN(length: int, _cmsg_len_intrinsic=_MOLT_CMSG_LEN):
    return _cmsg_len_intrinsic(length)


def CMSG_SPACE(length: int, _cmsg_space_intrinsic=_MOLT_CMSG_SPACE):
    return _cmsg_space_intrinsic(length)


def getaddrinfo(host, port, family=0, type=0, proto=0, flags=0):
    fn = getattr(_socket_mod, "getaddrinfo", _unsupported("getaddrinfo"))
    return fn(host, port, family, type, proto, flags)


def getdefaulttimeout():
    fn = getattr(_socket_mod, "getdefaulttimeout", _unsupported("getdefaulttimeout"))
    return fn()


def gethostbyaddr(host):
    fn = getattr(_socket_mod, "gethostbyaddr", _unsupported("gethostbyaddr"))
    return fn(host)


def gethostbyname(host):
    fn = getattr(_socket_mod, "gethostbyname", _unsupported("gethostbyname"))
    return fn(host)


def gethostname():
    fn = getattr(_socket_mod, "gethostname", _unsupported("gethostname"))
    return fn()


def getnameinfo(sockaddr, flags=0):
    fn = getattr(_socket_mod, "getnameinfo", _unsupported("getnameinfo"))
    return fn(sockaddr, flags)


def getservbyname(name, proto=None):
    fn = getattr(_socket_mod, "getservbyname", _unsupported("getservbyname"))
    return fn(name, proto)


def getservbyport(port, proto=None):
    fn = getattr(_socket_mod, "getservbyport", _unsupported("getservbyport"))
    return fn(port, proto)


def inet_aton(address: str, _af_inet=_MOLT_AF_INET):
    return inet_pton(_af_inet, address)


def inet_ntoa(packed: bytes, _af_inet=_MOLT_AF_INET):
    return inet_ntop(_af_inet, packed)


def inet_pton(family: int, address: str, _inet_pton_intrinsic=_MOLT_SOCKET_INET_PTON):
    return _inet_pton_intrinsic(family, address)


def inet_ntop(family: int, packed: bytes, _inet_ntop_intrinsic=_MOLT_SOCKET_INET_NTOP):
    return _inet_ntop_intrinsic(family, packed)


def htons(value: int, _htons_intrinsic=_MOLT_SOCKET_HTONS):
    return _htons_intrinsic(value)


def ntohs(value: int, _ntohs_intrinsic=_MOLT_SOCKET_NTOHS):
    return _ntohs_intrinsic(value)


def htonl(value: int, _htonl_intrinsic=_MOLT_SOCKET_HTONL):
    return _htonl_intrinsic(value)


def ntohl(value: int, _ntohl_intrinsic=_MOLT_SOCKET_NTOHL):
    return _ntohl_intrinsic(value)


def close(fd: int, _close_intrinsic=_MOLT_OS_CLOSE):
    return _close_intrinsic(fd)


def dup(fd: int, _dup_intrinsic=_MOLT_OS_DUP):
    return _dup_intrinsic(fd)


def setdefaulttimeout(timeout=None):
    fn = getattr(_socket_mod, "setdefaulttimeout", _unsupported("setdefaulttimeout"))
    return fn(timeout)


def socketpair(family=None, type=None, proto=None):
    fn = getattr(_socket_mod, "socketpair", _unsupported("socketpair"))
    if family is None and type is None and proto is None:
        return fn()
    return fn(family, type, proto)

if not _MOLT_HAS_CMSG:
    globals().pop("CMSG_LEN", None)
    globals().pop("CMSG_SPACE", None)


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
    "_MOLT_SOCKET_INET_PTON",
    "_MOLT_SOCKET_INET_NTOP",
    "_MOLT_SOCKET_HTONS",
    "_MOLT_SOCKET_NTOHS",
    "_MOLT_SOCKET_HTONL",
    "_MOLT_SOCKET_NTOHL",
    "_MOLT_AF_INET",
    "_MOLT_HAS_CMSG",
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
