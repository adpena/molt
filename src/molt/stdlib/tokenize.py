"""Minimal `tokenize` subset for Molt."""

from __future__ import annotations

from typing import Callable, Iterator

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TOKENIZE_RUNTIME_READY = _require_intrinsic(
    "molt_tokenize_runtime_ready", globals()
)

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


def _is_name_start(ch: str) -> bool:
    return ch == "_" or ("a" <= ch <= "z") or ("A" <= ch <= "Z")


def _is_name_char(ch: str) -> bool:
    return _is_name_start(ch) or ("0" <= ch <= "9")


def tokenize(readline: Callable[[], bytes]) -> Iterator[TokenInfo]:
    _MOLT_TOKENIZE_RUNTIME_READY()
    chunks: list[bytes] = []
    while True:
        chunk = readline()
        if not chunk:
            break
        chunks.append(bytes(chunk))
    source = b"".join(chunks).decode("utf-8", errors="replace")
    yield TokenInfo(ENCODING, "utf-8", (0, 0), (0, 0), "")

    lines = source.splitlines(True)
    line_no = 1
    for line in lines:
        col = 0
        if line.lstrip().startswith("#"):
            comment = line.strip()
            yield TokenInfo(
                COMMENT, comment, (line_no, 0), (line_no, len(comment)), line
            )
            if line.endswith("\n"):
                yield TokenInfo(
                    NL, "\n", (line_no, len(line) - 1), (line_no, len(line)), line
                )
            line_no += 1
            continue

        line_len = len(line)
        while col < line_len:
            ch = line[col]
            if ch in " \t\r\n":
                col += 1
                continue
            if ch == "#":
                comment = line[col:].rstrip("\r\n")
                yield TokenInfo(
                    COMMENT,
                    comment,
                    (line_no, col),
                    (line_no, col + len(comment)),
                    line,
                )
                break
            if _is_name_start(ch):
                start = col
                col += 1
                while col < line_len and _is_name_char(line[col]):
                    col += 1
                text = line[start:col]
                yield TokenInfo(NAME, text, (line_no, start), (line_no, col), line)
                continue
            if "0" <= ch <= "9":
                start = col
                col += 1
                while col < line_len and ("0" <= line[col] <= "9"):
                    col += 1
                text = line[start:col]
                yield TokenInfo(NUMBER, text, (line_no, start), (line_no, col), line)
                continue
            yield TokenInfo(OP, ch, (line_no, col), (line_no, col + 1), line)
            col += 1

        if line.endswith("\n"):
            tok_type = (
                NEWLINE if line.strip() and not line.lstrip().startswith("#") else NL
            )
            yield TokenInfo(
                tok_type, "\n", (line_no, len(line) - 1), (line_no, len(line)), line
            )
        line_no += 1

    yield TokenInfo(ENDMARKER, "", (line_no, 0), (line_no, 0), "")


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
