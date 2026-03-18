"""Intrinsic-backed compatibility surface for CPython's `_ssl`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from ssl import (
    CERT_NONE,
    CERT_OPTIONAL,
    CERT_REQUIRED,
    HAS_SNI,
    MemoryBIO,
    OPENSSL_VERSION,
    PROTOCOL_TLS_CLIENT,
    PROTOCOL_TLS_SERVER,
    Purpose,
    SSLCertVerificationError,
    SSLContext,
    SSLError,
    SSLSocket,
    SSLWantReadError,
    TLSVersion,
    create_default_context,
)

_MOLT_SSL_CONTEXT_NEW = _require_intrinsic("molt_ssl_context_new")

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
