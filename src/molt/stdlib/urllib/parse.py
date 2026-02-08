"""Intrinsic-backed urllib.parse subset for Molt."""

from __future__ import annotations

from typing import Iterable, Iterator

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "DefragResult",
    "ParseResult",
    "SplitResult",
    "parse_qs",
    "parse_qsl",
    "quote",
    "quote_plus",
    "unquote",
    "unquote_plus",
    "urlencode",
    "urldefrag",
    "urljoin",
    "urlparse",
    "urlsplit",
    "urlunparse",
    "urlunsplit",
]

_MOLT_URLLIB_QUOTE = _require_intrinsic("molt_urllib_quote", globals())
_MOLT_URLLIB_QUOTE_PLUS = _require_intrinsic("molt_urllib_quote_plus", globals())
_MOLT_URLLIB_UNQUOTE = _require_intrinsic("molt_urllib_unquote", globals())
_MOLT_URLLIB_UNQUOTE_PLUS = _require_intrinsic("molt_urllib_unquote_plus", globals())
_MOLT_URLLIB_PARSE_QSL = _require_intrinsic("molt_urllib_parse_qsl", globals())
_MOLT_URLLIB_PARSE_QS = _require_intrinsic("molt_urllib_parse_qs", globals())
_MOLT_URLLIB_URLENCODE = _require_intrinsic("molt_urllib_urlencode", globals())
_MOLT_URLLIB_URLSPLIT = _require_intrinsic("molt_urllib_urlsplit", globals())
_MOLT_URLLIB_URLPARSE = _require_intrinsic("molt_urllib_urlparse", globals())
_MOLT_URLLIB_URLUNSPLIT = _require_intrinsic("molt_urllib_urlunsplit", globals())
_MOLT_URLLIB_URLUNPARSE = _require_intrinsic("molt_urllib_urlunparse", globals())
_MOLT_URLLIB_URLDEFRAG = _require_intrinsic("molt_urllib_urldefrag", globals())
_MOLT_URLLIB_URLJOIN = _require_intrinsic("molt_urllib_urljoin", globals())


class _BaseResult:
    _fields: tuple[str, ...] = ()

    def __iter__(self) -> Iterator[str]:
        fields = self._fields or tuple(self.__dict__.keys())
        for name in fields:
            yield getattr(self, name)

    def __len__(self) -> int:
        fields = self._fields or tuple(self.__dict__.keys())
        return len(fields)

    def __getitem__(self, idx: int) -> str:
        return tuple(self)[idx]

    def _repr_items(self) -> str:
        parts = []
        fields = self._fields or tuple(self.__dict__.keys())
        for name in fields:
            parts.append(name + "=" + repr(getattr(self, name)))
        return ", ".join(parts)

    def __repr__(self) -> str:
        return self.__class__.__name__ + "(" + self._repr_items() + ")"


class ParseResult(_BaseResult):
    _fields = ("scheme", "netloc", "path", "params", "query", "fragment")

    def __init__(
        self,
        scheme: str,
        netloc: str,
        path: str,
        params: str,
        query: str,
        fragment: str,
    ) -> None:
        self._fields = ("scheme", "netloc", "path", "params", "query", "fragment")
        self.scheme = scheme
        self.netloc = netloc
        self.path = path
        self.params = params
        self.query = query
        self.fragment = fragment

    def geturl(self) -> str:
        return urlunparse(self)


class SplitResult(_BaseResult):
    _fields = ("scheme", "netloc", "path", "query", "fragment")

    def __init__(
        self,
        scheme: str,
        netloc: str,
        path: str,
        query: str,
        fragment: str,
    ) -> None:
        self._fields = ("scheme", "netloc", "path", "query", "fragment")
        self.scheme = scheme
        self.netloc = netloc
        self.path = path
        self.query = query
        self.fragment = fragment

    def geturl(self) -> str:
        return urlunsplit(self)


class DefragResult(_BaseResult):
    _fields = ("url", "fragment")

    def __init__(self, url: str, fragment: str) -> None:
        self._fields = ("url", "fragment")
        self.url = url
        self.fragment = fragment


def quote(string: str, safe: str = "/") -> str:
    out = _MOLT_URLLIB_QUOTE(string, safe)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.quote intrinsic returned invalid value")
    return out


def quote_plus(string: str, safe: str = "") -> str:
    out = _MOLT_URLLIB_QUOTE_PLUS(string, safe)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.quote_plus intrinsic returned invalid value")
    return out


def unquote(string: str) -> str:
    out = _MOLT_URLLIB_UNQUOTE(string)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.unquote intrinsic returned invalid value")
    return out


