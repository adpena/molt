"""Intrinsic-first `_socket` shim (CPython internal module).

CPython's `socket` stdlib module is layered on top of a C extension named
`_socket`. Molt doesn't embed CPython, so `_socket` is a thin shim that forwards
to Molt's intrinsic-backed `socket` stdlib module (which itself calls Rust
intrinsics).

Policy: no host-Python fallback. Missing intrinsics must raise immediately.
"""

from __future__ import annotations

import socket as _socket_mod

from _intrinsics import require_intrinsic as _require_intrinsic

# Avoid probe-only classification: `_socket` must be intrinsic-backed.
_require_intrinsic("molt_socket_constants", globals())

# Exception classes (CPython-compatible names).
error = _socket_mod.error
timeout = _socket_mod.timeout
gaierror = _socket_mod.gaierror
herror = _socket_mod.herror

# Core type(s)
SocketType = _socket_mod.socket

# Public helpers commonly surfaced by `_socket` in CPython.
socket = _socket_mod.socket
socketpair = getattr(_socket_mod, "socketpair", None)
fromfd = getattr(_socket_mod, "fromfd", None)

getaddrinfo = _socket_mod.getaddrinfo
getnameinfo = _socket_mod.getnameinfo
gethostname = _socket_mod.gethostname
gethostbyname = _socket_mod.gethostbyname
gethostbyaddr = _socket_mod.gethostbyaddr
getfqdn = _socket_mod.getfqdn
getservbyname = _socket_mod.getservbyname
getservbyport = _socket_mod.getservbyport
inet_pton = _socket_mod.inet_pton
inet_ntop = _socket_mod.inet_ntop
htons = _socket_mod.htons
ntohs = _socket_mod.ntohs
htonl = _socket_mod.htonl
ntohl = _socket_mod.ntohl

# Constants are injected into `socket` from runtime intrinsics; mirror them here.
for _name, _val in list(_socket_mod.__dict__.items()):
    if isinstance(_val, int) and (
        _name.startswith("AF_")
        or _name.startswith("SOCK_")
        or _name.startswith("IPPROTO_")
        or _name.startswith("SOL_")
        or _name.startswith("SO_")
        or _name.startswith("MSG_")
        or _name.startswith("SHUT_")
        or _name.startswith("TCP_")
        or _name.startswith("AI_")
        or _name.startswith("EAI_")
    ):
        globals()[_name] = _val

__all__ = [
    "SocketType",
    "socket",
    "socketpair",
    "fromfd",
    "error",
    "timeout",
    "gaierror",
    "herror",
    "getaddrinfo",
    "getnameinfo",
    "gethostname",
    "gethostbyname",
    "gethostbyaddr",
    "getfqdn",
    "getservbyname",
    "getservbyport",
    "inet_pton",
    "inet_ntop",
    "htons",
    "ntohs",
    "htonl",
    "ntohl",
]
