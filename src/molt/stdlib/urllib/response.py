"""Intrinsic-backed urllib.response surface for Molt."""

from __future__ import annotations

import tempfile

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")

_MOLT_RESP_READ = _require_intrinsic("molt_urllib_request_response_read")
_MOLT_RESP_READINTO = _require_intrinsic(
    "molt_urllib_request_response_readinto"
)
_MOLT_RESP_READ1 = _require_intrinsic("molt_urllib_request_response_read1")
_MOLT_RESP_READINTO1 = _require_intrinsic(
    "molt_urllib_request_response_readinto1"
)
_MOLT_RESP_READLINE = _require_intrinsic(
    "molt_urllib_request_response_readline"
)
_MOLT_RESP_READLINES = _require_intrinsic(
    "molt_urllib_request_response_readlines"
)
_MOLT_RESP_READABLE = _require_intrinsic(
    "molt_urllib_request_response_readable"
)
_MOLT_RESP_WRITABLE = _require_intrinsic(
    "molt_urllib_request_response_writable"
)
_MOLT_RESP_SEEKABLE = _require_intrinsic(
    "molt_urllib_request_response_seekable"
)
_MOLT_RESP_TELL = _require_intrinsic("molt_urllib_request_response_tell")
_MOLT_RESP_SEEK = _require_intrinsic("molt_urllib_request_response_seek")
_MOLT_RESP_CLOSE = _require_intrinsic("molt_urllib_request_response_close")
_MOLT_RESP_DROP = _require_intrinsic("molt_urllib_request_response_drop")
_MOLT_RESP_GETURL = _require_intrinsic("molt_urllib_request_response_geturl")
_MOLT_RESP_GETCODE = _require_intrinsic(
    "molt_urllib_request_response_getcode"
)
_MOLT_RESP_GETREASON = _require_intrinsic(
    "molt_urllib_request_response_getreason"
)
_MOLT_RESP_GETHEADER = _require_intrinsic(
    "molt_urllib_request_response_getheader"
)
_MOLT_RESP_GETHEADERS_LIST = _require_intrinsic(
    "molt_urllib_request_response_getheaders_list"
)
_MOLT_RESP_MESSAGE = _require_intrinsic(
    "molt_urllib_request_response_message"
)

__all__ = ["addbase", "addclosehook", "addinfo", "addinfourl"]

_TEMPORARY_FILE_WRAPPER_BASE = getattr(tempfile, "_TemporaryFileWrapper", None)
_ADD_BASE_PARENT = (
    _TEMPORARY_FILE_WRAPPER_BASE if _TEMPORARY_FILE_WRAPPER_BASE is not None else object
)


def _http_message_from_handle(handle: int):
    from http.client import HTTPMessage

    return HTTPMessage._from_handle(int(handle))


class _IntrinsicResponseFile:
    __slots__ = ("_handle", "_closed")

    def __init__(self, handle: int) -> None:
        self._handle = int(handle)
        self._closed = False

    @property
    def closed(self) -> bool:
        return self._closed

    def read(self, size: int = -1) -> bytes:
        amount = -1 if size is None else int(size)
        return _MOLT_RESP_READ(self._handle, amount)

    def read1(self, size: int = -1) -> bytes:
        amount = -1 if size is None else int(size)
        return _MOLT_RESP_READ1(self._handle, amount)

    def readline(self, size: int = -1) -> bytes:
        limit = -1 if size is None else int(size)
        return _MOLT_RESP_READLINE(self._handle, limit)

    def readlines(self, hint: int = -1) -> list[bytes]:
        bound = -1 if hint is None else int(hint)
        out = _MOLT_RESP_READLINES(self._handle, bound)
        if not isinstance(out, list):
            raise RuntimeError(
                "urllib.response response readlines intrinsic returned invalid value"
            )
        return [bytes(line) for line in out]

    def readinto(self, buffer) -> int:
        return int(_MOLT_RESP_READINTO(self._handle, buffer))

    def readinto1(self, buffer) -> int:
        return int(_MOLT_RESP_READINTO1(self._handle, buffer))

    def readable(self) -> bool:
        return bool(_MOLT_RESP_READABLE(self._handle))

    def writable(self) -> bool:
        return bool(_MOLT_RESP_WRITABLE(self._handle))

    def seekable(self) -> bool:
        return bool(_MOLT_RESP_SEEKABLE(self._handle))

    def tell(self) -> int:
        return int(_MOLT_RESP_TELL(self._handle))

    def seek(self, offset: int, whence: int = 0) -> int:
        return int(_MOLT_RESP_SEEK(self._handle, int(offset), int(whence)))

    def close(self) -> None:
        if not self._closed:
            _MOLT_RESP_CLOSE(self._handle)
            self._closed = True

    def __enter__(self):
        if self._closed:
            raise ValueError("I/O operation on closed file")
        return self

    def __exit__(self, exc_type, exc, tb):
        del exc_type, exc, tb
        self.close()
        return False

    def __iter__(self):
        return self

    def __next__(self) -> bytes:
        line = self.readline()
        if not line:
            raise StopIteration
        return line


