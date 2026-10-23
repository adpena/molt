"""Canonical parser for compiler and source-generator Make depfiles."""

from __future__ import annotations

from pathlib import Path
from dataclasses import dataclass
import os


@dataclass(frozen=True, slots=True)
class DependencyPath:
    """The compiler spelling and its selected regular-file content coordinate."""

    lexical: Path
    resolved: Path


def _rule_separator(text: str) -> int | None:
    escaped = False
    token_start = 0
    for index, character in enumerate(text):
        if escaped:
            escaped = False
            continue
        if character == "\\":
            escaped = True
            continue
        if character.isspace():
            token_start = index + 1
            continue
        if character != ":":
            continue
        # A Windows drive prefix is part of the path, not the Make rule
        # separator. Both clang and Cython can emit forward- or backslash paths.
        if (
            index == token_start + 1
            and text[token_start].isalpha()
            and index + 1 < len(text)
            and text[index + 1] in {"/", "\\"}
        ):
            continue
        return index
    return None


def parse_make_depfile(
    depfile: Path,
    *,
    cwd: Path,
    producer: str,
) -> tuple[tuple[DependencyPath, ...] | None, str | None]:
    """Parse one Make depfile without a shell and validate every dependency.

    The parser deliberately accepts only the single-rule shape emitted by the
    compiler/Cython commands Molt owns. It retains each lexical spelling and
    resolved content coordinate, deduplicating lexical paths in declaration
    order. Content-closure consumers explicitly project resolved coordinates;
    namespace consumers preserve aliases without walking ambient directories.
    Missing producer-declared regular files fail closed.
    """
    try:
        text = depfile.read_text(encoding="utf-8", errors="surrogateescape")
    except OSError as exc:
        return None, f"{producer} dependency file is unreadable: {exc}"
    text = text.replace("\\\r\n", "").replace("\\\n", "")
    separator = _rule_separator(text)
    if separator is None:
        return None, f"{producer} dependency file has no target: {depfile}"
    body = text[separator + 1 :]
    tokens: list[str] = []
    current: list[str] = []
    index = 0
    while index < len(body):
        character = body[index]
        if character.isspace():
            if current:
                tokens.append("".join(current))
                current.clear()
            index += 1
            continue
        if character == "\\" and index + 1 < len(body):
            following = body[index + 1]
            if following in {" ", "#", ":", "\\"}:
                current.append(following)
                index += 2
                continue
        current.append(character)
        index += 1
    if current:
        tokens.append("".join(current))
    if not tokens:
        return None, f"{producer} dependency file is empty: {depfile}"

    paths: list[DependencyPath] = []
    seen: set[Path] = set()
    for token in tokens:
        path = Path(token).expanduser()
        if not path.is_absolute():
            path = cwd / path
        lexical = Path(os.path.abspath(path))
        if lexical in seen:
            continue
        seen.add(lexical)
        resolved = lexical.resolve()
        if not resolved.is_file():
            return None, f"{producer}-declared dependency is missing: {resolved}"
        paths.append(DependencyPath(lexical, resolved))
    return tuple(paths), None
