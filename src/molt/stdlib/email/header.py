"""Public API surface shim for ``email.header``."""

from __future__ import annotations

import re

from _intrinsics import require_intrinsic as _require_intrinsic

from email.charset import Charset

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_EMAIL_HEADER_ENCODE_WORD = _require_intrinsic(
    "molt_email_header_encode_word", globals()
)

NL = "\n"
SPACE = " "
BSPACE = b" "
SPACE8 = "        "
EMPTYSTRING = ""
MAXLINELEN = 78
FWS = r"[ \t]+"

USASCII = Charset("us-ascii")
UTF8 = Charset("utf-8")

ecre = re.compile(r"=\?[^?]+\?[bBqQ]\?[^?]*\?=")
fcre = re.compile(r"^[^:\s][^:]*:")


class Header:
    def __init__(
        self,
        s: str | None = None,
        charset: Charset | str | None = None,
        maxlinelen: int = MAXLINELEN,
        header_name: str | None = None,
        continuation_ws: str = " ",
        errors: str = "strict",
    ):
        del header_name, continuation_ws, errors
        if isinstance(charset, str):
            charset = Charset(charset)
        self._charset = charset
        self._maxlinelen = int(maxlinelen)
        self._chunks: list[str] = []
        if s is not None:
            self.append(s, charset=charset)

    def append(
        self, s: str, charset: Charset | str | None = None, errors: str = "strict"
    ):
        del errors
        if isinstance(charset, str):
            charset = Charset(charset)
        text = s if isinstance(s, str) else str(s)
        self._chunks.append(text)

    def encode(
        self,
        splitchars: str = ";, \t",
        maxlinelen: int | None = None,
        linesep: str = "\n",
    ):
        del splitchars
        line_len = self._maxlinelen if maxlinelen is None else int(maxlinelen)
        text = EMPTYSTRING.join(self._chunks)
        if line_len <= 0:
            return text
        if len(text) <= line_len:
            return text
        parts = [text[i : i + line_len] for i in range(0, len(text), line_len)]
        return linesep.join(parts)

    def __str__(self):
        return EMPTYSTRING.join(self._chunks)


def decode_header(header):
    if isinstance(header, Header):
        header = str(header)
    if isinstance(header, bytes):
        return [(header, None)]
    return [(str(header).encode("utf-8", "replace"), "utf-8")]


def make_header(
    decoded_seq,
    maxlinelen: int = MAXLINELEN,
    header_name: str | None = None,
    continuation_ws: str = " ",
):
    del header_name, continuation_ws
    h = Header(maxlinelen=maxlinelen)
    for item, charset in decoded_seq:
        if isinstance(item, bytes):
            if charset is None:
                text = item.decode("ascii", "replace")
            else:
                text = item.decode(charset, "replace")
        else:
            text = str(item)
        h.append(text, charset=charset)
    return h
