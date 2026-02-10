"""Intrinsic-backed urllib.request subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from urllib.error import URLError

__all__ = [
    "BaseHandler",
    "HTTPErrorProcessor",
    "HTTPHandler",
    "HTTPCookieProcessor",
    "HTTPPasswordMgrWithDefaultRealm",
    "HTTPRedirectHandler",
    "HTTPSHandler",
    "OpenerDirector",
    "ProxyBasicAuthHandler",
    "ProxyHandler",
    "Request",
    "addinfourl",
    "build_opener",
    "urlopen",
]

_MOLT_REQUEST_INIT = _require_intrinsic("molt_urllib_request_request_init", globals())
_MOLT_OPENER_INIT = _require_intrinsic("molt_urllib_request_opener_init", globals())
_MOLT_OPENER_ADD_HANDLER = _require_intrinsic(
    "molt_urllib_request_add_handler",
    globals(),
)
_MOLT_OPENER_OPEN = _require_intrinsic("molt_urllib_request_open", globals())
_MOLT_PROCESS_HTTP_ERROR = _require_intrinsic(
    "molt_urllib_request_process_http_error",
    globals(),
)
_MOLT_RESP_READ = _require_intrinsic("molt_urllib_request_response_read", globals())
_MOLT_RESP_CLOSE = _require_intrinsic("molt_urllib_request_response_close", globals())
_MOLT_RESP_DROP = _require_intrinsic("molt_urllib_request_response_drop", globals())
_MOLT_RESP_GETURL = _require_intrinsic("molt_urllib_request_response_geturl", globals())
_MOLT_RESP_GETCODE = _require_intrinsic(
    "molt_urllib_request_response_getcode", globals()
)
_MOLT_RESP_GETREASON = _require_intrinsic(
    "molt_urllib_request_response_getreason",
    globals(),
)
_MOLT_RESP_GETHEADERS = _require_intrinsic(
    "molt_urllib_request_response_getheaders",
    globals(),
)


class Request:
    full_url: str
    data: object
    headers: dict[str, object]
    method: object
    timeout: object

    def __init__(self, url, data=None, headers=None, method=None):
        _MOLT_REQUEST_INIT(self, url, data, headers, method)
        self.timeout = None

    def get_full_url(self):
        return self.full_url

    def has_header(self, header_name):
        return str(header_name) in self.headers

    def add_header(self, key, value):
        self.headers[str(key)] = value

    def get_header(self, header_name, default=None):
        return self.headers.get(str(header_name), default)

    def remove_header(self, header_name):
        self.headers.pop(str(header_name), None)

    def get_method(self):
        if self.method is not None:
            return str(self.method)
        return "POST" if self.data is not None else "GET"


class _DataResponse:
    __slots__ = ("_data", "_pos", "closed", "url")

    def __init__(self, data, url):
        self._data = data
        self._pos = 0
        self.closed = False
        self.url = url

    def read(self, size=-1):
        if self.closed:
            raise ValueError("I/O operation on closed file.")
        if size is None or size < 0:
            size = len(self._data) - self._pos
        start = self._pos
        end = min(len(self._data), start + int(size))
        self._pos = end
        return self._data[start:end]

    def close(self):
        self.closed = True

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        del exc_type, exc, tb
        self.close()
        return False


class addinfourl:
    def __init__(self, handle):
        self._handle = handle
        self.closed = False
        self.url = self.geturl()
        self.headers = self.info()

    def read(self, size=-1):
        if self.closed:
            raise ValueError("I/O operation on closed file.")
        return _MOLT_RESP_READ(self._handle, int(size))

    def close(self):
        if not self.closed:
            _MOLT_RESP_CLOSE(self._handle)
            self.closed = True

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _MOLT_RESP_DROP(handle)
        except Exception:
            pass
        self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        del exc_type, exc, tb
        self.close()
        return False

    def geturl(self):
        return _MOLT_RESP_GETURL(self._handle)

    def getcode(self):
        return _MOLT_RESP_GETCODE(self._handle)

    @property
    def status(self):
        return self.getcode()

    @property
    def reason(self):
        return _MOLT_RESP_GETREASON(self._handle)

    @property
    def code(self):
        return self.getcode()

    @property
    def msg(self):
        return self.info()

    def info(self):
        out = _MOLT_RESP_GETHEADERS(self._handle)
        if not isinstance(out, dict):
            raise RuntimeError(
                "urllib.request response headers intrinsic returned invalid value"
            )
        return out

    def getheader(self, name, default=None):
        return self.info().get(str(name), default)

    def getheaders(self):
        return list(self.info().items())


class BaseHandler:
    handler_order = 500
    parent = None


class HTTPHandler(BaseHandler):
    handler_order = 400


class HTTPSHandler(BaseHandler):
    handler_order = 400


class HTTPRedirectHandler(BaseHandler):
    handler_order = 600


class HTTPCookieProcessor(BaseHandler):
    handler_order = 700

    def __init__(self, cookiejar=None):
        if cookiejar is None:
            from http.cookiejar import CookieJar

            cookiejar = CookieJar()
        self.cookiejar = cookiejar


class ProxyHandler(BaseHandler):
    handler_order = 100

    def __init__(self, proxies=None):
        self.proxies = dict(proxies or {})


class HTTPPasswordMgrWithDefaultRealm:
    def __init__(self):
        self._entries = {}

    def add_password(self, realm, uri, user, passwd):
        key = (None if realm is None else str(realm), str(uri))
        self._entries[key] = (str(user), str(passwd))

    def find_user_password(self, realm, authuri):
        key = (None if realm is None else str(realm), str(authuri))
        if key in self._entries:
            return self._entries[key]
        fallback = (None, str(authuri))
        return self._entries.get(fallback, (None, None))


class ProxyBasicAuthHandler(BaseHandler):
    handler_order = 800

    def __init__(self, password_mgr=None):
        self.passwd = password_mgr or HTTPPasswordMgrWithDefaultRealm()


class HTTPErrorProcessor(BaseHandler):
    handler_order = 1000

    def http_response(self, request, response):
        if not (
            isinstance(response, tuple)
            and len(response) == 2
            and response[0] in ("__molt_urllib_response__", b"__molt_urllib_response__")
        ):
            return response
        out = _MOLT_PROCESS_HTTP_ERROR(request, response)
        if isinstance(out, (str, bytes, int, float)):
            return response
        return out

    https_response = http_response


class OpenerDirector:
    def __init__(self):
        _MOLT_OPENER_INIT(self)
        self._molt_allow_data_fallback = False
        self._molt_raise_on_none = False

    def add_handler(self, handler):
        _MOLT_OPENER_ADD_HANDLER(self, handler)

    def _wrap_response(self, req, out):
        if out is None:
            if self._molt_raise_on_none:
                url = req.full_url
                scheme = url.split(":", 1)[0] if ":" in url else url
                raise URLError(f"unknown url type: {scheme}")
            return None
        if isinstance(out, tuple) and len(out) == 2:
            marker, handle = out
            if (
                marker == "__molt_urllib_response__"
                or marker == b"__molt_urllib_response__"
            ):
                return addinfourl(handle)
            if isinstance(handle, int):
                return addinfourl(handle)
            if isinstance(handle, float) and handle.is_integer():
                return addinfourl(int(handle))
        if isinstance(out, int):
            return addinfourl(out)
        if isinstance(out, float) and out.is_integer():
            return addinfourl(int(out))
        if isinstance(out, (bytes, bytearray)):
            return _DataResponse(bytes(out), req.full_url)
        return out

    def open(self, fullurl, data=None, timeout=None):
        req = fullurl if isinstance(fullurl, Request) else Request(fullurl, data)
        if data is not None and isinstance(req, Request):
            req.data = data
        req.timeout = timeout
        out = _MOLT_OPENER_OPEN(self, req)
        return self._wrap_response(req, out)


def build_opener(*handlers):
    opener = OpenerDirector()
    opener._molt_allow_data_fallback = True
    opener._molt_raise_on_none = True
    opener.add_handler(ProxyHandler())
    opener.add_handler(HTTPErrorProcessor())
    for handler in handlers:
        opener.add_handler(handler)
    return opener


def urlopen(url, data=None, timeout=None):
    return build_opener().open(url, data=data, timeout=timeout)
