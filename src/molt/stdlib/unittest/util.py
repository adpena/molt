"""unittest.util — helper utilities for Molt's unittest implementation.

CPython 3.12 parity for the public surface of ``unittest.util``.
"""

from __future__ import annotations

__all__ = [
    "safe_repr",
    "strclass",
    "sorted_list_difference",
    "unorderable_list_difference",
    "three_way_cmp",
    "get_diff_ratio",
]

_MAX_LENGTH = 80


def safe_repr(obj, short: bool = False) -> str:
    """Return a truncated repr of *obj* that will not raise."""
    try:
        result = repr(obj)
    except Exception:
        result = object.__repr__(obj)
    if short and len(result) > _MAX_LENGTH:
        result = result[:_MAX_LENGTH] + " [truncated]..."
    return result


def strclass(cls) -> str:
    """Return 'module.qualname' string for a class."""
    return f"{cls.__module__}.{cls.__qualname__}"


def sorted_list_difference(expected: list, actual: list) -> tuple[list, list]:
    """Return (missing, unexpected) comparing two *sorted* lists."""
    missing: list = []
    unexpected: list = []
    i = j = 0
    while i < len(expected) and j < len(actual):
        if expected[i] == actual[j]:
            i += 1
            j += 1
        elif expected[i] < actual[j]:
            missing.append(expected[i])
            i += 1
        else:
            unexpected.append(actual[j])
            j += 1
    missing.extend(expected[i:])
    unexpected.extend(actual[j:])
    return missing, unexpected


def unorderable_list_difference(expected: list, actual: list) -> tuple[list, list]:
    """Return (missing, unexpected) comparing two *unsorted* lists."""
    missing: list = []
    unexpected: list = list(actual)
    for item in expected:
        try:
            unexpected.remove(item)
        except ValueError:
            missing.append(item)
    return missing, unexpected


def three_way_cmp(x, y) -> int:
    """Return -1, 0, or +1 depending on comparison result."""
    if x < y:
        return -1
    if x > y:
        return 1
    return 0


def get_diff_ratio(a: str, b: str) -> float:
    """Very lightweight similarity ratio for two strings (0.0–1.0)."""
    if not a and not b:
        return 1.0
    if not a or not b:
        return 0.0
    # Count common characters (bag intersection)
    from_a: dict[str, int] = {}
    for ch in a:
        from_a[ch] = from_a.get(ch, 0) + 1
    common = 0
    for ch in b:
        if from_a.get(ch, 0) > 0:
            common += 1
            from_a[ch] -= 1
    return 2.0 * common / (len(a) + len(b))
