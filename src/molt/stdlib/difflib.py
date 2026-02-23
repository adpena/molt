"""Minimal intrinsic-gated `difflib` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- intrinsic bindings ---

_MOLT_DIFFLIB_RATIO = _require_intrinsic("molt_difflib_ratio", globals())
_MOLT_DIFFLIB_QUICK_RATIO = _require_intrinsic("molt_difflib_quick_ratio", globals())
_MOLT_DIFFLIB_GET_MATCHING_BLOCKS = _require_intrinsic(
    "molt_difflib_get_matching_blocks", globals()
)
_MOLT_DIFFLIB_GET_OPCODES = _require_intrinsic("molt_difflib_get_opcodes", globals())
_MOLT_DIFFLIB_IS_JUNK = _require_intrinsic("molt_difflib_is_junk", globals())
_MOLT_DIFFLIB_NDIFF = _require_intrinsic("molt_difflib_ndiff", globals())
_MOLT_DIFFLIB_UNIFIED_DIFF = _require_intrinsic("molt_difflib_unified_diff", globals())
_MOLT_DIFFLIB_CONTEXT_DIFF = _require_intrinsic("molt_difflib_context_diff", globals())
_MOLT_DIFFLIB_GET_CLOSE_MATCHES = _require_intrinsic(
    "molt_difflib_get_close_matches", globals()
)


class SequenceMatcher:
    """Compare pairs of sequences of any type.

    Delegates heavy lifting to Rust intrinsics.
    """

    def __init__(self, isjunk=None, a="", b="", autojunk=True):
        self._isjunk = isjunk
        self._a = a
        self._b = b
        self._autojunk = autojunk

    def set_seqs(self, a, b):
        self.set_seq1(a)
        self.set_seq2(b)

    def set_seq1(self, a):
        self._a = a

    def set_seq2(self, b):
        self._b = b

    def ratio(self):
        return float(_MOLT_DIFFLIB_RATIO(self._a, self._b))

    def quick_ratio(self):
        return float(_MOLT_DIFFLIB_QUICK_RATIO(self._a, self._b))

    def real_quick_ratio(self):
        la, lb = len(self._a), len(self._b)
        return 2.0 * min(la, lb) / (la + lb) if (la + lb) else 1.0

    def get_matching_blocks(self):
        raw = _MOLT_DIFFLIB_GET_MATCHING_BLOCKS(self._a, self._b)
        return [tuple(block) for block in raw]

    def get_opcodes(self):
        raw = _MOLT_DIFFLIB_GET_OPCODES(self._a, self._b)
        return [tuple(op) for op in raw]

    def get_grouped_opcodes(self, n=3):
        codes = self.get_opcodes()
        if not codes:
            codes = [("equal", 0, 1, 0, 1)]
        # Pad the beginning and end with 'equal' opcodes if needed.
        tag, i1, i2, j1, j2 = codes[0]
        if tag == "equal":
            codes[0] = (tag, max(i1, i2 - n), i2, max(j1, j2 - n), j2)
        tag, i1, i2, j1, j2 = codes[-1]
        if tag == "equal":
            codes[-1] = (
                tag,
                i1,
                min(i2, i1 + n),
                j1,
                min(j2, j1 + n),
            )
        groups = []
        group = []
        for tag, i1, i2, j1, j2 in codes:
            if tag == "equal" and i2 - i1 > 2 * n:
                group.append((tag, i1, min(i2, i1 + n), j1, min(j2, j1 + n)))
                groups.append(group)
                group = []
                group.append((tag, max(i1, i2 - n), i2, max(j1, j2 - n), j2))
            else:
                group.append((tag, i1, i2, j1, j2))
        if group:
            groups.append(group)
        return groups


def get_close_matches(word, possibilities, n=3, cutoff=0.6):
    """Return a list of the best close matches."""
    return list(
        _MOLT_DIFFLIB_GET_CLOSE_MATCHES(
            str(word), list(possibilities), int(n), float(cutoff)
        )
    )


def unified_diff(a, b, fromfile="", tofile="", lineterm="\n", n=3):
    """Compare two sequences of lines and generate a unified diff."""
    raw = _MOLT_DIFFLIB_UNIFIED_DIFF(
        list(a), list(b), str(fromfile), str(tofile), int(n)
    )
    return list(raw)


def context_diff(a, b, fromfile="", tofile="", lineterm="\n", n=3):
    """Compare two sequences of lines and generate a context diff."""
    raw = _MOLT_DIFFLIB_CONTEXT_DIFF(
        list(a), list(b), str(fromfile), str(tofile), int(n)
    )
    return list(raw)


def ndiff(a, b, linejunk=None, charjunk=None):
    """Compare two sequences of lines and generate a delta (ndiff)."""
    raw = _MOLT_DIFFLIB_NDIFF(list(a), list(b))
    return list(raw)


def IS_LINE_JUNK(line, pat=None):
    """Return True if line is ignorable (blank or only '#')."""
    stripped = line.rstrip("\r\n")
    return stripped == "" or stripped.lstrip() == "#"


def IS_CHARACTER_JUNK(ch, ws=" \t"):
    """Return True if character is junk (whitespace)."""
    return ch in ws


__all__ = [
    "SequenceMatcher",
    "get_close_matches",
    "unified_diff",
    "context_diff",
    "ndiff",
    "IS_LINE_JUNK",
    "IS_CHARACTER_JUNK",
]
