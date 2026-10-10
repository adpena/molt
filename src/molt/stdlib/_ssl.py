"""Intrinsic-backed compatibility surface for CPython's `_ssl`."""


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
