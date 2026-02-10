"""Intrinsic-backed http.cookiejar subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "CookieJar",
    "DefaultCookiePolicy",
]

_MOLT_COOKIEJAR_NEW = _require_intrinsic("molt_http_cookiejar_new", globals())
_MOLT_COOKIEJAR_LEN = _require_intrinsic("molt_http_cookiejar_len", globals())
_MOLT_COOKIEJAR_CLEAR = _require_intrinsic("molt_http_cookiejar_clear", globals())
_MOLT_COOKIEJAR_EXTRACT = _require_intrinsic("molt_http_cookiejar_extract", globals())
_MOLT_COOKIEJAR_HEADER_FOR_URL = _require_intrinsic(
    "molt_http_cookiejar_header_for_url", globals()
)


class DefaultCookiePolicy:
    """Placeholder policy object for compatibility with CookieJar(policy=...)."""


class CookieJar:
    def __init__(self, policy: object | None = None) -> None:
        self._policy = policy
        self._molt_cookiejar_handle = _MOLT_COOKIEJAR_NEW()

    def __len__(self) -> int:
        out = _MOLT_COOKIEJAR_LEN(self._molt_cookiejar_handle)
        if not isinstance(out, int):
            raise RuntimeError("http.cookiejar len intrinsic returned invalid value")
        return out

    def clear(self) -> None:
        _MOLT_COOKIEJAR_CLEAR(self._molt_cookiejar_handle)

    def extract_cookies(self, response: object, request: object) -> None:
        info = getattr(response, "info", None)
        if callable(info):
            headers = info()
        else:
            headers = getattr(response, "headers", {})
        full_url = getattr(request, "full_url", None)
        if full_url is None:
            raise RuntimeError("http.cookiejar request object is missing full_url")
        _MOLT_COOKIEJAR_EXTRACT(self._molt_cookiejar_handle, str(full_url), headers)

    def add_cookie_header(self, request: object) -> None:
        full_url = getattr(request, "full_url", None)
        if full_url is None:
            raise RuntimeError("http.cookiejar request object is missing full_url")
        header = _MOLT_COOKIEJAR_HEADER_FOR_URL(
            self._molt_cookiejar_handle, str(full_url)
        )
        if not header:
            return
        headers = getattr(request, "headers", None)
        if headers is None:
            raise RuntimeError("http.cookiejar request object is missing headers")
        cookie = headers.get("Cookie")
        if cookie:
            headers["Cookie"] = f"{cookie}; {header}"
        else:
            headers["Cookie"] = header
