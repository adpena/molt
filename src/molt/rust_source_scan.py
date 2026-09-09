"""Offset-preserving Rust lexical projections for source-analysis tools.

This leaf has no compiler, CLI, generator, or release-tool dependencies. It
recognizes comments and literals, not Rust syntax; malformed unterminated
comments/strings extend to EOF so their contents cannot become apparent code.
"""

from __future__ import annotations

from collections.abc import Iterator
import re
from typing import Literal, NamedTuple

# Python's Unicode \w is exactly the isalnum-or-underscore token boundary
# used here. Search only potential non-code starts, not every ordinary code
# character; quote alternatives still admit a literal after an invalid prefix.
_NON_CODE_START = re.compile(
    r"""//|/\*|(?<!\w)(?:br|cr|r)(?P<hashes>\#*)"|(?<!\w)(?:[bc]"|b')|["']"""
)
_COMMENT_DELIMITER = re.compile(r"/\*|\*/")
_STRING_DELIMITER = re.compile(r'["\\]')
_CHAR_LITERAL = re.compile(
    r"""'(?:[^'\\\r\n]|\\(?:[nrt\\0'"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}))'"""
)
_NON_NEWLINE = re.compile(r"[^\r\n]+")


class _NonCodeSpan(NamedTuple):
    kind: Literal["comment", "literal"]
    start: int
    end: int


def _non_code_spans(text: str) -> Iterator[_NonCodeSpan]:
    index = 0
    length = len(text)
    while match := _NON_CODE_START.search(text, index):
        start = match.start()
        token = match.group()
        if token == "//":
            end = text.find("\n", match.end())
            index = length if end < 0 else end
            yield _NonCodeSpan("comment", start, index)
            continue
        if token == "/*":
            depth = 1
            index = match.end()
            while depth:
                delimiter = _COMMENT_DELIMITER.search(text, index)
                if delimiter is None:
                    index = length
                    break
                depth += 1 if delimiter.group() == "/*" else -1
                index = delimiter.end()
            yield _NonCodeSpan("comment", start, index)
            continue
        hashes = match.group("hashes")
        if hashes is not None:
            terminator = '"' + hashes
            end = text.find(terminator, match.end())
            index = length if end < 0 else end + len(terminator)
            yield _NonCodeSpan("literal", start, index)
            continue
        if token.endswith('"'):
            index = match.end()
            while True:
                delimiter = _STRING_DELIMITER.search(text, index)
                if delimiter is None:
                    index = length
                    break
                if delimiter.group() == '"':
                    index = delimiter.end()
                    break
                # Backslash quotes exactly the following character, including
                # a newline. Advancing past it also handles escaped backslashes.
                index = min(length, delimiter.end() + 1)
            yield _NonCodeSpan("literal", start, index)
            continue
        char = _CHAR_LITERAL.match(text, match.end() - 1)
        if char is not None:
            index = char.end()
            yield _NonCodeSpan("literal", start, index)
        else:
            # The apostrophe belongs to a lifetime or malformed char, not a
            # literal. Prefix b was already proved not to begin a valid char.
            index = match.end()


class RustSourceProjection(NamedTuple):
    masked_code: str
    comments: list[tuple[int, str]]


def _project_rust_source(
    text: str, *, include_mask: bool, include_comments: bool
) -> RustSourceProjection:
    output: list[str] = []
    comments: list[tuple[int, str]] = []
    cursor = 0
    line = 1
    for span in _non_code_spans(text):
        if include_mask:
            output.append(text[cursor : span.start])
            output.append(
                _NON_NEWLINE.sub(
                    lambda run: " " * len(run[0]), text[span.start : span.end]
                )
            )
        if include_comments:
            line += text.count("\n", cursor, span.start)
            if span.kind == "comment":
                comments.append((line, text[span.start : span.end]))
            line += text.count("\n", span.start, span.end)
        cursor = span.end
    if include_mask:
        output.append(text[cursor:])
    return RustSourceProjection("".join(output) if include_mask else text, comments)


def project_rust_source(text: str) -> RustSourceProjection:
    """Produce code and line-numbered comments in one offset-preserving scan."""
    return _project_rust_source(text, include_mask=True, include_comments=True)


def mask_rust_comments_and_strings(text: str) -> str:
    """Blank comments and literals; preserve all offsets and CR/LF characters."""
    return _project_rust_source(
        text, include_mask=True, include_comments=False
    ).masked_code


def rust_comment_segments(text: str) -> list[tuple[int, str]]:
    """Return one-based source lines and original comments from the same scan."""
    return _project_rust_source(
        text, include_mask=False, include_comments=True
    ).comments
