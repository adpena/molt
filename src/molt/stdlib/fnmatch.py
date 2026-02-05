"""Filename matching helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

__all__ = ["fnmatch", "fnmatchcase"]


def fnmatch(name: str, pat: str) -> bool:
    return fnmatchcase(name, pat)


def fnmatchcase(name: str, pat: str) -> bool:
    return _match(pat, name)


def _parse_char_class(pat: str, idx: int) -> tuple[dict[str, bool], bool, int] | None:
    if idx >= len(pat) or pat[idx] != "[":
        return None
    idx += 1
    if idx >= len(pat):
        return None

    negate = False
    if pat[idx] == "!":
        negate = True
        idx += 1
    if idx >= len(pat):
        return None

    chars: dict[str, bool] = {}
    if pat[idx] == "]":
        chars["]"] = True
        idx += 1

    while idx < len(pat) and pat[idx] != "]":
        if idx + 2 < len(pat) and pat[idx + 1] == "-" and pat[idx + 2] != "]":
            start = ord(pat[idx])
            end = ord(pat[idx + 2])
            if start <= end:
                for code in range(start, end + 1):
                    chars[chr(code)] = True
            idx += 3
            continue
        chars[pat[idx]] = True
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
            if parsed is not None:
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
