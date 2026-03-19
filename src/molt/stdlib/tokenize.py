"""Minimal `tokenize` subset for Molt."""

from __future__ import annotations

from typing import Callable, Iterator

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TOKENIZE_RUNTIME_READY = _require_intrinsic(
    "molt_tokenize_runtime_ready"
)
_MOLT_TOKENIZE_SCAN = _require_intrinsic("molt_tokenize_scan")

ENDMARKER = 0
NAME = 1
NUMBER = 2
NEWLINE = 4
OP = 54
COMMENT = 64
NL = 65
ENCODING = 67


class TokenInfo:
    __slots__ = ("type", "string", "start", "end", "line")

    def __init__(
        self,
        tok_type: int,
        string: str,
        start: tuple[int, int],
        end: tuple[int, int],
        line: str,
    ) -> None:
        self.type = int(tok_type)
        self.string = str(string)
        self.start = start
        self.end = end
        self.line = line


def tokenize(
    readline: Callable[[], bytes],
    _runtime_ready_intrinsic=_MOLT_TOKENIZE_RUNTIME_READY,
    _tokenize_scan_intrinsic=_MOLT_TOKENIZE_SCAN,
) -> Iterator[TokenInfo]:
    _runtime_ready_intrinsic()
    chunks: list[bytes] = []
    while True:
        chunk = readline()
        if not chunk:
            break
        chunks.append(bytes(chunk))
    source = b"".join(chunks).decode("utf-8", errors="replace")
    yield TokenInfo(ENCODING, "utf-8", (0, 0), (0, 0), "")

    raw_tokens = _tokenize_scan_intrinsic(source)
    for tok in raw_tokens:
        yield TokenInfo(tok[0], tok[1], tok[2], tok[3], tok[4])


__all__ = [
    "COMMENT",
    "ENCODING",
    "ENDMARKER",
    "NAME",
    "NEWLINE",
    "NL",
    "NUMBER",
    "OP",
    "TokenInfo",
    "tokenize",
]

del _MOLT_TOKENIZE_RUNTIME_READY
del _MOLT_TOKENIZE_SCAN
