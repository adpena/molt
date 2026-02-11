"""Intrinsic-backed email.parser subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from . import message as _message
from . import policy as _policy

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_EMAIL_MESSAGE_FROM_BYTES = _require_intrinsic(
    "molt_email_message_from_bytes", globals()
)


class Parser:
    def __init__(self, *args, policy=_policy.compat32, **kwargs):
        if args or kwargs:
            raise RuntimeError("email.parser.Parser unsupported constructor arguments")
        self.policy = policy

    def parsestr(self, text: str, headersonly: bool = False):
        if not isinstance(text, str):
            raise TypeError("parsestr() expects str")
        if headersonly:
            text = text.split("\n\n", 1)[0] + "\n\n"
        handle = _MOLT_EMAIL_MESSAGE_FROM_BYTES(text.encode("utf-8", "surrogateescape"))
        return _message.EmailMessage._from_handle(handle, policy=self.policy)

    def parse(self, fp, headersonly: bool = False):
        reader = getattr(fp, "read", None)
        if not callable(reader):
            raise TypeError("parse() expects a file-like object with read()")
        text = reader()
        if not isinstance(text, str):
            raise TypeError("parse() expected read() to return str")
        return self.parsestr(text, headersonly=headersonly)


class BytesParser:
    def __init__(self, *args, policy=_policy.compat32, **kwargs):
        if args or kwargs:
            raise RuntimeError(
                "email.parser.BytesParser unsupported constructor arguments"
            )
        self.policy = policy

    def parsebytes(self, data, headersonly: bool = False):
        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("parsebytes() expects a bytes-like object")
        raw = bytes(data)
        if headersonly:
            marker = b"\n\n"
            split = raw.find(marker)
            if split >= 0:
                raw = raw[: split + len(marker)]
        handle = _MOLT_EMAIL_MESSAGE_FROM_BYTES(raw)
        return _message.EmailMessage._from_handle(handle, policy=self.policy)

    def parse(self, fp, headersonly: bool = False):
        reader = getattr(fp, "read", None)
        if not callable(reader):
            raise TypeError("parse() expects a file-like object with read()")
        data = reader()
        if isinstance(data, str):
            data = data.encode("utf-8", "surrogateescape")
        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("parse() expected read() to return bytes-like")
        return self.parsebytes(data, headersonly=headersonly)


__all__ = ["BytesParser", "Parser"]
