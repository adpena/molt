"""Intrinsic-backed urllib.error subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "ContentTooShortError",
    "HTTPError",
    "URLError",
]

_MOLT_URLERROR_INIT = _require_intrinsic("molt_urllib_error_urlerror_init", globals())
_MOLT_URLERROR_STR = _require_intrinsic("molt_urllib_error_urlerror_str", globals())
_MOLT_HTTPERROR_INIT = _require_intrinsic("molt_urllib_error_httperror_init", globals())
_MOLT_HTTPERROR_STR = _require_intrinsic("molt_urllib_error_httperror_str", globals())
_MOLT_CONTENT_TOO_SHORT_INIT = _require_intrinsic(
    "molt_urllib_error_content_too_short_init",
    globals(),
)


class URLError(OSError):
    reason: object
    filename: object

    def __init__(self, reason: object, filename: object | None = None) -> None:
        _MOLT_URLERROR_INIT(self, reason, filename)

    def __str__(self) -> str:
        out = _MOLT_URLERROR_STR(self.reason)
        if not isinstance(out, str):
            raise RuntimeError(
                "urllib.error.URLError.__str__ intrinsic returned invalid value"
            )
        return out


class HTTPError(URLError):
    code: object
    msg: object
    hdrs: object
    fp: object

    def __init__(
        self,
        url: object,
        code: object,
        msg: object,
        hdrs: object,
        fp: object,
    ) -> None:
        _MOLT_HTTPERROR_INIT(self, url, code, msg, hdrs, fp)

    def __str__(self) -> str:
        out = _MOLT_HTTPERROR_STR(self.code, self.msg)
        if not isinstance(out, str):
            raise RuntimeError(
                "urllib.error.HTTPError.__str__ intrinsic returned invalid value"
            )
        return out

    @property
    def headers(self) -> object:
        return self.hdrs

    @headers.setter
    def headers(self, headers: object) -> None:
        self.hdrs = headers

    def read(self, size: int = -1) -> object:
        fp = self.fp
        if fp is None:
            return b""
        read = getattr(fp, "read", None)
        if callable(read):
            return read(size)
        return b""

    def close(self) -> None:
        fp = self.fp
        if fp is None:
            return
        close = getattr(fp, "close", None)
        if callable(close):
            close()

    def info(self) -> object:
        return self.headers

    def geturl(self) -> object:
        return self.filename

    def getcode(self) -> object:
        return self.code


class ContentTooShortError(URLError):
    content: object

    def __init__(self, msg: object, content: object) -> None:
        _MOLT_CONTENT_TOO_SHORT_INIT(self, msg, content)
