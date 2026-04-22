"""Intrinsic-backed ssl module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Protocol constants
_MOLT_SSL_PROTOCOL_TLS_CLIENT = _require_intrinsic("molt_ssl_protocol_tls_client")
_MOLT_SSL_PROTOCOL_TLS_SERVER = _require_intrinsic("molt_ssl_protocol_tls_server")
_MOLT_SSL_CERT_NONE = _require_intrinsic("molt_ssl_cert_none")
_MOLT_SSL_CERT_OPTIONAL = _require_intrinsic("molt_ssl_cert_optional")
_MOLT_SSL_CERT_REQUIRED = _require_intrinsic("molt_ssl_cert_required")
_MOLT_SSL_HAS_SNI = _require_intrinsic("molt_ssl_has_sni")
_MOLT_SSL_OPENSSL_VERSION = _require_intrinsic("molt_ssl_openssl_version")

# Context operations
_MOLT_SSL_CONTEXT_NEW = _require_intrinsic("molt_ssl_context_new")
_MOLT_SSL_CONTEXT_DROP = _require_intrinsic("molt_ssl_context_drop")
_MOLT_SSL_CONTEXT_GET_PROTOCOL = _require_intrinsic("molt_ssl_context_get_protocol")
_MOLT_SSL_CONTEXT_VERIFY_MODE_GET = _require_intrinsic(
    "molt_ssl_context_verify_mode_get"
)
_MOLT_SSL_CONTEXT_VERIFY_MODE_SET = _require_intrinsic(
    "molt_ssl_context_verify_mode_set"
)
_MOLT_SSL_CONTEXT_CHECK_HOSTNAME_GET = _require_intrinsic(
    "molt_ssl_context_check_hostname_get"
)
_MOLT_SSL_CONTEXT_CHECK_HOSTNAME_SET = _require_intrinsic(
    "molt_ssl_context_check_hostname_set"
)
_MOLT_SSL_CONTEXT_SET_CIPHERS = _require_intrinsic("molt_ssl_context_set_ciphers")
_MOLT_SSL_CONTEXT_SET_DEFAULT_VERIFY_PATHS = _require_intrinsic(
    "molt_ssl_context_set_default_verify_paths"
)
_MOLT_SSL_CONTEXT_LOAD_CERT_CHAIN = _require_intrinsic(
    "molt_ssl_context_load_cert_chain"
)
_MOLT_SSL_CONTEXT_LOAD_VERIFY_LOCATIONS = _require_intrinsic(
    "molt_ssl_context_load_verify_locations"
)
_MOLT_SSL_CREATE_DEFAULT_CONTEXT = _require_intrinsic("molt_ssl_create_default_context")

# Socket wrapping
_MOLT_SSL_WRAP_SOCKET = _require_intrinsic("molt_ssl_wrap_socket")
_MOLT_SSL_SOCKET_DO_HANDSHAKE = _require_intrinsic("molt_ssl_socket_do_handshake")
_MOLT_SSL_SOCKET_READ = _require_intrinsic("molt_ssl_socket_read")
_MOLT_SSL_SOCKET_WRITE = _require_intrinsic("molt_ssl_socket_write")
_MOLT_SSL_SOCKET_CIPHER = _require_intrinsic("molt_ssl_socket_cipher")
_MOLT_SSL_SOCKET_VERSION = _require_intrinsic("molt_ssl_socket_version")
_MOLT_SSL_SOCKET_GETPEERCERT = _require_intrinsic("molt_ssl_socket_getpeercert")
_MOLT_SSL_SOCKET_UNWRAP = _require_intrinsic("molt_ssl_socket_unwrap")
_MOLT_SSL_SOCKET_CLOSE = _require_intrinsic("molt_ssl_socket_close")
_MOLT_SSL_SOCKET_DROP = _require_intrinsic("molt_ssl_socket_drop")

PROTOCOL_TLS_CLIENT = int(_MOLT_SSL_PROTOCOL_TLS_CLIENT())
PROTOCOL_TLS_SERVER = int(_MOLT_SSL_PROTOCOL_TLS_SERVER())

CERT_NONE = int(_MOLT_SSL_CERT_NONE())
CERT_OPTIONAL = int(_MOLT_SSL_CERT_OPTIONAL())
CERT_REQUIRED = int(_MOLT_SSL_CERT_REQUIRED())

HAS_SNI = bool(_MOLT_SSL_HAS_SNI())
OPENSSL_VERSION = str(_MOLT_SSL_OPENSSL_VERSION())


class SSLError(Exception):
    pass


class SSLWantReadError(SSLError):
    pass


class SSLCertVerificationError(SSLError):
    pass


class TLSVersion:
    TLSv1_2 = 0x0303
    TLSv1_3 = 0x0304


class Purpose:
    SERVER_AUTH = "SERVER_AUTH"
    CLIENT_AUTH = "CLIENT_AUTH"


class MemoryBIO:
    def __init__(self) -> None:
        self._buffer = bytearray()


class SSLSocket:
    def __init__(self, handle: object) -> None:
        self._handle = handle

    def do_handshake(self) -> None:
        try:
            _MOLT_SSL_SOCKET_DO_HANDSHAKE(self._handle)
        except Exception as exc:
            raise SSLError(str(exc)) from exc

    def read(self, length: int = 16384) -> bytes:
        return bytes(_MOLT_SSL_SOCKET_READ(self._handle, length))

    def write(self, data: bytes) -> int:
        return int(_MOLT_SSL_SOCKET_WRITE(self._handle, data))

    def cipher(self) -> tuple[str, str, int] | None:
        return _MOLT_SSL_SOCKET_CIPHER(self._handle)

    def version(self) -> str | None:
        return _MOLT_SSL_SOCKET_VERSION(self._handle)

    def getpeercert(self, binary_form: bool = False) -> object:
        return _MOLT_SSL_SOCKET_GETPEERCERT(self._handle, binary_form)

    def unwrap(self) -> object:
        return _MOLT_SSL_SOCKET_UNWRAP(self._handle)

    def close(self) -> None:
        _MOLT_SSL_SOCKET_CLOSE(self._handle)

    def __del__(self) -> None:
        try:
            _MOLT_SSL_SOCKET_DROP(self._handle)
        except Exception:
            pass


class SSLContext:
    def __init__(self, protocol: int = PROTOCOL_TLS_CLIENT) -> None:
        self._handle = _MOLT_SSL_CONTEXT_NEW(int(protocol))

    @property
    def protocol(self) -> int:
        return int(_MOLT_SSL_CONTEXT_GET_PROTOCOL(self._handle))

    @property
    def verify_mode(self) -> int:
        return int(_MOLT_SSL_CONTEXT_VERIFY_MODE_GET(self._handle))

    @verify_mode.setter
    def verify_mode(self, value: int) -> None:
        _MOLT_SSL_CONTEXT_VERIFY_MODE_SET(self._handle, int(value))

    @property
    def check_hostname(self) -> bool:
        return bool(_MOLT_SSL_CONTEXT_CHECK_HOSTNAME_GET(self._handle))

    @check_hostname.setter
    def check_hostname(self, value: bool) -> None:
        _MOLT_SSL_CONTEXT_CHECK_HOSTNAME_SET(self._handle, bool(value))

    def set_ciphers(self, spec: str) -> None:
        _MOLT_SSL_CONTEXT_SET_CIPHERS(self._handle, spec)

    def set_default_verify_paths(self) -> None:
        _MOLT_SSL_CONTEXT_SET_DEFAULT_VERIFY_PATHS(self._handle)

    def load_cert_chain(
        self,
        certfile: str,
        keyfile: str | None = None,
        password: str | None = None,
    ) -> None:
        _MOLT_SSL_CONTEXT_LOAD_CERT_CHAIN(self._handle, certfile, keyfile, password)

    def load_verify_locations(
        self,
        cafile: str | None = None,
        capath: str | None = None,
        cadata: bytes | None = None,
    ) -> None:
        _MOLT_SSL_CONTEXT_LOAD_VERIFY_LOCATIONS(self._handle, cafile, capath, cadata)

    def wrap_socket(
        self,
        sock: object,
        *,
        server_side: bool = False,
        server_hostname: str | None = None,
    ) -> SSLSocket:
        handle = _MOLT_SSL_WRAP_SOCKET(self._handle, sock, server_side, server_hostname)
        return SSLSocket(handle)

    def wrap_bio(
        self, incoming: MemoryBIO, outgoing: MemoryBIO, *, server_side: bool = False
    ) -> SSLSocket:
        # MemoryBIO wrapping delegates through the same socket intrinsic
        handle = _MOLT_SSL_WRAP_SOCKET(self._handle, None, server_side, None)
        return SSLSocket(handle)

    def __del__(self) -> None:
        try:
            _MOLT_SSL_CONTEXT_DROP(self._handle)
        except Exception:
            pass


def create_default_context(
    purpose: str = Purpose.SERVER_AUTH,
) -> SSLContext:
    handle = _MOLT_SSL_CREATE_DEFAULT_CONTEXT(purpose)
    ctx = SSLContext.__new__(SSLContext)
    ctx._handle = handle
    return ctx


__all__ = [
    "CERT_NONE",
    "CERT_OPTIONAL",
    "CERT_REQUIRED",
    "HAS_SNI",
    "MemoryBIO",
    "OPENSSL_VERSION",
    "PROTOCOL_TLS_CLIENT",
    "PROTOCOL_TLS_SERVER",
    "Purpose",
    "SSLCertVerificationError",
    "SSLContext",
    "SSLError",
    "SSLSocket",
    "SSLWantReadError",
    "TLSVersion",
    "create_default_context",
]

globals().pop("_require_intrinsic", None)
