"""Intrinsic-backed urllib.request subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from urllib.error import URLError
from urllib.response import addinfourl

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

_MOLT_REQUEST_INIT = _require_intrinsic("molt_urllib_request_request_init")
_MOLT_OPENER_INIT = _require_intrinsic("molt_urllib_request_opener_init")
_MOLT_OPENER_ADD_HANDLER = _require_intrinsic(
    "molt_urllib_request_add_handler",
)
_MOLT_OPENER_OPEN = _require_intrinsic("molt_urllib_request_open")
_MOLT_PROCESS_HTTP_ERROR = _require_intrinsic(
    "molt_urllib_request_process_http_error",
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


def _addinfourl_from_handle(handle):
    ctor = getattr(addinfourl, "_from_handle", None)
    if callable(ctor):
        return ctor(int(handle))
    raise RuntimeError("urllib.response.addinfourl is missing intrinsic handle bridge")


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


class _HTTPErrorResponseAdapter:
    __slots__ = ("_opener", "_bound")

    def __init__(self, opener, bound):
        self._opener = opener
        self._bound = bound

    def __call__(self, request, response):
        adapted = self._opener._wrap_response(request, response)
        return self._bound(request, adapted)


class OpenerDirector:
    def __init__(self):
        _MOLT_OPENER_INIT(self)
        self._molt_allow_data_fallback = False
        self._molt_raise_on_none = False

    def add_handler(self, handler):
        if isinstance(handler, HTTPErrorProcessor):
            marker = "_molt_http_response_adapter_wrapped"
            if not bool(getattr(handler, marker, False)):
                base_http = getattr(HTTPErrorProcessor, "http_response", None)
                base_https = getattr(HTTPErrorProcessor, "https_response", None)
                for method_name, base_method in (
                    ("http_response", base_http),
                    ("https_response", base_https),
                ):
                    class_method = getattr(type(handler), method_name, None)
                    if class_method is base_method:
                        continue
                    bound = getattr(handler, method_name, None)
                    if not callable(bound):
                        continue
                    setattr(
                        handler,
                        method_name,
                        _HTTPErrorResponseAdapter(self, bound),
                    )
                setattr(handler, marker, True)
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
                return _addinfourl_from_handle(handle)
            if isinstance(handle, int):
                return _addinfourl_from_handle(handle)
            if isinstance(handle, float) and handle.is_integer():
                return _addinfourl_from_handle(int(handle))
        if isinstance(out, int):
            return _addinfourl_from_handle(out)
        if isinstance(out, float) and out.is_integer():
            return _addinfourl_from_handle(int(out))
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
    has_custom_http_error_processor = any(
        isinstance(handler, HTTPErrorProcessor) for handler in handlers
    )
    if not has_custom_http_error_processor:
        opener.add_handler(HTTPErrorProcessor())
    for handler in handlers:
        opener.add_handler(handler)
    return opener


def urlopen(url, data=None, timeout=None):
    return build_opener().open(url, data=data, timeout=timeout)
