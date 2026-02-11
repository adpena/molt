"""Minimal intrinsic-gated ssl subset for context and MemoryBIO behavior."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()


PROTOCOL_TLS_CLIENT = 1
PROTOCOL_TLS_SERVER = 2

CERT_NONE = 0
CERT_REQUIRED = 2


class SSLError(Exception):
    pass


class SSLWantReadError(SSLError):
    pass


class TLSVersion:
    TLSv1_2 = 0x0303
    TLSv1_3 = 0x0304


class MemoryBIO:
    def __init__(self) -> None:
        self._buffer = bytearray()


class _SSLObject:
    def __init__(self, _incoming: MemoryBIO, _outgoing: MemoryBIO) -> None:
        self._incoming = _incoming
        self._outgoing = _outgoing

    def do_handshake(self) -> None:
        raise SSLWantReadError()


class SSLContext:
    def __init__(self, protocol: int) -> None:
        self.protocol = int(protocol)
        self._verify_mode = CERT_REQUIRED
        self._check_hostname = self.protocol == PROTOCOL_TLS_CLIENT
        self.minimum_version = TLSVersion.TLSv1_2
        self.maximum_version = TLSVersion.TLSv1_3
        self._ciphers = [{"name": "TLS_AES_128_GCM_SHA256"}]

    @property
    def check_hostname(self) -> bool:
        return self._check_hostname

    @check_hostname.setter
    def check_hostname(self, value: bool) -> None:
        enabled = bool(value)
        if enabled and self._verify_mode == CERT_NONE:
            self._verify_mode = CERT_REQUIRED
        self._check_hostname = enabled

    @property
    def verify_mode(self) -> int:
        return self._verify_mode

    @verify_mode.setter
    def verify_mode(self, value: int) -> None:
        mode = int(value)
        self._verify_mode = mode

    def set_ciphers(self, _spec: str) -> None:
        return None

    def get_ciphers(self) -> list[dict[str, str]]:
        return list(self._ciphers)

    def wrap_bio(
        self, incoming: MemoryBIO, outgoing: MemoryBIO, *, server_side: bool = False
    ) -> _SSLObject:
        _ = server_side
        return _SSLObject(incoming, outgoing)


def create_default_context() -> SSLContext:
    ctx = SSLContext(PROTOCOL_TLS_CLIENT)
    ctx.check_hostname = True
    ctx.verify_mode = CERT_REQUIRED
    return ctx


__all__ = [
    "CERT_NONE",
    "CERT_REQUIRED",
    "MemoryBIO",
    "PROTOCOL_TLS_CLIENT",
    "PROTOCOL_TLS_SERVER",
    "SSLContext",
    "SSLError",
    "SSLWantReadError",
    "TLSVersion",
    "create_default_context",
]
