"""Intrinsic-backed shlex support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["shlex", "split", "quote", "join"]

_MOLT_SHLEX_QUOTE = _require_intrinsic("molt_shlex_quote", globals())
_MOLT_SHLEX_SPLIT_EX = _require_intrinsic("molt_shlex_split_ex", globals())
_MOLT_SHLEX_JOIN = _require_intrinsic("molt_shlex_join", globals())


def quote(s: str) -> str:
    if not isinstance(s, str):
        raise TypeError("shlex.quote argument must be str")
    out = _MOLT_SHLEX_QUOTE(s)
    if not isinstance(out, str):
        raise RuntimeError("shlex.quote intrinsic returned invalid value")
    return out


def split(s, comments: bool = False, posix: bool = True) -> list[str]:
    if s is None:
        raise ValueError("s argument must not be None")
    if isinstance(s, str):
        source = s
    else:
        reader = getattr(s, "read")
        source = reader()
        if not isinstance(source, str):
            raise TypeError("shlex.split source reader must return str")
    parts = _MOLT_SHLEX_SPLIT_EX(
        source,
        " \t\r\n",
        bool(posix),
        bool(comments),
        True,
        "#",
        "",
    )
    if not isinstance(parts, list):
        raise RuntimeError("shlex.split intrinsic returned invalid value")
    for item in parts:
        if not isinstance(item, str):
            raise RuntimeError("shlex.split intrinsic returned invalid value")
    return parts


def join(split_command) -> str:
    out = _MOLT_SHLEX_JOIN(split_command)
    if not isinstance(out, str):
        raise RuntimeError("shlex.join intrinsic returned invalid value")
    return out


class shlex:
    def __init__(
        self, instream=None, infile=None, posix: bool = False, punctuation_chars=False
    ):
        if instream is None:
            source = ""
        elif isinstance(instream, str):
            source = instream
        else:
            reader = getattr(instream, "read")
            source = reader()
            if not isinstance(source, str):
                raise TypeError("instream reader must return str")
        self.instream = instream
        self.infile = infile
        self.posix = bool(posix)
        self.whitespace = " \t\r\n"
        self.whitespace_split = False
        self.commenters = "#"
        if punctuation_chars is True:
            self.punctuation_chars = "();<>|&"
        elif punctuation_chars is False:
            self.punctuation_chars = ""
        elif isinstance(punctuation_chars, str):
            self.punctuation_chars = punctuation_chars
        else:
            raise TypeError("punctuation_chars must be bool or str")
        self._source = source
        self._tokens: list[str] | None = None
        self._index = 0
        self._pushback: list[str] = []

    def _lex(self) -> list[str]:
        parts = _MOLT_SHLEX_SPLIT_EX(
            self._source,
            self.whitespace,
            self.posix,
            bool(self.commenters),
            bool(self.whitespace_split),
            self.commenters,
            self.punctuation_chars,
        )
        if not isinstance(parts, list):
            raise RuntimeError("shlex lexer intrinsic returned invalid value")
        for item in parts:
            if not isinstance(item, str):
                raise RuntimeError("shlex lexer intrinsic returned invalid value")
        return parts

    def _ensure_tokens(self) -> None:
        if self._tokens is None:
            self._tokens = self._lex()

    def push_token(self, tok: str) -> None:
        if not isinstance(tok, str):
            raise TypeError("token must be str")
        self._pushback.append(tok)

    def read_token(self) -> str:
        self._ensure_tokens()
        assert self._tokens is not None
        if self._index >= len(self._tokens):
            return ""
        token = self._tokens[self._index]
        self._index += 1
        return token

    def get_token(self) -> str:
        if self._pushback:
            return self._pushback.pop()
        return self.read_token()

    def __iter__(self):
        return self

    def __next__(self) -> str:
        token = self.get_token()
        if token == "":
            raise StopIteration
        return token
