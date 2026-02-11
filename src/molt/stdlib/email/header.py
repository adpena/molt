"""Intrinsic-backed email.header subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_EMAIL_HEADER_ENCODE_WORD = _require_intrinsic(
    "molt_email_header_encode_word", globals()
)


def _encode_word(text: str, charset: str | None) -> str:
    out = _MOLT_EMAIL_HEADER_ENCODE_WORD(text, charset)
    if not isinstance(out, str):
        raise RuntimeError("email.header encoding intrinsic returned invalid value")
    return out


class Header:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement RFC 2047 word-splitting/folding parity and mixed charset chunks.
    def __init__(
        self,
        s: str | bytes | None = None,
        charset: str | None = None,
        maxlinelen: int | None = None,
        header_name: str | None = None,
        continuation_ws: str = " ",
        errors: str = "strict",
    ) -> None:
        self._chunks: list[tuple[str, str | None]] = []
        self.maxlinelen = 78 if maxlinelen is None else int(maxlinelen)
        self.header_name = header_name
        self.continuation_ws = continuation_ws
        self.errors = errors
        self.default_charset = charset
        if s is not None:
            self.append(s, charset=charset, errors=errors)

    def append(
        self,
        s: str | bytes,
        charset: str | None = None,
        errors: str = "strict",
    ) -> None:
        if isinstance(s, bytes):
            active_charset = charset or self.default_charset or "ascii"
            text = s.decode(active_charset, errors=errors)
            self._chunks.append((text, active_charset))
            return
        self._chunks.append((str(s), charset or self.default_charset))

    def encode(
        self,
        splitchars: str = ";, \t",
        maxlinelen: int | None = None,
        linesep: str = "\n",
    ) -> str:
        _ = splitchars
        _ = maxlinelen
        _ = linesep
        parts: list[str] = []
        for text, charset in self._chunks:
            if charset is None:
                parts.append(_encode_word(text, None))
                continue
            lower = charset.lower()
            if lower in {"ascii", "us-ascii"}:
                parts.append(text)
                continue
            parts.append(_encode_word(text, lower))
        return " ".join(parts)

    def __str__(self) -> str:
        return self.encode()

    def __repr__(self) -> str:
        return f"Header({str(self)!r})"


__all__ = ["Header"]
