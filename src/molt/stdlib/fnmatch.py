"""Intrinsic-backed filename matching for Molt -- all operations delegated to Rust."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["filter", "fnmatch", "fnmatchcase", "translate"]


_MOLT_FNMATCH_FNMATCH = _require_intrinsic("molt_fnmatch_fnmatch")
_MOLT_FNMATCH_FNMATCHCASE = _require_intrinsic("molt_fnmatch_fnmatchcase")
_MOLT_FNMATCH_FILTER = _require_intrinsic("molt_fnmatch_filter")
_MOLT_FNMATCH_TRANSLATE = _require_intrinsic("molt_fnmatch_translate")


def fnmatch(name: str | bytes, pat: str | bytes) -> bool:
    """Test whether FILENAME matches PATTERN (via Rust intrinsic).

    Case-insensitive on Windows/macOS, case-sensitive on Linux.
    """
    return bool(_MOLT_FNMATCH_FNMATCH(name, pat))


def fnmatchcase(name: str | bytes, pat: str | bytes) -> bool:
    """Test whether FILENAME matches PATTERN, always case-sensitive (via Rust intrinsic)."""
    return bool(_MOLT_FNMATCH_FNMATCHCASE(name, pat))


def filter(names, pat: str | bytes):
    """Return the subset of the list NAMES that match PAT (via Rust intrinsic)."""
    matches = _MOLT_FNMATCH_FILTER(names, pat, False)
    if not isinstance(matches, list):
        raise RuntimeError("fnmatch filter intrinsic returned invalid value")
    return matches


def translate(pat: str) -> str:
    """Translate a shell PATTERN to a regular expression (via Rust intrinsic)."""
    out = _MOLT_FNMATCH_TRANSLATE(pat)
    if not isinstance(out, str):
        raise RuntimeError("fnmatch translate intrinsic returned invalid value")
    return out
