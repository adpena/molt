"""Minimal textwrap support for Molt."""

from __future__ import annotations

__all__ = ["TextWrapper", "indent"]


class TextWrapper:
    def __init__(self, width: int = 70) -> None:
        self.width = int(width)

    def wrap(self, text: str) -> list[str]:
        words = text.split()
        if not words:
            return []
        lines: list[str] = []
        current = words[0]
        for word in words[1:]:
            if len(current) + 1 + len(word) <= self.width:
                current = f"{current} {word}"
            else:
                lines.append(current)
                current = word
        lines.append(current)
        return lines

    def fill(self, text: str) -> str:
        return "\n".join(self.wrap(text))


def indent(text: str, prefix: str) -> str:
    return "\n".join(prefix + line for line in text.split("\n"))