class addbase(_ADD_BASE_PARENT):
    """Base class for addinfo and addclosehook."""

    def __init__(self, fp):
        if _TEMPORARY_FILE_WRAPPER_BASE is not None:
            super(addbase, self).__init__(fp, "<urllib response>", delete=False)
        else:
            self.file = fp
        self.fp = fp

    def __repr__(self):
        return "<%s at %r whose fp = %r>" % (
            self.__class__.__name__,
            id(self),
            self.file,
        )

    def __enter__(self):
        if self.fp.closed:
            raise ValueError("I/O operation on closed file")
        return self

    def __exit__(self, exc_type, exc, tb):
        del exc_type, exc, tb
        self.close()
        return None

    def close(self):
        if _TEMPORARY_FILE_WRAPPER_BASE is not None:
            super(addbase, self).close()
            return
        self.fp.close()

    def __getattr__(self, name):
        return getattr(self.fp, name)

    def __iter__(self):
        return iter(self.fp)


class addclosehook(addbase):
    """Class to add a close hook to an open file."""

    def __init__(self, fp, closehook, *hookargs):
        super(addclosehook, self).__init__(fp)
        self.closehook = closehook
        self.hookargs = hookargs

    def close(self):
        try:
            closehook = self.closehook
            hookargs = self.hookargs
            if closehook:
                self.closehook = None
                self.hookargs = None
                closehook(*hookargs)
        finally:
            super(addclosehook, self).close()


class addinfo(addbase):
    """Class to add an info() method to an open file."""

    def __init__(self, fp, headers):
        super(addinfo, self).__init__(fp)
        self.headers = headers

    def info(self):
        return self.headers


class addinfourl(addinfo):
    """Class to add info() and geturl() methods to an open file."""

    _molt_handle: int | None

    def __init__(self, fp, headers, url, code=None):
        super(addinfourl, self).__init__(fp, headers)
        self.url = url
        self.code = code
        self._molt_handle = None

    @classmethod
    def _from_handle(cls, handle: int) -> "addinfourl":
        response_handle = int(handle)
        url = _MOLT_RESP_GETURL(response_handle)
        code = _MOLT_RESP_GETCODE(response_handle)
        out = cls(
            _IntrinsicResponseFile(response_handle),
            None,
            url,
            code,
        )
        out._molt_handle = response_handle
        return out

    @property
    def status(self):
        return self.code

    @property
    def reason(self):
        handle = self._molt_handle
        if handle is None:
            return None
        return _MOLT_RESP_GETREASON(handle)

    @property
    def msg(self):
        return self.info()

    def getcode(self):
        return self.code

    def geturl(self):
        return self.url

    def info(self):
        handle = self._molt_handle
        if handle is None:
            return self.headers
        headers = self.headers
        if headers is None:
            message_handle = int(_MOLT_RESP_MESSAGE(handle))
            headers = _http_message_from_handle(message_handle)
            self.headers = headers
        return headers

    def getheader(self, name, default=None):
        handle = self._molt_handle
        if handle is not None:
            return _MOLT_RESP_GETHEADER(handle, name, default)
        info = self.info()
        getter = getattr(info, "get", None)
        if callable(getter):
            return getter(str(name), default)
        return default

    def getheaders(self):
        handle = self._molt_handle
        if handle is not None:
            out = _MOLT_RESP_GETHEADERS_LIST(handle)
            if not isinstance(out, list):
                raise RuntimeError(
                    "urllib.response response headers-list intrinsic returned invalid value"
                )
            return [(str(k), str(v)) for (k, v) in out]
        info = self.info()
        items = getattr(info, "items", None)
        if callable(items):
            return [(str(k), str(v)) for (k, v) in items()]
        return []

    def __del__(self):
        handle = getattr(self, "_molt_handle", None)
        if handle is None:
            return
        try:
            _MOLT_RESP_DROP(handle)
        except Exception:
            pass
        self._molt_handle = None
