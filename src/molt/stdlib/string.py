"""String constants and helpers for Molt."""

from __future__ import annotations

__all__ = [
    "ascii_letters",
    "ascii_lowercase",
    "ascii_uppercase",
    "digits",
    "hexdigits",
    "octdigits",
    "punctuation",
    "whitespace",
    "printable",
    "capwords",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): add Template + formatter helpers.

ascii_lowercase = "abcdefghijklmnopqrstuvwxyz"
ascii_uppercase = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
ascii_letters = ascii_lowercase + ascii_uppercase
digits = "0123456789"
hexdigits = digits + "abcdef" + "ABCDEF"
octdigits = "01234567"
punctuation = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"
whitespace = " \t\n\r\x0b\x0c"
printable = digits + ascii_letters + punctuation + whitespace


def capwords(s: str, sep: str | None = None) -> str:
    if sep is None:
        return " ".join(part.capitalize() for part in s.split())
    return sep.join(part.capitalize() for part in s.split(sep))
