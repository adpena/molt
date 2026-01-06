"""Filename matching helpers for Molt."""

from __future__ import annotations

__all__ = ["fnmatch", "fnmatchcase"]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement escape semantics.


def fnmatch(name: str, pat: str) -> bool:
    return fnmatchcase(name, pat)


def fnmatchcase(name: str, pat: str) -> bool:
    return _match(pat, name)


def _parse_char_class(pat: str, idx: int) -> tuple[set[str], bool, int] | None:
    if idx >= len(pat) or pat[idx] != "[":
        return None
    idx += 1
    negate = False
    if idx < len(pat) and pat[idx] in "!^":
        negate = True
        idx += 1
    chars: set[str] = set()
    while idx < len(pat) and pat[idx] != "]":
        if idx + 2 < len(pat) and pat[idx + 1] == "-" and pat[idx + 2] != "]":
            start = ord(pat[idx])
            end = ord(pat[idx + 2])
            if start <= end:
                for code in range(start, end + 1):
                    chars.add(chr(code))
            else:
                for code in range(end, start + 1):
                    chars.add(chr(code))
            idx += 3
            continue
        chars.add(pat[idx])
        idx += 1
    if idx >= len(pat) or pat[idx] != "]":
        return None
    return chars, negate, idx + 1


def _match(pat: str, text: str) -> bool:
    pi = 0
    ti = 0
    star_idx = -1
    match = 0

    while ti < len(text):
        if pi < len(pat) and pat[pi] == "*":
            while pi < len(pat) and pat[pi] == "*":
                pi += 1
            if pi == len(pat):
                return True
            star_idx = pi
            match = ti
            continue
        if pi < len(pat) and pat[pi] == "?":
            pi += 1
            ti += 1
            continue
        if pi < len(pat) and pat[pi] == "[":
            parsed = _parse_char_class(pat, pi)
            if parsed is None:
                return False
            chars, negate, next_idx = parsed
            if ti >= len(text):
                return False
            hit = text[ti] in chars
            if negate:
                hit = not hit
            if not hit:
                if star_idx != -1:
                    match += 1
                    ti = match
                    pi = star_idx
                    continue
                return False
            pi = next_idx
            ti += 1
            continue
        if pi < len(pat) and pat[pi] == text[ti]:
            pi += 1
            ti += 1
            continue
        if star_idx != -1:
            match += 1
            ti = match
            pi = star_idx
            continue
        return False

    while pi < len(pat) and pat[pi] == "*":
        pi += 1
    return pi == len(pat)
