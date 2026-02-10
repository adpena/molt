"""Capability-gated socket module for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full socket module surface (sendmsg/recvmsg ancillary edge cases, timeout nuance parity, and remaining low-level option/errno behavior) with CPython parity.

from __future__ import annotations

import errno
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "socket",
    "socketpair",
    "fromfd",
    "create_connection",
    "create_server",
    "getaddrinfo",
    "getnameinfo",
    "gethostname",
    "gethostbyname",
    "gethostbyaddr",
    "getfqdn",
    "getservbyname",
    "getservbyport",
    "inet_aton",
    "inet_pton",
    "inet_ntoa",
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


_molt_socket_constants = _require_intrinsic("molt_socket_constants", globals())
_molt_socket_has_ipv6 = _require_intrinsic("molt_socket_has_ipv6", globals())
_molt_socket_new = _require_intrinsic("molt_socket_new", globals())
_molt_socket_close = _require_intrinsic("molt_socket_close", globals())
_molt_socket_drop = _require_intrinsic("molt_socket_drop", globals())
_molt_socket_clone = _require_intrinsic("molt_socket_clone", globals())
_molt_os_dup = _require_intrinsic("molt_os_dup", globals())
_molt_os_close = _require_intrinsic("molt_os_close", globals())
_molt_os_get_inheritable = _require_intrinsic("molt_os_get_inheritable", globals())
_molt_os_set_inheritable = _require_intrinsic("molt_os_set_inheritable", globals())
_molt_socket_fileno = _require_intrinsic("molt_socket_fileno", globals())
_molt_socket_gettimeout = _require_intrinsic("molt_socket_gettimeout", globals())
_molt_socket_settimeout = _require_intrinsic("molt_socket_settimeout", globals())
_molt_socket_setblocking = _require_intrinsic("molt_socket_setblocking", globals())
_molt_socket_getblocking = _require_intrinsic("molt_socket_getblocking", globals())
_molt_socket_bind = _require_intrinsic("molt_socket_bind", globals())
_molt_socket_listen = _require_intrinsic("molt_socket_listen", globals())
_molt_socket_accept = _require_intrinsic("molt_socket_accept", globals())
_molt_socket_connect = _require_intrinsic("molt_socket_connect", globals())
_molt_socket_connect_ex = _require_intrinsic("molt_socket_connect_ex", globals())
_molt_socket_recv = _require_intrinsic("molt_socket_recv", globals())
_molt_socket_recv_into = _require_intrinsic("molt_socket_recv_into", globals())
_molt_socket_recvfrom_into = _require_intrinsic("molt_socket_recvfrom_into", globals())
_molt_socket_send = _require_intrinsic("molt_socket_send", globals())
_molt_socket_sendall = _require_intrinsic("molt_socket_sendall", globals())
_molt_socket_sendto = _require_intrinsic("molt_socket_sendto", globals())
_molt_socket_recvfrom = _require_intrinsic("molt_socket_recvfrom", globals())
_molt_socket_sendmsg = _require_intrinsic("molt_socket_sendmsg", globals())
_molt_socket_recvmsg = _require_intrinsic("molt_socket_recvmsg", globals())
_molt_socket_recvmsg_into = _require_intrinsic("molt_socket_recvmsg_into", globals())
_molt_socket_shutdown = _require_intrinsic("molt_socket_shutdown", globals())
_molt_socket_reader_new = _require_intrinsic("molt_socket_reader_new", globals())
_molt_socket_reader_read = _require_intrinsic("molt_socket_reader_read", globals())
_molt_socket_reader_readline = _require_intrinsic(
    "molt_socket_reader_readline", globals()
)
_molt_socket_reader_readline_limit = _require_intrinsic(
    "molt_socket_reader_readline_limit", globals()
)
_molt_socket_reader_drop = _require_intrinsic("molt_socket_reader_drop", globals())
_molt_socket_getsockname = _require_intrinsic("molt_socket_getsockname", globals())
_molt_socket_getpeername = _require_intrinsic("molt_socket_getpeername", globals())
_molt_socket_setsockopt = _require_intrinsic("molt_socket_setsockopt", globals())
_molt_socket_getsockopt = _require_intrinsic("molt_socket_getsockopt", globals())
_molt_socket_detach = _require_intrinsic("molt_socket_detach", globals())
_molt_socketpair = _require_intrinsic("molt_socketpair", globals())
_molt_socket_getaddrinfo = _require_intrinsic("molt_socket_getaddrinfo", globals())
_molt_socket_getnameinfo = _require_intrinsic("molt_socket_getnameinfo", globals())
_molt_socket_gethostname = _require_intrinsic("molt_socket_gethostname", globals())
_molt_socket_gethostbyname = _require_intrinsic("molt_socket_gethostbyname", globals())
_molt_socket_gethostbyaddr = _require_intrinsic("molt_socket_gethostbyaddr", globals())
_molt_socket_getfqdn = _require_intrinsic("molt_socket_getfqdn", globals())
_molt_socket_getservbyname = _require_intrinsic("molt_socket_getservbyname", globals())
_molt_socket_getservbyport = _require_intrinsic("molt_socket_getservbyport", globals())
_molt_socket_inet_pton = _require_intrinsic("molt_socket_inet_pton", globals())
_molt_socket_inet_ntop = _require_intrinsic("molt_socket_inet_ntop", globals())


def _init_constants() -> dict[str, int]:
    if _molt_socket_constants is None:
        raise RuntimeError("socket intrinsics unavailable")
    try:
        constants = _molt_socket_constants()
    except Exception as exc:
        raise RuntimeError("socket intrinsics unavailable") from exc
    if not isinstance(constants, dict):
        raise RuntimeError("socket intrinsics unavailable")
    return dict(constants)


_CONSTANTS = _init_constants()
globals().update(_CONSTANTS)
_EAI_CODES = {val for key, val in _CONSTANTS.items() if key.startswith("EAI_")}

if _molt_socket_has_ipv6 is None:
    raise RuntimeError("socket intrinsics unavailable")
try:
    has_ipv6 = bool(_molt_socket_has_ipv6())
except Exception as exc:
    raise RuntimeError("socket intrinsics unavailable") from exc

_DEFAULT_TIMEOUT: float | None = None


def getdefaulttimeout() -> float | None:
    return _DEFAULT_TIMEOUT


def _coerce_timeout(timeout: float | None) -> float | None:
    if timeout is None:
        return None
    try:
        value = float(timeout)
    except (TypeError, ValueError) as exc:
        raise TypeError(
            f"'{type(timeout).__name__}' object cannot be interpreted as an integer or float"
        ) from exc
    if value < 0.0:
        raise ValueError("Timeout value out of range")
    return value


def setdefaulttimeout(timeout: float | None) -> None:
    global _DEFAULT_TIMEOUT
    _DEFAULT_TIMEOUT = _coerce_timeout(timeout)


def _map_gaierror(exc: OSError) -> gaierror:
    code = _exc_errno(exc)
    return gaierror(0 if code is None else code, str(exc))


def _map_herror(exc: OSError) -> herror:
    code = _exc_errno(exc)
    return herror(0 if code is None else code, str(exc))


def _exc_errno(exc: OSError) -> int | None:
    value = getattr(exc, "errno", None)
    if value is None:
        return None
    try:
        return int(value)
    except Exception:
        return None


def _require_socket_intrinsic(fn: Any | None, name: str) -> Any:
    if fn is None:
        raise RuntimeError(f"socket intrinsic missing: {name}")
    return fn


def _socket_from_handle(family: int, type: int, proto: int, handle: Any) -> "socket":
    sock = socket.__new__(socket)
    sock.family = family
    sock.type = type
    sock.proto = proto
    sock._timeout = None
    sock._handle = handle
    return sock


class _SocketFile:
    def __init__(
        self,
        sock: "socket",
        mode: str = "r",
        buffering: int | None = None,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
    ) -> None:
        self._sock = sock
        self._mode = mode
        self._binary = "b" in mode
        self._readable = "r" in mode or "+" in mode
        self._writable = "w" in mode or "a" in mode or "+" in mode
        self._encoding = encoding or "utf-8"
        self._errors = errors or "strict"
        self._newline = newline
        self._buffering = buffering
        self._closed = False
        self._reader: Any | None = None
        if self._readable:
            handle = self._sock._require_handle()
            self._reader = _require_socket_intrinsic(
                _molt_socket_reader_new, "molt_socket_reader_new"
            )(handle)

    @property
    def closed(self) -> bool:
        return self._closed

    def close(self) -> None:
        if self._reader is not None:
            _require_socket_intrinsic(
                _molt_socket_reader_drop, "molt_socket_reader_drop"
            )(self._reader)
            self._reader = None
        self._closed = True

    def flush(self) -> None:
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def _ensure_open(self) -> None:
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def _ensure_readable(self) -> None:
        if not self._readable:
            raise OSError("File not open for reading")

    def _ensure_writable(self) -> None:
        if not self._writable:
            raise OSError("File not open for writing")

    def _require_reader(self) -> Any:
        reader = self._reader
        if reader is None:
            raise RuntimeError("socket reader is not initialized")
        return reader

    def _coerce_size(self, size: int | None) -> int:
        if size is None:
            return -1
        return int(size)

    def _coerce_bytes(self, data: Any) -> bytes:
        if self._binary:
            if isinstance(data, (bytes, bytearray, memoryview)):
                return bytes(data)
            name = type(data).__name__
            raise TypeError(f"a bytes-like object is required, not '{name}'")
        if not isinstance(data, str):
            name = type(data).__name__
            raise TypeError(f"write() argument must be str, not {name}")
        return data.encode(self._encoding, self._errors)

    def _reader_read(self, size: int) -> bytes:
        data = _require_socket_intrinsic(
            _molt_socket_reader_read, "molt_socket_reader_read"
        )(self._require_reader(), int(size))
        if isinstance(data, (bytes, bytearray, memoryview)):
            return bytes(data)
        raise RuntimeError("socket reader intrinsic returned invalid value")

    def _reader_readline(self, size: int) -> bytes:
        if size < 0:
            data = _require_socket_intrinsic(
                _molt_socket_reader_readline, "molt_socket_reader_readline"
            )(self._require_reader())
        else:
            data = _require_socket_intrinsic(
                _molt_socket_reader_readline_limit, "molt_socket_reader_readline_limit"
            )(self._require_reader(), int(size))
        if isinstance(data, (bytes, bytearray, memoryview)):
            return bytes(data)
        raise RuntimeError("socket reader intrinsic returned invalid value")

    def _sendall(self, payload: bytes) -> None:
        handle = self._sock._require_handle()
        _require_socket_intrinsic(_molt_socket_sendall, "sendall")(handle, payload, 0)

    def write(self, data: Any) -> int:
        self._ensure_open()
        self._ensure_writable()
        payload = self._coerce_bytes(data)
        self._sendall(payload)
        return len(payload)

    def read(self, size: int | None = -1) -> bytes | str:
        self._ensure_open()
        self._ensure_readable()
        data = self._reader_read(self._coerce_size(size))
        if self._binary:
            return data
        return data.decode(self._encoding, self._errors)

    def readline(self, size: int | None = -1) -> bytes | str:
        self._ensure_open()
        self._ensure_readable()
        data = self._reader_readline(self._coerce_size(size))
        if self._binary:
            return data
        return data.decode(self._encoding, self._errors)

    def __enter__(self) -> "_SocketFile":
        self._ensure_open()
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()


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
        self._handle: Any | None = None
        self._handle = _require_socket_intrinsic(_molt_socket_new, "new")(
            family, type, proto, fileno
        )
        if _DEFAULT_TIMEOUT is not None:
            self.settimeout(_DEFAULT_TIMEOUT)

    def __repr__(self) -> str:
        return f"<socket fd={self.fileno()}>"

    def __enter__(self) -> "socket":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def close(self) -> None:
        if getattr(self, "_handle", None) is not None:
            _require_socket_intrinsic(_molt_socket_close, "close")(self._handle)
            _require_socket_intrinsic(_molt_socket_drop, "drop")(self._handle)
            self._handle = None

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            return None

    def _require_handle(self) -> Any:
        handle = self._handle
        if handle is None:
            raise OSError(errno.EBADF, "Bad file descriptor")
        return handle

    def fileno(self) -> int:
        handle = self._handle
        if handle is None:
            return -1
        return int(_require_socket_intrinsic(_molt_socket_fileno, "fileno")(handle))

    def detach(self) -> int:
        handle = self._require_handle()
        raw = int(_require_socket_intrinsic(_molt_socket_detach, "detach")(handle))
        _require_socket_intrinsic(_molt_socket_drop, "drop")(handle)
        self._handle = None
        return raw

    def gettimeout(self) -> float | None:
        return _require_socket_intrinsic(_molt_socket_gettimeout, "gettimeout")(
            self._require_handle()
        )

    def settimeout(self, timeout: float | None) -> None:
        timeout_val = _coerce_timeout(timeout)
        self._timeout = timeout_val
        _require_socket_intrinsic(_molt_socket_settimeout, "settimeout")(
            self._require_handle(), timeout_val
        )

    def setblocking(self, flag: bool) -> None:
        # Keep timeout metadata in sync with CPython: setblocking(False) == settimeout(0.0),
        # setblocking(True) == settimeout(None). This is required for non-blocking connect_ex
        # semantics used by asyncio loop sock_connect.
        if bool(flag):
            self.settimeout(None)
        else:
            self.settimeout(0.0)

    def getblocking(self) -> bool:
        return bool(
            _require_socket_intrinsic(_molt_socket_getblocking, "getblocking")(
                self._require_handle()
            )
        )

    def bind(self, addr: Any) -> None:
        _require_socket_intrinsic(_molt_socket_bind, "bind")(
            self._require_handle(), addr
        )

    def listen(self, backlog: int = 0) -> None:
        _require_socket_intrinsic(_molt_socket_listen, "listen")(
            self._require_handle(), int(backlog)
        )

    def accept(self) -> tuple["socket", Any]:
        handle, addr = _require_socket_intrinsic(_molt_socket_accept, "accept")(
            self._require_handle()
        )
        sock = _socket_from_handle(self.family, self.type, self.proto, handle)
        return sock, addr

    def connect(self, addr: Any) -> None:
        _require_socket_intrinsic(_molt_socket_connect, "connect")(
            self._require_handle(), addr
        )

    def connect_ex(self, addr: Any) -> int:
        return int(
            _require_socket_intrinsic(_molt_socket_connect_ex, "connect_ex")(
                self._require_handle(), addr
            )
        )

    def recv(self, bufsize: int, flags: int = 0) -> bytes:
        return _require_socket_intrinsic(_molt_socket_recv, "recv")(
            self._require_handle(), int(bufsize), int(flags)
        )

    def recv_into(self, buffer: Any, nbytes: int = 0, flags: int = 0) -> int:
        size = int(nbytes)
        if size < 0:
            raise ValueError("negative buffersize in recv_into")
        if size == 0:
            size = -1
        return int(
            _require_socket_intrinsic(_molt_socket_recv_into, "recv_into")(
                self._require_handle(), buffer, size, int(flags)
            )
        )

    def send(self, data: Any, flags: int = 0) -> int:
        return int(
            _require_socket_intrinsic(_molt_socket_send, "send")(
                self._require_handle(), data, int(flags)
            )
        )

    def sendall(self, data: Any, flags: int = 0) -> None:
        _require_socket_intrinsic(_molt_socket_sendall, "sendall")(
            self._require_handle(), data, int(flags)
        )

    def sendto(self, data: Any, *args: Any) -> int:
        if len(args) == 1:
            flags = 0
            addr = args[0]
        elif len(args) == 2:
            flags = int(args[0])
            addr = args[1]
        else:
            raise TypeError("sendto() takes 2 or 3 positional arguments")
        return int(
            _require_socket_intrinsic(_molt_socket_sendto, "sendto")(
                self._require_handle(), data, int(flags), addr
            )
        )

    def recvfrom(self, bufsize: int, flags: int = 0) -> tuple[bytes, Any]:
        return _require_socket_intrinsic(_molt_socket_recvfrom, "recvfrom")(
            self._require_handle(), int(bufsize), int(flags)
        )

    def recvfrom_into(
        self, buffer: Any, nbytes: int = 0, flags: int = 0
    ) -> tuple[int, Any]:
        size = int(nbytes)
        if size < 0:
            raise ValueError("negative buffersize in recvfrom_into")
        if size == 0:
            size = -1
        return _require_socket_intrinsic(_molt_socket_recvfrom_into, "recvfrom_into")(
            self._require_handle(), buffer, size, int(flags)
        )

    def sendmsg(
        self,
        buffers: Any,
        ancdata: Any = (),
        flags: int = 0,
        address: Any = None,
    ) -> int:
        return int(
            _require_socket_intrinsic(_molt_socket_sendmsg, "sendmsg")(
                self._require_handle(), buffers, ancdata, int(flags), address
            )
        )

    def recvmsg(
        self, bufsize: int, ancbufsize: int = 0, flags: int = 0
    ) -> tuple[bytes, list[tuple[int, int, bytes]], int, Any]:
        return _require_socket_intrinsic(_molt_socket_recvmsg, "recvmsg")(
            self._require_handle(), int(bufsize), int(ancbufsize), int(flags)
        )

    def recvmsg_into(
        self, buffers: Any, ancbufsize: int = 0, flags: int = 0
    ) -> tuple[int, list[tuple[int, int, bytes]], int, Any]:
        return _require_socket_intrinsic(_molt_socket_recvmsg_into, "recvmsg_into")(
            self._require_handle(), buffers, int(ancbufsize), int(flags)
        )

    def shutdown(self, how: int) -> None:
        _require_socket_intrinsic(_molt_socket_shutdown, "shutdown")(
            self._require_handle(), int(how)
        )

    def getsockname(self) -> Any:
        return _require_socket_intrinsic(_molt_socket_getsockname, "getsockname")(
            self._require_handle()
        )

    def getpeername(self) -> Any:
        return _require_socket_intrinsic(_molt_socket_getpeername, "getpeername")(
            self._require_handle()
        )

    def setsockopt(self, level: int, optname: int, value: Any) -> None:
        _require_socket_intrinsic(_molt_socket_setsockopt, "setsockopt")(
            self._require_handle(), int(level), int(optname), value
        )

    def getsockopt(self, level: int, optname: int, buflen: int = 0) -> Any:
        length = int(buflen)
        if length <= 0:
            return _require_socket_intrinsic(_molt_socket_getsockopt, "getsockopt")(
                self._require_handle(), int(level), int(optname), None
            )
        return _require_socket_intrinsic(_molt_socket_getsockopt, "getsockopt")(
            self._require_handle(), int(level), int(optname), length
        )

    def get_inheritable(self) -> bool:
        return bool(
            _require_socket_intrinsic(
                _molt_os_get_inheritable, "molt_os_get_inheritable"
            )(self.fileno())
        )

    def set_inheritable(self, inheritable: bool) -> None:
        _require_socket_intrinsic(_molt_os_set_inheritable, "molt_os_set_inheritable")(
            self.fileno(), bool(inheritable)
        )

    def dup(self) -> "socket":
        handle = _require_socket_intrinsic(_molt_socket_clone, "clone")(
            self._require_handle()
        )
        return _socket_from_handle(self.family, self.type, self.proto, handle)

    def makefile(
        self,
        mode: str = "r",
        buffering: int | None = None,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
    ) -> _SocketFile:
        return _SocketFile(
            self,
            mode=mode,
            buffering=buffering,
            encoding=encoding,
            errors=errors,
            newline=newline,
        )


def fromfd(fd: int, family: int, type: int, proto: int = 0) -> socket:
    duped = int(_require_socket_intrinsic(_molt_os_dup, "molt_os_dup")(int(fd)))
    try:
        handle = _require_socket_intrinsic(_molt_socket_new, "new")(
            int(family), int(type), int(proto), duped
        )
        return _socket_from_handle(int(family), int(type), int(proto), handle)
    except Exception:
        try:
            _require_socket_intrinsic(_molt_os_close, "molt_os_close")(duped)
        except Exception:
            pass
        raise


def socketpair(
    family: int | None = None, type: int | None = None, proto: int | None = None
) -> tuple[socket, socket]:
    left_handle, right_handle = _require_socket_intrinsic(
        _molt_socketpair, "socketpair"
    )(family, type, proto)
    default_family = (
        _CONSTANTS.get("AF_UNIX")
        if _CONSTANTS.get("AF_UNIX") is not None
        else _CONSTANTS.get("AF_INET", 2)
    )
    fam = default_family if family is None else family
    sock_type = _CONSTANTS.get("SOCK_STREAM", 1) if type is None else type
    proto_val = 0 if proto is None else proto
    left = _socket_from_handle(fam, sock_type, proto_val, left_handle)
    right = _socket_from_handle(fam, sock_type, proto_val, right_handle)
    for sock in (left, right):
        if _DEFAULT_TIMEOUT is not None:
            sock.settimeout(_DEFAULT_TIMEOUT)
    return left, right


def getaddrinfo(
    host: str | bytes | None,
    port: int | str | bytes | None,
    family: int = 0,
    type: int = 0,
    proto: int = 0,
    flags: int = 0,
) -> list[tuple[int, int, int, str | None, Any]]:
    try:
        return _require_socket_intrinsic(_molt_socket_getaddrinfo, "getaddrinfo")(
            host, port, family, type, proto, flags
        )
    except OSError as exc:
        if _exc_errno(exc) in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise


def getnameinfo(addr: Any, flags: int) -> tuple[str, str]:
    try:
        return _require_socket_intrinsic(_molt_socket_getnameinfo, "getnameinfo")(
            addr, flags
        )
    except OSError as exc:
        if _exc_errno(exc) in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise


def gethostname() -> str:
    return _require_socket_intrinsic(_molt_socket_gethostname, "gethostname")()


def gethostbyname(hostname: str) -> str:
    try:
        return _require_socket_intrinsic(_molt_socket_gethostbyname, "gethostbyname")(
            hostname
        )
    except OSError as exc:
        if _exc_errno(exc) in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise


def gethostbyaddr(hostname: str) -> tuple[str, list[str], list[str]]:
    try:
        return _require_socket_intrinsic(_molt_socket_gethostbyaddr, "gethostbyaddr")(
            hostname
        )
    except OSError as exc:
        if _exc_errno(exc) in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise _map_herror(exc) from None


def getfqdn(name: str | None = None) -> str:
    try:
        return _require_socket_intrinsic(_molt_socket_getfqdn, "getfqdn")(name)
    except OSError as exc:
        if _exc_errno(exc) in _EAI_CODES:
            raise _map_gaierror(exc) from None
        raise


def getservbyname(name: str, proto: str | None = None) -> int:
    return int(
        _require_socket_intrinsic(_molt_socket_getservbyname, "getservbyname")(
            name, proto
        )
    )


def getservbyport(port: int, proto: str | None = None) -> str:
    return _require_socket_intrinsic(_molt_socket_getservbyport, "getservbyport")(
        int(port), proto
    )


def inet_aton(address: str) -> bytes:
    return inet_pton(_CONSTANTS.get("AF_INET", 2), address)


def inet_pton(family: int, address: str) -> bytes:
    return _require_socket_intrinsic(_molt_socket_inet_pton, "inet_pton")(
        int(family), address
    )


def inet_ntoa(packed: bytes) -> str:
    return inet_ntop(_CONSTANTS.get("AF_INET", 2), packed)


def inet_ntop(family: int, packed: bytes) -> str:
    return _require_socket_intrinsic(_molt_socket_inet_ntop, "inet_ntop")(
        int(family), packed
    )


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
        reuse_port_value = _CONSTANTS.get("SO_REUSEPORT")
        if reuse_port and reuse_port_value is not None:
            sock.setsockopt(_CONSTANTS.get("SOL_SOCKET", 1), reuse_port_value, 1)
        sock.bind((host, port))
        sock.listen(0 if backlog is None else backlog)
        return sock
    except Exception:
        sock.close()
        raise
