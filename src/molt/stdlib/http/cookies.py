"""Intrinsic-backed subset of `http.cookies` for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["BaseCookie", "CookieError", "Morsel", "SimpleCookie"]

_MOLT_HTTP_COOKIES_PARSE = _require_intrinsic("molt_http_cookies_parse")
_MOLT_HTTP_COOKIES_RENDER_MORSEL = _require_intrinsic(
    "molt_http_cookies_render_morsel"
)

_MORSEL_ATTR_ORDER = ("expires", "httponly", "max-age", "path", "secure")
_MORSEL_ATTR_DEFAULTS = {
    "expires": "",
    "httponly": "",
    "max-age": "",
    "path": "",
    "secure": "",
}


def _normalize_morsel_attrs(attrs: object | None) -> set:
    if attrs is None:
        return set(_MORSEL_ATTR_ORDER)
    try:
        iterator = iter(attrs)
    except TypeError as exc:
        raise TypeError("Morsel attrs filter must be iterable") from exc
    out: set = set()
    for attr in iterator:
        out.add(str(attr).lower())
    return out


class CookieError(Exception):
    """Raised when cookie parsing/handling fails."""


class Morsel:
    __slots__ = ("key", "value", "coded_value", "_attrs")

    def __init__(self, key: str, value: object) -> None:
        self.key = str(key)
        self._attrs = dict(_MORSEL_ATTR_DEFAULTS)
        self._set_cookie_value(value)

    def _set_cookie_value(self, value: object) -> None:
        rendered = str(value)
        self.value = rendered
        self.coded_value = rendered

    def __getitem__(self, attr: str):
        attr_key = str(attr).lower()
        if attr_key not in self._attrs:
            raise KeyError(attr)
        return self._attrs[attr_key]

    def __setitem__(self, attr: str, value: object) -> None:
        attr_key = str(attr).lower()
        if attr_key not in self._attrs:
            raise KeyError(attr)
        self._attrs[attr_key] = value

    def items(self):
        return self._attrs.items()

    def OutputString(self, attrs: object | None = None) -> str:
        include = _normalize_morsel_attrs(attrs)
        path = self._attrs["path"] if "path" in include else ""
        secure = self._attrs["secure"] if "secure" in include else ""
        httponly = self._attrs["httponly"] if "httponly" in include else ""
        max_age = self._attrs["max-age"] if "max-age" in include else ""
        expires = self._attrs["expires"] if "expires" in include else ""
        out = _MOLT_HTTP_COOKIES_RENDER_MORSEL(
            self.key, self.coded_value, path, secure, httponly, max_age, expires
        )
        return str(out)

    def output(self, attrs: object | None = None, header: str = "Set-Cookie:") -> str:
        payload = self.OutputString(attrs=attrs)
        if not header:
            return payload
        return f"{header} {payload}"


class BaseCookie(dict):
    def __setitem__(self, key: str, value: object) -> None:
        cookie_key = str(key)
        existing = dict.get(self, cookie_key)
        if existing is None:
            dict.__setitem__(self, cookie_key, Morsel(cookie_key, value))
            return
        existing._set_cookie_value(value)

    def __getitem__(self, key: str) -> Morsel:
        return dict.__getitem__(self, str(key))

    def load(self, rawdata: str) -> None:
        if not isinstance(rawdata, str):
            raise TypeError("SimpleCookie.load() requires a str input in Molt")
        parsed = _MOLT_HTTP_COOKIES_PARSE(rawdata)
        if not isinstance(parsed, list):
            raise RuntimeError("http.cookies parse intrinsic returned invalid data")
        for entry in parsed:
            if not isinstance(entry, tuple) or len(entry) != 2:
                raise RuntimeError(
                    "http.cookies parse intrinsic returned invalid cookie pair"
                )
            name = str(entry[0])
            value = entry[1]
            self[name] = value

    def output(
        self,
        attrs: object | None = None,
        header: str = "Set-Cookie:",
        sep: str = "\r\n",
    ) -> str:
        parts: list = []
        for key in sorted(dict.keys(self)):
            morsel = dict.__getitem__(self, key)
            parts.append(morsel.output(attrs=attrs, header=header))
        return sep.join(parts)


class SimpleCookie(BaseCookie):
    pass

globals().pop("_require_intrinsic", None)
