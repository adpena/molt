from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import sys
import types
from tests.stdlib_intrinsic_registry import install_registry


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
_fake_socket.SCM_RIGHTS = 1
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

install_registry({{
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
    "molt_socket_inet_pton": lambda family, text: b"\\x7f\\x00\\x00\\x01",
    "molt_socket_inet_ntop": lambda family, packed: "127.0.0.1",
    "molt_socket_htons": lambda value: value,
    "molt_socket_ntohs": lambda value: value,
    "molt_socket_htonl": lambda value: value,
    "molt_socket_ntohl": lambda value: value,
    "molt_socket_sethostname": lambda name: None,
}})


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
        and _private.htons(1) == 1
        and _private.ntohs(1) == 1
        and _private.htonl(1) == 1
        and _private.ntohl(1) == 1
        and _private.inet_aton("127.0.0.1") == b"\\x7f\\x00\\x00\\x01"
        and _private.inet_ntoa(b"\\x7f\\x00\\x00\\x01") == "127.0.0.1"
        and _private.inet_pton(2, "127.0.0.1") == b"\\x7f\\x00\\x00\\x01"
        and _private.inet_ntop(2, b"\\x7f\\x00\\x00\\x01") == "127.0.0.1"
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
        and "_MOLT_SOCKET_INET_PTON" not in _private.__dict__
        and "_MOLT_SOCKET_INET_NTOP" not in _private.__dict__
        and "_MOLT_SOCKET_HTONS" not in _private.__dict__
        and "_MOLT_SOCKET_NTOHS" not in _private.__dict__
        and "_MOLT_SOCKET_HTONL" not in _private.__dict__
        and "_MOLT_SOCKET_NTOHL" not in _private.__dict__
        and "_MOLT_AF_INET" not in _private.__dict__
        and "_MOLT_HAS_CMSG" not in _private.__dict__
        and "_MOLT_SOCKET_SETHOSTNAME" not in _private.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = run_surface_test_process(
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


_RESOLVER_PROBE = f"""
import importlib.util
import socket as oracle
import sys
from tests.stdlib_intrinsic_registry import install_registry


# CPython's actual module exception objects provide an independent identity and
# OSError-layout oracle. The fixture supplies the real lookup boundary once.
classes = {{"gaierror": oracle.gaierror, "herror": oracle.herror}}
lookups = []


def lookup(name):
    lookups.append(name)
    return classes[name]


def forbidden(*args, **kwargs):
    raise AssertionError("unexercised socket intrinsic was invoked")


current_error = None


def resolver(*args):
    raise current_error


behavior = {{name: forbidden for name in (
    'molt_os_close',
    'molt_os_dup',
    'molt_os_get_inheritable',
    'molt_os_set_inheritable',
    'molt_socket_accept',
    'molt_socket_bind',
    'molt_socket_clone',
    'molt_socket_close',
    'molt_socket_cmsg_len',
    'molt_socket_cmsg_space',
    'molt_socket_connect',
    'molt_socket_connect_ex',
    'molt_socket_detach',
    'molt_socket_drop',
    'molt_socket_fileno',
    'molt_socket_getblocking',
    'molt_socket_gethostname',
    'molt_socket_getpeername',
    'molt_socket_getprotobyname',
    'molt_socket_getservbyname',
    'molt_socket_getservbyport',
    'molt_socket_getsockname',
    'molt_socket_getsockopt',
    'molt_socket_gettimeout',
    'molt_socket_has_dualstack_ipv6',
    'molt_socket_htonl',
    'molt_socket_htons',
    'molt_socket_if_indextoname',
    'molt_socket_if_nameindex',
    'molt_socket_if_nametoindex',
    'molt_socket_inet_ntop',
    'molt_socket_inet_pton',
    'molt_socket_listen',
    'molt_socket_new',
    'molt_socket_ntohl',
    'molt_socket_ntohs',
    'molt_socket_reader_drop',
    'molt_socket_reader_new',
    'molt_socket_reader_read',
    'molt_socket_reader_readline',
    'molt_socket_reader_readline_limit',
    'molt_socket_recv',
    'molt_socket_recv_fds',
    'molt_socket_recv_into',
    'molt_socket_recvfrom',
    'molt_socket_recvfrom_into',
    'molt_socket_recvmsg',
    'molt_socket_recvmsg_into',
    'molt_socket_send',
    'molt_socket_send_fds',
    'molt_socket_sendall',
    'molt_socket_sendfile',
    'molt_socket_sendmsg',
    'molt_socket_sendmsg_afalg',
    'molt_socket_sendto',
    'molt_socket_sethostname',
    'molt_socket_setsockopt',
    'molt_socket_settimeout',
    'molt_socket_shutdown',
    'molt_socketpair',
)}}
behavior.update({{
    "molt_builtin_class_lookup": lookup,
    "molt_socket_constants": lambda: {{
        "AF_INET": 2, "SOCK_STREAM": 1, "MSG_PEEK": 2,
        "EAI_NONAME": 8, "EAI_SERVICE": 9, "EAI_SYSTEM": 11,
    }},
    "molt_socket_has_ipv6": lambda: False,
    "molt_socket_getaddrinfo": resolver,
    "molt_socket_getnameinfo": resolver,
    "molt_socket_gethostbyname": resolver,
    "molt_socket_gethostbyname_ex": resolver,
    "molt_socket_gethostbyaddr": resolver,
    "molt_socket_getfqdn": resolver,
}})
install_registry(behavior)


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


public = load("socket", {str(STDLIB_ROOT / "socket.py")!r})
private = load("_molt_resolver_socket", {str(STDLIB_ROOT / "_socket.py")!r})
assert lookups == ["gaierror", "herror"]
assert public.gaierror is private.gaierror is oracle.gaierror
assert public.herror is private.herror is oracle.herror

calls = {{
    "getaddrinfo": ("host", 80),
    "getnameinfo": (("127.0.0.1", 80), 0),
    "gethostbyname": ("host",),
    "gethostbyaddr": ("host",),
    "gethostbyname_ex": ("host",),
}}
for module in (public, private):
    for name, args in calls.items():
        for supplied in (
            oracle.gaierror(9, "resolver service"),
            oracle.gaierror(8, "resolver name"),
            OSError(9, "ordinary bad descriptor"),
            BlockingIOError(11, "ordinary pending operation"),
            oracle.herror(2, "host lookup"),
        ):
            current_error = supplied
            try:
                getattr(module, name)(*args)
            except OSError as caught:
                assert caught is supplied, (module.__name__, name, type(caught))
                assert caught.errno == supplied.errno
                assert caught.strerror == supplied.strerror
            else:
                raise AssertionError((module.__name__, name, "missing error"))
for supplied in (oracle.gaierror(9, "resolver"), OSError(9, "system")):
    current_error = supplied
    try:
        public.getfqdn("host")
    except OSError as caught:
        assert caught is supplied
    else:
        raise AssertionError("getfqdn swallowed intrinsic error")
print("canonical resolver identities and domains preserved")
"""


def test_socket_resolver_errors_preserve_canonical_identity_and_collisions() -> None:
    proc = run_surface_test_process(
        [sys.executable, "-c", _RESOLVER_PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    assert proc.stdout == "canonical resolver identities and domains preserved\n"
