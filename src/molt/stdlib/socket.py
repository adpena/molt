"""Capability-gated socket module for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full socket module surface (makefile, dup, sendmsg/recvmsg, ancillary data, timeouts, and error subclasses) with CPython parity.

from __future__ import annotations

import importlib as _importlib
from typing import TYPE_CHECKING, Any

import builtins as _builtins

if TYPE_CHECKING:
    from types import ModuleType

    import _socket as _socket_typing

    SocketType = _socket_typing.socket
    _socket_mod: ModuleType | None
else:
    SocketType = Any

try:
    _socket_mod = _importlib.import_module("_socket")
except Exception:  # pragma: no cover - CPython-only fallback
    _socket_mod = None

__all__ = [
    "socket",
    "socketpair",
    "create_connection",
    "create_server",
    "getaddrinfo",
    "getnameinfo",
    "gethostname",
    "getservbyname",
    "getservbyport",
    "inet_pton",
    "inet_ntop",
    "getdefaulttimeout",
    "setdefaulttimeout",
    "error",
    "gaierror",
    "herror",
    "timeout",
]


error = OSError
timeout = TimeoutError


class gaierror(OSError):
    pass


class herror(OSError):
    pass


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


_molt_socket_constants = _load_intrinsic("_molt_socket_constants")
_molt_socket_has_ipv6 = _load_intrinsic("_molt_socket_has_ipv6")
_molt_socket_new = _load_intrinsic("_molt_socket_new")
_molt_socket_close = _load_intrinsic("_molt_socket_close")
_molt_socket_drop = _load_intrinsic("_molt_socket_drop")
_molt_socket_clone = _load_intrinsic("_molt_socket_clone")
_molt_socket_fileno = _load_intrinsic("_molt_socket_fileno")
_molt_socket_gettimeout = _load_intrinsic("_molt_socket_gettimeout")
_molt_socket_settimeout = _load_intrinsic("_molt_socket_settimeout")
_molt_socket_setblocking = _load_intrinsic("_molt_socket_setblocking")
_molt_socket_getblocking = _load_intrinsic("_molt_socket_getblocking")
_molt_socket_bind = _load_intrinsic("_molt_socket_bind")
_molt_socket_listen = _load_intrinsic("_molt_socket_listen")
_molt_socket_accept = _load_intrinsic("_molt_socket_accept")
_molt_socket_connect = _load_intrinsic("_molt_socket_connect")
_molt_socket_connect_ex = _load_intrinsic("_molt_socket_connect_ex")
_molt_socket_recv = _load_intrinsic("_molt_socket_recv")
_molt_socket_recv_into = _load_intrinsic("_molt_socket_recv_into")
_molt_socket_send = _load_intrinsic("_molt_socket_send")
_molt_socket_sendall = _load_intrinsic("_molt_socket_sendall")
_molt_socket_sendto = _load_intrinsic("_molt_socket_sendto")
_molt_socket_recvfrom = _load_intrinsic("_molt_socket_recvfrom")
_molt_socket_shutdown = _load_intrinsic("_molt_socket_shutdown")
_molt_socket_getsockname = _load_intrinsic("_molt_socket_getsockname")
_molt_socket_getpeername = _load_intrinsic("_molt_socket_getpeername")
_molt_socket_setsockopt = _load_intrinsic("_molt_socket_setsockopt")
_molt_socket_getsockopt = _load_intrinsic("_molt_socket_getsockopt")
_molt_socket_detach = _load_intrinsic("_molt_socket_detach")
_molt_socketpair = _load_intrinsic("_molt_socketpair")
_molt_socket_getaddrinfo = _load_intrinsic("_molt_socket_getaddrinfo")
_molt_socket_getnameinfo = _load_intrinsic("_molt_socket_getnameinfo")
_molt_socket_gethostname = _load_intrinsic("_molt_socket_gethostname")
_molt_socket_getservbyname = _load_intrinsic("_molt_socket_getservbyname")
_molt_socket_getservbyport = _load_intrinsic("_molt_socket_getservbyport")
_molt_socket_inet_pton = _load_intrinsic("_molt_socket_inet_pton")
_molt_socket_inet_ntop = _load_intrinsic("_molt_socket_inet_ntop")

_HAVE_INTRINSICS = _molt_socket_new is not None


def _init_constants() -> dict[str, int]:
    if _molt_socket_constants is not None:
        return dict(_molt_socket_constants())
    if _socket_mod is None:
        return {}
    out: dict[str, int] = {}
    for name in (
        "AF_INET",
        "AF_INET6",
        "AF_UNIX",
        "SOCK_STREAM",
        "SOCK_DGRAM",
        "SOCK_RAW",
        "SOL_SOCKET",
        "SO_REUSEADDR",
        "SO_KEEPALIVE",
        "SO_SNDBUF",
        "SO_RCVBUF",
        "SO_ERROR",
        "SO_LINGER",
        "SO_BROADCAST",
        "SO_REUSEPORT",
        "IPPROTO_TCP",
        "IPPROTO_UDP",
        "IPPROTO_IPV6",
        "IPV6_V6ONLY",
        "TCP_NODELAY",
        "SHUT_RD",
        "SHUT_WR",
        "SHUT_RDWR",
        "AI_PASSIVE",
        "AI_CANONNAME",
        "AI_NUMERICHOST",
        "AI_NUMERICSERV",
        "NI_NUMERICHOST",
        "NI_NUMERICSERV",
        "MSG_PEEK",
        "MSG_DONTWAIT",
        "EAI_AGAIN",
        "EAI_FAIL",
        "EAI_FAMILY",
        "EAI_NONAME",
        "EAI_SERVICE",
        "EAI_SOCKTYPE",
    ):
        if hasattr(_socket_mod, name):
            out[name] = int(getattr(_socket_mod, name))
    return out


_CONSTANTS = _init_constants()
globals().update(_CONSTANTS)
_EAI_CODES = {val for key, val in _CONSTANTS.items() if key.startswith("EAI_")}

has_ipv6 = (
    bool(_molt_socket_has_ipv6())
    if _molt_socket_has_ipv6
    else bool(getattr(_socket_mod, "has_ipv6", False))
    if _socket_mod is not None
    else False
)


_DEFAULT_TIMEOUT: float | None = None


def getdefaulttimeout() -> float | None:
    return _DEFAULT_TIMEOUT


def setdefaulttimeout(timeout: float | None) -> None:
    global _DEFAULT_TIMEOUT
    _DEFAULT_TIMEOUT = timeout


def _map_gaierror(exc: OSError) -> gaierror:
    return gaierror(exc.errno or 0, str(exc))


def _ensure_intrinsics() -> None:
    if not _HAVE_INTRINSICS:
        raise RuntimeError("socket intrinsics not available")


def _require_intrinsic(fn: Any | None, name: str) -> Any:
    if fn is None:
        raise RuntimeError(f"socket intrinsic missing: {name}")
    return fn


class socket:
    def __init__(
        self,
        family: int = _CONSTANTS.get("AF_INET", 2),
        type: int = _CONSTANTS.get("SOCK_STREAM", 1),
        proto: int = 0,
        fileno: int | None = None,
    ) -> None:
        self.family = family
        self.type = type
        self.proto = proto
        self._timeout: float | None = None
        self._sock: SocketType | None = None
        self._handle: Any | None = None
        if _HAVE_INTRINSICS:
            self._handle = _require_intrinsic(_molt_socket_new, "new")(
                family, type, proto, fileno
            )
        else:
            if _socket_mod is None:
                raise RuntimeError("socket module not available")
            self._sock = _socket_mod.socket(family, type, proto, fileno=fileno)
        if _DEFAULT_TIMEOUT is not None:
            try:
                self.settimeout(_DEFAULT_TIMEOUT)
            except Exception:
                pass

    def __repr__(self) -> str:
        return f"<socket fd={self.fileno()}>"

    def __enter__(self) -> "socket":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def close(self) -> None:
        if _HAVE_INTRINSICS:
            if getattr(self, "_handle", None) is not None:
                _require_intrinsic(_molt_socket_close, "close")(self._handle)
                _require_intrinsic(_molt_socket_drop, "drop")(self._handle)
                self._handle = None
        else:
            self._require_sock().close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            return None

    def _require_sock(self) -> Any:
        sock = self._sock
        if sock is None:
            raise RuntimeError("socket backing not available")
        return sock

    def fileno(self) -> int:
        if _HAVE_INTRINSICS:
            return int(_require_intrinsic(_molt_socket_fileno, "fileno")(self._handle))
        return self._require_sock().fileno()

    def detach(self) -> int:
        if _HAVE_INTRINSICS:
            raw = int(_require_intrinsic(_molt_socket_detach, "detach")(self._handle))
            _require_intrinsic(_molt_socket_drop, "drop")(self._handle)
            self._handle = None
            return raw
        return self._require_sock().detach()

    def gettimeout(self) -> float | None:
        if _HAVE_INTRINSICS:
            return _require_intrinsic(_molt_socket_gettimeout, "gettimeout")(
                self._handle
            )
        return self._require_sock().gettimeout()

    def settimeout(self, timeout: float | None) -> None:
        self._timeout = timeout
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_settimeout, "settimeout")(
                self._handle, timeout
            )
        else:
            self._require_sock().settimeout(timeout)

    def setblocking(self, flag: bool) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_setblocking, "setblocking")(
                self._handle, bool(flag)
            )
        else:
            self._require_sock().setblocking(flag)

    def getblocking(self) -> bool:
        if _HAVE_INTRINSICS:
            return bool(
                _require_intrinsic(_molt_socket_getblocking, "getblocking")(
                    self._handle
                )
            )
        return self._require_sock().getblocking()

    def bind(self, addr: Any) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_bind, "bind")(self._handle, addr)
        else:
            self._require_sock().bind(addr)

    def listen(self, backlog: int = 0) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_listen, "listen")(
                self._handle, int(backlog)
            )
        else:
            self._require_sock().listen(backlog)

    def accept(self) -> tuple["socket", Any]:
        if _HAVE_INTRINSICS:
            handle, addr = _require_intrinsic(_molt_socket_accept, "accept")(
                self._handle
            )
            sock = socket.__new__(socket)
            sock.family = self.family
            sock.type = self.type
            sock.proto = self.proto
            sock._timeout = None
            sock._handle = handle
            return sock, addr
        conn, addr = self._require_sock().accept()
        sock = socket(self.family, self.type, self.proto, fileno=conn.detach())
        return sock, addr

    def connect(self, addr: Any) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_connect, "connect")(self._handle, addr)
        else:
            self._require_sock().connect(addr)

    def connect_ex(self, addr: Any) -> int:
        if _HAVE_INTRINSICS:
            return int(
                _require_intrinsic(_molt_socket_connect_ex, "connect_ex")(
                    self._handle, addr
                )
            )
        return self._require_sock().connect_ex(addr)

    def recv(self, bufsize: int, flags: int = 0) -> bytes:
        if _HAVE_INTRINSICS:
            return _require_intrinsic(_molt_socket_recv, "recv")(
                self._handle, int(bufsize), int(flags)
            )
        return self._require_sock().recv(bufsize, flags)

    def recv_into(self, buffer: Any, nbytes: int = 0, flags: int = 0) -> int:
        if _HAVE_INTRINSICS:
            return int(
                _require_intrinsic(_molt_socket_recv_into, "recv_into")(
                    self._handle, buffer, int(nbytes), int(flags)
                )
            )
        return self._require_sock().recv_into(buffer, nbytes, flags)

    def send(self, data: Any, flags: int = 0) -> int:
        if _HAVE_INTRINSICS:
            return int(
                _require_intrinsic(_molt_socket_send, "send")(
                    self._handle, data, int(flags)
                )
            )
        return self._require_sock().send(data, flags)

    def sendall(self, data: Any, flags: int = 0) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_sendall, "sendall")(
                self._handle, data, int(flags)
            )
        else:
            self._require_sock().sendall(data, flags)

    def sendto(self, data: Any, flags: int, addr: Any) -> int:
        if _HAVE_INTRINSICS:
            return int(
                _require_intrinsic(_molt_socket_sendto, "sendto")(
                    self._handle, data, int(flags), addr
                )
            )
        return self._require_sock().sendto(data, flags, addr)

    def recvfrom(self, bufsize: int, flags: int = 0) -> tuple[bytes, Any]:
        if _HAVE_INTRINSICS:
            return _require_intrinsic(_molt_socket_recvfrom, "recvfrom")(
                self._handle, int(bufsize), int(flags)
            )
        return self._require_sock().recvfrom(bufsize, flags)

    def shutdown(self, how: int) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_shutdown, "shutdown")(
                self._handle, int(how)
            )
        else:
            self._require_sock().shutdown(how)

    def getsockname(self) -> Any:
        if _HAVE_INTRINSICS:
            return _require_intrinsic(_molt_socket_getsockname, "getsockname")(
                self._handle
            )
        return self._require_sock().getsockname()

    def getpeername(self) -> Any:
        if _HAVE_INTRINSICS:
            return _require_intrinsic(_molt_socket_getpeername, "getpeername")(
                self._handle
            )
        return self._require_sock().getpeername()

    def setsockopt(self, level: int, optname: int, value: Any) -> None:
        if _HAVE_INTRINSICS:
            _require_intrinsic(_molt_socket_setsockopt, "setsockopt")(
                self._handle, int(level), int(optname), value
            )
        else:
            self._require_sock().setsockopt(level, optname, value)

    def getsockopt(self, level: int, optname: int, buflen: int = 0) -> Any:
        if _HAVE_INTRINSICS:
            return _require_intrinsic(_molt_socket_getsockopt, "getsockopt")(
                self._handle, int(level), int(optname), int(buflen)
            )
        if buflen:
            return self._require_sock().getsockopt(level, optname, buflen)
        return self._require_sock().getsockopt(level, optname)


def socketpair(
    family: int | None = None, type: int | None = None, proto: int | None = None
) -> tuple[socket, socket]:
    left_handle, right_handle = _require_intrinsic(_molt_socketpair, "socketpair")(
        family, type, proto
    )
    default_family = (
        _CONSTANTS.get("AF_UNIX")
        if _CONSTANTS.get("AF_UNIX") is not None
        else _CONSTANTS.get("AF_INET", 2)
    )
    fam = default_family if family is None else family
    sock_type = _CONSTANTS.get("SOCK_STREAM", 1) if type is None else type
    proto_val = 0 if proto is None else proto
    left = socket.__new__(socket)
    right = socket.__new__(socket)
    for sock, handle in ((left, left_handle), (right, right_handle)):
        sock.family = fam
        sock.type = sock_type
        sock.proto = proto_val
        sock._timeout = None
        sock._handle = handle
        if _DEFAULT_TIMEOUT is not None:
            try:
                sock.settimeout(_DEFAULT_TIMEOUT)
            except Exception:
                pass
    return left, right


def getaddrinfo(
    host: str | bytes | None,
    port: int | str | bytes | None,
    family: int = 0,
    type: int = 0,
    proto: int = 0,
    flags: int = 0,
) -> list[tuple[int, int, int, str | None, Any]]:
    _ensure_intrinsics()
    try:
        return _require_intrinsic(_molt_socket_getaddrinfo, "getaddrinfo")(
            host, port, family, type, proto, flags
        )
    except OSError as exc:
        if exc.errno in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise


def getnameinfo(addr: Any, flags: int) -> tuple[str, str]:
    _ensure_intrinsics()
    try:
        return _require_intrinsic(_molt_socket_getnameinfo, "getnameinfo")(addr, flags)
    except OSError as exc:
        if exc.errno in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise


def gethostname() -> str:
    _ensure_intrinsics()
    return _require_intrinsic(_molt_socket_gethostname, "gethostname")()


def getservbyname(name: str, proto: str | None = None) -> int:
    _ensure_intrinsics()
    return int(
        _require_intrinsic(_molt_socket_getservbyname, "getservbyname")(name, proto)
    )


def getservbyport(port: int, proto: str | None = None) -> str:
    _ensure_intrinsics()
    return _require_intrinsic(_molt_socket_getservbyport, "getservbyport")(
        int(port), proto
    )


def inet_pton(family: int, address: str) -> bytes:
    _ensure_intrinsics()
    return _require_intrinsic(_molt_socket_inet_pton, "inet_pton")(int(family), address)


def inet_ntop(family: int, packed: bytes) -> str:
    _ensure_intrinsics()
    return _require_intrinsic(_molt_socket_inet_ntop, "inet_ntop")(int(family), packed)


def create_connection(
    address: tuple[str, int],
    timeout: float | None = None,
    source_address: tuple[str, int] | None = None,
) -> socket:
    host, port = address
    if timeout is None:
        timeout = _DEFAULT_TIMEOUT
    err: OSError | None = None
    for res in getaddrinfo(host, port, 0, _CONSTANTS.get("SOCK_STREAM", 1), 0, 0):
        af, socktype, proto, _canon, sa = res
        sock = socket(af, socktype, proto)
        try:
            if timeout is not None:
                sock.settimeout(timeout)
            if source_address is not None:
                sock.bind(source_address)
            sock.connect(sa)
            return sock
        except OSError as exc:
            err = exc
            try:
                sock.close()
            except Exception:
                pass
    if err is not None:
        raise err
    raise OSError("getaddrinfo returned empty list")


def create_server(
    address: tuple[str, int],
    backlog: int | None = None,
    reuse_port: bool = False,
    dualstack_ipv6: bool = False,
) -> socket:
    host, port = address
    family = (
        _CONSTANTS.get("AF_INET6", 0)
        if dualstack_ipv6
        else _CONSTANTS.get("AF_INET", 2)
    )
    sock = socket(family, _CONSTANTS.get("SOCK_STREAM", 1))
    try:
        sock.setsockopt(
            _CONSTANTS.get("SOL_SOCKET", 1), _CONSTANTS.get("SO_REUSEADDR", 2), 1
        )
        if (
            reuse_port
            and _socket_mod is not None
            and hasattr(_socket_mod, "SO_REUSEPORT")
        ):
            sock.setsockopt(
                _CONSTANTS.get("SOL_SOCKET", 1),
                int(getattr(_socket_mod, "SO_REUSEPORT")),
                1,
            )
        sock.bind((host, port))
        sock.listen(0 if backlog is None else backlog)
        return sock
    except Exception:
        sock.close()
        raise
