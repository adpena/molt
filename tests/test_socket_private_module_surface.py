from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import sys
import types


class _SocketType:
    pass


_fake_socket = types.ModuleType("socket")
_fake_socket.error = RuntimeError
_fake_socket.timeout = TimeoutError
_fake_socket.gaierror = ValueError
_fake_socket.herror = OSError
_fake_socket.socket = _SocketType
_fake_socket.has_ipv6 = True
_fake_socket.AF_INET = 2
_fake_socket.SOCK_STREAM = 1
_fake_socket.getaddrinfo = lambda *args, **kwargs: [("info", args)]
_fake_socket.getdefaulttimeout = lambda: None
_fake_socket.gethostbyaddr = lambda host: ("name", [], [host])
_fake_socket.gethostbyname = lambda host: "127.0.0.1"
_fake_socket.gethostname = lambda: "localhost"
_fake_socket.getnameinfo = lambda sockaddr, flags=0: ("host", "port")
_fake_socket.getservbyname = lambda name, proto=None: 80
_fake_socket.getservbyport = lambda port, proto=None: "http"
_fake_socket.htonl = lambda value: value
_fake_socket.htons = lambda value: value
_fake_socket.inet_aton = lambda text: b"\\x7f\\x00\\x00\\x01"
_fake_socket.inet_ntoa = lambda packed: "127.0.0.1"
_fake_socket.inet_ntop = lambda family, packed: "127.0.0.1"
_fake_socket.inet_pton = lambda family, text: b"\\x7f\\x00\\x00\\x01"
_fake_socket.ntohl = lambda value: value
_fake_socket.ntohs = lambda value: value
_fake_socket.setdefaulttimeout = lambda value: None
_fake_socket.socketpair = lambda: ("a", "b")
sys.modules["socket"] = _fake_socket

builtins._molt_intrinsics = {{
    "molt_socket_constants": lambda: None,
    "molt_os_close": lambda fd: ("close", fd),
    "molt_os_dup": lambda fd: fd + 1,
    "molt_socket_getprotobyname": lambda name: 6,
    "molt_socket_gethostbyname_ex": lambda host: (host, [], ["127.0.0.1"]),
    "molt_socket_if_nameindex": lambda: [(1, "lo0")],
    "molt_socket_if_nametoindex": lambda name: 1,
    "molt_socket_if_indextoname": lambda idx: "lo0",
    "molt_socket_cmsg_len": lambda n: n + 1,
    "molt_socket_cmsg_space": lambda n: n + 2,
    "molt_socket_sethostname": lambda name: None,
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("_molt_private_socket", {str(STDLIB_ROOT / "_socket.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.CMSG_LEN(4) == 5
        and _private.CMSG_SPACE(4) == 6
        and _private.dup(3) == 4
        and _private.getprotobyname("tcp") == 6
        and _private.gethostbyname_ex("localhost") == ("localhost", [], ["127.0.0.1"])
        and _private.if_nameindex() == [(1, "lo0")]
        and _private.if_nametoindex("lo0") == 1
        and _private.if_indextoname(1) == "lo0"
    ),
    "private_handles_hidden": (
        "_MOLT_SOCKET_CONSTANTS" not in _private.__dict__
        and "_MOLT_OS_CLOSE" not in _private.__dict__
        and "_MOLT_OS_DUP" not in _private.__dict__
        and "_MOLT_SOCKET_GETPROTOBYNAME" not in _private.__dict__
        and "_MOLT_GETHOSTBYNAME_EX" not in _private.__dict__
        and "_MOLT_IF_NAMEINDEX" not in _private.__dict__
        and "_MOLT_IF_NAMETOINDEX" not in _private.__dict__
        and "_MOLT_IF_INDEXTONAME" not in _private.__dict__
        and "_MOLT_CMSG_LEN" not in _private.__dict__
        and "_MOLT_CMSG_SPACE" not in _private.__dict__
        and "_MOLT_SOCKET_SETHOSTNAME" not in _private.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__socket_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_socket_getprotobyname" not in names
    assert "getprotobyname" in names
    assert "CMSG_LEN" in names
    assert "CMSG_SPACE" in names
    assert "socket" in names
    assert "SocketType" in names
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
