"""Minimal urllib.parse support for Molt."""

from __future__ import annotations

from typing import Iterable, Iterator

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

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): bring
# urllib.parse parity to CPython (RFC-compliant parsing, params, IPv6, and encoding).


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


_ALWAYS_SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-~"


def quote(string: str, safe: str = "/") -> str:
    safe_set = set(_ALWAYS_SAFE + safe)
    out: list[str] = []
    for ch in string:
        if ch in safe_set:
            out.append(ch)
            continue
        for byte in ch.encode("utf-8"):
            out.append(f"%{byte:02X}")
    return "".join(out)


def quote_plus(string: str, safe: str = "") -> str:
    return quote(string, safe).replace("%20", "+")


def unquote(string: str) -> str:
    if "%" not in string:
        return string
    buf = bytearray()
    idx = 0
    length = len(string)
    while idx < length:
        ch = string[idx]
        if ch == "%" and idx + 2 < length:
            hex_pair = string[idx + 1 : idx + 3]
            try:
                buf.append(int(hex_pair, 16))
                idx += 3
                continue
            except ValueError:
                pass
        buf.extend(ch.encode("utf-8"))
        idx += 1
    return buf.decode("utf-8", errors="replace")


def unquote_plus(string: str) -> str:
    return unquote(string.replace("+", " "))


def urlencode(query: Iterable, doseq: bool = False, safe: str = "") -> str:
    if hasattr(query, "items"):
        items = list(query.items())  # type: ignore[attr-defined]
    else:
        items = list(query)
    pairs: list[str] = []
    for key, value in items:
        if doseq and isinstance(value, (list, tuple)):
            for entry in value:
                pairs.append(
                    quote_plus(str(key), safe) + "=" + quote_plus(str(entry), safe)
                )
        else:
            pairs.append(
                quote_plus(str(key), safe) + "=" + quote_plus(str(value), safe)
            )
    return "&".join(pairs)


def parse_qsl(
    qs: str,
    keep_blank_values: bool = False,
    strict_parsing: bool = False,
) -> list[tuple[str, str]]:
    pairs: list[tuple[str, str]] = []
    if not qs:
        return pairs
    for chunk in qs.split("&"):
        if not chunk and not keep_blank_values:
            continue
        if "=" in chunk:
            key, value = chunk.split("=", 1)
        else:
            if strict_parsing:
                raise ValueError("bad query field")
            key, value = chunk, ""
        if value or keep_blank_values:
            pairs.append((unquote_plus(key), unquote_plus(value)))
    return pairs


def parse_qs(
    qs: str,
    keep_blank_values: bool = False,
    strict_parsing: bool = False,
) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {}
    for key, value in parse_qsl(qs, keep_blank_values, strict_parsing):
        out.setdefault(key, []).append(value)
    return out


def _split_scheme(url: str, default: str) -> tuple[str, str]:
    for idx, ch in enumerate(url):
        if ch == ":":
            scheme = url[:idx]
            rest = url[idx + 1 :]
            if (
                scheme
                and _is_alpha(scheme[0])
                and all(_is_alnum(c) or c in "+-." for c in scheme)
            ):
                return scheme.lower(), rest
            break
        if ch in "/?#":
            break
    return default, url


def _is_alpha(ch: str) -> bool:
    return ("a" <= ch <= "z") or ("A" <= ch <= "Z")


def _is_alnum(ch: str) -> bool:
    return _is_alpha(ch) or ("0" <= ch <= "9")


def _split_netloc(rest: str) -> tuple[str, str]:
    for idx, ch in enumerate(rest):
        if ch in "/?#":
            return rest[:idx], rest[idx:]
    return rest, ""


def _split_query_fragment(rest: str, allow_fragments: bool) -> tuple[str, str, str]:
    fragment = ""
    if allow_fragments and "#" in rest:
        rest, fragment = rest.split("#", 1)
    query = ""
    if "?" in rest:
        rest, query = rest.split("?", 1)
    return rest, query, fragment


def urlsplit(
    url: str,
    scheme: str = "",
    allow_fragments: bool = True,
) -> SplitResult:
    parsed_scheme, rest = _split_scheme(url, scheme)
    netloc = ""
    if rest.startswith("//"):
        netloc, rest = _split_netloc(rest[2:])
    path, query, fragment = _split_query_fragment(rest, allow_fragments)
    return SplitResult(parsed_scheme, netloc, path, query, fragment)


def urlparse(
    url: str,
    scheme: str = "",
    allow_fragments: bool = True,
) -> ParseResult:
    split = urlsplit(url, scheme, allow_fragments)
    path = split.path
    params = ""
    if ";" in path:
        path, params = path.split(";", 1)
    return ParseResult(
        split.scheme, split.netloc, path, params, split.query, split.fragment
    )


def _unsplit(
    scheme: str,
    netloc: str,
    path: str,
    query: str,
    fragment: str,
) -> str:
    out = ""
    if scheme:
        out += scheme + ":"
    if netloc:
        out += "//" + netloc
    out += path
    if query:
        out += "?" + query
    if fragment:
        out += "#" + fragment
    return out


def urlunsplit(parts: Iterable[str]) -> str:
    if hasattr(parts, "scheme"):
        scheme = getattr(parts, "scheme")
        netloc = getattr(parts, "netloc")
        path = getattr(parts, "path")
        query = getattr(parts, "query")
        fragment = getattr(parts, "fragment")
    else:
        scheme, netloc, path, query, fragment = parts
    return _unsplit(scheme, netloc, path, query, fragment)


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
    if params:
        path = path + ";" + params
    return _unsplit(scheme, netloc, path, query, fragment)


def urldefrag(url: str) -> DefragResult:
    if "#" in url:
        base, frag = url.split("#", 1)
        return DefragResult(base, frag)
    return DefragResult(url, "")


def urljoin(base: str, url: str) -> str:
    if not base:
        return url
    target = urlsplit(url)
    if target.scheme:
        return url
    base_parts = urlparse(base)
    if url.startswith("//"):
        return base_parts.scheme + ":" + url
    if target.netloc:
        return urlunparse(
            (
                base_parts.scheme,
                target.netloc,
                target.path,
                "",
                target.query,
                target.fragment,
            )
        )
    path = target.path
    if not path:
        path = base_parts.path
    elif not path.startswith("/"):
        base_path = base_parts.path
        if "/" in base_path:
            base_dir = base_path.rsplit("/", 1)[0]
        else:
            base_dir = ""
        if base_dir:
            path = base_dir + "/" + path
        else:
            path = "/" + path
    return urlunparse(
        (base_parts.scheme, base_parts.netloc, path, "", target.query, target.fragment)
    )
