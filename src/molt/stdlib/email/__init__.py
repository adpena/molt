"""Intrinsic-backed email package surface for Molt.

This package intentionally avoids host-Python stdlib fallback paths. Required
behavior must be provided by runtime intrinsics.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_EMAIL_MESSAGE_FROM_BYTES = _require_intrinsic(
    "molt_email_message_from_bytes", globals()
)

from . import policy as policy  # noqa: E402


def message_from_bytes(data: bytes | bytearray | memoryview, *, policy=policy.default):
    if not isinstance(data, (bytes, bytearray, memoryview)):
        raise TypeError("message_from_bytes() argument 1 must be a bytes-like object")
    handle = _MOLT_EMAIL_MESSAGE_FROM_BYTES(bytes(data))
    from . import message as _message

    return _message.EmailMessage._from_handle(handle, policy=policy)


def message_from_string(text: str, *, policy=policy.default):
    if not isinstance(text, str):
        raise TypeError("message_from_string() argument 1 must be str")
    return message_from_bytes(text.encode("utf-8", "surrogateescape"), policy=policy)


def __getattr__(name: str):
    if name in {"header", "headerregistry", "message", "parser", "policy", "utils"}:
        module = __import__(f"{__name__}.{name}", fromlist=[name])
        globals()[name] = module
        return module
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


__all__ = [
    "header",
    "headerregistry",
    "message",
    "message_from_bytes",
    "message_from_string",
    "parser",
    "policy",
    "utils",
]