def unquote_plus(string: str) -> str:
    out = _MOLT_URLLIB_UNQUOTE_PLUS(string)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.unquote_plus intrinsic returned invalid value")
    return out


def parse_qsl(
    qs: str,
    keep_blank_values: bool = False,
    strict_parsing: bool = False,
) -> list[tuple[str, str]]:
    out = _MOLT_URLLIB_PARSE_QSL(qs, bool(keep_blank_values), bool(strict_parsing))
    if not isinstance(out, list):
        raise RuntimeError("urllib.parse.parse_qsl intrinsic returned invalid value")
    if not all(
        isinstance(item, tuple)
        and len(item) == 2
        and isinstance(item[0], str)
        and isinstance(item[1], str)
        for item in out
    ):
        raise RuntimeError("urllib.parse.parse_qsl intrinsic returned invalid value")
    return list(out)


def parse_qs(
    qs: str,
    keep_blank_values: bool = False,
    strict_parsing: bool = False,
) -> dict[str, list[str]]:
    out = _MOLT_URLLIB_PARSE_QS(qs, bool(keep_blank_values), bool(strict_parsing))
    if not isinstance(out, dict):
        raise RuntimeError("urllib.parse.parse_qs intrinsic returned invalid value")
    for key, value in out.items():
        if not isinstance(key, str) or not isinstance(value, list):
            raise RuntimeError("urllib.parse.parse_qs intrinsic returned invalid value")
        if not all(isinstance(entry, str) for entry in value):
            raise RuntimeError("urllib.parse.parse_qs intrinsic returned invalid value")
    return dict(out)


def urlsplit(
    url: str,
    scheme: str = "",
    allow_fragments: bool = True,
) -> SplitResult:
    out = _MOLT_URLLIB_URLSPLIT(url, scheme, bool(allow_fragments))
    if (
        not isinstance(out, tuple)
        or len(out) != 5
        or not all(isinstance(item, str) for item in out)
    ):
        raise RuntimeError("urllib.parse.urlsplit intrinsic returned invalid value")
    return SplitResult(*out)


def urlparse(
    url: str,
    scheme: str = "",
    allow_fragments: bool = True,
) -> ParseResult:
    out = _MOLT_URLLIB_URLPARSE(url, scheme, bool(allow_fragments))
    if (
        not isinstance(out, tuple)
        or len(out) != 6
        or not all(isinstance(item, str) for item in out)
    ):
        raise RuntimeError("urllib.parse.urlparse intrinsic returned invalid value")
    return ParseResult(*out)


def urlunsplit(parts: Iterable[str]) -> str:
    if hasattr(parts, "scheme"):
        scheme = getattr(parts, "scheme")
        netloc = getattr(parts, "netloc")
        path = getattr(parts, "path")
        query = getattr(parts, "query")
        fragment = getattr(parts, "fragment")
    else:
        scheme, netloc, path, query, fragment = parts
    out = _MOLT_URLLIB_URLUNSPLIT(scheme, netloc, path, query, fragment)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.urlunsplit intrinsic returned invalid value")
    return out


def urlunparse(parts: Iterable[str]) -> str:
    if hasattr(parts, "scheme"):
        scheme = getattr(parts, "scheme")
        netloc = getattr(parts, "netloc")
        path = getattr(parts, "path")
        params = getattr(parts, "params", "")
        query = getattr(parts, "query")
        fragment = getattr(parts, "fragment")
    else:
        scheme, netloc, path, params, query, fragment = parts
    out = _MOLT_URLLIB_URLUNPARSE(scheme, netloc, path, params, query, fragment)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.urlunparse intrinsic returned invalid value")
    return out


def urldefrag(url: str) -> DefragResult:
    out = _MOLT_URLLIB_URLDEFRAG(url)
    if (
        not isinstance(out, tuple)
        or len(out) != 2
        or not all(isinstance(item, str) for item in out)
    ):
        raise RuntimeError("urllib.parse.urldefrag intrinsic returned invalid value")
    return DefragResult(*out)


def urljoin(base: str, url: str) -> str:
    out = _MOLT_URLLIB_URLJOIN(base, url)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.urljoin intrinsic returned invalid value")
    return out


def urlencode(query: Iterable, doseq: bool = False, safe: str = "") -> str:
    if hasattr(query, "items"):
        items = list(query.items())  # type: ignore[attr-defined]
    else:
        items = list(query)
    out = _MOLT_URLLIB_URLENCODE(items, bool(doseq), safe)
    if not isinstance(out, str):
        raise RuntimeError("urllib.parse.urlencode intrinsic returned invalid value")
    return out
