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

_STATE = {{
    "next_context": 0,
    "next_socket": 0,
    "contexts": {{}},
    "sockets": {{}},
}}


def _new_context(protocol):
    handle = f"ctx:{{_STATE['next_context']}}"
    _STATE["next_context"] += 1
    _STATE["contexts"][handle] = {{
        "protocol": int(protocol),
        "verify_mode": 0,
        "check_hostname": False,
    }}
    return handle


def _ctx(handle):
    return _STATE["contexts"][handle]


def _new_socket(*args):
    # Real intrinsic: (sock_fd, ctx_handle, server_hostname, server_side)
    handle = f"sock:{{_STATE['next_socket']}}"
    _STATE["next_socket"] += 1
    _STATE["sockets"][handle] = {{
        "read": b"peer-bytes",
        "written": bytearray(),
        "args": args,
    }}
    return handle


def _sock(handle):
    return _STATE["sockets"][handle]


builtins._molt_intrinsics = {{
    "molt_ssl_protocol_tls_client": lambda: 16,
    "molt_ssl_protocol_tls_server": lambda: 17,
    "molt_ssl_cert_none": lambda: 0,
    "molt_ssl_cert_optional": lambda: 1,
    "molt_ssl_cert_required": lambda: 2,
    "molt_ssl_has_sni": lambda: True,
    "molt_ssl_openssl_version": lambda: "OpenSSL stub 1.0",
    "molt_ssl_context_new": _new_context,
    "molt_ssl_context_drop": lambda _handle: None,
    "molt_ssl_context_get_protocol": lambda handle: _ctx(handle)["protocol"],
    "molt_ssl_context_verify_mode_get": lambda handle: _ctx(handle)["verify_mode"],
    "molt_ssl_context_verify_mode_set": lambda handle, value: _ctx(handle).__setitem__("verify_mode", int(value)),
    "molt_ssl_context_check_hostname_get": lambda handle: _ctx(handle)["check_hostname"],
    "molt_ssl_context_check_hostname_set": lambda handle, value: _ctx(handle).__setitem__("check_hostname", bool(value)),
    "molt_ssl_context_set_ciphers": lambda _handle, _spec: None,
    "molt_ssl_context_set_default_verify_paths": lambda _handle: None,
    "molt_ssl_context_load_cert_chain": lambda _handle, _certfile, _keyfile, _password: None,
    "molt_ssl_context_load_verify_locations": lambda _handle, _cafile, _capath, _cadata: None,
    "molt_ssl_create_default_context": lambda _purpose: _new_context(16),
    "molt_ssl_wrap_socket": _new_socket,
    "molt_ssl_socket_do_handshake": lambda _handle: None,
    "molt_ssl_socket_read": lambda handle, _length: _sock(handle)["read"],
    "molt_ssl_socket_write": lambda handle, data: _sock(handle)["written"].extend(bytes(data)) or len(bytes(data)),
    "molt_ssl_socket_cipher": lambda _handle: ("TLS_AES_256_GCM_SHA384", "TLSv1.3", 256),
    "molt_ssl_socket_version": lambda _handle: "TLSv1.3",
    "molt_ssl_socket_getpeercert": lambda _handle, binary_form: b"cert" if binary_form else {{"subject": ((("commonName", "stub"),),)}},
    "molt_ssl_socket_unwrap": lambda _handle: "raw-socket",
    "molt_ssl_socket_close": lambda _handle: None,
    "molt_ssl_socket_drop": lambda _handle: None,
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


_load_module("ssl", {str(STDLIB_ROOT / "ssl.py")!r})
_private = _load_module("_ssl", {str(STDLIB_ROOT / "_ssl.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

ctx = _private.SSLContext()
ctx.verify_mode = _private.CERT_REQUIRED
ctx.check_hostname = True
class _StubSocket:
    def fileno(self): return 7
    def getpeername(self): return ("example.com", 443)
    def getsockname(self): return ("127.0.0.1", 12345)
_stub_sock = _StubSocket()
wrapped = ctx.wrap_socket(_stub_sock, server_hostname="example.com")

checks = {{
    "constants": (
        _private.PROTOCOL_TLS_CLIENT == 16
        and _private.PROTOCOL_TLS_SERVER == 17
        and _private.CERT_NONE == 0
        and _private.CERT_OPTIONAL == 1
        and _private.CERT_REQUIRED == 2
        and _private.HAS_SNI is True
        and _private.OPENSSL_VERSION == "OpenSSL stub 1.0"
    ),
    "context": (
        isinstance(ctx, _private.SSLContext)
        and ctx.protocol == 16
        and ctx.verify_mode == 2
        and ctx.check_hostname is True
    ),
    "socket": (
        isinstance(wrapped, _private.SSLSocket)
        and wrapped.read() == b"peer-bytes"
        and wrapped.write(b"abc") == 3
        and wrapped.cipher() == ("TLS_AES_256_GCM_SHA384", "TLSv1.3", 256)
        and wrapped.version() == "TLSv1.3"
        and wrapped.getpeercert(binary_form=True) == b"cert"
        and wrapped.unwrap() is _stub_sock
        and wrapped.fileno() == 7
    ),
    "default_context": isinstance(_private.create_default_context(), _private.SSLContext),
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


def test__ssl_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [
        ("CERT_NONE", "int", "False"),
        ("CERT_OPTIONAL", "int", "False"),
        ("CERT_REQUIRED", "int", "False"),
        ("HAS_SNI", "bool", "False"),
        ("MemoryBIO", "type", "True"),
        ("OPENSSL_VERSION", "str", "False"),
        ("PROTOCOL_TLS_CLIENT", "int", "False"),
        ("PROTOCOL_TLS_SERVER", "int", "False"),
        ("Purpose", "type", "True"),
        ("SSLCertVerificationError", "type", "True"),
        ("SSLContext", "type", "True"),
        ("SSLError", "type", "True"),
        ("SSLSocket", "type", "True"),
        ("SSLWantReadError", "type", "True"),
        ("TLSVersion", "type", "True"),
        ("create_default_context", "function", "True"),
    ]
    assert checks == {
        "constants": "True",
        "context": "True",
        "default_context": "True",
        "socket": "True",
    }
